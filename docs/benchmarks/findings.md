# Benchmark findings

Notes on things the committed runs taught us about the providers and about our own scorer. Each
entry says what was observed, how it was checked, and what (if anything) changed because of it.

## 2026-09-24 — Scorer v2: the first combined-v1 run was partly scoring the scorer

The first `combined-v1` run (`benchmark/results/2026-09-11-combined-v1.json`, outputs under
`benchmark/results/outputs/run-20260911T111039Z/`) exposed five defects in
`puffinparse_core::bench`. All are fixed in scorer v2 (`SCORER_VERSION = 2`), and both committed runs
were re-scored offline with `puffinparse bench rescore` (no provider calls; latency and cost are the
originally measured values).

| Defect (scorer v1) | Effect | Fix (scorer v2) |
|---|---|---|
| `table_score` and `table_cell` only read markdown pipe tables | A correct HTML `<table>` scored 0. Reducto `r-1` emits HTML for merged-cell tables: ParseBench `637951191e7b…_pg2_pg1_page1` went from `table_score` 0.000 to 0.865 | Tolerant HTML table reader (`bench/tables.rs`): `thead`/`tbody`, `th`/`td`, `colspan`/`rowspan` repeated into every slot exactly like the ParseBench truth, entities, implicit closes, nested tables flattened |
| Pipe-table cells were compared with their markup (`<b>Region</b>`) | Reducto `r-1` lost 5–12 points of `table_score` on every synthetic table because of bold header cells | Cells are normalised like the rest of the text (tags and emphasis stripped) |
| ParseBench rule text is tokenised: `(this " agreement ")` | `present` / `order` rules failed on correct output | Rule matching drops every space adjacent to punctuation, on both the prediction and the rule |
| `bag_of_sentences` at `threshold: 1.0`, exact match | Failed for every model whenever one "sentence" was a reference artifact (`"j i 25 k"`, two lines fused): dead signal | Per-sentence fuzzy match (≥ 0.8 similarity, Sellers substring distance) and a 0.8 threshold, written into the ParseBench rules and the adapter |
| `Summary.char_similarity` held the table-only headline | The **Char sim** column mixed two metrics | Explicit `summary.headline` (and a per-document `headline`); `char_similarity` is literal again |

Also new in v2: `teds_grid` (TEDS on the `table > row > cell` grid, i.e. structure plus content)
reported next to `table_score`, HTML entity decoding in normalisation, and olmOCR's `max_diffs`
honoured in `present` / `absent` / `order` / `table_cell`.

Why 0.8 for `bag_of_sentences`: with fuzzy matching, correct parses find all or all-but-one of
the bag (for example 40/42 on `text_misc_mark2` for **every** model; the two misses are fused
reference lines), while broken parses are far below: 0/17 for the empty outputs described below,
5/94 for Extend on the Bengali page, 0–1/3 on the handwritten maths page. 0.8 separates those
cleanly; 1.0 separated nothing.

### Before / after (combined-v1, same saved outputs)

| Model | Overall v1 → v2 | Table v1 → v2 | TEDS v2 | Rules v1 → v2 | Char sim v1 → v2 |
|---|---:|---:|---:|---:|---:|
| `llamaparse/agentic` | 92.08 → **93.08** | 0.908 → 0.922 | 0.859 | 84.2 % → 85.4 % | 0.921 → 0.821 |
| `reducto/r-1` | 87.19 → **91.37** | 0.830 → 0.924 | 0.872 | 74.5 % → 75.9 % | 0.872 → 0.802 |
| `llamaparse/cost_effective` | 89.09 → **90.38** | 0.866 → 0.886 | 0.850 | 80.0 % → 81.3 % | 0.891 → 0.803 |
| `reducto/standard` | 88.66 → **89.91** | 0.895 → 0.913 | 0.868 | 69.6 % → 71.2 % | 0.887 → 0.794 |
| `extend/parse_light` | 87.68 → **88.02** | 0.891 → 0.892 | 0.863 | 69.6 % → 71.2 % | 0.877 → 0.775 |
| `extend/parse_performance` | 87.44 → **87.80** | 0.889 → 0.889 | 0.853 | 68.8 % → 70.7 % | 0.874 → 0.777 |

`reducto/r-1` moves from last to second on `combined-v1`: most of its v1 deficit was the HTML
tables. "Char sim" drops for everyone because v1 reported the headline in that column; v2 reports
the literal whole-page similarity, which is low by construction on `table-only` pages.
`synthetic-v1` **Overall** is unchanged for every model (it has no table-only or rule documents);
its Table column changes only for `reducto/r-1` (0.951 → 1.000, the bold header cells).

