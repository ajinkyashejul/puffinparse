# Vendor-published OCR / document-parsing benchmarks — feasibility review for LiteOCR

Researched 2026-09-11. All three benchmarks were **verified by actually downloading a sample**
into `/tmp/claude-0/-home-user-liteocr/b117fcb6-379d-5abe-b4ce-bac65d298215/scratchpad/benchres/`.
Nothing under `/home/user/liteocr` was modified.

LiteOCR manifest target format (from `benchmark/README.md`):
`benchmark/datasets/<name>/manifest.json` = `{name, version, description, license,
documents:[{id, file, truth, pages, category, tags}]}` with inputs in `docs/` and
**ground-truth markdown per document** in `truth/`. Scoring is deterministic char/word
Levenshtein + reading order + table sub-score against that truth markdown.

---

## Summary table

| Name | Public data? | Location | License | Size | GT format | Convertible to LiteOCR manifest? |
|---|---|---|---|---|---|---|
| **ParseBench** (LlamaIndex) | Yes, fully | HF `llamaindex/ParseBench`; code `github.com/run-llama/ParseBench` | Apache-2.0 (data card + code LICENSE) — redistribution permitted | 592 MB total (517 MB docs, 2,079 files; 71 MB rule JSONL) | 169,011 **rule** assertions in 5 JSONL files; **no reference markdown** except 503 HTML tables | **Partially.** Table split → direct (503 docs, HTML→md truth). Text splits → only a *reconstructable approximation* from `bag_of_sentence` + pairwise `order` rules. Chart/layout/formatting splits → not convertible. |
| **RealDoc-Bench** (Extend) — QA track | Yes | HF `Extend-AI/RealDoc-Bench`; code `github.com/extend-hq/realdoc-bench` | Annotations **CC-BY-4.0**; **source PDFs explicitly excluded from that license**, rights vary (471/581 "not_established") | 540 MB (538 MB = 581 PDFs; `qa_bank.json` 1.1 MB) | 1,356 question → typed `gold_dict` JSON pairs (+137 capability tags) | **No** for char-level markdown scoring — there is no page text truth at all. Only usable as a *separate QA-over-parse* track. PDFs are **download-on-demand only**, do not redistribute. |
| **RealDoc-Bench-Layout** (Extend) | Yes | HF `Extend-AI/RealDoc-Bench-Layout` | Annotations CC-BY-4.0; page images keep original per-source licenses | 374 MB (1,500 PNG/JPG = 371 MB; annotations 2.3 MB) | COCO bbox + 9 block classes per page. **No text in annotations** | **No.** Bboxes only, zero transcription. |
| **LongExtractBench-50** (micro1, commissioned by Reducto) | Yes (50-doc subset of a 225-doc corpus) | HF `micro1-inc/longextract-bench-50`; code `github.com/micro1-research/longextract-bench` | Labels **CC-BY-4.0** (micro1); **`document.pdf` retains original source rights** — "verify before redistributing" | 375 MB / 50 folders × 3 files (largest folder 84 MB; median GT ~640 KB) | `schema.json` (JSON Schema) + `ground_truth.json` (nested JSON extraction) | **No** for markdown scoring — GT is a structured extraction, not page text. Excellent as a *long-document extraction* track; PDFs are 1–200+ pages so they are also a good latency/robustness stress corpus. |

**Bottom line for a "combined open benchmark dataset" in LiteOCR's current manifest shape
(input file + expected markdown per document): only ParseBench's `table` split drops in
cleanly (503 documents, Apache-2.0, redistributable).** Everything else measures either
field extraction or layout, and would need a second manifest/scorer kind.

---

## 1. ParseBench — LlamaIndex / LlamaParse

- Website `parsebench.ai` · Paper arXiv:2604.08538 (Zhang, Acosta, Carlson, Bron, Doulcet,
  Ospina, Suo, 2026) · Code `github.com/run-llama/ParseBench` (Apache-2.0) ·
  Data `huggingface.co/datasets/llamaindex/ParseBench`.

### What it measures
Parsing/OCR fidelity (not extraction), split into **five capability dimensions**:

