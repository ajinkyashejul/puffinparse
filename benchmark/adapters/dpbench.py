"""DP-Bench adapter — Upstage's document-parsing benchmark → PuffinParse transcripts.

Upstream: https://huggingface.co/datasets/upstage/dp-bench (MIT, per the dataset card). Data and
the evaluation code (``evaluate.py``, ``src/layout_evaluation.py``, ``src/table_evaluation.py``)
live in the same repository, pinned together at :data:`REVISION`.

DP-Bench ships 200 single-page PDFs and one ``dataset/reference.json``:
``{<pdf>: {"elements": [{category, coordinates, id, page, content: {text, html, markdown}}]}}``.
``id`` is the human reading order, and the list is already sorted by it. There are 12 element
categories: Paragraph, Heading1, Footer, Caption, Header, List, Chart, Footnote, Equation, Figure,
Table and Index. Upstream scores two things:

* **NID** (``src/layout_evaluation.py``): the ``text`` of every element *except* ``figure``,
  ``table`` and ``chart`` (``evaluate.py --ignore-classes-for-layout``), concatenated in element
  order and compared with ``rapidfuzz.fuzz.ratio``. Headers, footers and footnotes **are**
  scored, so this adapter keeps them.
* **TEDS / TEDS-S** (``src/table_evaluation.py``) over the ``html`` of the ``Table`` elements.

Each page therefore becomes one ``kind: "transcript"`` document whose truth is, in reading order:

* ``Heading1`` → ``# heading``;
* ``Table`` → its HTML converted to a GitHub pipe table (merged cells flattened by repetition,
  tag ``merged-cells``; the Rust scorer reads pipe tables and HTML into the same grid, so
  ``table_score`` / ``teds_grid`` measure the table like upstream's TEDS does);
* ``Equation`` → its LaTeX inside ``$$ … $$`` (tag ``has-formula``);
* ``Figure`` and ``Chart`` → dropped, exactly as upstream's NID drops them (chart ``text`` holds
  axis ticks and legend fragments, not prose);
* every other category → its ``text`` verbatim.

No ``rules`` are emitted: a document has one kind, and the table truth is already scored by
``table_score`` / ``teds_grid`` inside the transcript, which is closer to upstream's TEDS than a
handful of ``table_cell`` assertions would be.

Documents are tagged with the element categories they contain (``has-table``, ``has-chart``,
``has-header`` …) and filed under a ``category`` that names their dominant layout feature. The
reference carries no per-page source (Library of Congress / OER / Upstage) or domain, so those
cannot be tagged.
"""

from __future__ import annotations

import json
import os
import random
import re
import shutil
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any, Optional

from .base import (
    KIND_TRANSCRIPT,
    Adapter,
    Doc,
    Manifest,
    Upstream,
    html_table_to_markdown,
    human_size,
    pdf_page_count,
    register,
    sha256_file,
    slugify,
    write_json,
    write_manifest,
)

REPO_ID = "upstage/dp-bench"
#: Pinned upstream commit: data and ``evaluate.py`` together.
REVISION = "24702c61a2fb13325534be664653bc6e60250d13"

LICENSE = "MIT"
ATTRIBUTION = (
    "DP-Bench: Document Parsing Benchmark (Upstage AI) · dataset "
    "https://huggingface.co/datasets/upstage/dp-bench · MIT (dataset card). Pages from the "
    "Library of Congress, Open Educational Resources and Upstage internal documents."
)

REFERENCE_FILE = "dataset/reference.json"
#: What ``download`` fetches besides the PDFs: the card (licence), the reference and the
#: evaluation code whose semantics the truth construction mirrors.
METADATA_FILES = (
    "README.md",
    REFERENCE_FILE,
    "evaluate.py",
    "src/layout_evaluation.py",
    "src/table_evaluation.py",
)

#: Upstream's default ``--ignore-classes-for-layout``: NID never scores these.
NID_IGNORED = frozenset({"figure", "table", "chart"})
#: Of those, the ones without a text truth here. Tables are kept (TEDS scores them).
DROPPED = NID_IGNORED - {"table"}

