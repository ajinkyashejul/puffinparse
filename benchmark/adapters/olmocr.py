"""olmOCR-bench adapter — AI2's unit-test benchmark for PDF → markdown → PuffinParse rules.

Upstream: https://huggingface.co/datasets/allenai/olmOCR-bench (ODC-BY-1.0), scorer at
https://github.com/allenai/olmocr (``olmocr/bench/tests.py``, Apache-2.0), paper arXiv:2502.18443.

olmOCR-bench ships 1,403 single-page PDFs and 7,010 unit tests in seven JSONL files, one per
document source. Every test is already a machine-checkable assertion, so each document becomes a
``kind: "rules"`` document. The mapping onto the common schema (see
``docs/benchmarks/adapters.md``):

========== ================================== =================================================
upstream   PuffinParse                            fidelity
========== ================================== =================================================
present    ``present``                        exact substring (upstream: fuzzy, ``max_diffs``)
absent     ``absent``                         only without ``first_n`` / ``last_n``; positional
                                              absences are skipped (whole-page absence would
                                              fail a page number that also occurs in the body)
order      ``order``                          exact; case-sensitive like upstream
table      ``table_cell``                     ``top_heading`` → ``col_header``,
                                              ``left_heading`` → ``row_header``,
                                              ``left`` / ``right`` → ``row_header`` (same row —
                                              adjacency is relaxed); ``up`` / ``down`` skipped
math       —                                  skipped: KaTeX-rendered equivalence has no
                                              text-rule analogue
baseline   —                                  skipped: repetition / charset heuristic
========== ================================== =================================================

``max_diffs`` is carried on every converted rule so a fuzzy scorer can honour it later. Every
skip is counted by upstream type and reason in the build summary, the manifest ``notes`` and
``benchmark/datasets/olmocr/README.md`` — nothing is dropped silently.
"""

from __future__ import annotations

import json
import os
import random
import shutil
from collections import Counter, defaultdict
from collections.abc import Iterator
from pathlib import Path
from typing import Any, Optional

from .base import (
    KIND_RULES,
    Adapter,
    Doc,
    Manifest,
    Rule,
    Upstream,
    human_size,
    pdf_page_count,
    register,
    sha256_file,
    slugify,
    write_json,
    write_manifest,
    write_rules,
)

REPO_ID = "allenai/olmOCR-bench"
#: Pinned upstream dataset commit.
REVISION = "54a96a6fb6a2bd3b297e59869491db4d3625b711"
#: The scorer whose semantics the mapping follows (``olmocr/bench/tests.py``).
SCORER_REPO = "https://github.com/allenai/olmocr"
SCORER_REVISION = "f7cfe4c22098b154c76b6ec950d1c0a464eecf8d"

LICENSE = "ODC-By-1.0"
ATTRIBUTION = (
    "olmOCR-bench (Allen Institute for AI) — Poznanski et al., 'olmOCR: Unlocking Trillions of "
    "Tokens in PDFs with Vision Language Models', arXiv:2502.18443 · dataset "
    "https://huggingface.co/datasets/allenai/olmOCR-bench · ODC-BY-1.0, for research and "
    "educational use under AI2's Responsible Use Guidelines. Each page keeps its original "
    "source (`source_url`)."
)

#: The seven upstream test files (``bench_data/<split>.jsonl``).
SPLITS = (
    "arxiv_math",
    "headers_footers",
    "long_tiny_text",
    "multi_column",
    "old_scans",
    "old_scans_math",
    "table_tests",
)

#: Documents committed per split. The two math splits contain nothing but ``math`` tests, so no
#: document survives conversion there.
SUBSET_PER_SPLIT = {
    "headers_footers": 8,
    "long_tiny_text": 8,
    "multi_column": 8,
    "old_scans": 8,
    "table_tests": 8,
}
MAX_DOC_BYTES = 400_000
MAX_TOTAL_BYTES = 6 * 1024 * 1024
#: A document needs at least this many converted rules to be worth committing.
MIN_RULES = 3