| Dimension | Metric | Pages | Docs | Rules |
|---|---|---:|---:|---:|
| Tables | GTRM = mean(GriTS, TableRecordMatch) | 503 | 284 | — (continuous) |
| Charts | ChartDataPointMatch | 568 | 99 | 4,864 |
| Content Faithfulness | Content Faithfulness Score | 506 | 506 | 141,322 |
| Semantic Formatting | Semantic Formatting Score | 476 | 476 | 5,997 |
| Layout / Visual Grounding | Element Pass Rate (IoA + class + attribution) | 500 | 321 | 16,325 |
| **Total unique** | | **2,078** | **1,211** | **169,011** |

Content Faithfulness and Semantic Formatting share the same 507–508 text pages with
different rule sets.

### Documents & languages
Publicly-sourced enterprise documents: insurance (SERFF filings), finance (10-K, proxy
statements, annual reports), government publications (UN E-Government Survey, OECD), plus
newspapers, timetables, contracts. **One page per document file** — every `docs/*` entry is a
single-page PDF/JPG/PNG cut from a larger source. Card language field is `en`, but the
text split has a `multilang` tag covering **20+ languages / all major scripts (47 docs)**;
also `handwritting` (13), `ocr` scans (119), `multicolumns` (97), `dense` (14), `sparse` (14),
`simple` (170), `misc` (33).

### Data availability — VERIFIED
Public, no auth, no gating. Downloaded:

```
parsebench/                                   7.3 MB downloaded
├── README.md, eval.yaml
├── chart.jsonl              1,591,287 B
├── table.jsonl              3,087,333 B
├── text_formatting.jsonl    1,785,910 B
├── tc_head.jsonl              300,001 B   (first 300 KB of text_content.jsonl via HTTP Range)
└── docs/chart/(Web_version)_E-Government_Survey_2024_1392024_p101.pdf   183,569 B (1 page)
```

Full repo: **592.2 MB**, 2,113 files —
`docs/` 2,079 files / 517 MB (chart 568, text 508, table 503, layout 500;
2,037 `.pdf`, 23 `.jpg`, 19 `.png`), `text_content.jsonl` 55.4 MB,
`layout.jsonl` 9.6 MB, `table.jsonl` 3.1 MB, `text_formatting.jsonl` 1.8 MB,
`chart.jsonl` 1.6 MB, `thumbnails/` 3.5 MB.

Command that worked (no TLS games needed, just point the CA bundle at the proxy):

```bash
export REQUESTS_CA_BUNDLE=/root/.ccr/ca-bundle.crt SSL_CERT_FILE=/root/.ccr/ca-bundle.crt
python -c "from huggingface_hub import snapshot_download; snapshot_download(
  'llamaindex/ParseBench', repo_type='dataset', local_dir='parsebench',
  allow_patterns=['README.md','eval.yaml','*.jsonl','docs/chart/…p101.pdf'])"
```

### License / redistribution
`license: apache-2.0` in the dataset card front-matter, and the card's Copyright Statement:
*"All documents are sourced from public online channels. The dataset is released under the
Apache 2.0 License. If there are any copyright concerns, please contact us via the GitHub
repository."* SPDX: **Apache-2.0**. Redistribution in another repo is therefore **permitted by
the publisher's terms** — this is the only one of the three where that is true.
Caveat worth recording in our own README: the underlying pages are third-party corporate and
government documents that LlamaIndex re-licensed unilaterally; for a low-risk posture we could
still ship only the manifest + SHA-256 and fetch on demand.

### Ground-truth format — VERIFIED
One JSONL line per **test rule**, identical schema across all five files:

```json
{"pdf":"docs/chart/report_p41.pdf","category":"chart","id":"b17e5e98d6fc2763",
 "type":"chart_data_point","rule":"{...json-encoded payload...}","page":null,
 "expected_markdown":null,"tags":["need_estimate"]}
```

Real rows pulled from the download:

- **table.jsonl** (the only one with reference content):
  `id: "0000027_page1_expected_markdown"`, `type: "expected_markdown"`, `rule: {}`,
  `tags: ["easy"]`, and
  `expected_markdown: "<table>\n<tr><th>Business activity</th><th>Registered office</th>…<tr><td colspan=\"6\"><strong>JOINT VENTURES CONSOLIDATED USING THE EQUITY METHOD</strong></td></tr>…"`
  — i.e. **ground-truth HTML tables** (with `colspan`/`rowspan`/`<br/>`/`<strong>`), 503 of them.
