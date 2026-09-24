# LiteOCR benchmark site

A static, framework-free viewer for the open LiteOCR benchmark and the place to verify every
claim it makes: every run, every model, every document, every score — with the page itself, each
model's output side by side, a word diff against the truth or a pass/fail checklist of every
assertion, and layout boxes when a unified response was saved.

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
| `--home-url URL` | empty | Where the header's "LiteOCR" link points (`website/build.py` passes the site root). Empty = the viewer's own leaderboard. |
| `--docs-url URL` | empty | Adds a "Docs" link to the header (`website/build.py` passes `/docs/`). |
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
| `data/index.json` | every run (newest first) with its dataset info, normalisation options, `scorer_version` (when the result has one), category list, per-model summaries and an `outputs` inventory (per model slug: which documents have a unified `json`, which have no markdown — `missing` — so the viewer never probes for absent files); plus a `datasets` map with name, version, licence and description |
| `data/runs/<run_id>.json` | the complete result file for one run: per-model, per-document metrics, latency and cost |
| `data/outputs/<run_id>/<model>/<doc_id>.md` | each model's raw markdown output per document (`/` in a model name becomes `_`, matching `bench run --save-outputs`) |
| `data/outputs/<run_id>/<model>/<doc_id>.json` | the unified `ParseResponse`, copied when `bench run --save-outputs` saved one next to the markdown; its `pages[].blocks[].bbox` drives the layout-box overlay |
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

`src/` is a single-page app in plain ES2018: no framework, no build step (see the ADR "results
viewer stays vanilla"). The one third-party script is **pdf.js 3.11.174**, loaded lazily from
cdnjs with a pinned SRI hash (the worker is fetched with the same integrity check and run from a
blob URL) the first time a PDF input is opened. If it cannot load, synthetic PDFs fall back to the
build-time page-1 PNG and third-party PDFs to a download link. Nothing else is fetched from
outside `data/`.

Every view is a shareable hash link:

- `#/leaderboard?run=…&sort=…&dir=…&x=cost|latency` — sortable model table ranked by
  `summary.headline` when the result file has one (a number, or an object with
  `score`/`overall`/`value` and an optional `label`/`description`), otherwise `overall`; char
  similarity, CER, WER, word F1, order, table, rules, **p50 / p90 / p95** latency (p90 is computed
  in the browser from the per-document latencies, with the CLI's nearest-rank rule), ms/page,
  $/1k pages, failures. Below it: a **score-vs-cost** (or latency) scatter in inline SVG with the
  Pareto frontier, a **per-source** table (the prefix of combined-dataset ids: `synthetic`,
  `parsebench`, later `olmocr`, `omnidocbench`) and a **source × category** heatmap. Per-document
  scores follow the scorer: character similarity, the table score on `table_only` documents, the
  pass rate on rule documents, 0 for a failed call.
- `#/documents?run=…&src=…&cat=…&q=…` — every document with a per-model score column.
- `#/<run_id>/<model_slug>/<doc_id>?tab=…&diff=…&ov=…&page=…` — the **inspector**:
  - the page itself (images directly, PDFs through pdf.js with a pager), and, when a
    `<doc>.json` unified response exists for that model, its **layout boxes** drawn over the page,
    coloured by block type (eight groups, legend below; hover a box for its type and text);
  - model chips with each model's score on this document;
  - tabs: **Diff vs truth** (word-level LCS diff, side by side or unified, compared after the
    scorer's case/markdown normalisation; inline HTML and pure markdown syntax are ignored),
    **All models** (every model's output side by side), **Ground truth**, **Output**; for a rule
    document the first tab is the **Rule checklist**: every assertion with pass/fail, the expected
    text, the reason it failed and, for `bag_of_sentences`, which sentences are missing. The
    checklist is a line-for-line JS port of `score_rules` / `normalize` / `markdown_to_text` in
    `liteocr-core` (scorer v2: HTML and markdown tables, punctuation-spacing-insensitive
    matching, fuzzy `bag_of_sentences`, olmOCR `max_diffs`); its `passed / total` is compared with
    the recorded score and flagged if they ever differ (the recorded Rust score stays
    authoritative). Keep the port in step with `SCORER_VERSION`: a result re-scored by a newer
    scorer than the port would show the mismatch badge.
  - the exact `liteocr parse` / `bench score` (or `bench run --filter`) commands for that pair.
- Old `#/doc/<id>?run=…&model=…` links are redirected to the inspector.

Keyboard: `j`/`k` next/previous document (within the current source/category/filter), `m`/`M`
cycle models, `1`–`4` tabs, `d` split/unified diff, `o` layout boxes, `[`/`]` pages, `g l` /
`g d` leaderboard / documents, `?` help. Light and dark follow the product site (`data-theme`
and the shared `liteocr-theme` key, with a toggle in the header); the layout works down to phone
width with no horizontal page scroll (wide tables scroll inside their frame).

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
