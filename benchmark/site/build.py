#!/usr/bin/env python3
"""Assemble the static PuffinParse benchmark results site.

Reads the committed benchmark artefacts (`benchmark/results/*.json`, the saved model
outputs under `benchmark/results/outputs/`, and the datasets under `benchmark/datasets/`)
and writes a completely static, dependency-free site into `benchmark/site/dist/`:

    index.html  app.js  styles.css        copied from benchmark/site/src/
    tokens.css                            the PuffinParse design tokens (website/assets/)
    data/index.json                       every run + dataset info + model summaries + labels
    data/runs/<run_id>.json               the full result file for a run
    data/outputs/<run_id>/<model>/<doc>.md   each model's markdown output per document
    data/datasets/<name>/manifest.json    manifest (+ `preview`/`previews`, `title`, labels)
    data/datasets/<name>/truth/<doc>.md   ground truth markdown
    data/datasets/<name>/rules/<doc>.json machine-checkable assertions (`kind: "rules"`)
    data/datasets/<name>/docs/<doc>.<ext> the input documents themselves

A manifest may reference files outside its own directory (`combined-v1` points at
`../synthetic-v1/...` and `../parsebench/...`). Those paths are resolved against the
manifest and copied to the matching place under `data/datasets/`, so the very same
relative path keeps working in the browser and no bytes are copied twice.

The script uses only the standard library. Two optional packages make PDF inputs visible
without a client-side PDF library: with **pypdfium2** (plus Pillow) every PDF page, up to
`PREVIEW_MAX_PAGES`, is rendered to `<id>.p<n>.webp`; with Pillow alone only the page-1
image embedded in PuffinParse's own synthetic PDFs is extracted (`<id>.p1.png`). Without
either, PDFs are still copied and the viewer falls back to pdf.js or a download link.

Every document in a copied manifest also gets a human name: `title` ("Headers & footers
3"), `source_label` ("olmOCR-bench") and `category_label`, so no view shows a raw id.

Usage:
    python benchmark/site/build.py [--out DIR] [--base-url /benchmark-results/]
                                   [--home-url /] [--docs-url /docs/]
"""

from __future__ import annotations

import argparse
import io
import json
import os
import re
import shutil
import sys
import zlib
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Optional

SITE_DIR = Path(__file__).resolve().parent
REPO_ROOT = SITE_DIR.parents[1]
SRC_DIR = SITE_DIR / "src"
RESULTS_DIR = REPO_ROOT / "benchmark" / "results"
OUTPUTS_DIR = RESULTS_DIR / "outputs"
DATASETS_DIR = REPO_ROOT / "benchmark" / "datasets"

STATIC_FILES = ("index.html", "app.js", "styles.css")
# The shared PuffinParse design tokens (docs/DESIGN.md), linked before styles.css.
TOKENS_CSS = REPO_ROOT / "website" / "assets" / "tokens.css"

# Human names for the dataset sources (the prefix of a combined-dataset id).
SOURCE_LABELS = {
    "synthetic": "Synthetic",
    "parsebench": "ParseBench",
    "olmocr": "olmOCR-bench",
    "omnidocbench": "OmniDocBench",
    "dpbench": "DP-Bench",
}
# Category names that sentence case alone would get wrong or leave clumsy.
CATEGORY_LABELS = {
    "headers_footers": "Headers & footers",
    "long_tiny_text": "Tiny text",
    "table_tests": "Tables",
    "old_scans": "Old scans",
    "multi_column": "Multi-column",
    "two_column": "Two-column",
    "complex_table": "Complex table",
    "noisy_scan": "Noisy scan",
    "low_res": "Low resolution",
    "key_value": "Key-value",
    "academic_literature": "Academic paper",
    "colorful_textbook": "Textbook",
    "exam_paper": "Exam paper",
    "historical_document": "Historical document",
    "research_report": "Research report",
    "ppt2pdf": "Slides",
    "multipage": "Multi-page",
    "note": "Handwritten note",
}

# pypdfium2 previews: rendered width in pixels, and how many pages of a long PDF to render.
PREVIEW_RENDER_WIDTH = 1000
PREVIEW_MAX_PAGES = 4

