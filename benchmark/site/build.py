#!/usr/bin/env python3
"""Assemble the static LiteOCR benchmark results site.

Reads the committed benchmark artefacts (`benchmark/results/*.json`, the saved model
outputs under `benchmark/results/outputs/`, and the datasets under `benchmark/datasets/`)
and writes a completely static, dependency-free site into `benchmark/site/dist/`:

    index.html  app.js  styles.css        copied from benchmark/site/src/
    data/index.json                       every run + dataset info + model summaries
    data/runs/<run_id>.json               the full result file for a run
    data/outputs/<run_id>/<model>/<doc>.md   each model's markdown output per document
    data/datasets/<name>/manifest.json    manifest (+ a `preview` path per document)
    data/datasets/<name>/truth/<doc>.md   ground truth markdown
    data/datasets/<name>/rules/<doc>.json machine-checkable assertions (`kind: "rules"`)
    data/datasets/<name>/docs/<doc>.<ext> the input documents themselves

A manifest may reference files outside its own directory (`combined-v1` points at
`../synthetic-v1/...` and `../parsebench/...`). Those paths are resolved against the
manifest and copied to the matching place under `data/datasets/`, so the very same
relative path keeps working in the browser and no bytes are copied twice.

The script uses only the standard library. Pillow is optional: when it is importable the
first page of each PDF input is rendered to `<id>.p1.png` so the browser can show a
preview; without it PDFs are still copied and the site links to them instead.

Usage:
    python benchmark/site/build.py [--out DIR] [--base-url /benchmark-results/]
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
from typing import Any, Optional

SITE_DIR = Path(__file__).resolve().parent
REPO_ROOT = SITE_DIR.parents[1]
SRC_DIR = SITE_DIR / "src"
RESULTS_DIR = REPO_ROOT / "benchmark" / "results"
OUTPUTS_DIR = RESULTS_DIR / "outputs"
DATASETS_DIR = REPO_ROOT / "benchmark" / "datasets"

STATIC_FILES = ("index.html", "app.js", "styles.css")

# Widest PDF page-1 preview we render; wider pages are downscaled to keep dist small.
PREVIEW_MAX_WIDTH = 1240
# Smallest embedded image we are willing to call "page 1". Third-party PDFs (ParseBench)
# are vector text with a logo or a figure embedded: those are not the page, and showing
# one as the input preview would be a lie, so anything below page size is refused.
PREVIEW_MIN_WIDTH = 700
PREVIEW_MIN_HEIGHT = 900

# `<meta name="liteocr-base">` tells app.js what to prefix its `data/` URLs with; the
# stylesheet and script tags are rewritten with the same prefix.
BASE_META_RE = re.compile(r'(<meta\s+name="liteocr-base"\s+content=")[^"]*(")')
ASSET_REF_RE = re.compile(r'((?:href|src)=")(styles\.css|app\.js)(")')


# --------------------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------------------


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
        print('  ! index.html has no <meta name="liteocr-base">: data URLs stay relative', file=sys.stderr)
    return ASSET_REF_RE.sub(lambda m: m.group(1) + base + m.group(2) + m.group(3), text)


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


def copy_outputs(run: dict[str, Any], dist: Path) -> int:
    """Copy the saved per-document markdown outputs for one run.

    Document ids may contain `/` (the combined dataset prefixes them with their source),
    which `bench run --save-outputs` turns into a subdirectory; the layout is mirrored.
    """
    run_id = str(run["run_id"])
    src_root = OUTPUTS_DIR / run_id
    if not src_root.is_dir():
        print(f"  ! no saved outputs for run {run_id} (looked in {src_root})", file=sys.stderr)
        return 0
    copied = 0
    for model in run.get("models", []):
        slug = model_slug(str(model["model"]))
        src_dir = src_root / slug
        if not src_dir.is_dir():
            print(f"  ! no outputs for {model['model']} in run {run_id}", file=sys.stderr)
            continue
        for doc in model.get("docs", []):
            rel = f"{doc['id']}.md"
            src = normalised(src_dir / rel)
            if not (within(src_dir, src) and src.is_file()):
                continue
            copy_file(src, dist / "data" / "outputs" / run_id / slug / rel)
            copied += 1
    return copied


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
            # PDFs cannot be shown inline, so render page 1 next to the PDF for preview.
            preview_rel = f"{rel_file[: -len(input_src.suffix)]}.p1.png"
            preview_spot = place(preview_rel)
            if preview_spot is None:
                continue
            preview_dst = preview_spot[1]
            if preview_dst in seen:
                doc["preview"] = preview_rel  # rendered while copying another dataset
            elif render_pdf_preview(input_src, preview_dst):
                doc["preview"] = preview_rel
                seen.add(preview_dst)
                previews += 1
        else:
            doc["preview"] = rel_file

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


def run_index_entry(run: dict[str, Any]) -> dict[str, Any]:
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
    models.sort(key=lambda m: m["summary"].get("overall") or 0.0, reverse=True)
    categories = sorted(
        {str(d.get("category", "")) for m in run.get("models", []) for d in m.get("docs", [])}
    )
    return {
        "run_id": run["run_id"],
        "created_at": run.get("created_at"),
        "liteocr_version": run.get("liteocr_version"),
        "dataset": run.get("dataset", {}),
        "normalize": run.get("normalize", {}),
        "categories": categories,
        "models": models,
        "file": f"data/runs/{run['run_id']}.json",
    }


def build(dist: Path, base: str = "./") -> int:
    if not SRC_DIR.is_dir():
        print(f"error: missing source directory {SRC_DIR}", file=sys.stderr)
        return 1

    if dist.exists():
        shutil.rmtree(dist)
    dist.mkdir(parents=True)

    print(f"Building LiteOCR benchmark site -> {dist} (base {base})")

    for name in STATIC_FILES:
        src = SRC_DIR / name
        if not src.is_file():
            print(f"error: missing {src}", file=sys.stderr)
            return 1
        if name == "index.html":
            (dist / name).write_text(apply_base(src.read_text(encoding="utf-8"), base), encoding="utf-8")
        else:
            copy_file(src, dist / name)
    print(f"  static: {', '.join(STATIC_FILES)}")

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
        total_outputs += copy_outputs(run, dist)

        dataset_name = str(run.get("dataset", {}).get("name", ""))
        if dataset_name and dataset_name not in datasets:
            datasets[dataset_name] = copy_dataset(dataset_name, dist, seen)

        index_runs.append(run_index_entry(run))

    write_json(
        dist / "data" / "index.json",
        {
            "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "repo": "https://github.com/ajinkyashejul/liteocr",
            "base": base,
            "datasets": datasets,
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
    args = parser.parse_args(argv)
    return build(Path(args.out).resolve(), normalize_base(args.base_url))


if __name__ == "__main__":
    raise SystemExit(main())