## 2026-09-24 — Empty output for ParseBench `text_multicolumns_2col`

**Observation.** In run `run-20260911T111039Z`, `reducto/r-1` (`<empty/>`), `reducto/standard`
(empty string), `extend/parse_light` and `extend/parse_performance` (a single newline) returned no
text for `parsebench/text_multicolumns_2col`, with HTTP 200 and a normal bill. Both LlamaParse
tiers transcribed it (17/17 bag sentences).

**Root cause: the page is one Form XObject with a degenerate `/BBox`.** The PDF (Adobe InDesign
19.3, re-saved by macOS Quartz in append mode: two `%%EOF`, one incremental update) has a normal
text layer: `pdffonts` lists three embedded Type 1C fonts (MacRoman, fully decodable), `pdftotext`
extracts the full text, and `pdfimages` shows a single photo. But the page content stream is only
`q Q q 0 0 612 792 re W n /Fm1 Do Q`, and `/Fm1` (object 5) declares

```
/BBox [-89884656743115785407263711865852178399035283762922498299458738401578630390014269380294779316383439085770229476757191232117160663444732091384233773351768758493024955288275641038122745045194664472037934254227566971152291618451611474082904279666061674137398913102072361584369088590459649940625202013092062429184 …]
```

— four 308/309-digit integers, ±2^1023 (half of `DBL_MAX`), Quartz's "infinite" rectangle. That
overflows any renderer that keeps coordinates in `float`; the form's clip becomes empty or
invalid and the whole page renders blank. Checked locally: poppler's `pdftoppm` renders the
original page pure white, and renders it normally once that one `/BBox` is rewritten to
`[0 0 612 792]` (padded to the same byte length so the xref stays valid). Text extraction that
walks the content stream without applying the form clip (poppler's `pdftotext`, and whatever
LlamaParse uses) is unaffected; pipelines that rasterise the page, or drop content clipped by an
invalid form, see nothing.

**Live checks (4 calls, ≈ $0.085 total, 2026-09-24, `cargo run -p puffinparse-cli -- parse …`):**

| # | Call | Result |
|---|---|---|
| 1 | `reducto/agentic` (`enhance.agentic` for text and tables) on the original PDF | empty markdown, billed 2 credits ($0.03) |
| 2 | `reducto/standard` on the `/BBox`-fixed PDF | full transcript, 2,226 chars |
| 3 | `extend/parse_performance` on the `/BBox`-fixed PDF | full transcript, 2,459 chars (plus a figure caption) |
| 4 | `reducto/standard` with `settings.ocr_system = "legacy"` on the original PDF | empty (`chunks: [{content: "", blocks: []}]`), billed 1 credit |

Neither Reducto option fixes it; fixing the file does, for both providers. No Extend option was
tried: Extend's documented passthrough options (`docs/providers/extend.md` §8) have no OCR or
render-mode switch, so the remaining call went to Reducto's OCR system instead.

**What we changed.** Nothing in the benchmark defaults or the dataset. The PDF is upstream
ParseBench's, and "a real-world PDF with a quirk that some parsers survive" is exactly what the
benchmark should measure; the document keeps scoring 0 for those four models. The behaviour is
noted in `benchmark/README.md` (Caveats: an empty parse is scored, not failed).

**Workarounds for users.** Re-serialise the page so the form `/BBox` becomes finite before
uploading (a PDF tool that rewrites the object, or the one-line patch above), or route such files
to LlamaParse. A provider-agnostic follow-up worth doing in the runner: flag a successful call
whose text is empty (`empty_output: true`) so it stands out in the viewer instead of hiding among
low scores.

## Update 2026-09-24: the empty page is not deterministic

In the first `combined-v2` run (run-20260924T211006Z) the same `parsebench/text_multicolumns_2col`
PDF came back empty only from `reducto/standard` (and from `tesseract/default`, whose `pdftoppm`
rasterisation hits the same degenerate `/BBox`); `reducto/r-1`, both Extend engines and both
LlamaParse tiers returned the full text. Provider rendering paths evidently vary between runs or
releases, so a single empty result is not a stable property of a model. Results now flag these
cases as `empty_output: true` (shown as `(+N empty)` in the leaderboard) so they are visible rather
than blending into a low score.