- **chart.jsonl**: `rule: {"labels":["IF","193 UN Member States"],"max_diffs":0,
  "normalize_numbers":true,"value":"0.8079"}` — a spot-check data point, no page text.
- **text_formatting.jsonl**: `rule: {"text":"BAOTOU 包头","level":1}`, `type: "is_title"`,
  `tags: ["dense","hard"]`.
- **text_content.jsonl** (11 rule types seen in the first 300 KB — counts in that window:
  `missing_specific_word` 520, `missing_specific_sentence` 96, `order` 71, plus one each of the
  aggregate types). The aggregate rules carry the actual reference content as **bags**:
  - `{"bag_of_sentence": {"BAOTOU 包头":1, "Baotou is the largest city within the Inner Mongolia
    Autonomous Region in China":1, …}}`
  - `{"bag_of_word": {"10":1,"12":2,…}}`
  - `{"bag_of_digit": {"0":60,"1":46,…}}`
  - `{"before":"Baotou is the largest city…","after":"It's population of more than 1.6 million…","max_diffs":0}`

Metadata: `category` (chart/table/text/layout), `tags` = difficulty (`easy`/`hard`) +
document type (`dense`,`sparse`,`simple`,`multicolumns`,`ocr`,`multilang`,`misc`,`handwritting`)
+ chart flags (`need_estimate`, `3d_chart`). `page` is 1-indexed, used by layout rules only.
Images/PDFs are plain files under `docs/<category>/<name>.{pdf,jpg,png}`, referenced by the
relative `pdf` field.

### Scoring / official eval code
`github.com/run-llama/ParseBench` (Apache-2.0), a full harness with 180+ pipeline
configurations (OpenAI, Anthropic, Google, LlamaParse, specialised parsers), parallel runs,
per-dimension scoring and cross-pipeline comparison; `eval.yaml` declares six tasks
(`mean`, plus one per split). **Deterministic and rule-based — no LLM judge by default.**
Metrics: GTRM (GriTS + TableRecordMatch, bag-of-records, order-insensitive) for tables;
ChartDataPointMatch (orientation-insensitive, numeric-tolerant) for charts;
rule pass-rate scores for content faithfulness and formatting; Element Pass Rate
(IoA localisation + classification + attribution) for layout. Leaderboard headline:
LlamaParse Agentic Plus 90.20, LlamaParse Agentic 87.01 (vendor-run).

### Conversion into the LiteOCR manifest
- **`table` split → clean fit.** 503 single-page PDFs, each with one ground-truth HTML table.
  Convert HTML → markdown pipe table for `truth/<id>.md`, `pages: 1`, `category: "table"`,
  `tags: ["easy"|"hard","parsebench"]`. Our `table_score` metric applies directly; our
  `char_similarity` becomes a table-fidelity proxy. **Lost:** GriTS structural scoring,
  merged-cell/colspan semantics (markdown pipe tables cannot express `colspan`/`rowspan`, so
  hierarchical headers get flattened) — this is a real fidelity loss on the `hard` tables.
- **`text` split (508 pages) → approximate fit only.** There is no ordered reference markdown.
  You could reconstruct a pseudo-truth by taking `bag_of_sentence` and topologically sorting it
  with the pairwise `order` rules, but ordering is only partially constrained, sentence
  boundaries are the annotator's, and all formatting/whitespace is gone — so CER/WER against it
  would be systematically wrong. Not recommended as ground truth; better to keep ParseBench's
  own rule scorer for that split.
- **`chart`, `layout`, `text_formatting` → not convertible.** Data-point assertions, bboxes and
  style flags have no text-similarity analogue.
- Net: **~503 of 2,078 pages (24%) usable in the current manifest shape.**

---

## 2. RealDoc-Bench — Extend (extend.ai)

