# LiteOCR benchmark site

A static, dependency-free viewer for the open LiteOCR benchmark: every run, every model, every
document, every score — plus the input, the ground truth and each model's raw output side by side,
so anyone can check a number instead of trusting it.

It is built from the artefacts that are already committed to this repository
(`benchmark/results/*.json`, `benchmark/results/outputs/`, `benchmark/datasets/`) and deployed to
GitHub Pages by [`.github/workflows/pages.yml`](../../.github/workflows/pages.yml).

```
benchmark/site/
  build.py        assembles dist/ from the committed benchmark artefacts (stdlib only)
  src/            zero-build vanilla-JS viewer: index.html, app.js, styles.css
  dist/           build output (git-ignored)
```

## Build and preview locally

```bash
python benchmark/site/build.py
python -m http.server -d benchmark/site/dist 8000   # then open http://localhost:8000
```

`--out DIR` writes somewhere else (CI uses `--out site-dist`). The build needs nothing but Python
3.9+. If **Pillow** is importable (`pip install pillow`) the first page of every PDF input is also
rendered to a preview PNG; without it the PDFs are still copied and the site links to them.

The site must be **served over HTTP** — it fetches its data with relative URLs, which browsers
block on `file://`.

## What `build.py` produces

Everything the front-end needs lives under `<out>/data/`, so the viewer in `src/` can be swapped
for any other front-end without changing the build:

| Path | Contents |
|---|---|
| `index.html`, `app.js`, `styles.css` | copied verbatim from `src/` |
| `data/index.json` | every run (newest first) with its dataset info, normalisation options, category list and per-model summaries; plus a `datasets` map with name, version, licence and description |
| `data/runs/<run_id>.json` | the complete result file for one run: per-model, per-document metrics, latency and cost |
| `data/outputs/<run_id>/<model>/<doc_id>.md` | each model's raw markdown output per document (`/` in a model name becomes `_`, matching `bench run --save-outputs`) |
| `data/datasets/<name>/manifest.json` | the dataset manifest, with a `preview` path added to each document |
| `data/datasets/<name>/truth/<doc_id>.md` | ground-truth markdown |
| `data/datasets/<name>/docs/<doc_id>.<ext>` | the input documents (PNG/PDF copied as-is) |
| `data/datasets/<name>/docs/<doc_id>.p1.png` | page-1 preview of a PDF input, when Pillow is available |

`documents[].preview` in the copied manifest always points at something a browser can display
inline (the PNG itself, or the rendered first page of a PDF); it is absent when no preview could be
made. For `synthetic-v1` the whole directory is about 11 MB, dominated by the input images.

`build.py` picks up any `benchmark/results/*.json` that looks like a result file, so adding a new
run is a matter of committing the result JSON plus its `results/outputs/<run_id>/` directory — no
site change needed.

## The viewer

`src/` is a single-page app in plain ES2018: no framework, no build step, no CDN, no external
resource of any kind. Hash routing:

- `#/leaderboard?run=…` — sortable model table (overall, char similarity, CER, WER, word F1, order,
  table, p50/p95 latency, ms/page, $/1k pages, failures), a per-category heatmap, the methodology
  summary and the exact commands to reproduce the run.
- `#/documents?run=…&cat=…` — every document in the run's dataset with a per-model score column.
- `#/doc/<id>?run=…&model=…` — the input preview, the ground truth as markdown *source*, the
  selected model's output with a word-level LCS diff against the truth (green = added by the model,
  red = missing from it), that document's metrics, latency and cost, and a "verify it yourself"
  panel with the `liteocr parse` / `liteocr bench score` commands for exactly that pair.

Responsive to ~400px, light and dark via `prefers-color-scheme`, sortable tables with `aria-sort`.

> A richer React front-end (Extend UI) is planned under `benchmark/site/app/`. It consumes the same
> `data/` directory; the vanilla app in `src/` stays as the zero-build fallback.

## Deployment

`.github/workflows/pages.yml` runs on every push to `main` (and on manual dispatch): it installs
Pillow, runs `python benchmark/site/build.py --out site-dist`, then uploads and deploys that
directory with `actions/configure-pages@v5`, `actions/upload-pages-artifact@v3` and
`actions/deploy-pages@v4`.

**A maintainer must enable Pages once**: repository *Settings → Pages → Source: GitHub Actions*.
Until then the build succeeds and the deploy step fails.