#: ``category`` of a page: its first matching layout feature, in this priority order.
LAYOUT_PRIORITY = (
    ("Table", "table"),
    ("Equation", "equation"),
    ("Chart", "chart"),
    ("Figure", "figure"),
    ("Index", "index"),
    ("List", "list"),
)
DEFAULT_LAYOUT = "text"

#: Pages committed per layout category (40 in total). Tables are over-weighted because they are
#: DP-Bench's emphasis; every category has at least 3.
SUBSET_PER_LAYOUT = {
    "table": 10,
    "text": 7,
    "chart": 6,
    "figure": 5,
    "equation": 5,
    "list": 4,
    "index": 3,
}
#: Pages with merged table cells put first in their layout bucket (upstream has 8 such tables).
MIN_MERGED_CELL_PAGES = 4
MAX_DOC_BYTES = 600 * 1024
MAX_TOTAL_BYTES = 8 * 1024 * 1024
MIN_TRUTH_CHARS = 40

_LICENSE_LINE = re.compile(r"^license:\s*(\S+)\s*$", re.M)


def _category_tag(category: str) -> str:
    return "has-" + slugify(category).replace("_", "-")


def page_layout(categories: set[str]) -> str:
    """The page's ``category``: its dominant layout feature (see :data:`LAYOUT_PRIORITY`)."""
    for upstream, name in LAYOUT_PRIORITY:
        if upstream in categories:
            return name
    return DEFAULT_LAYOUT


def page_truth(page: dict[str, Any]) -> tuple[Optional[str], dict[str, Any]]:
    """Markdown truth for one DP-Bench page, or ``(None, info)`` when it must be skipped.

    ``info`` carries ``reason`` (for a skip), ``merged_cells``, the element ``categories`` and
    per-category element counts.
    """
    elements = sorted(page.get("elements") or [], key=lambda e: int(e.get("id", 0)))
    info: dict[str, Any] = {
        "merged_cells": False,
        "categories": sorted({str(e.get("category", "")) for e in elements}),
        "elements": Counter(str(e.get("category", "")) for e in elements),
    }
    if any(int(e.get("page", 1)) != 1 for e in elements):
        info["reason"] = "multi_page_reference"
        return None, info
    parts: list[str] = []
    for element in elements:
        category = str(element.get("category", ""))
        key = category.lower()
        content = element.get("content") or {}
        if key in DROPPED:
            continue
        if key == "table":
            conv = html_table_to_markdown(str(content.get("html") or ""))
            if not conv.markdown.strip():
                info["reason"] = "table_without_html"
                return None, info
            info["merged_cells"] = info["merged_cells"] or conv.merged_cells
            parts.append(conv.markdown.strip())
            continue
        text = str(content.get("text") or "").strip()
        if not text:
            continue
        if key == "equation":
            parts.append(text if text.startswith("$$") else f"$$\n{text}\n$$")
        elif key == "heading1":
            parts.append("# " + " ".join(text.split()))
        else:
            parts.append(text)
    truth = "\n\n".join(parts).strip()
    if len(truth) < MIN_TRUTH_CHARS:
        info["reason"] = "truth_too_short"
        return None, info
    return truth + "\n", info


def page_tags(info: dict[str, Any]) -> list[str]:
    tags = {"dpbench"}
    tags.update(_category_tag(c) for c in info["categories"] if c)
    if "has-equation" in tags:
        tags.add("has-formula")
    if info.get("merged_cells"):
        tags.add("merged-cells")
    return sorted(tags)