def _read_jsonl(path: Path) -> Iterator[dict[str, Any]]:
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if line:
                yield json.loads(line)


def _text(value: Any) -> Optional[str]:
    """A non-blank string with whitespace runs collapsed to one space, or ``None``.

    Upstream's ``normalize_text`` collapses whitespace on both sides before matching, so this
    loses nothing — and it matters for table cells: a header annotated as ``"Chinese\nexperimenter"``
    can never be matched inside a one-line markdown table cell otherwise.
    """
    if not isinstance(value, str) or not value.strip():
        return None
    return " ".join(value.split())


def convert_test(test: dict[str, Any]) -> tuple[Optional[Rule], str]:
    """Map one olmOCR-bench test onto the common rule schema.

    Returns ``(rule, reason)``: ``reason`` is ``"converted"``, ``"relaxed:<why>"`` when the rule
    is kept in a weaker form than upstream, or ``"skipped:<why>"`` with ``rule is None``.
    """
    kind = test.get("type", "")
    upstream_id = str(test.get("id", ""))
    rule_id = slugify(upstream_id, 96) or "rule"
    max_diffs = int(test.get("max_diffs") or 0)

    if kind in ("present", "absent"):
        text = _text(test.get("text"))
        if text is None:
            return None, "skipped:empty_text"
        positional = bool(test.get("first_n") or test.get("last_n"))
        if kind == "absent" and positional:
            # Upstream only looks at the first/last N characters; a whole-page absence would
            # fail a correct parse whose body happens to repeat the header or page number.
            return None, "skipped:positional_absent"
        # Upstream default is case-sensitive for `present`; the absent tests say `false`.
        case_sensitive = bool(test.get("case_sensitive", True))
        rule = Rule(
            id=rule_id,
            type=kind,
            text=text,
            case_sensitive=case_sensitive,
            max_diffs=max_diffs,
            source=upstream_id,
        )
        return rule, "relaxed:positional_present" if positional else "converted"

    if kind == "order":
        before, after = _text(test.get("before")), _text(test.get("after"))
        if before is None or after is None:
            return None, "skipped:empty_text"
        # TextOrderTest never lowercases: order is case-sensitive upstream.
        rule = Rule(
            id=rule_id,
            type="order",
            before=before,
            after=after,
            case_sensitive=True,
            max_diffs=max_diffs,
            source=upstream_id,
        )
        return rule, "converted"

    if kind == "table":
        value = _text(test.get("cell"))
        if value is None:
            return None, "skipped:empty_text"
        top, left_h = _text(test.get("top_heading")), _text(test.get("left_heading"))
        left, right = _text(test.get("left")), _text(test.get("right"))
        up, down = _text(test.get("up")), _text(test.get("down"))
        cell: dict[str, Any] = {}
        reason = "converted"
        if up or down:
            # Vertical adjacency would need a column relation the schema does not have.
            # Weakening it to "the value exists in some table" would inflate scores.
            return None, "skipped:table_vertical_neighbour"
        if left_h:
            cell["row_header"] = left_h
        if top:
            cell["col_header"] = top
        if left or right:
            if "row_header" in cell:
                return None, "skipped:table_conflicting_row_constraints"
            # "Immediately left/right of" becomes "in the same row as": strictly weaker.
            cell["row_header"] = left or right
            reason = "relaxed:table_same_row"
        cell["value"] = value
        rule = Rule(
            id=rule_id,
            type="table_cell",
            cell=cell,
            case_sensitive=True,
            max_diffs=max_diffs,
            source=upstream_id,
        )
        return rule, reason

    if kind == "math":
        return None, "skipped:math"
    if kind == "baseline":
        return None, "skipped:baseline"
    return None, f"skipped:unknown_type_{kind or 'none'}"