- Blog: `extend.ai/resources/realdocbench` and `/resources/parse-2-and-realdocbench-launch` ·
  Paper arXiv:2606.07401 (CC BY 4.0 on arXiv) ·
  Code `github.com/extend-hq/realdoc-bench` (Apache-2.0, `pip install realdoc-bench`) ·
  Data `huggingface.co/datasets/Extend-AI/RealDoc-Bench` and
  `huggingface.co/datasets/Extend-AI/RealDoc-Bench-Layout`.

### What it measures
Two tracks, neither of which is OCR-similarity:

1. **QA track** — *field-level extraction accuracy through a parser*. The parser produces
   markdown; an LLM reader (Gemini 3 Flash in the official harness) answers each question using
   **only** that markdown; the answer is scored against a typed gold dict. Reported as
   **per-field accuracy** and **strict per-question accuracy**, plus cost and latency.
   1,356 questions / 3,742 fields / 581 documents.
2. **Layout track** — bounding-box + block-type detection over 1,500 page images.
   Hungarian matcher with adjacency-aware split/merge recovery; strict **F1**, **adjusted F1**
   (allows merging adjacent same-type fragments), **mAP**, with per-class breakdowns.

Headline (vendor-run): Extend Parse 2.0 96.0% per-field / 90.9% per-question,
LlamaParse (Agentic) 92.2% / 84.5%; layout 0.781 strict F1 / 0.847 adjusted F1.

### Documents & languages
Four domains — counted from the downloaded `qa_bank.json`: **mortgage 478, finance 378,
supply_chain 319, medical_healthcare 181** questions. Document types: hospital intake forms,
EOBs, tax and ACORD insurance forms, mortgage packets, bills of lading, "systems-of-record"
documents; dense forms, checkboxes, handwriting, stamps, barcodes, messy scans.
Language: `en` only per the card. The two PDFs I fetched were **1 page each** (461 KB and
27 KB), so the QA corpus is mostly short form-like documents (581 docs / 538 MB ≈ 0.93 MB each).
Layout track domains include `government`, `billing`, etc. (from `manifest.csv`).

### Data availability — VERIFIED
Public, no auth. Downloaded:

```
realdocbench/                                 2.2 MB downloaded
├── README.md
├── qa_bank.json          1.09 MB   (1,356 items)
├── manifest.json         0.44 MB   (581 document provenance records)
└── docs/finance_1.pdf  460,998 B (1 page) ; docs/mortgage_1.pdf 26,930 B (1 page)

realdocbench_layout/                          1.4 MB downloaded
├── README.md, manifest.csv (0.43 MB)
├── annotations/0008ff17-….json
└── images/0008ff17-….png
```

Full repos: QA **539.6 MB** / 585 files (`docs/` 581 PDFs = 538 MB);
Layout **373.9 MB** / 3,003 files (`images/` 1,500 = 371 MB, `annotations/` 1,500 = 2.3 MB).

### License / redistribution — **the blocker**
Card: *"The QA bank and gold answers are licensed under CC BY 4.0. **Source documents in
`docs/` are excluded from this annotation license.** Their applicable rights and reuse terms
vary… Some source rights remain unverified. A source URL, public availability, or AI
modification does not by itself establish permission to redistribute or relicense a document."*

`manifest.json` (`schema_version 1.0`, `metadata_as_of 2026-09-09`, `annotation_license
CC-BY-4.0`) makes this concrete — aggregated over all 581 documents:

| `source_rights.status` | count |
|---|---:|
| `not_established` | 471 |
| `public_domain_us_federal_work` | 58 |
| `copyright_notice_no_reuse_license` | 31 |
| `explicit_license` | 8 |
| `agency_reuse_policy_with_conditions` | 8 |
| `distribution_restrictions_no_open_license` | 5 |

`document_license` is **`null` for all 581**. `ai_status`: `ai_generated_edit` 291,
`collected_real_document` 196, `unknown` 94 — i.e. **half the corpus is synthetically edited**,
which matters if we claim "real documents". Each record carries `sha256`, `source.url`,
`source.type` (`original_document` 290 / `original_template` 288), `source.match`,
`source.availability`. Monthly takedown process with `takedowns/removed_ids.jsonl`.