# Widest PDF page-1 preview we render; wider pages are downscaled to keep dist small.
PREVIEW_MAX_WIDTH = 1240
# Smallest embedded image we are willing to call "page 1". Third-party PDFs (ParseBench)
# are vector text with a logo or a figure embedded: those are not the page, and showing
# one as the input preview would be a lie, so anything below page size is refused.
PREVIEW_MIN_WIDTH = 700
PREVIEW_MIN_HEIGHT = 900

# `<meta name="puffinparse-base">` tells app.js what to prefix its `data/` URLs with; the
# stylesheet and script tags are rewritten with the same prefix.
BASE_META_RE = re.compile(r'(<meta\s+name="puffinparse-base"\s+content=")[^"]*(")')
# `<meta name="puffinparse-home">` / `puffinparse-docs`: where the header's "PuffinParse" and "Docs"
# links point when the viewer is embedded in the product site (empty = standalone).
LINK_META_RE = re.compile(r'(<meta\s+name="puffinparse-(home|docs)"\s+content=")[^"]*(")')
ASSET_REF_RE = re.compile(r'((?:href|src)=")(tokens\.css|styles\.css|app\.js)(")')


# --------------------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------------------


def source_label(source: str) -> str:
    """`olmocr` -> "olmOCR-bench"; a bare dataset name drops its version (`synthetic-v1`)."""
    return SOURCE_LABELS.get(source) or SOURCE_LABELS.get(re.sub(r"-v\d+$", "", source), source)


def category_label(category: str) -> str:
    """snake_case -> sentence case, with the overrides in `CATEGORY_LABELS`."""
    if category in CATEGORY_LABELS:
        return CATEGORY_LABELS[category]
    words = category.replace("-", " ").replace("_", " ").split()
    return " ".join(words).capitalize() if words else "Uncategorised"


def doc_source(doc_id: str, fallback: str) -> str:
    """The source prefix of a combined-dataset id, else the dataset itself (as in app.js)."""
    cut = doc_id.find("/")
    return doc_id[:cut] if cut > 0 else fallback


def name_documents(documents: list[dict[str, Any]], dataset: str) -> None:
    """Give every document `title`, `ordinal`, `source_label` and `category_label` in place.

    The title is "<Category label> <n>", n counting the documents of the same source and
    category in id order, so it is stable for a manifest and never shows a hash.
    """
    counters: dict[tuple[str, str], int] = {}
    ordinal: dict[str, int] = {}
    for doc in sorted(documents, key=lambda d: str(d.get("id", ""))):
        doc_id = str(doc.get("id", ""))
        key = (doc_source(doc_id, dataset), str(doc.get("category", "")))
        counters[key] = counters.get(key, 0) + 1
        ordinal[doc_id] = counters[key]
    for doc in documents:
        doc_id = str(doc.get("id", ""))
        doc["source_label"] = source_label(doc_source(doc_id, dataset))
        doc["category_label"] = category_label(str(doc.get("category", "")))
        doc["ordinal"] = ordinal.get(doc_id, 1)
        doc["title"] = f"{doc['category_label']} {doc['ordinal']}"


def model_slug(model: str) -> str:
    """Directory-safe form of a model name, matching `bench run --save-outputs`."""
    return model.replace("/", "_")


def human_size(num_bytes: int) -> str:
    size = float(num_bytes)
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1024 or unit == "GB":
            return f"{size:.1f} {unit}" if unit != "B" else f"{int(size)} B"
        size /= 1024
    return f"{size:.1f} GB"


def dir_size(path: Path) -> int:
    return sum(p.stat().st_size for p in path.rglob("*") if p.is_file())


def write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, separators=(",", ":"), sort_keys=False), encoding="utf-8")


def copy_file(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, dst)


def normalize_base(value: str) -> str:
    """`/benchmark-results` -> `/benchmark-results/`; empty -> `./` (page-relative)."""
    raw = (value or "").strip()
    if not raw or raw in (".", "./"):
        return "./"
    if raw.startswith(("http://", "https://")):
        return raw if raw.endswith("/") else raw + "/"
    trimmed = raw.strip("/")
    return f"/{trimmed}/" if trimmed else "/"


