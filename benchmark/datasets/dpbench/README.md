# dpbench

A curated subset of **[DP-Bench](https://huggingface.co/datasets/upstage/dp-bench)** (Upstage),
converted into PuffinParse `kind: "transcript"` documents by
[`benchmark/adapters/dpbench.py`](../../adapters/dpbench.py).

- Upstream data and evaluation code: `upstage/dp-bench`, pinned to commit
  **`24702c61a2fb13325534be664653bc6e60250d13`** (`dataset/reference.json`, `evaluate.py`,
  `src/layout_evaluation.py`, `src/table_evaluation.py` at that commit).
- License: **MIT**, as declared by the dataset card front-matter (`license: mit`) at that
  revision. The repository has no separate `LICENSE` file; the adapter re-reads the card and
  refuses to build if the declaration changes. Upstream pages come from the Library of Congress
  (largely public domain), Open Educational Resources (openly licensed) and 20 Upstage internal
  documents, which are covered only by the MIT declaration.
- Attribution: *DP-Bench: Document Parsing Benchmark*, Upstage AI,
  https://huggingface.co/datasets/upstage/dp-bench. Every document in `manifest.json` carries
  this in `attribution`.

A copy of the MIT licence as it applies to this subset is in [`LICENSE-MIT`](LICENSE-MIT). The
dataset card asks for no particular citation; please credit *DP-Bench: Document Parsing
Benchmark* (Upstage AI) with the link above when you use these documents or scores. The
truth construction mirrors the semantics of upstream's `evaluate.py` (MIT); no upstream code is
copied.

## Rebuild

```bash
python -m benchmark.adapters dpbench          # fetches reference.json + 40 PDFs at the pinned commit
```

The build is deterministic for a given `--seed` (default `1234`): rebuilding from an empty cache
reproduces `manifest.json`, `conversion-stats.json`, `docs/` and `truth/` byte for byte. This
README is hand-written.

## Layout

```
benchmark/datasets/dpbench/
  manifest.json           # the 40 committed documents
  conversion-stats.json   # upstream counts, skips by reason, selection counts
  docs/<id>.pdf           # 40 single-page PDFs (4.5 MB); <id> is the upstream file stem
  truth/<id>.md           # reading-order markdown truth
```

## How the truth is built

DP-Bench's reference lists every layout element of a page in reading order. Upstream scores
**NID** over the text of every element except `figure`, `table` and `chart`, and **TEDS / TEDS-S**
over the tables. The truth mirrors that:

| Upstream category | Truth |
|---|---|
| `Heading1` | `# heading` |
| `Table` | GitHub pipe table converted from the element's HTML; `colspan` / `rowspan` flattened by repeating the cell (tag `merged-cells`) |
| `Equation` | LaTeX inside `$$ … $$` |
| `Figure`, `Chart` | dropped (upstream's NID drops them too) |
| `Paragraph`, `Caption`, `List`, `Footnote`, `Index`, `Header`, `Footer` | the element text, verbatim |

**Headers and footers are kept**, because DP-Bench's NID scores them. That differs from
`omnidocbench`, whose own evaluation drops page furniture. A parser that strips running
headers and page numbers loses a little here.

No rule assertions are emitted: tables are scored inside the transcript by `table_score` and
`teds_grid`.

## What is committed

`category` is the page's dominant layout feature, first match in the order below. Tags list
every element category on the page (`has-table`, `has-header`, `has-footer`, `has-chart`, …)
plus `merged-cells` and `has-formula`.

| Category | Upstream pages | Committed |
|---|---:|---:|
| `table` | 42 | 10 (4 with merged cells) |
| `equation` | 18 | 5 |
| `chart` | 43 | 6 |
| `figure` | 38 | 5 |
| `index` | 10 | 3 |
| `list` | 9 | 4 |
| `text` | 40 | 7 |
| **Total** | **200** | **40** |

All 200 upstream pages convert; none is skipped. One candidate over the 600 KB per-page limit
was passed over during selection. The reference carries no per-page source or domain field, so
the adapter does not balance or tag the subset by Library of Congress / OER / Upstage; the
provenance audit below was done by hand.

## Provenance and licence audit (2026-10-08)

The dataset card says the 200 pages come from 90 Library of Congress pages, 90 Open Educational
Resources pages and 20 Upstage internal documents, and `reference.json` has no source field.
Reading the reference text of every page at the pinned commit, the ids fall into contiguous
blocks: `…0001`–`…0090` are Library of Congress material (they end with the Law Library of
Congress report `…0085`–`…0090`), `…0091`–`…0180` are openly licensed textbooks and reports, and
**`…0181`–`…0200` are the 20 Upstage documents**: an Upstage company deck (`…0181`–`…0184`), the
SOLAR 10.7B paper (`…0185`–`…0197`, arXiv:2312.15166) and an Upstage OCR Pack deck
(`…0198`–`…0200`).

**5 of our 40 vendored PDFs are Upstage documents:**

| Id | Content | Rights |
|---|---|---|
| `01030000000181` | Upstage company deck ("Making AI Beneficial") | Upstage's own; covered only by the dataset's MIT declaration |
| `01030000000199` | Upstage OCR Pack deck, model evaluation slide | Upstage's own; covered only by the dataset's MIT declaration |
| `01030000000187` | SOLAR 10.7B paper, Table 1 page | Upstage authors; MIT here, and the paper is also CC BY 4.0 on arXiv |
| `01030000000191` | SOLAR 10.7B paper, acknowledgements page | as above |
| `01030000000195` | SOLAR 10.7B paper, appendix A | as above |

They are kept. Upstage is the copyright holder of all five and published them inside a dataset
it declares MIT, which permits redistribution with the copyright and permission notice; every
manifest entry carries the DP-Bench attribution and `license: MIT`. The residual risk is that
the MIT declaration is only in the card front-matter (no `LICENSE` file), which the adapter
re-checks on every build. If Upstage ever narrows the declaration, these five ids are the ones to
drop (re-run the adapter with them excluded and bump the dataset version). The 35 others are
Library of Congress or OER pages, whose own licences are public domain or open.

## Caveats

- **Not comparable to DP-Bench's published leaderboard.** Upstream NID joins element text after
  *removing* newlines and uses `rapidfuzz.fuzz.ratio` on text only; its TEDS works on the full
  HTML tree. PuffinParse scores a normalised markdown transcript, tables included, and `teds_grid`
  on the flattened grid.
- **Chart and figure text counts against a parser here.** Upstream drops prediction elements that
  fall inside a ground-truth figure/chart region (`--filter-by-gt-area`), so transcribing axis
  labels costs nothing there. A transcript has no regions, so text a parser emits for a chart is
  extra text against this truth. Tesseract, which OCRs every label, scores 66 on the `chart`
  pages against 99 on `text` pages. Read `has-chart` / `has-figure` pages with that in mind.
- Upstream text is kept verbatim, including end-of-line hyphenation (`func-\ntions`), so a parser
  that de-hyphenates loses a character there.
- Equations are compared as literal LaTeX.