**Verdict: annotations SPDX CC-BY-4.0 and redistributable with attribution; PDFs are
download-on-demand only.** We can ship a manifest + sha256 + HF path, never the bytes.
The layout images are the same story (annotations CC-BY-4.0, images keep per-source licenses).

### Ground-truth format — VERIFIED
`qa_bank.json` = `{name, domains:["finance","medical_healthcare","mortgage","supply_chain"],
items:[…1,356…]}`. A real item:

```json
{
  "question_id": "finance_q1",
  "source_file": "finance_1",
  "domain": "finance",
  "question": "In the PRIOR CARRIER INFORMATION (continued) table, for the year 201 entry with an expiration date of 12/31/2024, what is the premium for the automobile category?",
  "response_format": "Return exactly: automobile_premium=<number>",
  "gold_answer": "automobile_premium=12800",
  "gold_dict": {"automobile_premium": 12800},
  "capabilities": ["field_value_pairing","multi_column_grid","repeated_labels","row_binding","table_structure"]
}
```

(the HF card mentions a `template` field; in the shipped file it is absent/`null` for all
1,356 items — the typing lives in `response_format` + `gold_dict`.)
**137 distinct capability tags** across 8 buckets; most common: `field_value_pairing` 502,
`checkbox_state` 385, `column_alignment` 298, `row_binding` 288, `table_structure` 272,
`form_region` 222, `parallel_columns` 203, `line_binding` 180, `scanned_form` 160,
`multi_column_grid` 150, `handdrawn_check` 127, `blank_field` 122. These are excellent
difficulty/category metadata and map well onto our `tags` field.

Layout annotation (`annotations/<pageId>.json`, COCO-style):

```json
{"image":{"id":20,"file_name":"human/0008ff17-….png","width":640,"height":1102,"domain":"government"},
 "annotations":[{"id":198,"image_id":20,"category_id":0,"bbox":[23,24,58,13]}, …],
 "categories":[{"id":0,"name":"text"},{"id":1,"name":"heading"},{"id":2,"name":"section_heading"},
               {"id":3,"name":"header"},{"id":4,"name":"footer"},{"id":5,"name":"page_number"},
               {"id":6,"name":"figure"},{"id":7,"name":"table"},{"id":8,"name":"key_value"}],
 "page_info":{…}}
```

Note: **no `content`/text field on the annotations** — pure geometry + class. `manifest.csv`
is the canonical row list: `image_id,file_name,domain,pageId,match_status,originalImageUrl,sourceUrl`.

### Official eval code
`pip install realdoc-bench`; pipeline `download → parse → score → report`, per-parser scoping
with cached intermediates:

```bash
realdoc-bench evaluate download --run-dir runs/v1 --dataset Extend-AI/Realdoc-Bench
realdoc-bench evaluate run --run-dir runs/v1 -p extend_performance_v2_0_0_advanced
realdoc-bench evaluate run --run-dir runs/smoke -p pymupdf --limit 20
```

Layout normaliser at `realdoc_bench/layout/normalizers/coco.py`. Apache-2.0.
Scoring is **exact-match over typed gold dicts**, but the *answering* step uses an LLM reader,
so runs are not bit-reproducible and cost money — that conflicts with LiteOCR's
"no LLM judge, deterministic metrics" principle #2.

### Conversion into the LiteOCR manifest
**Not convertible as-is.** There is no page transcription anywhere in either repo, so nothing
can populate `truth/<id>.md`. Options:
- Add a **second manifest kind** (`qa`) — `{id, file, questions:[{question, response_format,
  gold_dict, capabilities}], pages, category, tags}` — plus a scorer that runs the parse, feeds
  the markdown to a reader model, and exact-matches `gold_dict`. That is a genuinely different
  (and non-deterministic, paid) axis from `bench run`.
- Or use the layout track as a third kind for bbox F1.
- Either way `docs/` stays **download-on-demand** (manifest records `sha256` + HF repo path;
  a `liteocr bench fetch` step pulls 540 MB / 374 MB on first use).
- **Lost if forced into the markdown shape:** everything — you'd be inventing truth.

---

## 3. LongExtractBench — micro1 (commissioned by Reducto)

