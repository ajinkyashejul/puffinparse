# LiteOCR benchmark site

A static, dependency-free viewer for the open LiteOCR benchmark: every run, every model, every
document, every score — plus the input, the ground truth and each model's raw output side by side,
so anyone can check a number instead of trusting it.

It is built from the artefacts that are already committed to this repository
(`benchmark/results/*.json`, `benchmark/results/outputs/`, `benchmark/datasets/`) and published as
part of the product site at **`/benchmark-results/`** — `website/build.py` invokes this script (see
*Deployment* below).

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

| Flag | Default | What it does |
|---|---|---|
| `--out DIR` | `benchmark/site/dist` | Output directory. Wiped and rebuilt on every run. |
| `--base-url PATH` | page-relative | URL prefix the viewer is served from, e.g. `/benchmark-results/`. It is written into `<meta name="liteocr-base">` and onto the `styles.css` / `app.js` tags; `app.js` prefixes every `data/` URL with it. The default keeps every URL relative to the page, which works at any path served with a trailing slash. |

The build needs nothing but Python 3.9+. If **Pillow** is importable (`pip install pillow`) the
first page of every PDF input is also rendered to a preview PNG; without it the PDFs are still
copied and the site links to them.

The site must be **served over HTTP** — it fetches its data with relative URLs, which browsers
block on `file://`. Routing is hash based, so no rewrite rules are needed either.

## What `build.py` produces

Everything the front-end needs lives under `<out>/data/`, so the viewer in `src/` can be swapped
for any other front-end without changing the build:

| Path | Contents |
|---|---|
| `index.html`, `app.js`, `styles.css` | copied from `src/` (only `index.html` changes, and only when `--base-url` is given) |
| `data/index.json` | every run (newest first) with its dataset info, normalisation options, category list and per-model summaries; plus a `datasets` map with name, version, licence and description |
| `data/runs/<run_id>.json` | the complete result file for one run: per-model, per-document metrics, latency and cost |
| `data/outputs/<run_id>/<model>/<doc_id>.md` | each model's raw markdown output per document (`/` in a model name becomes `_`, matching `bench run --save-outputs`) |
| `data/datasets/<name>/manifest.json` | the dataset manifest, with a `preview` path added to each document |
| `data/datasets/<name>/truth/<doc_id>.md` | ground-truth markdown |
| `data/datasets/<name>/rules/<doc_id>.json` | the assertions of a `kind: "rules"` document |
| `data/datasets/<name>/docs/<doc_id>.<ext>` | the input documents (PNG/PDF copied as-is) |
| `data/datasets/<name>/docs/<doc_id>.p1.png` | page-1 preview of a PDF input, when Pillow is available |

**Manifests that point outside themselves.** `combined-v1` copies no bytes: every document is a
`../synthetic-v1/…` or `../parsebench/…` path relative to its manifest. The build resolves those
against the manifest and writes each file to the matching place under `data/datasets/`, so the same
relative URL resolves in the browser, and a file (or its rendered PDF preview) shared by two
datasets is written exactly once. Anything that would escape `data/` is refused.

**Documents with `kind: "rules"`** (the ParseBench text split) have no markdown truth: `truth` is
empty and `rules` names an assertion file, which is copied alongside the inputs. The viewer shows
the rule list where a transcript document shows its ground truth, and reports `rule_pass_rate`
wherever a result carries it. Anything the build cannot resolve — a missing input, truth, rule file
or preview — is reported on stderr and skipped; it never fails the build.

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
  panel with the `liteocr parse` / `liteocr bench score` commands for exactly that pair. For a
  rule-scored document the truth panel becomes the list of assertions (type and text), the metrics
  show the pass rate and `passed / total`, there is no diff to draw, and the second command is the
  `liteocr bench run --filter <id>` that re-checks the rules.

Responsive to ~400px, light and dark via `prefers-color-scheme`, sortable tables with `aria-sort`.

> A richer React front-end (Extend UI) is planned under `benchmark/site/app/`. It consumes the same
> `data/` directory; the vanilla app in `src/` stays as the zero-build fallback.

## Deployment

The viewer ships with the product site, at
[`https://liteocr.vercel.app/benchmark-results/`](https://liteocr.vercel.app/benchmark-results/).
`website/build.py` imports this script and runs it with
`--out <dist>/benchmark-results --base-url /benchmark-results/` as part of every build
(`--no-benchmark` skips it). Nothing in `vercel.json` is specific to the viewer beyond `--with
pillow` in the build command, which is what renders the PDF page-1 previews.

`.github/workflows/pages.yml` builds the *whole* site the same way, so GitHub Pages would mirror
Vercel — landing page, `/docs/` and `/benchmark-results/` — rather than publishing the viewer on
its own. **A maintainer must enable Pages once**: repository *Settings → Pages → Source: GitHub
Actions*. Until then the build succeeds and the deploy step fails.