def apply_base(html_text: str, base: str) -> str:
    """Point the viewer's own asset and data URLs at `base`. `./` leaves them relative."""
    if base == "./":
        return html_text
    text, found = BASE_META_RE.subn(lambda m: m.group(1) + base + m.group(2), html_text)
    if not found:
        print(
            '  ! index.html has no <meta name="puffinparse-base">: data URLs stay relative', file=sys.stderr
        )
    return ASSET_REF_RE.sub(lambda m: m.group(1) + base + m.group(2) + m.group(3), text)


def apply_links(html_text: str, home: str = "", docs: str = "") -> str:
    """Fill the `puffinparse-home` / `puffinparse-docs` meta tags (HTML-escaped)."""
    values = {"home": home or "", "docs": docs or ""}

    def repl(match: re.Match[str]) -> str:
        value = values[match.group(2)].replace("&", "&amp;").replace('"', "&quot;")
        return match.group(1) + value + match.group(3)

    return LINK_META_RE.sub(repl, html_text)


def within(root: Path, candidate: Path) -> bool:
    try:
        candidate.relative_to(root)
        return True
    except ValueError:
        return False


def normalised(path: Path) -> Path:
    """Collapse `..` segments textually (the paths need not exist yet)."""
    return Path(os.path.normpath(str(path)))


# --------------------------------------------------------------------------------------
# PDF page-1 preview (optional, needs Pillow)
# --------------------------------------------------------------------------------------

_PDF_STREAM_RE = re.compile(rb"\d+\s+0\s+obj\s*<<(.*?)>>\s*stream\r?\n", re.S)


def _first_pdf_image(data: bytes) -> Optional[Any]:
    """Return the first embedded image of a PDF as a PIL image, or None.

    The benchmark PDFs are written by Pillow: every page is one full-page image XObject,
    laid out in page order, so the first image object is page 1. This keeps the preview
    path working with nothing but Pillow installed (Pillow itself cannot open PDFs).
    Third-party PDFs (ParseBench) are mostly vector text: they either yield nothing or a
    logo, which `render_pdf_preview` rejects on size.
    """
    from PIL import Image  # optional dependency, imported lazily

    for match in _PDF_STREAM_RE.finditer(data):
        header = match.group(1)
        if b"/Image" not in header:
            continue
        length_match = re.search(rb"/Length\s+(\d+)", header)
        if not length_match:
            continue
        raw = data[match.end() : match.end() + int(length_match.group(1))]
        if b"/DCTDecode" in header:
            return Image.open(io.BytesIO(raw))
        if b"/FlateDecode" in header:
            width_match = re.search(rb"/Width\s+(\d+)", header)
            height_match = re.search(rb"/Height\s+(\d+)", header)
            if not (width_match and height_match):
                continue
            size = (int(width_match.group(1)), int(height_match.group(1)))
            mode = "RGB" if b"/DeviceRGB" in header else "L"
            try:
                return Image.frombytes(mode, size, zlib.decompress(raw))
            except (zlib.error, ValueError):
                continue
    return None


def render_pdf_preview(pdf_path: Path, out_png: Path) -> bool:
    """Render page 1 of `pdf_path` to `out_png`. Returns False if it could not be done."""
    try:
        from PIL import Image  # noqa: F401  (probe only; the real work is below)
    except ImportError:
        return False
    try:
        image = _first_pdf_image(pdf_path.read_bytes())
        if image is None:
            return False
        if image.width < PREVIEW_MIN_WIDTH or image.height < PREVIEW_MIN_HEIGHT:
            return False  # a logo or a figure, not a rendered page
        if image.width > PREVIEW_MAX_WIDTH:
            height = round(image.height * PREVIEW_MAX_WIDTH / image.width)
            image = image.resize((PREVIEW_MAX_WIDTH, height))
        if image.mode not in ("L", "RGB"):
            image = image.convert("RGB")
        out_png.parent.mkdir(parents=True, exist_ok=True)
        image.save(out_png, format="PNG", optimize=True)
        return True
    except Exception as exc:  # a preview is best-effort, never fatal
        print(f"  ! preview failed for {pdf_path.name}: {exc}", file=sys.stderr)
        return False