- Site `micro1.ai/benchmark/long-extraction` · Code `github.com/micro1-research/longextract-bench`
  (**MIT**) · Data `huggingface.co/datasets/micro1-inc/longextract-bench-50` (CC-BY-4.0) ·
  PR: "Reducto Deep Extract Ranks First Overall in LongExtractBench" (PRNewswire).

### What it measures
**Schema-driven structured extraction, not OCR.** Given `document.pdf` + `schema.json`, a
system must emit JSON validating against the schema; it is compared cell-by-cell to
`ground_truth.json`. Three capabilities: extraction fidelity, schema conformance, and
long-document handling. Seven providers evaluated: Reducto, Extend, LlamaExtract, OpenAI,
Anthropic Claude, Google Gemini, Datalab. Reducto's reported result: 99.6% recall,
99.6% precision, 99.3% leaf accuracy, 0 failures, only provider at 100% coverage.

### Documents, pages, languages
Public HF release is a **50-document curated subset** of a **225-document** corpus
(benchmark run dated 2026-06-26) — the other 175 are not released. Stratified by page count
(short → multi-hundred pages), schema complexity (flat → deeply nested multi-array), and
domain: government/public-sector statistics, financial filings (10-K / 10-Q / DEF 14A proxy),
healthcare & clinical reporting, regulatory/compliance, energy, education, census & demographics.
**Predominantly English, with a few German and Dutch documents** (e.g. the folder
`b9489a19__Statistisch Jaarboek 2025`). Verified page count on the sample I pulled:
`06_19_Bankruptcy_Filings_Statistics/document.pdf` = **202 pages** in 823 KB — these are long,
dense, table-heavy, mostly born-digital PDFs, not scans.

### Data availability — VERIFIED
Public, no auth, no gating. Downloaded:

```
longextractbench/                             1.6 MB downloaded
├── README.md
└── 06_19_Bankruptcy_Filings_Statistics/
    ├── document.pdf       822,849 B   (202 pages)
    ├── ground_truth.json  652,812 B
    └── schema.json          3,505 B
```

Full repo: **374.7 MB**, 152 files = 50 folders × 3 files + `.gitattributes`.
Largest folders: `UK_asylum-applications-datasets-mar-2023` 84.3 MB,
`Capital_improvement_plan___CIP_project_budget_report` 36.3 MB,
`std__Annual_report_10-K_-_wfc-20201231_d2` 19.9 MB,
`06_19_Government_zoning_and_land_use_geospatial_datasets` 17.8 MB.
Folder slugs carry an informal difficulty prefix (`std__`, `hard__`, `m1__`, date prefixes)
that could feed our `tags`.

### License / redistribution
Card front-matter `license: cc-by-4.0`, but the License section is precise:
*"The label files (`schema.json`, `ground_truth.json`) are released by micro1. The underlying
`document.pdf` files originate from public sources and retain their original rights — verify
the terms of an individual document before redistributing it."*
SPDX: **CC-BY-4.0 for labels; unspecified/per-source for PDFs.** Code is **MIT**.
Same posture as RealDoc-Bench: **labels redistributable, PDFs download-on-demand.**
Note the disclosure: ground truth is **model-assisted** (drafted by a frontier model,
reconciled by humans) — "high quality but not guaranteed error-free, and may share blind spots
with the LLMs being evaluated" — and the benchmark was **commissioned by Reducto**, who then
won it. Both facts belong in any README we write.

### Ground-truth format — VERIFIED
`schema.json` is a standard JSON Schema: `type: "object"`, `additionalProperties: false`,
a `required` list, and a **natural-language `description` on every field** that pins down
formatting and null-handling. From the downloaded sample:

```json
{"title":"Bankruptcy Filing Statistics Data Export Schema","type":"object",
 "additionalProperties":false,
 "properties":{
   "covered_year_end":{"type":"integer","description":"The largest year value appearing in the printed 'year' column … Emit as a four-digit integer with no quotes, commas, or decimal point…"},
   "covered_year_start":{"type":"integer","description":"…"},
   "filing_count_records":{"type":"array","description":"One item for each non-header data row … Preserve the document's row order from the first data row through the final data row, continuing across page breaks…",
     "items":{"type":"object","additionalProperties":false,
       "required":["year","chapter","district","case_count"],
       "properties":{"year":{"type":"integer","description":"…"},
                     "chapter":{"type":"integer","description":"…"},
                     "district":{"type":"string","description":"…preserving lowercase letters and any printed alphabetic suffix…"},
                     "case_count":{"type":"integer","description":"…remove thousands separators…"}}}}},
 "required":["covered_year_start","covered_year_end","filing_count_records"]}
```

