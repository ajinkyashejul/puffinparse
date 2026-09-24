# Academic and community OCR / document-parsing benchmarks: a survey for LiteOCR

Researched 2026-09-24. Companion to the [vendor benchmark review](vendor-benchmarks.md), which
covers ParseBench, RealDoc-Bench and LongExtractBench. Every licence below was read from the
dataset card on Hugging Face (`https://huggingface.co/api/datasets/<id>` plus the README at the
pinned revision) or from the repository's `LICENSE` file. Revisions are the commits that were
current when this was written. **olmOCR-bench** and **OmniDocBench** now have adapters (see
[adapters.md](adapters.md)). For the others this page recommends what to do next.

LiteOCR scores two document kinds (SPEC §10): `transcript` (reference markdown, scored with
char/word edit distance, reading order and `table_score`) and `rules` (machine-checkable
`present` / `absent` / `order` / `table_cell` / `bag_of_sentences` assertions). "Maps to" below
means into one of those two kinds.

## Summary

| Benchmark | Measures | Size | Licence (verified) | Redistributable in LiteOCR (MIT)? | GT format | Maps to | Recommendation |
|---|---|---|---|---|---|---|---|
| **olmOCR-bench** (AI2) | PDF → markdown unit tests | 1,403 PDFs, 7,010 tests | ODC-BY-1.0 (card) | **Yes**, with attribution | JSONL unit tests | `rules`: 3,040 of 7,019 tests (43%) | **Adapted**: 40-doc subset committed |
| **OmniDocBench** (OpenDataLab) | end-to-end page parsing: text, tables, formulas, reading order | 1,651 pages, 10 doc types, EN + ZH | none on the card; "research purposes only and not for commercial use" | **No**, index only | one JSON: blocks + order + text/LaTeX/HTML | `transcript` (1,443 of 1,651 pages) | **Adapted**: 40-page index, fetched at run time |
| **DP-Bench** (Upstage) | element serialisation (NID), table structure (TEDS / TEDS-S) | 200 single-page PDFs, 36 MB | MIT (card) | Yes (see the caveat below) | `reference.json`: elements with category, coordinates, text/html/markdown | `transcript` | **Adapted** (`benchmark/datasets/dpbench`, in `combined-v3`) |
| **READoc** (ISCAS) | realistic PDF → markdown on whole documents | 2,233 docs (arXiv + GitHub) | MIT (card) | Ground truth yes; PDFs doubtful | one markdown file per document | `transcript`, multi-page | Adapter for a long-document track; fetch PDFs at run time |
| **Nanonets IDP leaderboard** | aggregator: olmOCR-bench, OmniDocBench, "IDP Core" (KIE, VQA, OCR, tables, classification) | 6,406 IDP Core samples plus the two above | harness MIT; datasets mixed | Per dataset | per-dataset | — | No adapter of its own; its two public page benchmarks are covered above |
| **Fox** (UCAS) | fine-grained, region/line/colour-focused page OCR, EN + ZH | 1 zip (`focus_benchmark_test.zip`) | CC-BY-NC-SA-4.0 (card) | **No** (non-commercial, share-alike) | prompt → text pairs | partly `transcript` | Skip: focus-prompted task, NC licence |
| **CC-OCR** (Alibaba / Qwen) | LMM literacy: scene text, multilingual, doc parsing, KIE | 39 subsets, 7,058 images | card says MIT; README says only "the source code is licensed under MIT" | Unclear for the images | TSV with base64 images | `doc_parsing` track → `transcript` | Maybe later, fetched at run time |
| **OCRBench v2** | LMM OCR QA across 31 scenarios | 10,000 QA pairs, 1.1 GB | MIT (card); images drawn from many source datasets | Unclear for the images | QA pairs + eval type | — (QA, not page transcription) | Skip |

Pinned revisions:

| Benchmark | Data | Revision | Scorer / code | Revision |
|---|---|---|---|---|
| olmOCR-bench | HF `allenai/olmOCR-bench` | `54a96a6fb6a2bd3b297e59869491db4d3625b711` | `github.com/allenai/olmocr` (Apache-2.0), `olmocr/bench/tests.py` | `f7cfe4c22098b154c76b6ec950d1c0a464eecf8d` |
| OmniDocBench | HF `opendatalab/OmniDocBench` | `aa1ee96d106dbe53d0ae59474d75c6e6d9b53fec` | `github.com/opendatalab/OmniDocBench` (Apache-2.0) | `f133a71e9e91c3621c7ce8994200a7b394a06eb3` |
| DP-Bench | HF `upstage/dp-bench` (data and `evaluate.py` together) | `24702c61a2fb13325534be664653bc6e60250d13` | same repo | same |
| READoc | HF `lazyc/READoc` | `782bf954e4d9a31ae2bcfe14ff59f1f4b2467592` | `github.com/icip-cas/READoc` | not pinned (not adapted) |
| Fox | HF `ucaslcl/Fox_benchmark_data` | `d6c6f5202d61bf28c8cf9b7b777754dc9bbdc0e0` | `github.com/ucaslcl/Fox` | not pinned |
| CC-OCR | HF `wulipc/CC-OCR` | `c64517e92179991d509776064174776700cdd5a2` | `github.com/AlibabaResearch/AdvancedLiterateMachinery` (`Benchmarks/CC-OCR`) | not pinned |
| OCRBench v2 | HF `ling99/OCRBench_v2` | `c7e7cdf23bdb6774661e9b0caf0d9935a42feb8b` | `github.com/Yuliang-Liu/MultimodalOCR` | not pinned |
| IDP leaderboard | — | — | `github.com/NanoNets/idp-leaderboard-benchmarks` (MIT) | not pinned |

---

## 1. olmOCR-bench (Allen Institute for AI): adapted