def pdf_preview_ext() -> Optional[str]:
    """Extension of pypdfium2 page previews, or None without pypdfium2 and Pillow."""
    try:
        import pypdfium2  # noqa: F401  (optional dependency, probe only)
        from PIL import features
    except ImportError:
        return None
    return ".webp" if features.check("webp") else ".png"


def render_pdf_pages(pdf_path: Path, out_paths: list[Path]) -> int:
    """Render the first `len(out_paths)` pages of a PDF with pypdfium2; returns pages written.

    Each page is rendered `PREVIEW_RENDER_WIDTH` pixels wide and saved in the format its
    extension names (WebP at quality 80 keeps a text page around 60-120 KB).
    """
    try:
        import pypdfium2 as pdfium  # optional dependency, imported lazily
    except ImportError:
        return 0
    written = 0
    try:
        pdf = pdfium.PdfDocument(str(pdf_path))
        try:
            for index, out in enumerate(out_paths[: len(pdf)]):
                page = pdf[index]
                width = page.get_width() or 612.0
                image = page.render(scale=PREVIEW_RENDER_WIDTH / width).to_pil()
                if image.mode not in ("L", "RGB"):
                    image = image.convert("RGB")
                out.parent.mkdir(parents=True, exist_ok=True)
                if out.suffix == ".webp":
                    image.save(out, format="WEBP", quality=80, method=6)
                else:
                    image.save(out, format="PNG", optimize=True)
                written += 1
        finally:
            pdf.close()
    except Exception as exc:  # a preview is best-effort, never fatal
        print(f"  ! page preview failed for {pdf_path.name}: {exc}", file=sys.stderr)
    return written


def headline_score(summary: dict[str, Any]) -> float:
    """The number a model is ranked by: `summary.headline` when the run has one, else `overall`.

    `headline` may be a bare number or an object carrying `score` / `overall` / `value`; the
    viewer applies the same rule (see `headlineOf` in app.js).
    """
    head = summary.get("headline")
    value: Any = head
    if isinstance(head, dict):
        value = next(
            (head[k] for k in ("score", "overall", "value") if isinstance(head.get(k), (int, float))),
            None,
        )
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        overall = summary.get("overall")
        if value <= 1.0 and isinstance(overall, (int, float)) and overall > 1.0:
            return float(value) * 100.0  # a 0..1 headline next to a 0..100 overall
        return float(value)
    overall = summary.get("overall")
    return float(overall) if isinstance(overall, (int, float)) else 0.0


# --------------------------------------------------------------------------------------
# build steps
# --------------------------------------------------------------------------------------


def load_runs() -> list[dict[str, Any]]:
    """Load every result file, newest first."""
    runs: list[dict[str, Any]] = []
    for path in sorted(RESULTS_DIR.glob("*.json")):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            print(f"  ! skipping {path.name}: {exc}", file=sys.stderr)
            continue
        if not isinstance(data, dict) or "run_id" not in data or "models" not in data:
            print(f"  ! skipping {path.name}: not a benchmark result file", file=sys.stderr)
            continue
        data["_source"] = path
        runs.append(data)
    runs.sort(key=lambda r: (str(r.get("created_at") or ""), str(r.get("run_id"))), reverse=True)
    return runs