`ground_truth.json` for that document (653 KB) begins:

```json
{"covered_year_end": 2025, "covered_year_start": 2008,
 "filing_count_records": [
   {"case_count": 245, "chapter": 11, "district": "akbk",  "year": 2008},
   {"case_count": 17,  "chapter": 11, "district": "almbk", "year": 2008},
   {"case_count": 84,  "chapter": 11, "district": "alnbk", "year": 2008}, … ]}
```

So: document-level scalars + one or more arrays of row objects mirroring tables, median GT
~640 KB. No per-page metadata, no categories/difficulty field (only the folder-slug prefixes).

### Scoring / official eval code
`github.com/micro1-research/longextract-bench` (MIT), runnable CLI, dataset downloads on first
run via `src/longextract_bench/dataset.py`. Metrics:
- **Precision / Recall over array rows**, matched to GT rows by a **key the grader infers per
  array** (not by position) — precision penalises hallucinated/duplicated/extra rows, recall
  penalises misses.
- **Leaf accuracy** = fraction of scalar leaf values exactly matching.
- **Completion is a first-class result**: accuracy is computed only over completed documents and
  always reported next to a completion count, with failure rate + reasons and latency tracked
  separately — explicitly to stop systems from looking good by silently dropping hard docs.
Deterministic, no LLM judge.

### Conversion into the LiteOCR manifest
**Not convertible to `truth/*.md`.** The ground truth is a nested JSON extraction of selected
fields, not a transcription — most of the 202-page PDF's text is deliberately *not* in the GT.
Realistic uses:
- **A third manifest kind (`extract`)**: `{id, file, schema, truth_json, pages, category, tags}`
  with a scorer implementing row-keyed precision/recall + leaf accuracy. That is a
  well-specified, deterministic, LLM-judge-free metric — a good fit for LiteOCR's principle #2,
  and it exercises the `extract`-style endpoints our providers expose (Reducto Deep Extract,
  Extend, LlamaExtract) rather than the parse endpoints.
- **A latency/robustness corpus for the existing parse benchmark**: 50 PDFs of 1–200+ pages is
  exactly the "ms per page, p95, failure rate" stress we currently lack (synthetic-v1 tops out
  at `multipage`). We'd run parse on them and report latency/cost/failure only — **accuracy
  would have to be omitted**, since there is no text truth.
- **Lost:** if you tried to synthesise markdown truth from `ground_truth.json` you'd get a
  tiny table fragment vs. a 202-page document; CER would be meaningless.
- PDFs stay **download-on-demand** (374.7 MB); labels (schema + GT, a few MB) are CC-BY-4.0 and
  could be vendored.

---

## Recommendations

1. **ParseBench `table` split is the one drop-in win** — 503 single-page PDFs + HTML table truth,
   Apache-2.0, redistributable. Write an `adapters/parsebench_table.py` that pulls
   `table.jsonl` + the 503 `docs/table/*.pdf` (~130 MB of the 517 MB) and emits
   `manifest.json` + `truth/*.md`. Flag in the dataset README that colspan/rowspan is flattened.
