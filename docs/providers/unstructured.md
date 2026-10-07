# Unstructured

> **Status: docs-only.** Everything below comes from the official Unstructured documentation and
> the live OpenAPI spec at `https://api.unstructuredapp.io/general/openapi.json` (read 2026-09-11),
> and from `crates/puffinparse-core/src/providers/unstructured.rs`. No live call has been made — this
> repository has no Unstructured key. `crates/puffinparse-core/tests/fixtures/unstructured_elements.json`
> is hand-built from the documented element shapes. Mark this page **verified** only after the
> `#[ignore]`d live test in `providers/unstructured.rs` passes with a real key.
> Help verify it: [issue #10](https://github.com/ajinkyashejul/puffinparse/issues/10).

## 1. Summary

| | |
|---|---|
| Provider name | `unstructured` |
| Base URL | `https://api.unstructuredapp.io` (override: `base_url` on the request, or `UNSTRUCTURED_BASE_URL`). **Business accounts get their own URL** at sign-up and must use it |
| API key | `UNSTRUCTURED_API_KEY` (or `api_key` on the request) — sent as the `unstructured-api-key` header |
| Docs | <https://docs.unstructured.io/api-reference/partition/overview> |
| API version | Path-versioned: `POST /general/v0/general`. The deployed spec reports its own build (`1.5.99` when read) |
| Checked against | **not live-verified** (see banner) — documentation read 2026-09-11 |
| Implementation | `crates/puffinparse-core/src/providers/unstructured.rs` |

The Partition Endpoint is the only synchronous, single-file API Unstructured offers, and it is the
one PuffinParse uses. Unstructured labels it **legacy** and steers production users to the Pipelines /
Workflow API (`/api/v1/jobs`, connectors, chunking, embeddings), which is a different product shape
that does not fit a one-document-in / one-document-out call.

## 2. Models exposed by PuffinParse

| Model | Provider parameter | Modes | List price (`pricing.json`) |
|---|---|---|---|
| `unstructured/hi_res` *(default)* | `strategy=hi_res` | `parse`, `ocr` (derived) | $0.015 / page |
| `unstructured/fast` | `strategy=fast` | `parse`, `ocr` (derived) | $0.015 / page |
| `unstructured/auto` | `strategy=auto` | `parse`, `ocr` (derived) | $0.015 / page |

Pricing is **flat per page for every strategy**: <https://unstructured.io/pricing> lists
Pay-As-You-Go at **$0.015 per page** with the first 10 000 pages free ("Let's Go"), all features
included; Business is custom. There is no cheaper rate for `fast` — the saving is latency, not
money.

What the strategies do (<https://docs.unstructured.io/concepts/partitioning>):

* `hi_res` — layout model + OCR; the only strategy that reliably produces `coordinates`,
  `text_as_html` for tables and `detection_class_prob`. Default here for that reason.
* `fast` — rule-based text extraction from the file's own text layer. No OCR, so it cannot read
  scans, and it is **rejected for image files**.
* `auto` — routes each page to fast / hi_res / VLM at runtime.

`ocr_only`, `od_only` and `vlm` are also accepted by the endpoint but are not exposed as models;
reach them with `provider_options={"strategy": "vlm", "vlm_model_provider": …, "vlm_model": …}`
(which overrides the model's strategy — see §3).

## 3. Request flow PuffinParse uses

One synchronous call; there is no job to poll.

`POST {base}/general/v0/general`, `multipart/form-data`, headers `unstructured-api-key` and
`accept: application/json`:

| Field | Value |
|---|---|
| `files` | the document bytes with filename and guessed MIME type (note the plural — it is `files`, not `file`) |
| `strategy` | the model name (`hi_res` \| `fast` \| `auto`) |
| `output_format` | `application/json` |
| `coordinates` | `true` — **off by default**, and without it no element has a bounding box |
| `include_page_breaks` | `true` — emits `PageBreak` elements, which is how page numbers are recovered for file types with no page metadata |
| `languages` | the request's `language`, when set (repeatable field of Tesseract codes) |

Response: a JSON **array** of element objects (no envelope).

**URL inputs are downloaded first.** The Partition Endpoint has no remote-URL parameter, so a
`DocumentInput::Url` is fetched by PuffinParse (respecting the call deadline) and uploaded as bytes. A
non-2xx download, or an empty body, is an `input` error.

**`pages` is applied client-side.** The endpoint has no page-selection parameter (`starting_page_number`
only renumbers pre-split PDFs), so PuffinParse filters elements by `metadata.page_number` after the fact
and sets `metadata.unstructured_pages_filtered_client_side = true`. **You are still billed for the
whole document**, so `Usage.pages` reports every page the API processed, not the filtered subset.

**Where `provider_options` are merged:** the request is a multipart form, so options become extra
text fields. Strings pass through; booleans become `"true"`/`"false"`; numbers are stringified;
arrays become **repeated fields** (which is what `languages`, `extract_image_block_types` and
`skip_infer_table_types` expect); `null` is skipped; objects are serialised as JSON text. A key
PuffinParse already set is replaced, so `provider_options` can override `strategy`, `coordinates` or
`output_format`.

## 4. Response mapping

| Unstructured field | PuffinParse unified field | Notes |
|---|---|---|
| `type` (+ `metadata.category_depth`) | `Block.type` | See the table below. |
| `text` | `Block.text` | Always the plain text of the element; for tables, the cell text run together. |
| `text` / `metadata.text_as_html` | `Block.content` | Markdown rendering: `Title` → `# `, deeper headings → `##`… by `category_depth`, `ListItem` → `- `, `Table` → `text_as_html` converted to a markdown table when it is simple (rectangular, no `colspan`/`rowspan`, not nested) and left as HTML otherwise. With `output="text"` the raw `text` is used. |
| `metadata.page_number` | `Block.page_number` → `Page.page_number` | Missing page numbers fall back to a counter that increments on every `PageBreak`. |
| `metadata.coordinates.points` | `Block.bbox` | Four polygon corners **in pixels**, top-left origin, listed counter-clockwise from the top-left. PuffinParse takes the enclosing axis-aligned box. |
| `metadata.coordinates.layout_width` / `layout_height` | `Page.width` / `Page.height`, and the bbox divisor | Also read from `coordinates.system` when a payload nests them there. No dimensions ⇒ `bbox = None`. |
| `metadata.detection_class_prob` | `Block.confidence` | Only produced by `hi_res`. |
| — | `Page.markdown` / `Page.text` | Assembled from the page's blocks in order (`pages_from_blocks`); Unstructured returns no page-level rendering of its own. |
| — | `Usage.pages` | Number of distinct pages seen in the response (before any `pages` filter), at least 1. |
| `metadata.filetype` | `metadata.unstructured_filetype` | |
| `element_id`, `metadata.parent_id`, `languages`, `filename`, `last_modified`, `emphasized_text_*`, `image_base64`, `links`, `orig_elements` | — | Not mapped; visible with `include_raw=True`. |
| `PageBreak` elements | — | Consumed as page delimiters, never emitted as blocks. |

**Element types** (<https://docs.unstructured.io/concepts/document-elements>):

| Unstructured `type` | `BlockType` |
|---|---|
| `Title` with `category_depth` 0 / absent | `title` |
| `Title` with `category_depth` ≥ 1, `SectionHeader`, `Headline`, `Subtitle` | `section_header` |
| `NarrativeText`, `UncategorizedText`, `Text`, `CompositeElement`, `Address`, `EmailAddress`, `CodeSnippet`, `FormKeysValues`, `Field-Name`, `Value`, `Abstract`, `Threading` | `text` |
| `ListItem`, `List-item`, `BulletedText` | `list` |
| `Table`, `TableChunk` | `table` |
| `Image`, `Picture`, `Figure` | `figure` |
| `Header`, `PageHeader` | `header` |
| `Footer`, `PageFooter` | `footer` |
| `Footnote` | `footnote` |
| `FigureCaption`, `Caption` | `caption` |
| `Formula` | `formula` |
| `PageNumber` and anything unlisted | `other` |
| `PageBreak` | *(dropped)* |

**`ocr` mode** is derived from `parse` (`TextResponse::from_parse`): each block's text becomes one
or more lines carrying the block's box; `words` have no geometry. Unstructured's own output is
element-level — there is no word-level API on this endpoint.

## 5. Errors, status codes, rate limits, timeouts

| Status | Body | PuffinParse `ErrorKind` |
|---|---|---|
| 401 / 403 | `{"detail": "…"}` (missing or invalid `unstructured-api-key`) | `authentication` |
| 402 | payment required / quota exhausted | `bad_request` — **not** an auth error |
| 422 | `{"detail": [ValidationError…]}` (bad enum, unparsable field) | `bad_request` |
| 429 | rate limited | `rate_limit` (retried) |
| 4xx other | `{"detail": "…"}` | `bad_request` |
| 5xx | `{"detail": "An error occurred"}` (`ServerError`) | `provider` (500/502/503/504 retried) |

`Error::from_http` reads `detail` (stringifying the validation-array form), so the provider message
is never swallowed.

**Rate limits and quotas** are not published as numbers in the docs; the troubleshooting page only
describes `HTTP 402 Payment Required` and `HTTP 429 Too Many Requests` and tells you to slow down or
upgrade. Assume blind backoff — PuffinParse's default (`max_retries=2`, exponential with full jitter).

**Timeouts.** The call is synchronous and can take minutes for a large `hi_res` PDF; the quickstart
says so explicitly. `timeout_secs` (default 300) covers the URL download plus the single request.
Raise it for long scans rather than lowering `max_retries` — a retry re-runs (and re-bills) the
whole document.

**Billing pages** are counted as: one page per page/slide/image for `.pdf`, `.pptx`, `.tiff`; the
page metadata for `.docx`; **file size ÷ 100 KB for everything else**. PuffinParse's `Usage.pages` is a
page *count from the response*, so for HTML, email, text and similar inputs it will not match the
billed number.

## 6. Gotchas

* **This endpoint is officially "legacy".** Unstructured recommends the Pipelines / Workflow API for
  production (`/api/v1/jobs`, "latest and highest-performing models"). The Partition Endpoint is
  single-file, synchronous, and the only thing that fits PuffinParse's one-call contract today.
* **`coordinates` is off by default.** Without `coordinates=true` every element comes back without
  geometry, silently. PuffinParse always sends it.
* **The file field is `files`, not `file`.** A `file` part is ignored and the request fails
  validation.
* **`fast` cannot read images.** Sending a PNG/JPG with `strategy=fast` is an error documented as its
  own support page; use `hi_res`, `auto` or `vlm` for images.
* **Tables come back as HTML, not markdown.** `text` is the cell text run together (lossy, no row
  structure) and `metadata.text_as_html` is the structured form. Unstructured's own examples emit
  `<thead><th>…</th></thead>` **without a `<tr>`**, so a naive row splitter produces nothing;
  PuffinParse's converter treats a bare `<thead>` run as one row, and falls back to keeping the HTML
  whenever the table is ragged, spanning or nested.
* **`Title` is used for both the document title and every heading.** `category_depth` is the only
  way to tell them apart, and it is not always present — expect the occasional section header mapped
  to `title`.
* **No page selection.** `pages` is applied client-side and you are billed for the whole document;
  `starting_page_number` only renumbers a PDF you split yourself.
* **No remote URL input**, so PuffinParse downloads and re-uploads, which doubles the bytes on the wire
  for URL inputs.
* **Coordinates are pixels of the rendered page**, not PDF points, and the origin is top-left with
  `y` increasing downwards (the same convention as `BBox`), but the `points` are listed
  **counter-clockwise** from the top-left — only the enclosing box is stable, which is what PuffinParse
  stores.
* **`detection_class_prob` only exists under `hi_res`**, so `Block.confidence` is `None` for `fast`
  and often for `auto`.
* **Chunking changes the element vocabulary.** Passing `chunking_strategy` through
  `provider_options` replaces elements with `CompositeElement` / `TableChunk` chunks (mapped to
  `text` / `table`), and page numbers become chunk-level. Leave it off unless you want chunks.
* **`ocr_languages` is deprecated** in favour of `languages`; both exist in the spec.
* **Business accounts have their own base URL**, handed out at account creation. The documented
  `https://api.unstructuredapp.io` default is the serverless SaaS host; set `UNSTRUCTURED_BASE_URL`
  if yours differs.

## 7. Useful `provider_options` passthrough

```python
# 1. VLM strategy (overrides the model's strategy), e.g. for handwriting-heavy scans.
puffinparse.parse("scan.pdf", model="unstructured/hi_res",
              provider_options={"strategy": "vlm", "vlm_model_provider": "openai",
                                "vlm_model": "gpt-4o"})

# 2. Pick the hi_res layout model and keep table inference on for every file type.
puffinparse.parse("report.pdf", model="unstructured/hi_res",
              provider_options={"hi_res_model_name": "yolox", "pdf_infer_table_structure": True})

# 3. Multi-language OCR (array values become repeated form fields).
puffinparse.parse("contract.pdf", model="unstructured/hi_res",
              provider_options={"languages": ["eng", "deu"]})

# 4. Base64 crops of images and tables in the raw payload.
puffinparse.parse("paper.pdf", model="unstructured/hi_res", include_raw=True,
              provider_options={"extract_image_block_types": ["Image", "Table"]})

# 5. Chunk on the server for a RAG pipeline (changes the element vocabulary — see gotchas).
puffinparse.parse("handbook.pdf", model="unstructured/auto",
              provider_options={"chunking_strategy": "by_title", "max_characters": 2000,
                                "combine_under_n_chars": 500, "include_orig_elements": False})

# 6. Skip table inference for speed, and give every element a UUID.
puffinparse.parse("minutes.docx", model="unstructured/fast",
              provider_options={"skip_infer_table_types": ["docx"], "unique_element_ids": True})
```

## 8. Links

* Partition Endpoint overview: <https://docs.unstructured.io/api-reference/partition/overview>
* Live OpenAPI spec: <https://api.unstructuredapp.io/general/openapi.json> ·
  Swagger UI: <https://api.unstructuredapp.io/general/docs>
* Document elements and metadata: <https://docs.unstructured.io/concepts/document-elements>
* Partitioning strategies: <https://docs.unstructured.io/concepts/partitioning>
* Quota / billing / rate limiting:
  <https://docs.unstructured.io/support/issues/quota-billing-rate-limiting>
* Pricing: <https://unstructured.io/pricing>
* Element type definitions (source of truth):
  <https://github.com/Unstructured-IO/unstructured/blob/main/unstructured/documents/elements.py>
* Machine-readable docs index: <https://docs.unstructured.io/llms.txt>