def copy_outputs(run: dict[str, Any], dist: Path) -> tuple[int, dict[str, dict[str, list[str]]]]:
    """Copy the saved per-document outputs for one run.

    Every document has a `<doc>.md` (the markdown that was scored). When `bench run
    --save-outputs` also saved the unified `ParseResponse` as `<doc>.json` next to it, that is
    copied too: its `pages[].blocks[].bbox` is what the viewer draws as layout overlays.

    Document ids may contain `/` (the combined dataset prefixes them with their source),
    which `bench run --save-outputs` turns into a subdirectory; the layout is mirrored.

    Returns the number of files copied and, per model slug, which documents have a unified
    JSON (`json`) and which have no markdown at all (`missing`), so the viewer never has to
    probe for a file that is not there.
    """
    run_id = str(run["run_id"])
    src_root = OUTPUTS_DIR / run_id
    inventory: dict[str, dict[str, list[str]]] = {}
    if not src_root.is_dir():
        print(f"  ! no saved outputs for run {run_id} (looked in {src_root})", file=sys.stderr)
        return 0, inventory
    copied = 0
    for model in run.get("models", []):
        slug = model_slug(str(model["model"]))
        src_dir = src_root / slug
        entry: dict[str, list[str]] = {"json": [], "missing": []}
        inventory[slug] = entry
        if not src_dir.is_dir():
            print(f"  ! no outputs for {model['model']} in run {run_id}", file=sys.stderr)
            entry["missing"] = [str(doc["id"]) for doc in model.get("docs", [])]
            continue
        for doc in model.get("docs", []):
            doc_id = str(doc["id"])
            for ext in (".md", ".json"):
                rel = f"{doc_id}{ext}"
                src = normalised(src_dir / rel)
                if not (within(src_dir, src) and src.is_file()):
                    if ext == ".md":
                        entry["missing"].append(doc_id)
                    continue
                copy_file(src, dist / "data" / "outputs" / run_id / slug / rel)
                copied += 1
                if ext == ".json":
                    entry["json"].append(doc_id)
    return copied, inventory


def attach_pdf_previews(
    doc: dict[str, Any],
    rel_file: str,
    input_src: Path,
    place: Callable[[str], Optional[tuple[Path, Path]]],
    seen: set[Path],
) -> int:
    """Set `preview` (page 1) and `previews` (every rendered page) on a PDF document.

    Pages are rendered next to the PDF with pypdfium2 when it is installed; otherwise page 1
    is extracted with Pillow alone. Images already written for another manifest are reused.
    Returns the number of images newly written.
    """
    stem = rel_file[: -len(input_src.suffix)]
    ext = pdf_preview_ext()
    if ext:
        count = max(1, min(int(doc.get("pages") or 1), PREVIEW_MAX_PAGES))
        rels = [f"{stem}.p{n}{ext}" for n in range(1, count + 1)]
        spots = [place(rel) for rel in rels]
        dsts = [spot[1] for spot in spots if spot is not None]
        if len(dsts) == len(rels):
            fresh = 0
            if dsts[0] in seen:
                done = sum(1 for dst in dsts if dst in seen)
            else:
                done = fresh = render_pdf_pages(input_src, dsts)
                seen.update(dsts[:done])
            if done:
                doc["previews"] = rels[:done]
                doc["preview"] = rels[0]
                return fresh
    preview_rel = f"{stem}.p1.png"
    preview_spot = place(preview_rel)
    if preview_spot is None:
        return 0
    preview_dst = preview_spot[1]
    if preview_dst in seen:  # rendered while copying another dataset
        doc["preview"] = preview_rel
        doc["previews"] = [preview_rel]
        return 0
    if render_pdf_preview(input_src, preview_dst):
        doc["preview"] = preview_rel
        doc["previews"] = [preview_rel]
        seen.add(preview_dst)
        return 1
    return 0


