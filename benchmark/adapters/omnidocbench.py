"""OmniDocBench adapter — OpenDataLab's page-level parsing benchmark → LiteOCR transcripts.

Upstream: https://huggingface.co/datasets/opendatalab/OmniDocBench (evaluation code at
https://github.com/opendatalab/OmniDocBench, Apache-2.0; paper arXiv:2412.07626).

OmniDocBench ships 1,651 page images and one JSON file (``OmniDocBench.json``) with, per page,
every layout block (category, polygon, reading ``order``, text / LaTeX / HTML) and page
attributes (``data_source``, ``language``, ``layout``, ``special_issue``, ``subset``). That is
enough to rebuild a reference transcript, so each page becomes a ``kind: "transcript"`` document:

* blocks with a reading ``order`` are emitted in that order; blocks split across columns
  (``truncated`` relations) are merged first, as upstream's ``tools/json2md.py`` does;
* ``title`` → ``# heading``; ``table`` → its HTML converted to a GitHub pipe table (merged cells
  flattened, ``merged-cells`` tag); ``equation_isolated`` → its ``$$…$$`` LaTeX; every other
  text block → its text;
* page furniture that OmniDocBench itself does not score (``header``, ``footer``,
  ``page_number``, ``page_footnote``, ``abandon``) and ``figure`` regions are left out;
* pages with masked regions (``*_mask``: content deliberately left unannotated) are not selected,
  because a correct parse of the masked text would be penalised.

**Licence.** The dataset card carries no licence; its copyright statement says the data is "for
research purposes only and not for commercial use". That does not permit redistribution in an
MIT repository, so nothing but the index is committed: ``manifest.json`` lists each page's
``upstream_path``, image ``sha256`` and ``truth_sha256``, and this adapter downloads the images
and regenerates the truth locally (both git-ignored) at the pinned revision.
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
    register,
    sha256_bytes,
    sha256_file,
    slugify,
    write_json,
    write_manifest,
)

REPO_ID = "opendatalab/OmniDocBench"
#: Pinned upstream dataset commit.
REVISION = "aa1ee96d106dbe53d0ae59474d75c6e6d9b53fec"
#: The evaluation code the truth construction follows (``tools/json2md.py``).
SCORER_REPO = "https://github.com/opendatalab/OmniDocBench"
SCORER_REVISION = "f133a71e9e91c3621c7ce8994200a7b394a06eb3"

#: No SPDX identifier exists for "research only, non-commercial, no explicit licence".
LICENSE = "LicenseRef-OmniDocBench-research-only"
ATTRIBUTION = (
    "OmniDocBench (OpenDataLab / Shanghai AI Laboratory) — Ouyang et al., 'OmniDocBench: "
    "Benchmarking Diverse PDF Document Parsing with Comprehensive Annotations', "
    "arXiv:2412.07626 · dataset https://huggingface.co/datasets/opendatalab/OmniDocBench · "
    "research use only, not for commercial use; not redistributed by LiteOCR."
)

JSON_FILE = "OmniDocBench.json"

#: Blocks OmniDocBench leaves out of its own text matching, plus regions with no text.
EXCLUDED_BLOCKS = frozenset(
    {
        "header",
        "footer",
        "page_number",
        "page_footnote",
        "abandon",
        "figure",
        "equation_semantic",
        "list_group",
    }
)
#: Pages containing these are not selected (unannotated content inside the page).
MASK_SUFFIX = "_mask"

SUBSET_SIZE = 40
#: Target language mix of the subset (upstream: 755 English, 765 Simplified Chinese, 116 mixed,
#: 13 Traditional Chinese, 2 other).
LANGUAGE_SHARE = {"english": 0.4, "simplified_chinese": 0.4, "en_ch_mixed": 0.15, "traditional_chinese": 0.05}
MAX_IMAGE_BYTES = 5 * 1024 * 1024
MIN_TRUTH_CHARS = 120

_CJK = re.compile(r"[぀-ヿ㐀-䶿一-鿿豈-﫿]")


def _text_norm(text: str) -> str:
    """Upstream ``json2md.text_norm`` plus the literal ``\\t`` indents the annotations carry."""
    text = text.replace("\\t", "")
    text = re.sub(r"_{4,}", "____", text)
    text = re.sub(r" {4,}", "    ", text)
    text = re.sub(r"([^a-zA-Z0-9])\1{10,}", r"\1\1\1\1", text)
    return text.strip()


def _join_truncated(parts: list[str]) -> str:
    """Join a block split across columns/pages: Latin text with a space (or de-hyphenated)."""
    out = ""
    for part in parts:
        part = part.strip()
        if not out:
            out = part
        elif _CJK.search(part[:1] or "") or _CJK.search(out[-1:]):
            out += part
        elif out.endswith("-"):
            out = out[:-1] + part
        else:
            out += " " + part
    return out


def page_truth(page: dict[str, Any]) -> tuple[Optional[str], dict[str, Any]]:
    """Markdown truth for one OmniDocBench page, or ``(None, info)`` when it must be skipped.

    ``info`` carries ``reason`` (for a skip) and ``merged_cells`` / ``has_table`` /
    ``has_formula`` flags for tagging.
    """
    info: dict[str, Any] = {"merged_cells": False, "has_table": False, "has_formula": False}
    dets = page.get("layout_dets") or []
    if any(str(b.get("category_type", "")).endswith(MASK_SUFFIX) for b in dets):
        info["reason"] = "masked_region"
        return None, info
    blocks = {
        b["anno_id"]: b
        for b in dets
        if b.get("order") is not None
        and not b.get("ignore")
        and b.get("category_type") not in EXCLUDED_BLOCKS
    }

    # Merge `truncated` chains (a paragraph continued in the next column), keyed by first block.
    groups: list[list[Any]] = []
    for rel in (page.get("extra") or {}).get("relation") or []:
        if rel.get("relation_type") != "truncated":
            continue
        src, dst = rel.get("source_anno_id"), rel.get("target_anno_id")
        if src not in blocks or dst not in blocks:
            continue
        for g in groups:
            if src in g or dst in g:
                g.extend(x for x in (src, dst) if x not in g)
                break
        else:
            groups.append([src, dst])
    merged_away: set[Any] = set()
    merged_text: dict[Any, str] = {}
    for g in groups:
        ordered = sorted(g, key=lambda a: blocks[a]["order"])
        merged_text[ordered[0]] = _join_truncated([str(blocks[a].get("text") or "") for a in ordered])
        merged_away.update(ordered[1:])

    parts: list[str] = []
    for anno_id, b in sorted(blocks.items(), key=lambda kv: (kv[1]["order"], str(kv[0]))):
        if anno_id in merged_away:
            continue
        cat = b.get("category_type")
        if cat == "table":
            html = b.get("html") or ""
            conv = html_table_to_markdown(html)
            if not conv.markdown.strip():
                info["reason"] = "table_without_html"
                return None, info
            info["has_table"] = True
            info["merged_cells"] = info["merged_cells"] or conv.merged_cells
            parts.append(conv.markdown.strip())
            continue
        if cat == "equation_isolated":
            latex = str(b.get("latex") or "").strip()
            if latex:
                info["has_formula"] = True
                parts.append(latex if latex.startswith("$$") else f"$$\n{latex}\n$$")
            continue
        text = merged_text.get(anno_id, b.get("text"))
        if not isinstance(text, str) or not text.strip():
            continue
        text = _text_norm(text)
        if not text:
            continue
        if cat == "title":
            text = "# " + text.strip("#").strip()
        parts.append(text)
    truth = "\n\n".join(parts).strip()
    if len(truth) < MIN_TRUTH_CHARS:
        info["reason"] = "truth_too_short"
        return None, info
    return truth + "\n", info


def page_tags(attrs: dict[str, Any], info: dict[str, Any]) -> list[str]:
    tags = {"omnidocbench", "fetch-required"}
    for key, prefix in (
        ("data_source", ""),
        ("language", "lang-"),
        ("layout", "layout-"),
        ("subset", "subset-"),
    ):
        value = attrs.get(key)
        if isinstance(value, str) and value:
            tags.add(prefix + slugify(value))
    for issue in attrs.get("special_issue") or []:
        if isinstance(issue, str) and issue and issue != "None":
            tags.add("issue-" + slugify(issue))
    for flag, tag in (
        ("has_table", "has-table"),
        ("has_formula", "has-formula"),
        ("merged_cells", "merged-cells"),
    ):
        if info.get(flag):
            tags.add(tag)
    return sorted(tags)


@register
class OmniDocBenchAdapter(Adapter):
    """Indexes OmniDocBench into ``benchmark/datasets/omnidocbench/`` and materialises it locally."""

    name = "omnidocbench"
    license = LICENSE
    default_out = "benchmark/datasets/omnidocbench"
    description = (
        "OpenDataLab OmniDocBench: diverse real-world pages (10 document types, English and "
        "Chinese) with reading-order markdown ground truth."
    )
    upstream = Upstream(repo_id=REPO_ID, revision=REVISION, url=f"https://huggingface.co/datasets/{REPO_ID}")

    def _hf(self) -> Any:
        os.environ.setdefault("REQUESTS_CA_BUNDLE", "/root/.ccr/ca-bundle.crt")
        os.environ.setdefault("SSL_CERT_FILE", os.environ["REQUESTS_CA_BUNDLE"])
        try:
            import huggingface_hub
        except ImportError as exc:  # pragma: no cover - environment problem, not logic
            raise SystemExit("huggingface_hub is required: pip install huggingface_hub") from exc
        return huggingface_hub

    def _fetch(self, cache_dir: Path, repo_path: str) -> Path:
        hub = self._hf()
        return Path(
            hub.hf_hub_download(
                REPO_ID, repo_path, repo_type="dataset", revision=REVISION, cache_dir=str(cache_dir)
            )
        )

    def download(self, cache_dir: Path) -> Path:
        """Fetch ``OmniDocBench.json`` (42 MB) at the pinned revision; images come per page."""
        cache_dir.mkdir(parents=True, exist_ok=True)
        path = self._fetch(cache_dir, JSON_FILE)
        self._fetch(cache_dir, "README.md")
        self.log(f"annotations {path}")
        return path.parent

    def _annotations(self, cache_dir: Path) -> list[dict[str, Any]]:
        pattern = f"datasets--{REPO_ID.replace('/', '--')}/snapshots/{REVISION}/{JSON_FILE}"
        matches = (
            [cache_dir / pattern]
            if (cache_dir / pattern).is_file()
            else sorted(cache_dir.glob(f"**/{pattern}"))
        )
        if not matches:
            raise SystemExit(f"no {JSON_FILE} under {cache_dir}; run without --no-download")
        data = json.loads(matches[0].read_text(encoding="utf-8"))
        if not isinstance(data, list):
            raise SystemExit(f"{JSON_FILE}: expected a list of pages")
        return data

    # -- selection -----------------------------------------------------------------------------

    def _select(self, pages: list[dict[str, Any]], rng: random.Random) -> list[dict[str, Any]]:
        """Round-robin over data sources; inside a source, the language furthest below its share.

        Upstream is roughly half English, half Simplified Chinese, with a tail of mixed and
        Traditional Chinese pages; :data:`LANGUAGE_SHARE` keeps the subset close to that.
        """
        by_source: dict[str, dict[str, list[dict[str, Any]]]] = defaultdict(lambda: defaultdict(list))
        for p in sorted(pages, key=lambda p: p["image_path"]):
            by_source[p["attrs"].get("data_source", "")][p["attrs"].get("language", "")].append(p)
        for langs in by_source.values():
            for name in sorted(langs):
                rng.shuffle(langs[name])
        taken: Counter[str] = Counter()
        out: list[dict[str, Any]] = []
        while any(any(b for b in langs.values()) for langs in by_source.values()):
            for source in sorted(by_source):
                langs = by_source[source]
                available = sorted(n for n, b in langs.items() if b)
                if not available:
                    continue
                share = LANGUAGE_SHARE
                pick = min(available, key=lambda n: (taken[n] / share.get(n, 0.01), n))
                taken[pick] += 1
                out.append(langs[pick].pop(0))
        return out

    # -- build ---------------------------------------------------------------------------------

    def build(self, out_dir: Path, limit: Optional[int] = None, seed: int = 1234) -> Manifest:
        cache_dir = self.cache_dir
        raw_pages = self._annotations(cache_dir)
        rng = random.Random(seed)
        want = SUBSET_SIZE if limit is None else min(limit, SUBSET_SIZE)

        usable: list[dict[str, Any]] = []
        for page in raw_pages:
            info_page = page.get("page_info") or {}
            truth, info = page_truth(page)
            if truth is None:
                self.bump(f"pages_skipped_{info.get('reason', 'unknown')}")
                continue
            usable.append(
                {
                    "image_path": info_page["image_path"],
                    "attrs": info_page.get("page_attribute") or {},
                    "truth": truth,
                    "info": info,
                }
            )
        self.stats["upstream_pages"] = len(raw_pages)
        self.stats["convertible_pages"] = len(usable)

        docs_dir, truth_dir = out_dir / "docs", out_dir / "truth"
        for d in (docs_dir, truth_dir):
            if d.is_dir():
                shutil.rmtree(d)
            d.mkdir(parents=True, exist_ok=True)
        (out_dir / ".gitignore").write_text(
            "# OmniDocBench is research-only / non-commercial: images and derived truth are fetched\n"
            "# by `python -m benchmark.adapters omnidocbench`, never committed.\n/docs/\n/truth/\n",
            encoding="utf-8",
        )

        committed: list[Doc] = []
        fetched = 0
        for entry in self._select(usable, rng):
            if len(committed) >= want:
                break
            repo_path = f"images/{entry['image_path']}"
            src = self._fetch(cache_dir, repo_path)
            size = src.stat().st_size
            if size > MAX_IMAGE_BYTES:
                self.bump("skipped_too_large")
                continue
            stem = Path(entry["image_path"]).stem
            doc_id = slugify(stem)
            suffix = Path(entry["image_path"]).suffix.lower()
            dest = docs_dir / f"{doc_id}{suffix}"
            if dest.exists():
                self.bump("skipped_duplicate_id")
                continue
            shutil.copyfile(src, dest)
            fetched += size
            truth_bytes = entry["truth"].encode("utf-8")
            (truth_dir / f"{doc_id}.md").write_bytes(truth_bytes)
            attrs = entry["attrs"]
            committed.append(
                Doc(
                    id=doc_id,
                    file=f"docs/{dest.name}",
                    truth=f"truth/{doc_id}.md",
                    pages=1,
                    category=slugify(str(attrs.get("data_source") or "unknown")),
                    tags=page_tags(attrs, entry["info"]),
                    kind=KIND_TRANSCRIPT,
                    source_id=entry["image_path"],
                    upstream_path=repo_path,
                    sha256=sha256_file(dest),
                    truth_sha256=sha256_bytes(truth_bytes),
                    license=LICENSE,
                    attribution=ATTRIBUTION,
                )
            )

        self.stats["committed_docs"] = len(committed)
        self.stats["fetched_bytes_not_committed"] = human_size(fetched)
        self.stats["by_category"] = Counter(d.category for d in committed)
        self.stats["by_language"] = Counter(
            next((t for t in d.tags if t.startswith("lang-")), "?") for d in committed
        )

        manifest = Manifest(
            name="omnidocbench",
            version="1.0.0",
            description=(
                "Curated subset of OpenDataLab OmniDocBench: real-world pages across 10 document "
                "types in English and Chinese, with reading-order markdown truth (kind=transcript). "
                "Index only — run `python -m benchmark.adapters omnidocbench` to fetch the images "
                "and generate the truth before benchmarking."
            ),
            license=LICENSE,
            documents=committed,
            generator="benchmark/adapters/omnidocbench.py",
            upstream=self.upstream,
            attribution=ATTRIBUTION,
            notes=[
                f"Upstream revision {REVISION} of {REPO_ID}; truth construction follows "
                f"tools/json2md.py at {SCORER_REVISION}.",
                "docs/ and truth/ are git-ignored: the data is research-only / non-commercial and is "
                "not redistributed. `sha256` and `truth_sha256` pin what a local build must produce.",
                "Until fetched, a run reports every document here as an error (file not found), "
                "and the dataset sha256 in the result covers only the manifest.",
                "Headers, footers, page numbers, page footnotes, abandoned regions and figures are "
                "excluded from the truth, as in OmniDocBench's own text matching.",
                "Tables are GitHub pipe tables converted from upstream HTML; merged cells are "
                "flattened (tag `merged-cells`). Display formulas are kept as $$…$$ LaTeX "
                "(tag `has-formula`), which a text metric compares literally.",
            ],
        )
        write_manifest(out_dir / "manifest.json", manifest)
        write_json(
            out_dir / "conversion-stats.json",
            {
                k: (dict(sorted(v.items())) if isinstance(v, Counter) else v)
                for k, v in sorted(self.stats.items())
            },
        )
        self.log(f"indexed {len(committed)} pages ({human_size(fetched)} fetched locally, not committed)")
        return manifest
