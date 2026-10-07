# Datalab (Marker)

> **Status: docs-only.** Everything below comes from the official Datalab documentation and
> OpenAPI spec (read 2026-09-11), from the open-source Marker renderer that produces the payloads,
> and from `crates/puffinparse-core/src/providers/datalab.rs`. No live call has been made — this
> repository has no Datalab key. `crates/puffinparse-core/tests/fixtures/datalab_convert.json` is
> hand-built from the documented shapes. Mark this page **verified** only after the `#[ignore]`d
> live test in `providers/datalab.rs` passes with a real key.
> Help verify it: [issue #10](https://github.com/ajinkyashejul/puffinparse/issues/10).

## 1. Summary

| | |
|---|---|
| Provider name | `datalab` |
| Base URL | `https://www.datalab.to` (override: `base_url` on the request, or `DATALAB_BASE_URL`) |
| API key | `DATALAB_API_KEY` (or `api_key` on the request) — sent as the `X-API-Key` header |
| Docs | <https://documentation.datalab.to> |
| API version | Path-versioned (`/api/v1/convert`). The response echoes the engine versions in `versions` (`marker`, `surya`) |
| Checked against | **not live-verified** (see banner) — documentation read 2026-09-11 |
| Implementation | `crates/puffinparse-core/src/providers/datalab.rs` |

Datalab is the hosted version of **Marker** (plus Surya and Chandra), the open-source PDF → markdown
pipeline. The response shapes below are Marker's own: `markdown`, an HTML-carrying block tree
(`json`), pre-chunked blocks (`chunks`), and a `metadata` dictionary with `page_stats`.

## 2. Models exposed by PuffinParse

| Model | Provider parameter | Modes | List price (`pricing.json`) |
|---|---|---|---|
| `datalab/fast` | `mode=fast` | `parse`, `ocr` (derived) | $0.004 / page |
| `datalab/balanced` *(default)* | `mode=balanced` | `parse`, `ocr` (derived) | $0.004 / page |
| `datalab/accurate` | `mode=accurate` | `parse`, `ocr` (derived) | $0.010 / page |

From the rate card at <https://www.datalab.to/pricing>: *Convert — fast / balanced* is **$4 per
1 000 pages**, *Convert — accurate* is **$10 per 1 000 pages**. `balanced` is the default because
the docs recommend it ("balance of speed and accuracy (recommended)"); `fast` suits clean digital
PDFs at high throughput, `accurate` suits scans, dense layouts and complex tables.

**Naming note.** These names track the documented `mode` parameter of the **current** `/api/v1/convert`
endpoint. The older `/api/v1/marker` endpoint (with `use_llm`, `force_ocr`, `format_lines`) is
marked deprecated in the API reference in favour of `/convert`, `/extract`, `/segment` and `/agent`,
so there is no `datalab/marker` / `datalab/marker-llm` pair: `use_llm` no longer exists as a request
field, and its role is taken by `mode=accurate`.

Add-ons that change the bill are **not** enabled by PuffinParse and must be opted into through
`provider_options`: `word_bboxes` (+$3/1k pages), `extras="table_cell_bboxes"` or `"list_item_bboxes"`
(+$6/1k each, word prediction included once), `chart_understanding` (+$3/1k), `infographic`
(+$4/1k), `merge_cross_page` (variable compute, ~$0.50/document), and EU `processing_location`
(1.25× usage).

## 3. Request flow PuffinParse uses

1. **Submit.** `POST {base}/api/v1/convert`, `multipart/form-data`, header `X-API-Key`:

   | Field | Value |
   |---|---|
   | `mode` | the model name (`fast` \| `balanced` \| `accurate`) |
   | `output_format` | `json,markdown` — **both formats in one conversion**, one page charge |
   | `paginate` | `true` (page delimiters in the markdown) |
   | `page_range` | from `pages`, **converted 1-based → 0-based**: `"1-3,5"` → `"0-2,4"`; an open range `"10-"` becomes `"9-6999"` (7 000 pages is the per-request ceiling) |
   | `file` | the document bytes with filename and guessed MIME type — **path / bytes input only** |
   | `file_url` | the URL string — **URL input only**, in place of `file`; Datalab downloads it |

   Response: `{"success": true, "request_id": "…", "request_check_url": "https://www.datalab.to/api/v1/convert/…", "versions": {…}}`.
2. **Poll.** `GET` the check URL every 2 s, backing off ×1.5 to 10 s, with the same `X-API-Key`.
   Done when `status == "complete"`; a failure can also appear as `success == false` (with `status`
   still `"complete"`) or `status == "failed"`, so all three are terminal for PuffinParse.
   The returned `request_check_url` is re-hosted on the configured base URL (path only), so a
   `base_url` override or proxy keeps working.
3. **Download, when regional.** If the poll body carries `result_url`, the document content lives
   there instead of inline (EU processing, and any other region that requires it). PuffinParse fetches
   it **without** the API key — the signed URL authorises by itself — and merges: downloaded body
   first, then every non-null field from the poll response on top, because billing and score fields
   can be updated after the document was stored.
4. **Normalise.** `json` gives the block tree (types + polygons), `markdown` gives Marker's own page
   rendering.

**Where `provider_options` are merged:** the request is a multipart form, so options are flattened
into extra text fields exactly like the built-in ones. Strings pass through; booleans become
`"true"`/`"false"`; numbers are stringified; objects/arrays are serialised as JSON text (which is
what `additional_config` expects); `null` is skipped. A key PuffinParse already set is **replaced**, so
`provider_options={"output_format": "markdown"}` really does turn the JSON block tree off (and with
it, all `Block`s).

## 4. Response mapping

| Datalab field | PuffinParse unified field | Notes |
|---|---|---|
| `request_id` | `ParseResponse.provider_job_id` | |
| `page_count` | `Usage.pages` | Falls back to the number of reconstructed pages. |
| `json.children[]` | one `Page` each | The top-level `json` object is Marker's `Document` block; its children are `Page` blocks. |
| page block `id` (`"/page/10/Page/366"`) | `Page.page_number` | The `/page/<n>/` segment is the **0-based page index in the original document**; PuffinParse adds 1. Falls back to the position in `children`. |
| page block `polygon` | `Page.width` / `Page.height` | Marker pages start at the origin, so the polygon's max x/y are the page size (PDF points for digital PDFs). |
| page block `children[]` | `Page.blocks[]` | Group blocks (`TableGroup`, `FigureGroup`, `ListGroup`, `PictureGroup`) have HTML made only of `<content-ref src=…>` placeholders, so PuffinParse descends into their children instead of emitting the group. |
| block `block_type` | `Block.type` | See the table below. |
| block `html` | `Block.content` | Converted to markdown best-effort: `<h1>`–`<h6>` → `#`s, `<li>` → `- `, `<table>` → a markdown table when it is simple (rectangular, no `colspan`/`rowspan`, not nested) and the original HTML otherwise, `<math>` → `$$…$$`, everything else → tag-stripped text. With `output="text"` the tag-stripped text is used. |
| block `html` (stripped) | `Block.text` | |
| block `polygon` | `Block.bbox` | 4 points in page units; PuffinParse takes the enclosing box and divides by the page polygon's size. |
| — | `Block.confidence` | Marker reports no per-block confidence. `parse_quality_score` is document-level. |
| `markdown` (paginated) | `Page.markdown` | Split on Marker's page markers; preferred over the blocks joined together. `Page.text` is `markdown_to_text` of it. |
| `parse_quality_score` | `metadata.datalab_parse_quality_score` | 0–5; < 3.0 is Datalab's own "retry with `accurate`" threshold. |
| `cost_breakdown` | `metadata.datalab_cost_breakdown` | Documented as "cost in cents", shape unspecified, so it is passed through verbatim rather than mapped to `Usage.provider_cost_usd`. |
| `checkpoint_id` | `metadata.datalab_checkpoint_id` | Only when `save_checkpoint=true` was requested. |
| `metadata.failed_pages` | `metadata.datalab_failed_pages` | Only when non-empty. 0-based original page numbers. |
| `images`, `metadata.table_of_contents`, `metadata.page_stats`, `versions`, `html`, `chunks`, `runtime` | — | Not mapped; visible with `include_raw=True`. |

**Page markdown splitting.** With `paginate=true` Marker writes `\n\n{<page_id>}` + 48 dashes +
`\n\n` **before** each page's content (`marker/renderers/markdown.py`), where `page_id` is the same
0-based index used in block ids. PuffinParse recognises any `{n}` + ≥ 8 dashes line, so a custom
`page_separator` still works.

**Block types** (Marker's vocabulary, `marker/schema/__init__.py`):

| Marker `block_type` | `BlockType` |
|---|---|
| `SectionHeader` rendered as `<h1>` | `title` |
| `SectionHeader` (`<h2>`…`<h6>`) | `section_header` |
| `Text`, `TextInlineMath`, `Handwriting`, `Form`, `Code`, `Reference`, `Span`, `Line` | `text` |
| `ListItem`, `ListGroup` | `list` |
| `Table`, `TableGroup`, `TableCell` | `table` |
| `Figure`, `FigureGroup`, `Picture`, `PictureGroup` | `figure` |
| `Caption` | `caption` |
| `Footnote` | `footnote` |
| `PageHeader` | `header` |
| `PageFooter` | `footer` |
| `Equation` | `formula` |
| `TableOfContents`, `ComplexRegion`, `Document`, `Page`, anything else | `other` |

Marker has no `Title` type: the document title is a `SectionHeader` rendered as `<h1>`, which
PuffinParse maps to `title` so the block vocabulary matches the other providers.

**`ocr` mode** is derived from `parse` (`TextResponse::from_parse`): lines are block text split on
newlines, carrying the block's box; `words` have no geometry. Datalab does have a real word-level
product (`word_bboxes=true`, +$3 per 1 000 pages) but it annotates **HTML output** with
`data-bbox` / `data-confidence` spans, which PuffinParse does not request or parse.

## 5. Errors, status codes, rate limits, timeouts

Every HTTP error is `{"detail": "message"}` (or a FastAPI validation array), which `Error::from_http`
picks up:

| Status | Datalab type | PuffinParse `ErrorKind` |
|---|---|---|
| 400 | `invalid_request_error` (bad file type, file too large) | `bad_request` |
| 401 | `authentication_error` (`"Invalid API key provided. Set the X-API-Key header…"`) | `authentication` |
| 402 | `spend_cap_error` (30-day spend cap reached) | `bad_request` — **not** an auth error, watch for it |
| 403 | `permission_error` (no active subscription, expired plan, failed payment) | `authentication` |
| 404 | `not_found_error` (request id expired — results live **1 hour**) | `bad_request` |
| 413 | `request_too_large` (> 200 MB) | `bad_request` |
| 422 | validation error | `bad_request` |
| 429 | `rate_limit_error` (requests/min or concurrency) | `rate_limit` (retried) |
| 500 / 529 | `api_error` / `overloaded_error` | `provider` (500 retried; 529 is **not** in the retry set) |

**Job-level failures** come back with HTTP 200: `{"success": false, "error": "…"}`. PuffinParse turns
those into a `provider` error carrying the message and the `request_id` — except the **page
concurrency limit**, which is enforced during processing rather than at submission
(`"Page rate limit exceeded. Your team has … pages in flight …"`) and is classified as `rate_limit`
so a router can back off or fall back.

**Limits.** 200 MB per file, 7 000 pages per request, 1 hour result retention. Rate limits are
per plan: free tier 25 requests/min and 25 concurrent (the API-limits page still says 10/5), Team
400/400. The page-concurrency ceiling is 5 000 pages in flight per team by default.

**Timeouts.** `timeout_secs` (default 300) covers submit + polling + the `result_url` download and
caps each request.

## 6. Gotchas

* **`/api/v1/marker` is deprecated.** The reference points at `/convert`, `/extract`, `/segment` and
  `/agent`. `use_llm`, `force_ocr` and `format_lines` are gone; `mode` (`fast`/`balanced`/`accurate`)
  replaces them.
* **`status: "complete"` does not mean success.** Check `success`; a failed document is reported as
  complete with `success: false` and an `error` string.
* **EU results are not inline.** When `result_url` is present the content is only there, and the
  download must go out **without** the `X-API-Key` header. Keep the non-null poll fields on top of
  the downloaded body: billing and confidence numbers can change after the document was stored.
  (Datalab's own Python SDK 0.5.0 does not follow `result_url` for `convert()`.)
* **Results are deleted one hour after processing.** There is no way to re-fetch afterwards.
* **Page numbers are original, not sequential.** With `page_range="5-7"` the block ids stay
  `/page/5/…`, so PuffinParse's pages are 6, 7, 8 — deliberately, so boxes and page numbers still refer
  to the input document. `Usage.pages` is `page_count`, i.e. the pages actually converted.
* **`page_range` is 0-based** while PuffinParse's `pages` is 1-based; for spreadsheets the same field
  selects **sheet** indices instead.
* **Group blocks carry no content.** `TableGroup`, `FigureGroup`, `ListGroup` and `PictureGroup`
  have `html` consisting only of `<content-ref src='…'>` placeholders. PuffinParse replaces them with
  their children; a naive client that reads group HTML gets empty blocks.
* **Blocks are HTML, pages are markdown.** The `json` output never contains markdown unless you set
  `include_markdown_in_chunks=true`. PuffinParse requests both formats so blocks keep their types and
  boxes while the page text stays Marker's own rendering.
* **Caching is on by default.** A repeated conversion of the same file can be served from cache and
  `checkpoint_reused: true` means the conversion step is not re-billed — good for cost, fatal for
  latency benchmarks. Pass `provider_options={"skip_cache": True}` when measuring.
* **Spreadsheets bill by cells, not pages** (2 500 cells per page capped at $0.60/sheet in simple
  mode, 500 cells per page in advanced), so `Usage.pages × per_page_usd` is wrong for `.xlsx` input.
* **Images count as one page each**, and multi-page TIFF frames count individually.
* **HTML prettifying changes spacing.** The default HTML is indented per tag, which browsers render
  as stray spaces around inline tags (`( 23 )`); `disable_html_prettify=true` fixes it. It affects
  `Block.content` for blocks kept as HTML.
* **`merge_cross_page` bills a variable compute surcharge** (~$0.50/document) that the price table
  cannot model, and it applies on every run.

## 7. Useful `provider_options` passthrough

```python
# 1. Benchmarking: defeat the result cache so latency and cost are real.
puffinparse.parse("doc.pdf", model="datalab/balanced", provider_options={"skip_cache": True})

# 2. Per-cell and per-list-item boxes (HTML output, +$6 per 1k pages each).
puffinparse.parse("statement.pdf", model="datalab/accurate", include_raw=True,
              provider_options={"extras": "table_cell_bboxes,list_item_bboxes", "word_bboxes": True,
                                "output_format": "json,html", "disable_html_prettify": True})

# 3. Keep running headers and footers, and preserve spreadsheet formatting.
puffinparse.parse("report.pdf", model="datalab/balanced",
              provider_options={"additional_config": {"keep_pageheader_in_output": True,
                                                      "keep_pagefooter_in_output": True,
                                                      "keep_spreadsheet_formatting": True}})

# 4. Stitch tables and paragraphs that continue across page breaks (beta, compute-billed).
puffinparse.parse("annual-report.pdf", model="datalab/accurate",
              provider_options={"merge_cross_page": True})

# 5. Token-efficient markdown for an LLM pipeline, no images or synthetic captions.
puffinparse.parse("doc.pdf", model="datalab/fast",
              provider_options={"token_efficient_markdown": True, "disable_image_extraction": True,
                                "disable_image_captions": True})

# 6. EU data residency (requires file_url or a pre-uploaded datalab:// reference, 1.25x usage).
puffinparse.parse("https://example.com/doc.pdf", model="datalab/balanced",
              provider_options={"processing_location": "eu"})

# 7. Save a checkpoint so a later /extract or /segment call skips re-parsing.
puffinparse.parse("doc.pdf", model="datalab/balanced", provider_options={"save_checkpoint": True})
```

## 8. Links

* Docs home: <https://documentation.datalab.to>
* Convert API guide: <https://documentation.datalab.to/docs/recipes/conversion/conversion-api-overview>
* `POST /api/v1/convert` reference: <https://documentation.datalab.to/api-reference/convert-document>
* Result polling reference: <https://documentation.datalab.to/api-reference/convert-result-check>
* OpenAPI spec: <https://www.datalab.to/openapi.json>
* Error codes: <https://documentation.datalab.to/platform/errors>
* Limits and rate limiting: <https://documentation.datalab.to/docs/common/limits>
* Billing (what counts as a page): <https://documentation.datalab.to/platform/billing>
* Pricing rate card: <https://www.datalab.to/pricing>
* Marker (the open-source engine, JSON/chunks/metadata shapes):
  <https://github.com/datalab-to/marker>
* Machine-readable docs index: <https://documentation.datalab.to/llms.txt>