@register
class OlmOcrBenchAdapter(Adapter):
    """Converts olmOCR-bench into ``benchmark/datasets/olmocr/``."""

    name = "olmocr"
    license = LICENSE
    default_out = "benchmark/datasets/olmocr"
    description = (
        "AI2 olmOCR-bench: single-page PDFs with machine-checkable unit tests (text presence and "
        "absence, reading order, table cells)."
    )
    upstream = Upstream(repo_id=REPO_ID, revision=REVISION, url=f"https://huggingface.co/datasets/{REPO_ID}")

    # -- download ------------------------------------------------------------------------------

    def _hf(self) -> Any:
        os.environ.setdefault("REQUESTS_CA_BUNDLE", "/root/.ccr/ca-bundle.crt")
        os.environ.setdefault("SSL_CERT_FILE", os.environ["REQUESTS_CA_BUNDLE"])
        try:
            import huggingface_hub
        except ImportError as exc:  # pragma: no cover - environment problem, not logic
            raise SystemExit("huggingface_hub is required: pip install huggingface_hub") from exc
        return huggingface_hub

    def download(self, cache_dir: Path) -> Path:
        """Fetch the seven test JSONL files (≈2.5 MB) at the pinned revision."""
        hub = self._hf()
        cache_dir.mkdir(parents=True, exist_ok=True)
        root = Path(
            hub.snapshot_download(
                REPO_ID,
                repo_type="dataset",
                revision=REVISION,
                cache_dir=str(cache_dir),
                allow_patterns=["README.md", "bench_data/*.jsonl"],
            )
        )
        self.log(f"snapshot {root}")
        return root

    def _snapshot(self, cache_dir: Path) -> Path:
        pattern = f"datasets--{REPO_ID.replace('/', '--')}/snapshots/{REVISION}"
        candidate = cache_dir / pattern
        if candidate.is_dir():
            return candidate
        matches = sorted(cache_dir.glob(f"**/{pattern}"))
        if matches:
            return matches[0]
        raise SystemExit(f"no olmOCR-bench snapshot under {cache_dir}; run without --no-download")

    def _fetch_doc(self, cache_dir: Path, repo_path: str) -> Path:
        hub = self._hf()
        return Path(
            hub.hf_hub_download(
                REPO_ID, repo_path, repo_type="dataset", revision=REVISION, cache_dir=str(cache_dir)
            )
        )

    # -- conversion ----------------------------------------------------------------------------

    def _convert(self, snapshot: Path) -> list[dict[str, Any]]:
        """Every upstream PDF with its converted rules, in upstream file order."""
        by_pdf: dict[str, dict[str, Any]] = {}
        by_type: Counter[str] = Counter()
        outcome: Counter[str] = Counter()
        for split in SPLITS:
            path = snapshot / "bench_data" / f"{split}.jsonl"
            if not path.is_file():
                raise SystemExit(f"{path} missing from the snapshot")
            for test in _read_jsonl(path):
                pdf = test["pdf"]
                entry = by_pdf.setdefault(
                    pdf,
                    {"pdf": pdf, "split": split, "rules": [], "url": test.get("url"), "skipped": Counter()},
                )
                by_type[test.get("type", "")] += 1
                rule, reason = convert_test(test)
                outcome[f"{test.get('type', '')}:{reason}"] += 1
                if rule is None:
                    entry["skipped"][reason] += 1
                    continue
                entry["rules"].append(rule)
        self.stats["upstream_tests_by_type"] = by_type
        self.stats["upstream_tests"] = sum(by_type.values())
        self.stats["upstream_pdfs"] = len(by_pdf)
        self.stats["outcome_by_type"] = outcome
        self.stats["rules_converted"] = sum(v for k, v in outcome.items() if ":skipped:" not in k)
        self.stats["rules_skipped"] = sum(v for k, v in outcome.items() if ":skipped:" in k)
        # The upstream runner adds one implicit `baseline` test per PDF that has none; count them
        # too so the skip total matches what olmOCR's own harness would run.
        explicit_baseline = {e["pdf"] for e in by_pdf.values() if e["skipped"].get("skipped:baseline")}
        self.stats["implicit_baseline_tests_skipped"] = len(by_pdf) - len(explicit_baseline)
        return list(by_pdf.values())

    # -- selection -----------------------------------------------------------------------------

    def _candidates(self, docs: list[dict[str, Any]], rng: random.Random) -> dict[str, list[dict[str, Any]]]:
        by_split: dict[str, list[dict[str, Any]]] = defaultdict(list)
        for d in sorted(docs, key=lambda d: d["pdf"]):
            if len(d["rules"]) >= MIN_RULES:
                by_split[d["split"]].append(d)
        for bucket in by_split.values():
            rng.shuffle(bucket)
        return by_split

    # -- build ---------------------------------------------------------------------------------

    def build(self, out_dir: Path, limit: Optional[int] = None, seed: int = 1234) -> Manifest:
        cache_dir = self.cache_dir
        snapshot = self._snapshot(cache_dir)
        rng = random.Random(seed)
        docs = self._convert(snapshot)
        convertible = [d for d in docs if d["rules"]]
        self.stats["convertible_pdfs"] = len(convertible)
        self.stats["pdfs_without_convertible_rules"] = len(docs) - len(convertible)

        want = dict(SUBSET_PER_SPLIT)
        if limit is not None:
            total = sum(want.values())
            want = {k: max(0, round(limit * v / total)) for k, v in want.items()}

        docs_dir, rules_dir = out_dir / "docs", out_dir / "rules"
        for d in (docs_dir, rules_dir):
            if d.is_dir():
                shutil.rmtree(d)
            d.mkdir(parents=True, exist_ok=True)

        committed: list[Doc] = []
        used = 0
        for split, bucket in sorted(self._candidates(docs, rng).items()):
            taken = 0
            for entry in bucket:
                if taken >= want.get(split, 0):
                    break
                repo_path = f"bench_data/pdfs/{entry['pdf']}"
                src = self._fetch_doc(cache_dir, repo_path)
                size = src.stat().st_size
                if size > MAX_DOC_BYTES:
                    self.bump("skipped_too_large")
                    continue
                if used + size > MAX_TOTAL_BYTES:
                    self.bump("skipped_over_budget")
                    continue
                pages = pdf_page_count(src)
                if pages != 1:
                    self.bump("skipped_multipage")
                    continue
                stem = Path(entry["pdf"]).stem
                doc_id = slugify(f"{split}_{stem}")
                dest = docs_dir / f"{doc_id}.pdf"
                if dest.exists():
                    self.bump("skipped_duplicate_id")
                    continue
                shutil.copyfile(src, dest)
                used += size
                taken += 1
                write_rules(rules_dir / f"{doc_id}.json", entry["rules"])
                committed.append(self._doc(entry, doc_id, f"docs/{dest.name}", pages, sha256_file(dest)))

        self.stats["committed_docs"] = len(committed)
        self.stats["committed_bytes"] = used
        committed_rules = [
            json.loads((out_dir / d.rules).read_text(encoding="utf-8")) for d in committed if d.rules
        ]
        self.stats["committed_rules"] = sum(len(r) for r in committed_rules)
        self.stats["committed_rules_by_type"] = Counter(r["type"] for rs in committed_rules for r in rs)

        manifest = Manifest(
            name="olmocr",
            version="1.0.0",
            description=(
                "Curated subset of AI2 olmOCR-bench: single-page PDFs scored by machine-checkable "
                "rules (present / absent / order / table_cell). Built by "
                "`python -m benchmark.adapters olmocr`."
            ),
            license=LICENSE,
            documents=committed,
            generator="benchmark/adapters/olmocr.py",
            upstream=self.upstream,
            attribution=ATTRIBUTION,
            notes=self._notes(),
        )
        write_manifest(out_dir / "manifest.json", manifest)
        self._write_full_index(out_dir, convertible, committed)
        write_json(out_dir / "conversion-stats.json", self._stats_payload())
        (out_dir / "subset-committed.txt").write_text(
            "\n".join(sorted(d.id for d in committed)) + "\n", encoding="utf-8"
        )
        self.log(
            f"committed {len(committed)} docs ({human_size(used)}), {self.stats['committed_rules']} rules"
        )
        return manifest

    def _doc(self, entry: dict[str, Any], doc_id: str, file: str, pages: int, sha: Optional[str]) -> Doc:
        tags = {"olmocr", "rules", entry["split"]}
        if all(r.type == "absent" for r in entry["rules"]):
            # An empty parse passes every rule of such a page (upstream guards this with its
            # implicit `baseline` test, which has no analogue here). Tag it so readers can tell.
            tags.add("absent-only")
        return Doc(
            id=doc_id,
            file=file,
            truth="",
            pages=pages,
            category=entry["split"],
            tags=sorted(tags),
            kind=KIND_RULES,
            rules=f"rules/{doc_id}.json",
            source_id=entry["pdf"],
            upstream_path=f"bench_data/pdfs/{entry['pdf']}",
            sha256=sha,
            source_url=entry.get("url") or None,
            license=LICENSE,
            attribution=ATTRIBUTION,
        )

    def _notes(self) -> list[str]:
        outcome: Counter[str] = self.stats["outcome_by_type"]
        skipped = ", ".join(f"{k} {v}" for k, v in sorted(outcome.items()) if ":skipped:" in k)
        relaxed = ", ".join(f"{k} {v}" for k, v in sorted(outcome.items()) if ":relaxed:" in k)
        return [
            f"Upstream revision {REVISION} of {REPO_ID}; rule semantics follow "
            f"olmocr/bench/tests.py at {SCORER_REVISION}.",
            f"Whole upstream: {self.stats['upstream_tests']} tests over {self.stats['upstream_pdfs']} PDFs; "
            f"{self.stats['rules_converted']} converted, {self.stats['rules_skipped']} skipped "
            f"(plus {self.stats['implicit_baseline_tests_skipped']} implicit per-PDF baseline tests "
            "the upstream runner adds, also skipped).",
            f"Skipped by type and reason: {skipped}.",
            f"Kept in a weaker form: {relaxed}.",
            "`max_diffs` is upstream's fuzzy-match allowance. The PuffinParse scorer matches exactly after "
            "normalisation, which is stricter than upstream for present/order/table_cell and looser "
            "for absent whenever max_diffs > 0.",
            "Scores aggregate per document (mean of passed/total), not per test category as "
            "olmOCR's leaderboard does, so they are not directly comparable to published numbers.",
        ]

    def _stats_payload(self) -> dict[str, Any]:
        out: dict[str, Any] = {}
        for key, value in sorted(self.stats.items()):
            if key in ("committed_bytes",):
                continue
            out[key] = dict(sorted(value.items())) if isinstance(value, Counter) else value
        return out

    def _write_full_index(
        self, out_dir: Path, convertible: list[dict[str, Any]], committed: list[Doc]
    ) -> None:
        """Index of every convertible upstream PDF; bytes and rules are fetched on demand."""
        have = {d.source_id for d in committed}
        docs: list[Doc] = []
        for entry in convertible:
            stem = Path(entry["pdf"]).stem
            doc_id = slugify(f"{entry['split']}_{stem}")
            doc = self._doc(entry, doc_id, f"docs/{doc_id}.pdf", 1, None)
            doc.attribution = None
            doc.license = None
            if entry["pdf"] in have:
                doc.tags = sorted({*doc.tags, "committed"})
            docs.append(doc)
        full = Manifest(
            name="olmocr-full",
            version="1.0.0",
            description=(
                "Index of every olmOCR-bench PDF with at least one convertible test. Entries without "
                "the `committed` tag are not in the repository: rebuild with a larger "
                "SUBSET_PER_SPLIT / MAX_TOTAL_BYTES into a scratch --out directory."
            ),
            license=LICENSE,
            documents=docs,
            generator="benchmark/adapters/olmocr.py",
            upstream=self.upstream,
            attribution=ATTRIBUTION,
            notes=["Not runnable as-is: `file` and `rules` exist only for `committed` entries."],
        )
        write_manifest(out_dir / "manifest.full.json", full)
        self.stats["full_index_docs"] = len(docs)