def copy_dataset(name: str, dist: Path, seen: set[Path]) -> dict[str, Any]:
    """Copy a dataset's manifest, truth/rule files and inputs. Returns the dataset summary.

    Every per-document path is resolved against the manifest's own directory, so a
    manifest that points outside itself (`combined-v1` -> `../parsebench/...`) works; the
    destination keeps the same relative shape under `data/datasets/`, which both mirrors
    the repository layout and lets the browser resolve the identical relative URL.
    `seen` holds the destinations written so far, so shared files (and their PDF
    previews) are copied exactly once even when several datasets reference them.
    """
    dataset_dir = (DATASETS_DIR / name).resolve()
    manifest_path = dataset_dir / "manifest.json"
    if not manifest_path.is_file():
        print(f"  ! dataset {name} not found at {dataset_dir}", file=sys.stderr)
        return {"name": name, "missing": True, "documents": 0, "categories": []}

    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"  ! dataset {name}: unreadable manifest ({exc})", file=sys.stderr)
        return {"name": name, "missing": True, "documents": 0, "categories": []}

    data_root = (dist / "data").resolve()
    out_dir = data_root / "datasets" / name
    inputs = truths = rules = previews = 0
    kinds: dict[str, int] = {}

    def place(rel: str) -> Optional[tuple[Path, Path]]:
        """(source, destination) for a manifest-relative path, or None if out of bounds."""
        if not rel:
            return None
        src = normalised(dataset_dir / rel)
        dst = normalised(out_dir / rel)
        if not within(DATASETS_DIR.resolve(), src) or not within(data_root, dst):
            print(f"  ! dataset {name}: path escapes the site: {rel}", file=sys.stderr)
            return None
        return src, dst

    def take(rel: str, label: str, doc_id: str, quiet: bool = False) -> bool:
        spot = place(rel)
        if spot is None:
            return False
        src, dst = spot
        if dst in seen:
            return True
        if not src.is_file():
            if not quiet:
                print(f"  ! missing {label} for {doc_id}: {src}", file=sys.stderr)
            return False
        copy_file(src, dst)
        seen.add(dst)
        return True

    for doc in manifest.get("documents", []):
        doc_id = str(doc.get("id", "?"))
        kind = str(doc.get("kind", "transcript") or "transcript")
        kinds[kind] = kinds.get(kind, 0) + 1

        rel_truth = str(doc.get("truth", "") or "")
        rel_rules = str(doc.get("rules", "") or "")
        rel_file = str(doc.get("file", "") or "")

        # `kind: "rules"` documents carry assertions instead of a reference transcript;
        # their `truth` is an empty string, so do not complain about it.
        if rel_truth and take(rel_truth, "truth", doc_id):
            truths += 1
        elif not rel_truth and kind != "rules":
            print(f"  ! missing truth for {doc_id}", file=sys.stderr)
        if rel_rules and take(rel_rules, "rules", doc_id):
            rules += 1

        spot = place(rel_file)
        if spot is None or not spot[0].is_file():
            print(f"  ! missing input for {doc_id}: {rel_file or '(no file)'}", file=sys.stderr)
            continue
        input_src, input_dst = spot
        if input_dst not in seen:
            copy_file(input_src, input_dst)
            seen.add(input_dst)
        inputs += 1

        if input_src.suffix.lower() == ".pdf":
            # Browsers cannot show a PDF inline, so render its pages next to it.
            previews += attach_pdf_previews(doc, rel_file, input_src, place, seen)
        else:
            doc["preview"] = rel_file

    name_documents(manifest.get("documents", []), str(manifest.get("name", name)))
    write_json(out_dir / "manifest.json", manifest)
    print(
        f"  dataset {name} v{manifest.get('version', '?')}: "
        f"{inputs} inputs, {truths} truth files, {rules} rule files, {previews} PDF previews"
    )
    return {
        "name": manifest.get("name", name),
        "version": manifest.get("version"),
        "description": manifest.get("description"),
        "license": manifest.get("license"),
        "generator": manifest.get("generator"),
        "sources": manifest.get("sources"),
        "attribution": manifest.get("attribution"),
        "documents": len(manifest.get("documents", [])),
        "kinds": kinds,
        "categories": sorted({str(d.get("category", "")) for d in manifest.get("documents", [])}),
    }


def run_index_entry(
    run: dict[str, Any], outputs: Optional[dict[str, dict[str, list[str]]]] = None
) -> dict[str, Any]:
    """The trimmed record for `data/index.json`: everything the leaderboard needs."""
    models = []
    for model in run.get("models", []):
        models.append(
            {
                "model": model["model"],
                "slug": model_slug(str(model["model"])),
                "summary": model.get("summary", {}),
            }
        )
    models.sort(key=lambda m: headline_score(m["summary"]), reverse=True)
    categories = sorted(
        {str(d.get("category", "")) for m in run.get("models", []) for d in m.get("docs", [])}
    )
    return {
        "run_id": run["run_id"],
        "created_at": run.get("created_at"),
        "puffinparse_version": run.get("puffinparse_version") or run.get("liteocr_version"),
        "scorer_version": run.get("scorer_version"),
        "dataset": run.get("dataset", {}),
        "normalize": run.get("normalize", {}),
        "categories": categories,
        "models": models,
        "outputs": outputs or {},
        "file": f"data/runs/{run['run_id']}.json",
    }