2. **Do not vendor any PDFs from RealDoc-Bench or LongExtractBench.** Both explicitly carve the
   source documents out of their CC-BY-4.0 annotation licence. A `liteocr bench fetch` that
   snapshot_downloads from HF and verifies `sha256` (RealDoc-Bench ships them; LongExtractBench
   does not, so we'd record our own) keeps us clean and matches the manifest's existing
   "downloaded by the user and converted" plan.
3. **Two new manifest kinds would unlock the rest**: `extract` (JSON Schema + expected JSON,
   deterministic row-keyed P/R + leaf accuracy — LongExtractBench, and LlamaIndex's companion
   ExtractBench) and `qa` (question + typed gold dict, needs a reader model — RealDoc-Bench).
   Only the `extract` kind preserves LiteOCR's "no LLM judge" principle.
4. **Provenance caveats to carry through** into any combined dataset README: RealDoc-Bench is
   ~50% `ai_generated_edit` and 471/581 documents have `not_established` rights;
   LongExtractBench was commissioned by the vendor that won it and its labels are
   model-drafted; ParseBench annotations are frontier-VLM auto-labelled with targeted human
   correction. All three are vendor-published and each vendor leads its own leaderboard.

## Reproduce the downloads

```bash
cd /tmp/claude-0/-home-user-liteocr/b117fcb6-379d-5abe-b4ce-bac65d298215/scratchpad/benchres
export REQUESTS_CA_BUNDLE=/root/.ccr/ca-bundle.crt SSL_CERT_FILE=/root/.ccr/ca-bundle.crt
pip install huggingface_hub
python - <<'PY'
from huggingface_hub import snapshot_download as d
d("llamaindex/ParseBench",       repo_type="dataset", local_dir="parsebench",
  allow_patterns=["README.md","eval.yaml","chart.jsonl","table.jsonl","text_formatting.jsonl",
                  "docs/chart/(Web_version)_E-Government_Survey_2024_1392024_p101.pdf"])
d("Extend-AI/RealDoc-Bench",     repo_type="dataset", local_dir="realdocbench",
  allow_patterns=["README.md","qa_bank.json","manifest.json","docs/finance_1.pdf","docs/mortgage_1.pdf"])
d("Extend-AI/RealDoc-Bench-Layout", repo_type="dataset", local_dir="realdocbench_layout",
  allow_patterns=["README.md","manifest.csv","annotations/0008ff17-5fec-43a0-9889-2a9e664700f7.json",
                  "images/0008ff17-5fec-43a0-9889-2a9e664700f7.*"])
d("micro1-inc/longextract-bench-50", repo_type="dataset", local_dir="longextractbench",
  allow_patterns=["README.md","06_19_Bankruptcy_Filings_Statistics/*"])
PY
# 55 MB text_content.jsonl sampled without a full fetch:
curl -sSL --cacert /root/.ccr/ca-bundle.crt -H "Range: bytes=0-300000" \
  https://huggingface.co/datasets/llamaindex/ParseBench/resolve/main/text_content.jsonl \
  -o parsebench/tc_head.jsonl
```

Total downloaded here: **12.5 MB** across the four repos (full corpora would be
592 + 540 + 374 + 375 MB ≈ **1.88 GB**).

## Sources

- [llamaindex/ParseBench (HF)](https://huggingface.co/datasets/llamaindex/ParseBench)
- [run-llama/ParseBench (GitHub)](https://github.com/run-llama/ParseBench)
- [ParseBench paper, arXiv:2604.08538](https://arxiv.org/abs/2604.08538)
- [Extend-AI/RealDoc-Bench (HF)](https://huggingface.co/datasets/Extend-AI/RealDoc-Bench)
- [Extend-AI/RealDoc-Bench-Layout (HF)](https://huggingface.co/datasets/Extend-AI/RealDoc-Bench-Layout)
- [extend-hq/realdoc-bench (GitHub)](https://github.com/extend-hq/realdoc-bench)
- [RealDocBench paper, arXiv:2606.07401](https://arxiv.org/html/2606.07401v1)
- [Parse 2.0 and RealDoc-Bench launch (Extend blog)](https://www.extend.ai/resources/parse-2-and-realdocbench-launch)
- [micro1-inc/longextract-bench-50 (HF)](https://huggingface.co/datasets/micro1-inc/longextract-bench-50)
- [micro1-research/longextract-bench (GitHub)](https://github.com/micro1-research/longextract-bench)
- [LongExtractionBench (micro1)](https://www.micro1.ai/benchmark/long-extraction)
- [Reducto tops LongExtractBench (PRNewswire)](https://www.prnewswire.com/news-releases/reducto-deep-extract-ranks-first-overall-in-longextractbench-an-independent-benchmark-for-complex-document-extraction-302815264.html)