- Paper: arXiv:2502.18443 ("olmOCR: Unlocking Trillions of Tokens in PDFs with Vision Language
  Models"). Code: `allenai/olmocr` (Apache-2.0). Data: `allenai/olmOCR-bench`.

**What it measures.** Whether a PDF → markdown converter gets specific, checkable facts right.
It does not compare the output against a reference transcript. Each test is a unit test: this
sentence is present, this running header is absent, this paragraph precedes that one, this
table cell sits under that heading, this equation renders the same as the reference.

**Size.** 1,403 single-page PDFs and 7,010 tests in seven JSONL files, one per source. The
runner also adds one implicit `baseline` test per PDF. There are 9 explicit baseline tests, so
the files hold 7,019 rows.

| Split | PDFs | Tests | Test types |
|---|---:|---:|---|
| `arxiv_math` | 522 | 2,927 | math |
| `old_scans_math` | 36 | 458 | math |
| `table_tests` | 188 | 1,020 (+2 baseline) | table |
| `old_scans` | 98 | 526 | present 279, absent 70, order 177 |
| `headers_footers` | 266 | 753 (+7 baseline) | absent |
| `multi_column` | 231 | 884 | order |
| `long_tiny_text` | 62 | 442 | present |

**Licence.** The card front-matter says `license: odc-by`, and the README says the dataset is
"licensed under ODC-BY-1.0 … intended for research and educational use in accordance with AI2's
Responsible Use Guidelines". ODC-BY allows redistribution with attribution. The underlying pages
are third-party (arXiv, Library of Congress, Internet Archive, crawled PDFs), and every test
carries the page's origin `url`. The adapter keeps that URL as `source_url` on each document.

**Ground truth and scorer.** One JSON object per test:
`{pdf, page, id, type, max_diffs, checked, url, …}` plus type-specific keys: `text`,
`case_sensitive`, `first_n`, `last_n` (present/absent); `before`, `after` (order); `cell`, `up`,
`down`, `left`, `right`, `top_heading`, `left_heading` (table); `math` (LaTeX). Semantics, read
from `olmocr/bench/tests.py`:

- Both sides go through `normalize_text`: whitespace collapsed, `**`/`__`/`*`/`_` emphasis
  stripped, NFC, and typographic quotes and dashes folded to ASCII.
- `present` / `absent` use `rapidfuzz.partial_ratio` with threshold
  `1 - max_diffs/len(text)`. `first_n` / `last_n` restrict the search to the start or end of the
  output. `present` is case-sensitive by default.
- `order` uses `fuzzysearch.find_near_matches` with `max_l_dist = max_diffs` and passes if any
  `before` match starts before any `after` match. It is case-sensitive.
- `table` parses markdown and HTML tables, finds a cell by `fuzz.ratio`, then checks the
  immediate `up`/`down`/`left`/`right` neighbour or the column/row heading.
- `math` renders both equations with KaTeX and compares the rendered symbol layout.
- `baseline` fails on empty output, long repeated n-grams, or CJK and emoji characters.

The leaderboard averages pass rates per split, not per document.

**Mapping to LiteOCR.** Every test is already a rule, so documents are `kind: rules`:

| Upstream | → LiteOCR | Converted | Skipped | Fidelity |
|---|---|---:|---:|---|
| `present` | `present` | 721 | 0 | exact substring, stricter than fuzzy when `max_diffs > 0` |
| `absent` (no `first_n`/`last_n`) | `absent` | 622 | — | exact, looser than fuzzy when `max_diffs > 0` |
| `absent` with `first_n`/`last_n` | — | 0 | 201 | a whole-page absence would fail a page number that also appears in the body |
| `order` | `order` (case-sensitive) | 1,061 | 0 | exact; 94% of the `multi_column` tests have `max_diffs > 0`, so this is stricter than upstream |
| `table` with `top_heading` / `left_heading` / no relation | `table_cell` with `col_header` / `row_header` | 309 | 0 | exact cell equality (upstream: `fuzz.ratio ≥ max(0.5, …)`) |
| `table` with `left` / `right` | `table_cell` with `row_header` = neighbour | 327 | 0 | **relaxed**: "immediately left/right of" becomes "in the same row as" |
| `table` with `up` / `down` | — | 0 | 384 | the schema has no column-adjacency relation, and "value exists" would inflate scores |
| `math` | — | 0 | 3,385 | KaTeX-rendered equivalence has no text-rule analogue |
| `baseline` | — | 0 | 9 (+1,394 implicit) | repetition and charset heuristic |
| **Total** | | **3,040** | **3,979** | 824 of 1,403 PDFs keep at least one rule |

`max_diffs` is kept on every converted rule, so a fuzzy scorer can honour it later.

**Recommendation.** Done. See [`benchmark/datasets/olmocr/README.md`](../../benchmark/datasets/olmocr/README.md).
Two scorer extensions would recover most of what is skipped or approximated: fuzzy matching
driven by `max_diffs`, and `up`/`down`/`left`/`right` neighbour fields on `table_cell`. Math is
deliberately out of scope until LiteOCR has a formula metric.

## 2. OmniDocBench (OpenDataLab / Shanghai AI Laboratory): adapted as an index

- Paper: arXiv:2412.07626. Code: `opendatalab/OmniDocBench` (Apache-2.0). Data:
  `opendatalab/OmniDocBench`, not gated.

**What it measures.** End-to-end parsing of diverse real pages: text (normalised edit
distance), tables (TEDS), display formulas (CDM and edit distance) and reading order (edit
distance over block order). It also has layout-detection and single-module tracks.

**Size.** 1,651 page images (PNG/JPG, over 1 GB; the first 1,000 alone are 1,019 MB) plus `OmniDocBench.json` (42 MB), which
includes a 296-page hard subset added 2026-04-09. Page attributes:

- `data_source` (10): book 276, PPT2PDF 253, academic_literature 215, exam_paper 193,
  colorful_textbook 159, newspaper 151, magazine 149, research_report 132, note 118,
  historical_document 5.
- `language`: simplified_chinese 765, english 755, en_ch_mixed 116, traditional_chinese 13,
  other 2.
- `layout`: single / double / three column, mixed, other.
- `special_issue`: watermark, fuzzy_scan, colorful_background, table flags, and so on.
- `subset`: v1.5, table_hard, layout_hard, equation_hard.

**Licence.** The card has **no licence field** (`cardData` is null in the API). The only terms
are the Copyright Statement: *"The PDFs are collected from public online channels and community
user contributions. Content that is not allowed for distribution has been removed. The dataset
is for research purposes only and not for commercial use."* That grants no redistribution right
and forbids commercial use, so LiteOCR, an MIT repository, **does not vendor any of it**: not the
images, and not the truth derived from the annotations. The evaluation code is Apache-2.0.

**Ground truth.** Per page: `page_info` (image path, size, attributes) and `layout_dets`. Each
block has a `category_type` (28 block classes), a polygon, a reading `order`, and
`text` / `latex` / `html`. `extra.relation` holds `truncated` links, which join paragraphs split
across columns, and `parent_son` links, which attach captions. Upstream's `tools/json2md.py`
shows how to turn this into markdown.

**Mapping to LiteOCR.** One `transcript` document per page. Blocks are emitted in `order`, with
truncated chains merged. Titles become `#` headings, tables become pipe tables converted from
the HTML (merged cells flattened, tagged `merged-cells`), display formulas stay as `$$…$$`, and
headers, footers, page numbers, page footnotes, `abandon` regions and figures are dropped. Those
are the page furniture OmniDocBench itself does not score. Pages with `*_mask` regions (126)
are skipped because their content is deliberately unannotated. So are pages whose truth is
under 120 characters (82). That leaves **1,443 of 1,651 pages** convertible. `data_source` is
the `category`. Language, layout, subset, special issues and `has-table` / `has-formula` are
tags.

What is lost: TEDS table structure (LiteOCR's `table_score` compares cell text row by row), CDM
formula matching (LaTeX is compared as text), and OmniDocBench's block-level matching, which
forgives reading-order differences. LiteOCR's `char_similarity` over the whole page does not.

**Recommendation.** Done as a fetch-at-run-time index. See
[`benchmark/datasets/omnidocbench/README.md`](../../benchmark/datasets/omnidocbench/README.md).
If OpenDataLab ever publishes an explicit licence that permits redistribution, the same adapter
can commit the files by deleting its `.gitignore`.

## 3. DP-Bench (Upstage): adapted

- Data, inference scripts and `evaluate.py` all live in HF `upstage/dp-bench`. There is no paper.

**What it measures.** Two things:

- **NID** (normalised indel distance: edit distance without substitutions) over the text
  elements serialised in reading order. Tables, figures and charts are excluded.
- **TEDS / TEDS-S** over the 55 tables.

**Size.** 200 single-page PDFs (36 MB, largest 4.3 MB): 90 from the Library of Congress, 90
from Open Educational Resources, 20 from Upstage internal documents. The documents contain
1,822 layout elements in 12 classes: Paragraph 804, Heading1 194, Footer 168, Caption 154,
Header 101, List 91, Chart 67, Footnote 63, Equation 58, Figure 57, Table 55, Index 10.

**Licence.** The card front-matter says `license: mit`. The Library of Congress pages are
largely public domain and OER pages are openly licensed. The 20 Upstage internal pages are
covered only by the MIT declaration, so record that in the dataset README.

**Ground truth.** `dataset/reference.json` (1.5 MB) is
`{<pdf>: {elements: [{category, coordinates, id, page, content: {text, html, markdown}}]}}`.
Tables are HTML, equations LaTeX, and `id` is the reading order.

**Mapping.** A `transcript` per page: elements in `id` order, tables as pipe tables via
`html_table_to_markdown`, and headers and footers either dropped (NID excludes nothing but
tables, figures and charts, so they could stay) or tagged. This is structurally the same as the
OmniDocBench adapter, so it is roughly a day's work, and the whole dataset (36 MB) is small
enough that a 40–60 page subset commits in about 5 MB.

**Status.** Adapted by `benchmark/adapters/dpbench.py`: headers and footers are kept (NID scores
them), figures and charts dropped, 40 pages committed (4.5 MB), folded into `combined-v3`. See
[`adapters.md`](adapters.md#dp-bench-mapping).

## 4. READoc (Institute of Software, CAS)

- Paper: arXiv:2409.05137. Code: `icip-cas/READoc`. Data: `lazyc/READoc`.

**What it measures.** Realistic document structured extraction: a *whole* PDF (arXiv papers,
GitHub READMEs rendered to PDF) converted to one markdown file. It scores text, headings,
tables, formulas and reading order after a standardisation and segmentation step (the "DSE
Evaluation Suite").

**Size.** 2,233 documents: `arxiv_ground_truth/` has 1,009 markdown files and
`github_ground_truth/` has 1,224. The PDFs come as `arxiv.zip` (1.19 GB), `github.zip`
(0.59 GB) and `zenodo.zip` (1.45 GB).

**Licence.** The card says `license: mit`. The arXiv PDFs keep their authors' licences, most
often arXiv's non-exclusive distribution licence, which does not let third parties
redistribute. Treat the markdown truth as MIT and the PDFs as fetch-only.

**Ground truth.** One markdown file per document, e.g. `arxiv_ground_truth/0705.4297.md`
(91 KB).

**Mapping.** `transcript`, multi-page, exactly LiteOCR's existing kind, but with documents of 10
to 40 pages. That makes it a latency and cost stress test as much as an accuracy one.

**Recommendation.** Worth an adapter as a separate long-document track (`readoc-arxiv`, about
30 documents). Ship the index plus truth and fetch the PDFs from the zips at run time. Selective
extraction needs HTTP range reads of the zip central directory; downloading 1.2 GB for 30
documents is wasteful. Keep it out of `combined-*` because its per-document cost dwarfs the
single-page sets.

## 5. Nanonets IDP leaderboard

- Site `idp-leaderboard.org`. Harness `NanoNets/idp-leaderboard-benchmarks` (MIT). Toolkit
  `NanoNets/docext`.

**What it measures.** It is an aggregator rather than a dataset. It runs **olmOCR-bench**
(from `allenai/olmOCR-bench`), **OmniDocBench** (from `opendatalab/OmniDocBench`) and its own
"IDP Core": key-information extraction, VQA, OCR, document classification, long-document
processing, table extraction and confidence calibration. IDP Core has 6,406 samples drawn from
existing datasets (Nanonets KIE, DocILE, handwritten forms, ChartQA, DocVQA, OCR handwriting and
diacritics sets, table-extraction sets), loaded through `docext` from Hugging Face.

**Licence.** The harness is MIT. The IDP Core datasets keep their own terms, and several need
an agreement or are research-only (DocILE, DocVQA). **Not verified dataset by dataset here.**

**Recommendation.** No adapter of its own. Its two page-parsing benchmarks are now LiteOCR
adapters, so LiteOCR can report comparable per-benchmark numbers directly. The KIE and table
tracks belong with a future `extract`-mode benchmark and would need a per-dataset licence audit.

## 6. Fox (UCAS / MEGVII)

- Data `ucaslcl/Fox_benchmark_data`, a single `focus_benchmark_test.zip`.

**What it measures.** Fine-grained, focus-prompted document understanding in English and
Chinese: OCR of a region given a box, a line, or a colour highlight, plus page-level OCR and
multi-page variants. Scoring uses edit distance, F1, BLEU and METEOR.

**Licence.** CC-BY-NC-SA-4.0 (card and tags). The non-commercial and share-alike terms are
incompatible with vendoring into an MIT repository.

**Recommendation.** Skip. The task is prompt-conditioned, and LiteOCR's providers expose no
"focus this box" input. Only the plain page-OCR slice would map to `transcript`, and the licence
keeps it fetch-only.

## 7. CC-OCR (Alibaba / Qwen team)

- Paper: arXiv:2412.02210. Data `wulipc/CC-OCR`, the TSV version used by VLMEvalKit.

**What it measures.** Literacy of large multimodal models across four tracks: multi-scene text
reading, multilingual text reading, **document parsing** (documents, tables, formulas; scored
with normalised edit distance and TEDS), and key-information extraction. It has 39 subsets and
7,058 images, of which 41% come from real applications.

**Licence.** The card front-matter says `mit`, but the README's licence section says only that
"the source code is licensed under the MIT License". The images' own terms are not stated.

**Mapping.** The `doc_parsing` subsets give an image and a reference markdown, HTML or LaTeX
string, which maps to `transcript`, with tables via HTML → pipe table.

**Recommendation.** Possible later, as a fetch-at-run-time index like OmniDocBench, until the
data licence is clarified. Low priority: the parsing track is small and mostly overlaps
OmniDocBench.

## 8. OCRBench v2

- Paper: arXiv:2501.00321. Data `ling99/OCRBench_v2` (parquet, 1.1 GB, 10,000 QA pairs). The
  first OCRBench (`echo840/OCRBench`) carries no licence on its card.

**What it measures.** LMM OCR ability as question answering over 31 scenarios and 23 tasks:
text recognition, referring, spotting, key-information extraction, parsing, reasoning. Scoring
is task-specific (exact match, ANLS, TEDS, IoU, and so on).

**Licence.** The card says `license: mit`. The images are collected from many public datasets
that keep their own terms.

**Recommendation.** Skip. It is QA over images, not page transcription, and it conflicts with
the "score `parse` output" design exactly as RealDoc-Bench's QA track does.

---

## What changes in LiteOCR because of this survey

1. `benchmark/adapters/olmocr.py` and `benchmark/adapters/omnidocbench.py` exist, and
   `combined-v2` includes both. `combined-v1` is unchanged because it has committed results.
2. Scorer work that would raise fidelity, in priority order:
   - `max_diffs` fuzzy matching for `present`, `absent`, `order` and `table_cell`. The field is
     already in the rule files.
   - Neighbour relations (`up`, `down`, `left`, `right`) on `table_cell`.
   - HTML tables in predictions (in progress elsewhere).
   - A structural table metric (TEDS) next to `table_score`.
3. Next adapter: READoc as a long-document track (DP-Bench is done: `combined-v3`).