def run_labels(runs: list[dict[str, Any]]) -> dict[str, dict[str, str]]:
    """Human names for every source and category a run mentions (the leaderboard tables)."""
    sources: dict[str, str] = {}
    categories: dict[str, str] = {}
    for run in runs:
        fallback = str(run.get("dataset", {}).get("name", "dataset"))
        for model in run.get("models", []):
            for doc in model.get("docs", []):
                src = doc_source(str(doc.get("id", "")), fallback)
                sources.setdefault(src, source_label(src))
                cat = str(doc.get("category", ""))
                categories.setdefault(cat, category_label(cat))
    return {"sources": sources, "categories": categories}


def build(dist: Path, base: str = "./", home: str = "", docs: str = "") -> int:
    if not SRC_DIR.is_dir():
        print(f"error: missing source directory {SRC_DIR}", file=sys.stderr)
        return 1

    if dist.exists():
        shutil.rmtree(dist)
    dist.mkdir(parents=True)

    print(f"Building PuffinParse benchmark site -> {dist} (base {base})")

    for name in STATIC_FILES:
        src = SRC_DIR / name
        if not src.is_file():
            print(f"error: missing {src}", file=sys.stderr)
            return 1
        if name == "index.html":
            page = apply_links(apply_base(src.read_text(encoding="utf-8"), base), home, docs)
            (dist / name).write_text(page, encoding="utf-8")
        else:
            copy_file(src, dist / name)
    if TOKENS_CSS.is_file():
        copy_file(TOKENS_CSS, dist / "tokens.css")
    else:
        print(f"  ! {TOKENS_CSS} not found: the viewer renders without its tokens", file=sys.stderr)
    print(f"  static: {', '.join(STATIC_FILES)}, tokens.css")

    runs = load_runs()
    if not runs:
        print(f"error: no benchmark result files in {RESULTS_DIR}", file=sys.stderr)
        return 1

    datasets: dict[str, dict[str, Any]] = {}
    seen: set[Path] = set()
    total_outputs = 0
    index_runs = []

    for run in runs:
        source = run.pop("_source")
        run_id = str(run["run_id"])
        model_names = [str(m["model"]) for m in run.get("models", [])]
        doc_count = len(run["models"][0].get("docs", [])) if run.get("models") else 0
        print(f"  run {run_id} ({source.name}): {len(model_names)} models x {doc_count} documents")

        write_json(dist / "data" / "runs" / f"{run_id}.json", run)
        copied, inventory = copy_outputs(run, dist)
        total_outputs += copied

        dataset_name = str(run.get("dataset", {}).get("name", ""))
        if dataset_name and dataset_name not in datasets:
            datasets[dataset_name] = copy_dataset(dataset_name, dist, seen)

        index_runs.append(run_index_entry(run, inventory))

    write_json(
        dist / "data" / "index.json",
        {
            "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "repo": "https://github.com/ajinkyashejul/puffinparse",
            "base": base,
            "datasets": datasets,
            "labels": run_labels(runs),
            "runs": index_runs,
        },
    )

    files = sum(1 for p in dist.rglob("*") if p.is_file())
    print(
        f"\nDone: {len(index_runs)} run(s), {len(datasets)} dataset(s), "
        f"{total_outputs} model output file(s)\n"
        f"      {files} files, {human_size(dir_size(dist))} in {dist}\n"
        f"      preview locally: python -m http.server -d {dist} 8000"
    )
    return 0


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--out",
        default=str(SITE_DIR / "dist"),
        help="output directory for the built site (default: benchmark/site/dist)",
    )
    parser.add_argument(
        "--base-url",
        default="",
        help=(
            "URL prefix the viewer is served from, e.g. /benchmark-results/. "
            "Default: page-relative URLs, which work at any path served with a trailing slash."
        ),
    )
    parser.add_argument(
        "--home-url",
        default="",
        help="URL of the product site the viewer is embedded in (header 'PuffinParse' link)",
    )
    parser.add_argument(
        "--docs-url",
        default="",
        help="URL of the documentation (header 'Docs' link)",
    )
    args = parser.parse_args(argv)
    return build(Path(args.out).resolve(), normalize_base(args.base_url), args.home_url, args.docs_url)


if __name__ == "__main__":
    raise SystemExit(main())
