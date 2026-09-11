"""ParseBench adapter — LlamaIndex's public parsing benchmark → LiteOCR manifests.

Upstream: https://huggingface.co/datasets/llamaindex/ParseBench (Apache-2.0), code at
https://github.com/run-llama/ParseBench, paper arXiv:2604.08538.

ParseBench ships 2,078 single-page documents and 169,011 assertions across five JSONL rule
files. Two of the five convert into LiteOCR's manifest:

* ``table.jsonl`` — 503 rows of ``type: "expected_markdown"`` carrying a ground-truth **HTML**
  table. Converted to a GitHub-markdown pipe table → ``kind: "transcript"``.
* ``text_content.jsonl`` — 141,322 assertions over 506 text pages. The convertible subset
  (``missing_sentence_percent`` → ``bag_of_sentences``, ``order`` → ``order``,
  ``missing_specific_sentence`` / ``missing_specific_word`` → ``present``) becomes
  ``kind: "rules"``.

``chart.jsonl``, ``layout.jsonl`` and ``text_formatting.jsonl`` have no analogue in the common
rule schema (data-point readings, bounding boxes, style flags) and are skipped; the counts land
in the build summary and in ``docs/benchmarks/adapters.md``.

The full conversion is written as an index only (``manifest.full.json``); the ~40-document
curated subset in ``manifest.json`` is the part committed to the repository.
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
    KIND_TRANSCRIPT,
    Adapter,
    Doc,
    Manifest,
    Rule,
    Upstream,
    html_table_to_markdown,
    human_size,
    pdf_page_count,
    register,
    sha256_file,
    slugify,
    write_manifest,
    write_rules,
)

REPO_ID = "llamaindex/ParseBench"
#: Pinned upstream commit. Everything this adapter produces is reproducible from it.
REVISION = "2805a1d940f95a203e0ae4b88be9934f7765b3fc"

ATTRIBUTION = (
    "ParseBench (LlamaIndex) — Zhang, Acosta, Carlson, Bron, Doulcet, Ospina, Suo, "
    "arXiv:2604.08538, https://parsebench.ai · dataset https://huggingface.co/datasets/"
    "llamaindex/ParseBench · code https://github.com/run-llama/ParseBench · Apache-2.0."
)

#: Rule files pulled on ``download``. The five ``docs/`` trees (517 MB) are fetched per file,
#: only for the documents that make the curated subset.
RULE_FILES = ("table.jsonl", "text_content.jsonl")
#: Downloaded so the "skipped" counts in the build summary are measured, not quoted.
SURVEY_FILES = ("text_formatting.jsonl", "chart.jsonl", "layout.jsonl")

#: ParseBench ``type`` → common-schema type. ``None`` means "cannot be expressed".
TEXT_RULE_MAP: dict[str, Optional[str]] = {
    "missing_sentence_percent": "bag_of_sentences",
    "missing_specific_sentence": "present",
    "missing_specific_word": "present",
    "order": "order",
    # Precision-direction and aggregate counters: checking them needs the *complete* reference
    # text, which ParseBench never publishes for the text split.
    "unexpected_sentence_percent": None,
    "too_many_sentence_occurence_percent": None,
    "missing_word_percent": None,
    "unexpected_word_percent": None,
    "too_many_word_occurence_percent": None,
    "bag_of_digit_percent": None,
    # Layout assertions ("this string is the running header"): no text-similarity analogue.
    "is_header": None,
    "is_footer": None,
}

#: Per-file and total budgets for the committed subset.
MAX_DOC_BYTES = 400_000
MAX_TOTAL_BYTES = 11 * 1024 * 1024
SUBSET_TABLE = 25
SUBSET_TEXT = 15
#: Document-type tags used to spread the text subset across ParseBench's difficulty buckets.
TEXT_TYPE_TAGS = (
    "simple",
    "ocr",
    "multicolumns",
    "multilang",
    "misc",
    "dense",
    "sparse",
    "handwritting",
)


def _read_jsonl(path: Path) -> Iterator[dict[str, Any]]:
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if line:
                yield json.loads(line)


def _rule_payload(row: dict[str, Any]) -> dict[str, Any]:
    """ParseBench stores the rule payload as a JSON *string* in the ``rule`` field."""
    raw = row.get("rule") or "{}"
    if isinstance(raw, dict):
        return raw
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError:
        return {}
    return payload if isinstance(payload, dict) else {}


def _doc_type(tags: list[str]) -> str:
    for tag in TEXT_TYPE_TAGS:
        if tag in tags:
            return tag
    return "misc"


def _family(stem: str) -> str:
    """Group table pages cut from the same source document (``foo_page3`` → ``foo``)."""
    base = stem
    if "_page" in base:
        base = base.rsplit("_page", 1)[0]
    return base.lower()


@register
class ParseBenchAdapter(Adapter):
    """Converts ParseBench into ``benchmark/datasets/parsebench/``."""

    name = "parsebench"
    license = "Apache-2.0"
    default_out = "benchmark/datasets/parsebench"
    description = (
        "LlamaIndex ParseBench: single-page enterprise documents with ground-truth HTML tables "
        "(converted to markdown) and machine-checkable text-content rules."
    )
    upstream = Upstream(
        repo_id=REPO_ID,
        revision=REVISION,
        url=f"https://huggingface.co/datasets/{REPO_ID}",
    )

    # -- download ------------------------------------------------------------------------------

    def _hf(self) -> Any:
        # Keep huggingface_hub an optional dependency: only `download` needs it.
        os.environ.setdefault("REQUESTS_CA_BUNDLE", "/root/.ccr/ca-bundle.crt")
        os.environ.setdefault("SSL_CERT_FILE", os.environ["REQUESTS_CA_BUNDLE"])
        try:
            import huggingface_hub
        except ImportError as exc:  # pragma: no cover - environment problem, not logic
            raise SystemExit(
                "huggingface_hub is required to download ParseBench: pip install huggingface_hub"
            ) from exc
        return huggingface_hub

    def download(self, cache_dir: Path) -> Path:
        """Fetch the rule JSONL files (≈71 MB) at the pinned revision; return the snapshot root."""
        hub = self._hf()
        cache_dir.mkdir(parents=True, exist_ok=True)
        root = Path(
            hub.snapshot_download(
                REPO_ID,
                repo_type="dataset",
                revision=REVISION,
                cache_dir=str(cache_dir),
                allow_patterns=["README.md", "eval.yaml", *RULE_FILES, *SURVEY_FILES],
            )
        )
        self.log(f"snapshot {root}")
        return root

    def _snapshot(self, cache_dir: Path) -> Path:
        """Locate an already-downloaded snapshot without hitting the network."""
        pattern = f"datasets--{REPO_ID.replace('/', '--')}/snapshots/{REVISION}"
        candidate = cache_dir / pattern
        if candidate.is_dir():
            return candidate
        matches = sorted(cache_dir.glob(f"**/{pattern}"))
        if matches:
            return matches[0]
        raise SystemExit(f"no ParseBench snapshot under {cache_dir}; run without --no-download to fetch it")

    def _fetch_doc(self, cache_dir: Path, repo_path: str) -> Path:
        hub = self._hf()
        return Path(
            hub.hf_hub_download(
                REPO_ID,
                repo_path,
                repo_type="dataset",
                revision=REVISION,
                cache_dir=str(cache_dir),
            )
        )

    # -- conversion ----------------------------------------------------------------------------

    def _table_docs(self, snapshot: Path) -> list[dict[str, Any]]:
        """Every ``table.jsonl`` row, converted to markdown truth."""
        out: list[dict[str, Any]] = []
        for row in _read_jsonl(snapshot / "table.jsonl"):
            if row.get("type") != "expected_markdown":
                self.bump("table_rules_skipped")
                continue
            html = row.get("expected_markdown") or ""
            conv = html_table_to_markdown(html)
            if not conv.markdown.strip():
                self.bump("table_rules_skipped")
                continue
            self.bump("table_rules_converted")
            if conv.merged_cells:
                self.bump("table_docs_with_merged_cells")
            out.append(
                {
                    "pdf": row["pdf"],
                    "upstream_id": row["id"],
                    "tags": list(row.get("tags") or []),
                    "markdown": conv.markdown,
                    "merged": conv.merged_cells,
                    "cols": conv.cols,
                    "rows": conv.rows,
                }
            )
        return out

    def _text_docs(self, snapshot: Path) -> list[dict[str, Any]]:
        """Every ``text_content.jsonl`` page, with its rules mapped into the common schema."""
        by_doc: dict[str, dict[str, Any]] = {}
        for row in _read_jsonl(snapshot / "text_content.jsonl"):
            pdf = row["pdf"]
            entry = by_doc.setdefault(pdf, {"pdf": pdf, "tags": [], "rules": [], "skipped": Counter()})
            for tag in row.get("tags") or []:
                if tag not in entry["tags"]:
                    entry["tags"].append(tag)
            kind = row.get("type", "")
            target = TEXT_RULE_MAP.get(kind)
            payload = _rule_payload(row)
            rule = self._convert_text_rule(kind, target, payload, row.get("id", ""))
            if rule is None:
                entry["skipped"][kind] += 1
                self.bump("text_rules_skipped")
                self.stats.setdefault("text_skipped_by_type", Counter())[kind] += 1
                continue
            entry["rules"].append(rule)
            self.bump("text_rules_converted")
            self.stats.setdefault("text_converted_by_type", Counter())[rule.type] += 1
        return list(by_doc.values())

    def _convert_text_rule(
        self, kind: str, target: Optional[str], payload: dict[str, Any], upstream_id: str
    ) -> Optional[Rule]:
        if target is None:
            return None
        if target == "bag_of_sentences":
            bag = payload.get("bag_of_sentence") or {}
            sentences = [s for s in bag if isinstance(s, str) and s.strip()]
            if not sentences:
                return None
            return Rule(
                id=slugify(upstream_id, 96) or "rule",
                type="bag_of_sentences",
                sentences=sorted(sentences),
                threshold=1.0,
                case_sensitive=False,
                source=upstream_id,
            )
        if target == "present":
            text = payload.get("sentence") or payload.get("word") or payload.get("text")
            if not isinstance(text, str) or not text.strip():
                return None
            return Rule(
                id=slugify(upstream_id, 96) or "rule",
                type="present",
                text=text,
                case_sensitive=False,
                source=upstream_id,
            )
        if target == "order":
            before, after = payload.get("before"), payload.get("after")
            if not isinstance(before, str) or not isinstance(after, str) or not before or not after:
                return None
            return Rule(
                id=slugify(upstream_id, 96) or "rule",
                type="order",
                before=before,
                after=after,
                case_sensitive=False,
                source=upstream_id,
            )
        return None

    def _survey_skips(self, snapshot: Path) -> None:
        """Count the dimensions that have no representation in the common rule schema."""
        for fname, label in (
            ("text_formatting.jsonl", "formatting"),
            ("chart.jsonl", "chart"),
            ("layout.jsonl", "layout"),
        ):
            path = snapshot / fname
            if not path.is_file():
                self.log(f"{fname} not in the snapshot; its skip count will be missing")
                continue
            self.bump(f"{label}_rules_skipped", sum(1 for _ in _read_jsonl(path)))

    # -- selection -----------------------------------------------------------------------------

    def _select_tables(self, docs: list[dict[str, Any]], n: int, rng: random.Random) -> list[dict[str, Any]]:
        """Candidate order: at most one page per source document, ~1/3 of them ``hard``.

        Returns more candidates than ``n`` so the caller can drop oversized documents and still
        fill the subset. Deterministic for a given seed.
        """
        seen_family: dict[tuple[str, str], dict[str, Any]] = {}
        for d in sorted(docs, key=lambda d: d["pdf"]):
            difficulty = "hard" if "hard" in d["tags"] else "easy"
            seen_family.setdefault((difficulty, _family(Path(d["pdf"]).stem)), d)
        hard = [d for (diff, _), d in sorted(seen_family.items()) if diff == "hard"]
        easy = [d for (diff, _), d in sorted(seen_family.items()) if diff == "easy"]
        rng.shuffle(hard)
        rng.shuffle(easy)
        want_hard = max(1, n // 3)
        order = hard[:want_hard] + easy
        order.extend(hard[want_hard:])
        return order

    def _select_texts(self, docs: list[dict[str, Any]], n: int, rng: random.Random) -> list[dict[str, Any]]:
        """One document per type tag, round-robin, so all 8 ParseBench text buckets appear."""
        by_type: dict[str, list[dict[str, Any]]] = defaultdict(list)
        for d in docs:
            by_type[_doc_type(d["tags"])].append(d)
        for bucket in by_type.values():
            bucket.sort(key=lambda d: d["pdf"])
            rng.shuffle(bucket)
        order: list[dict[str, Any]] = []
        types = [t for t in TEXT_TYPE_TAGS if by_type.get(t)]
        i = 0
        while any(by_type[t] for t in types) and len(order) < n * 4:
            t = types[i % len(types)]
            if by_type[t]:
                order.append(by_type[t].pop(0))
            i += 1
        return order

    # -- build ---------------------------------------------------------------------------------

    def build(self, out_dir: Path, limit: Optional[int] = None, seed: int = 1234) -> Manifest:
        cache_dir = self.cache_dir
        snapshot = self._snapshot(cache_dir)
        rng = random.Random(seed)

        tables = self._table_docs(snapshot)
        texts = self._text_docs(snapshot)
        self._survey_skips(snapshot)
        self.stats["upstream_table_docs"] = len(tables)
        self.stats["upstream_text_docs"] = len(texts)

        want_tables, want_texts = SUBSET_TABLE, SUBSET_TEXT
        if limit is not None:
            share = max(1, round(limit * SUBSET_TABLE / (SUBSET_TABLE + SUBSET_TEXT)))
            want_tables = min(SUBSET_TABLE, share)
            want_texts = max(0, limit - want_tables)

        docs_dir, truth_dir, rules_dir = out_dir / "docs", out_dir / "truth", out_dir / "rules"
        for d in (docs_dir, truth_dir, rules_dir):
            if d.is_dir():
                shutil.rmtree(d)
            d.mkdir(parents=True, exist_ok=True)

        committed: list[Doc] = []
        budget = MAX_TOTAL_BYTES
        used = 0

        def take(entry: dict[str, Any], kind: str) -> Optional[Doc]:
            nonlocal used
            repo_path = entry["pdf"]
            src = self._fetch_doc(cache_dir, repo_path)
            size = src.stat().st_size
            if size > MAX_DOC_BYTES:
                self.bump("skipped_too_large")
                return None
            if used + size > budget:
                self.bump("skipped_over_budget")
                return None
            pages = pdf_page_count(src)
            if pages != 1:
                self.bump("skipped_multipage")
                return None
            stem = Path(repo_path).stem
            doc_id = slugify(stem)
            suffix = Path(repo_path).suffix.lower()
            dest = docs_dir / f"{doc_id}{suffix}"
            if dest.exists():
                self.bump("skipped_duplicate_id")
                return None
            shutil.copyfile(src, dest)
            used += size
            tags = sorted({*entry["tags"], "parsebench"})
            doc = Doc(
                id=doc_id,
                file=f"docs/{dest.name}",
                pages=pages,
                tags=tags,
                kind=kind,
                source_id=stem,
                upstream_path=repo_path,
                sha256=sha256_file(dest),
                license=self.license,
                attribution=ATTRIBUTION,
            )
            if kind == KIND_TRANSCRIPT:
                doc.category = "table"
                doc.truth = f"truth/{doc_id}.md"
                if entry["merged"]:
                    tags = sorted({*tags, "merged-cells"})
                doc.tags = sorted({*tags, "table-only"})
                (truth_dir / f"{doc_id}.md").write_text(entry["markdown"], encoding="utf-8")
            else:
                doc.category = "text"
                doc.truth = ""
                doc.rules = f"rules/{doc_id}.json"
                write_rules(rules_dir / f"{doc_id}.json", entry["rules"])
                doc.tags = sorted({*tags, "rules"})
            return doc

        for entry in self._select_tables(tables, want_tables, rng):
            if sum(1 for d in committed if d.kind == KIND_TRANSCRIPT) >= want_tables:
                break
            doc = take(entry, KIND_TRANSCRIPT)
            if doc is not None:
                committed.append(doc)
        for entry in self._select_texts(texts, want_texts, rng):
            if sum(1 for d in committed if d.kind == KIND_RULES) >= want_texts:
                break
            if not entry["rules"]:
                continue
            doc = take(entry, KIND_RULES)
            if doc is not None:
                committed.append(doc)

        self.stats["committed_bytes"] = used
        self.stats["committed_docs"] = len(committed)
        self.stats["committed_rules"] = sum(
            len(json.loads((out_dir / d.rules).read_text(encoding="utf-8"))) for d in committed if d.rules
        )

        manifest = Manifest(
            name="parsebench",
            version="1.0.0",
            description=(
                "Curated, redistributable subset of LlamaIndex ParseBench: table pages with "
                "markdown ground truth (kind=transcript, scored with table_score) and text pages "
                "with machine-checkable assertions (kind=rules). Built by "
                "`python -m benchmark.adapters parsebench`."
            ),
            license=self.license,
            documents=committed,
            generator="benchmark/adapters/parsebench.py",
            upstream=self.upstream,
            attribution=ATTRIBUTION,
            notes=[
                "kind=transcript documents carry a table only, not the whole page: score them "
                "with table_score, and read char_similarity/CER as a table-content proxy.",
                "kind=rules documents have no markdown truth and are skipped by scorers that "
                "only understand transcripts; `truth` is the empty string so the manifest still "
                "deserialises into the current Rust ManifestDoc.",
                f"Upstream revision {REVISION} of {REPO_ID}.",
            ],
        )
        write_manifest(out_dir / "manifest.json", manifest)
        self._write_full_index(out_dir, tables, texts, committed)
        (out_dir / "subset-committed.txt").write_text(
            "\n".join(sorted(d.id for d in committed)) + "\n", encoding="utf-8"
        )
        self.log(
            f"committed {len(committed)} docs ({human_size(used)}), {self.stats['committed_rules']} rules"
        )
        return manifest

    def _write_full_index(
        self,
        out_dir: Path,
        tables: list[dict[str, Any]],
        texts: list[dict[str, Any]],
        committed: list[Doc],
    ) -> None:
        """Index of the whole convertible conversion; the bytes are fetched on demand."""
        have = {d.upstream_path for d in committed}
        docs: list[Doc] = []
        for entry in tables:
            stem = Path(entry["pdf"]).stem
            extra = ["merged-cells"] if entry["merged"] else []
            tags = sorted({*entry["tags"], "parsebench", "table-only", *extra})
            docs.append(
                Doc(
                    id=slugify(stem),
                    file=f"docs/{slugify(stem)}{Path(entry['pdf']).suffix.lower()}",
                    truth=f"truth/{slugify(stem)}.md",
                    category="table",
                    tags=sorted({*tags, *(["committed"] if entry["pdf"] in have else [])}),
                    kind=KIND_TRANSCRIPT,
                    source_id=stem,
                    upstream_path=entry["pdf"],
                )
            )
        for entry in texts:
            if not entry["rules"]:
                continue
            stem = Path(entry["pdf"]).stem
            tags = sorted({*entry["tags"], "parsebench", "rules"})
            docs.append(
                Doc(
                    id=slugify(stem),
                    file=f"docs/{slugify(stem)}{Path(entry['pdf']).suffix.lower()}",
                    truth="",
                    rules=f"rules/{slugify(stem)}.json",
                    category="text",
                    tags=sorted({*tags, *(["committed"] if entry["pdf"] in have else [])}),
                    kind=KIND_RULES,
                    source_id=stem,
                    upstream_path=entry["pdf"],
                )
            )
        full = Manifest(
            name="parsebench-full",
            version="1.0.0",
            description=(
                "Index of every ParseBench document LiteOCR can convert. The document bytes and "
                "the truth/rule files are NOT committed for entries without the `committed` tag: "
                "fetch them by `upstream_path` from the pinned revision, or raise "
                "MAX_TOTAL_BYTES / MAX_DOC_BYTES in benchmark/adapters/parsebench.py and "
                "rebuild into a scratch --out directory."
            ),
            license=self.license,
            documents=docs,
            generator="benchmark/adapters/parsebench.py",
            upstream=self.upstream,
            attribution=ATTRIBUTION,
            notes=[
                "Not runnable as-is: the files referenced by `file` are absent unless fetched.",
                "`committed` tag marks the documents present in manifest.json.",
            ],
        )
        write_manifest(out_dir / "manifest.full.json", full)
        self.stats["full_index_docs"] = len(docs)