@register
class DpBenchAdapter(Adapter):
    """Converts DP-Bench into ``benchmark/datasets/dpbench/``."""

    name = "dpbench"
    license = LICENSE
    default_out = "benchmark/datasets/dpbench"
    description = (
        "Upstage DP-Bench: single-page PDFs (Library of Congress, OER, Upstage) with every layout "
        "element in reading order; tables as HTML."
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
        """Fetch the card, ``reference.json`` (1.5 MB) and the evaluation code; PDFs come per page."""
        hub = self._hf()
        cache_dir.mkdir(parents=True, exist_ok=True)
        root = Path(
            hub.snapshot_download(
                REPO_ID,
                repo_type="dataset",
                revision=REVISION,
                cache_dir=str(cache_dir),
                allow_patterns=list(METADATA_FILES),
            )
        )
        self.log(f"snapshot {root}")
        return root

    def _snapshot(self, cache_dir: Path) -> Path:
        pattern = f"datasets--{REPO_ID.replace('/', '--')}/snapshots/{REVISION}"
        candidate = cache_dir / pattern
        if (candidate / REFERENCE_FILE).is_file():
            return candidate
        matches = sorted(p.parent.parent for p in cache_dir.glob(f"**/{pattern}/{REFERENCE_FILE}"))
        if matches:
            return matches[0]
        raise SystemExit(f"no DP-Bench snapshot under {cache_dir}; run without --no-download")

    def _fetch_doc(self, cache_dir: Path, repo_path: str) -> Path:
        snapshot_copy = cache_dir / f"datasets--{REPO_ID.replace('/', '--')}/snapshots/{REVISION}/{repo_path}"
        if snapshot_copy.is_file():
            return snapshot_copy
        hub = self._hf()
        return Path(
            hub.hf_hub_download(
                REPO_ID, repo_path, repo_type="dataset", revision=REVISION, cache_dir=str(cache_dir)
            )
        )

    def _check_license(self, snapshot: Path) -> None:
        """Refuse to vendor anything unless the card at the pinned revision still says MIT."""
        card = (snapshot / "README.md").read_text(encoding="utf-8")
        m = _LICENSE_LINE.search(card.split("---", 2)[1] if card.startswith("---") else "")
        declared = m.group(1).lower() if m else None
        if declared != "mit":
            raise SystemExit(f"DP-Bench card declares licence {declared!r}, expected 'mit'; not vendoring")
        self.stats["upstream_license"] = "mit (dataset card front-matter; no LICENSE file in the repo)"

    # -- conversion ----------------------------------------------------------------------------

    def _convert(self, snapshot: Path) -> list[dict[str, Any]]:
        reference = json.loads((snapshot / REFERENCE_FILE).read_text(encoding="utf-8"))
        if not isinstance(reference, dict):
            raise SystemExit(f"{REFERENCE_FILE}: expected an object keyed by PDF name")
        usable: list[dict[str, Any]] = []
        elements: Counter[str] = Counter()
        skipped: Counter[str] = Counter()
        for pdf in sorted(reference):
            truth, info = page_truth(reference[pdf])
            elements.update(info["elements"])
            if truth is None:
                skipped[str(info.get("reason", "unknown"))] += 1
                continue
            usable.append(
                {"pdf": pdf, "truth": truth, "info": info, "layout": page_layout(set(info["categories"]))}
            )
        self.stats["upstream_pages"] = len(reference)
        self.stats["upstream_elements_by_category"] = elements
        self.stats["convertible_pages"] = len(usable)
        self.stats["pages_skipped"] = sum(skipped.values())
        self.stats["pages_skipped_by_reason"] = skipped
        self.stats["convertible_pages_by_layout"] = Counter(u["layout"] for u in usable)
        self.stats["elements_dropped_figure_chart"] = elements["Figure"] + elements["Chart"]
        return usable

    # -- build ---------------------------------------------------------------------------------

    def build(self, out_dir: Path, limit: Optional[int] = None, seed: int = 1234) -> Manifest:
        cache_dir = self.cache_dir
        snapshot = self._snapshot(cache_dir)
        self._check_license(snapshot)
        rng = random.Random(seed)
        usable = self._convert(snapshot)

        want = dict(SUBSET_PER_LAYOUT)
        if limit is not None:
            total = sum(want.values())
            want = {k: max(0, round(limit * v / total)) for k, v in want.items()}

        by_layout: dict[str, list[dict[str, Any]]] = defaultdict(list)
        for entry in usable:
            by_layout[entry["layout"]].append(entry)
        for name in sorted(by_layout):
            bucket = by_layout[name]
            rng.shuffle(bucket)
            # Merged-cell tables are the hard TEDS cases; make sure a few are in the subset.
            merged = [e for e in bucket if e["info"]["merged_cells"]][:MIN_MERGED_CELL_PAGES]
            by_layout[name] = merged + [e for e in bucket if e not in merged]

        docs_dir, truth_dir = out_dir / "docs", out_dir / "truth"
        for d in (docs_dir, truth_dir):
            if d.is_dir():
                shutil.rmtree(d)
            d.mkdir(parents=True, exist_ok=True)

        committed: list[Doc] = []
        used = 0
        for layout in sorted(by_layout):
            taken = 0
            for entry in by_layout[layout]:
                if taken >= want.get(layout, 0):
                    break
                repo_path = f"dataset/pdfs/{entry['pdf']}"
                src = self._fetch_doc(cache_dir, repo_path)
                size = src.stat().st_size
                if size > MAX_DOC_BYTES:
                    self.bump("candidates_skipped_too_large")
                    continue
                if used + size > MAX_TOTAL_BYTES:
                    self.bump("candidates_skipped_over_budget")
                    continue
                pages = pdf_page_count(src)
                if pages != 1:
                    self.bump("candidates_skipped_multipage")
                    continue
                doc_id = slugify(Path(entry["pdf"]).stem)
                dest = docs_dir / f"{doc_id}.pdf"
                if dest.exists():
                    self.bump("candidates_skipped_duplicate_id")
                    continue
                shutil.copyfile(src, dest)
                used += size
                taken += 1
                truth_bytes = entry["truth"].encode("utf-8")
                (truth_dir / f"{doc_id}.md").write_bytes(truth_bytes)
                committed.append(
                    Doc(
                        id=doc_id,
                        file=f"docs/{dest.name}",
                        truth=f"truth/{doc_id}.md",
                        pages=pages,
                        category=layout,
                        tags=page_tags(entry["info"]),
                        kind=KIND_TRANSCRIPT,
                        source_id=entry["pdf"],
                        upstream_path=repo_path,
                        sha256=sha256_file(dest),
                        license=LICENSE,
                        attribution=ATTRIBUTION,
                    )
                )

        self.stats["committed_docs"] = len(committed)
        self.stats["committed_bytes"] = used
        self.stats["committed_by_layout"] = Counter(d.category for d in committed)
        self.stats["committed_merged_cells"] = sum("merged-cells" in d.tags for d in committed)

        manifest = Manifest(
            name="dpbench",
            version="1.0.0",
            description=(
                "Curated subset of Upstage DP-Bench: single-page PDFs with reading-order markdown "
                "truth built from every layout element (tables as pipe tables). Built by "
                "`python -m benchmark.adapters dpbench`."
            ),
            license=LICENSE,
            documents=committed,
            generator="benchmark/adapters/dpbench.py",
            upstream=self.upstream,
            attribution=ATTRIBUTION,
            notes=[
                f"Upstream revision {REVISION} of {REPO_ID} (data and evaluate.py pinned together).",
                "Truth keeps what upstream's NID scores (every element's text except figure, table "
                "and chart, headers and footers included) plus the tables TEDS scores, as pipe "
                "tables. Figures and charts are dropped.",
                "Heading1 becomes `# heading`; equations stay LaTeX inside $$…$$ (tag "
                "`has-formula`), which a text metric compares literally.",
                "Merged table cells are flattened by repetition (tag `merged-cells`): "
                "`teds_grid` compares grids, not upstream's full HTML tree.",
                "`category` is the page's dominant layout feature (table > equation > chart > "
                "figure > index > list > text); the reference has no per-page source or domain.",
            ],
        )
        write_manifest(out_dir / "manifest.json", manifest)
        write_json(out_dir / "conversion-stats.json", self._stats_payload())
        self.log(f"committed {len(committed)} docs ({human_size(used)})")
        return manifest

    def _stats_payload(self) -> dict[str, Any]:
        out: dict[str, Any] = {}
        for key, value in sorted(self.stats.items()):
            out[key] = dict(sorted(value.items())) if isinstance(value, Counter) else value
        return out
