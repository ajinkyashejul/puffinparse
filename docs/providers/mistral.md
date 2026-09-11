# Mistral

## 1. Summary

| | |
|---|---|
| Provider name | `mistral` |
| Base URL | `https://api.mistral.ai` (override: `base_url` on the request, or `MISTRAL_BASE_URL`) |
| API key | `MISTRAL_API_KEY` (or `api_key` on the request) — sent as `Authorization: Bearer <key>` |
| Docs | <https://docs.mistral.ai/capabilities/OCR/basic_ocr> · API reference <https://docs.mistral.ai/api/> |
| API version | Unversioned path prefix `/v1`; no version header. Model version is pinned through the model id (`mistral-ocr-4-1`, …). |
| Verified | 2026-09-11, against the published docs and the machine-readable spec at <https://docs.mistral.ai/openapi.yaml> (no live key in this environment — see §6) |
| Implementation | `crates/liteocr-core/src/providers/mistral.rs` |

Mistral's Document AI OCR is a **single synchronous call**: `POST /v1/ocr` returns the whole document's
markdown, per-page image boxes and (on OCR 4+) paragraph-level blocks in one response. There is no job
queue, no polling and no result indirection, which makes it the simplest provider in LiteOCR. The same
endpoint also does schema-driven extraction (`document_annotation_format`), so every `mistral` model
serves all three modes — `parse`, `ocr` and `extract` — from the same call.

## 2. Models exposed by LiteOCR

| Model | Mistral model id | List price (`pricing.json`) |
|---|---|---|
| `mistral/ocr-latest` *(default)* | `mistral-ocr-latest` (currently aliases OCR 4.1) | $0.004 / page · $0.005 / annotated page |
| `mistral/ocr-4-1` | `mistral-ocr-4-1` (OCR 4.1, GA 2026-07-16) | $0.004 / page · $0.005 / annotated page |
| `mistral/ocr-4-0` | `mistral-ocr-4-0` (OCR 4.0, GA 2026-06-23) | $0.004 / page · $0.005 / annotated page |
| `mistral/ocr-2512` | `mistral-ocr-2512` (OCR 3, GA 2025-12-18) | $0.002 / page · $0.003 / annotated page |

All four serve `parse`, `ocr` and `extract`. Prices are the public per-1 000-page list prices from the
model cards (<https://docs.mistral.ai/models/ocr-4-1>, <https://docs.mistral.ai/models/ocr-3-25-12>):
$4 / 1 000 pages and $5 / 1 000 *annotated* pages for OCR 4.x, $2 / $3 for OCR 3. The "annotated page"
rate is what `extract` mode bills, which is why the `extract` price in `pricing.json` is higher than
`parse`/`ocr` for the same model.

`mistral-ocr-2503` and `mistral-ocr-2505` (OCR 1 / OCR 2) are **retired** (2025-12-31 and 2026-05-31)
and are deliberately not exposed.

Feature availability differs by version and LiteOCR does not paper over it:

| Feature | OCR 3 (`ocr-2512`) | OCR 4.0 | OCR 4.1 |
|---|---|---|---|
| Markdown + image boxes | yes | yes | yes |
| `include_blocks` (paragraph blocks + labels) | accepted, returns empty | yes | yes |
| `confidence_scores_granularity` block scores | no | no | yes |
| `table_format`, `extract_header`, `extract_footer` | yes (OCR 2512+) | yes | yes |
| Annotations (`extract`) | yes | yes | yes |

## 3. Request flow LiteOCR uses

Every request carries `Authorization: Bearer $MISTRAL_API_KEY`. There is no workspace or version header.

1. **Input reference.** LiteOCR builds the `document` chunk:
   * **URL input** — passed straight through, and Mistral downloads it server-side.
     `{"type":"document_url","document_url":"…","document_name":"y.pdf"}`, or
     `{"type":"image_url","image_url":"…"}` when the URL's extension is an image type.
   * **Local image ≤ 10 MB** (path or bytes) — inlined as a data URL:
     `{"type":"image_url","image_url":"data:image/png;base64,…"}`. No upload round-trip, nothing left
     behind in the workspace's file storage.
   * **Everything else** (PDFs, DOCX, PPTX, large images) — `POST {base}/v1/files`,
     `multipart/form-data` with `purpose=ocr` and a `file` part → `{"id":"<uuid>",…}`, then
     `GET {base}/v1/files/{id}/url?expiry=1` → `{"url":"https://…blob.core.windows.net/…?sig=…"}`, and
     that signed URL becomes `document_url`. The expiry is one hour — the shortest the API allows — and
     it only needs to outlive the OCR call.
2. **OCR.** `POST {base}/v1/ocr` with `Content-Type: application/json`:

   ```json
   {
     "model": "mistral-ocr-latest",
     "document": { "type": "document_url", "document_url": "…", "document_name": "invoice.pdf" },
     "include_image_base64": false
   }
   ```

   * `pages="1-3,7"` becomes `"pages": [0,1,2,6]` — **LiteOCR's page numbers are 1-based, Mistral's are
     0-based**. Open-ended ranges (`"10-"`) are rejected with an `input` error, because the API has no
     way to express "to the end".
   * `include_image_base64` is pinned to `false` so responses stay small; set it through
     `provider_options` when you want the cropped images in `resp.raw`.
   * `language` is **ignored** — the OCR endpoint has no language parameter (the model is multilingual
     across 40+ languages and auto-detects).
   * LiteOCR does not send `include_blocks`; the API defaults it to `true`, so OCR 4.x returns blocks.
3. **Extract.** Same call, plus:

   ```json
   {
     "document_annotation_format": {
       "type": "json_schema",
       "json_schema": { "name": "document_annotation", "schema": { …your JSON Schema… }, "strict": true }
     },
     "document_annotation_prompt": "…instructions, when given…"
   }
   ```

   `strict` is `true` only when your schema already declares `"additionalProperties": false`; Mistral's
   strict mode requires a closed schema, so an open schema is sent with `strict: false` rather than
   being rejected by the API. `ExtractRequest.instructions` maps to `document_annotation_prompt`.
4. **Ocr mode** is derived from `parse` by `TextResponse::from_parse` — Mistral has no word- or
   line-level text endpoint, so `Line`s come from block content and `Word`s carry no geometry.

**Where `provider_options` are merged:** the whole object is deep-merged into the request body after
LiteOCR's own fields, so its keys are top-level `/v1/ocr` request keys — `include_image_base64`,
`image_limit`, `image_min_size`, `table_format`, `extract_header`, `extract_footer`, `include_blocks`,
`confidence_scores_granularity`, `bbox_annotation_format`, `document_annotation_prompt`, and even
`document`/`pages` if you want to override the resolved input. Nested objects merge key-wise, so
`{"document_annotation_format": {"json_schema": {"strict": true}}}` flips just that flag.

## 4. Response mapping

| Mistral field | LiteOCR unified field | Notes |
|---|---|---|
| — | `OcrResponse.provider_job_id` | Never set: the call is synchronous and returns no job id. |
| `pages[].index` | `Page.page_number` | **0-based on the wire**, `page_number = index + 1`. |
| `pages[].markdown` | `Page.markdown` | Used verbatim; `output="text"` runs it through `markdown_to_text`. Whole-document `markdown` is the pages joined by a blank line. |
| `pages[].dimensions.{width,height}` | `Page.width` / `Page.height` | Pixels of the page screenshot at `dimensions.dpi` (typically 200), not PDF points. |
| `pages[].dimensions.dpi` | `metadata.mistral_dpi` | From the first page. |
| `pages[].blocks[]` | `Page.blocks[]` | Present on OCR 4+ (`include_blocks` defaults to `true`), in reading order. |
| `blocks[].type` | `Block.type` | See mapping below. |
| `blocks[].content` | `Block.content` | Markdown (or HTML for tables when `table_format: "html"`). |
| `blocks[].{top_left_x,top_left_y,bottom_right_x,bottom_right_y}` | `Block.bbox` | Absolute pixels → divided by `dimensions.width`/`height`, origin top-left. No dimensions ⇒ `None`. |
| `blocks[].confidence_scores.average_content_confidence_score` | `Block.confidence` | Only populated when `confidence_scores_granularity: "block"` is requested (OCR 4.1). |
| `pages[].images[]` | `Page.blocks[]` (`figure`) | **Fallback only**, when `blocks` is absent/empty: one `figure` block per image with `content` = the markdown placeholder `![img-0.jpeg](img-0.jpeg)` and the image's box. |
| `pages[].markdown` | `Page.blocks[0]` (`text`) | **Fallback only**: a single `text` block per page carrying the page markdown, with no bbox. |
| `usage_info.pages_processed` | `Usage.pages` | Falls back to the number of returned pages when 0/absent. |
| `usage_info.doc_size_bytes` | `metadata.mistral_doc_size_bytes` | |
| `model` | `metadata.mistral_model` | The concrete model that served the call (`mistral-ocr-4-1` even when you asked for `-latest`). |
| `document_annotation` | `ExtractResponse.data` | A JSON **string**, parsed into `data`. Missing or unparseable ⇒ `provider` error. |
| — | `ExtractResponse.fields` | Always empty: Mistral returns no per-field confidence or citations. |
| — | `Usage.credits`, `Usage.provider_cost_usd` | Never set; `cost_usd` comes from `pricing.json`. |

Block types: `text`, `aside_text` → `text`; `title` → `title`; `list` → `list`; `table` → `table`;
`image` → `figure`; `equation` → `formula`; `caption` → `caption`; `header` → `header`; `footer` →
`footer`; `code`, `references`, `signature` and anything unknown → `other`.

Trimmed response (`crates/liteocr-core/tests/fixtures/mistral_ocr.json`, second page only, base64 redacted):

```json
{
  "pages": [
    {
      "index": 1,
      "markdown": "![img-0.jpeg](img-0.jpeg)\n\nFigure 1: Quarterly revenue by segment.\n\nReference: ABC-9876",
      "images": [
        {
          "id": "img-0.jpeg",
          "top_left_x": 292, "top_left_y": 217,
          "bottom_right_x": 1405, "bottom_right_y": 649,
          "image_base64": "data:image/jpeg;base64,REDACTED",
          "image_annotation": null
        }
      ],
      "tables": [], "hyperlinks": [], "header": null, "footer": null,
      "dimensions": { "dpi": 200, "height": 2200, "width": 1700 },
      "confidence_scores": null,
      "blocks": null
    }
  ],
  "model": "mistral-ocr-latest",
  "document_annotation": null,
  "usage_info": { "pages_processed": 2, "doc_size_bytes": 30021 }
}
```

`crates/liteocr-core/tests/fixtures/mistral_ocr_blocks.json` covers the OCR 4.x `blocks` payload
(labels, boxes, block confidence) and `mistral_annotation.json` the `document_annotation` string.

## 5. Errors, status codes, rate limits, timeouts

Every non-2xx body goes through `Error::from_http`, which pulls a message out of `message` / `detail` /
`error` and classifies by status:

| Status | Mistral body | LiteOCR `ErrorKind` |
|---|---|---|
| 400 | `{"object":"error","message":"…","type":"invalid_request_error","param":null,"code":null}` — unreachable `document_url`, unsupported file type, bad page index | `bad_request` |
| 401 | `{"message":"Unauthorized","request_id":"…"}` — missing/invalid key | `authentication` |
| 403 | Key lacks access to the model (e.g. a Premier model on a free workspace) | `authentication` |
| 404 | Unknown `file_id` on `/v1/files/{id}/url` | `bad_request` |
| 422 | FastAPI validation array: `{"detail":[{"loc":["body","document"],"msg":"…","type":"…"}]}` — the whole array is kept as the message | `bad_request` |
| 429 | Requests-per-second or tokens-per-minute limit | `rate_limit` (retried) |
| 500 | `{"object":"error","message":"Internal Server Error",…}` | `provider` (retried) |
| 502/503/504 | Gateway / capacity | `provider` (retried) |

**Retries.** `max_retries` (default 2) with exponential backoff and full jitter, on rate-limit, network
and 500/502/503/504 only. 4xx is never retried. The retry wraps each of the three calls (upload, signed
URL, OCR) independently.

**Rate limits.** Enforced per workspace as requests/second *and* tokens/minute, with a monthly token
cap; the limits in force are shown at Admin ▸ API ▸ Limits. Mistral does return an
`X-RateLimit-Remaining` header — the only provider in LiteOCR that does — but LiteOCR does not read it
today, so backoff is still blind. Free-tier workspaces have the lowest limits.

**Timeouts.** `timeout_secs` (default 300) is a whole-call deadline covering upload, signed-URL
retrieval and the OCR call, and also caps each individual HTTP request. Because `/v1/ocr` is
synchronous, a 1 000-page PDF is one long request — raise `timeout_secs` rather than expecting a job
id, or use Mistral's Batch API directly for bulk work.

## 6. Gotchas (verified)

* **No live-key verification.** Unlike the other provider pages, this one was written from the published
  docs and the OpenAPI spec (`https://docs.mistral.ai/openapi.yaml`), not from live traffic; the
  fixtures are built from the documented response examples. Treat exact field-by-field behaviour as
  "documented", not "observed", until the `#[ignore]`d live tests in `providers/mistral.rs`
  (`mistral_live_parse`, `mistral_live_extract`) are run with a key.
* **`pages` is 0-based.** The request array, and `pages[].index` in the response. LiteOCR converts in
  both directions; if you pass `pages` through `provider_options` yourself, you own the conversion.
* **The API reference's example response shows `"index": 1` for the first page**, contradicting the
  schema ("The page index in a pdf document starting from 0") and the `pages` parameter description.
  LiteOCR trusts the schema: `page_number = index + 1`.
* **50 MB / 1 000 pages per document.** Larger files are rejected. The files API itself accepts up to
  512 MB, so the OCR limit is what binds.
* **`include_blocks` defaults to `true`** (per the OpenAPI spec) but OCR 3 and older *accept the
  parameter and return an empty array*. That is why LiteOCR keeps the "one text block + one figure block
  per image" fallback: with `mistral/ocr-2512` you get exactly that, with `ocr-4-x` you get real
  paragraph blocks. Block counts therefore differ sharply between models.
* **Confidence is opt-in.** No `confidence_scores_granularity` ⇒ every `Block.confidence` is `None`.
  Ask for `"block"` (OCR 4.1) to populate it; `"word"` adds a large `word_confidence_scores` array that
  LiteOCR does not surface (visible in `resp.raw` with `include_raw=True`).
* **Images and tables appear in the markdown as placeholders** — `![img-0.jpeg](img-0.jpeg)` and, when
  `table_format` is set, `[tbl-3.html](tbl-3.html)`. The bytes/HTML live in `pages[].images[]` and
  `pages[].tables[]`. LiteOCR leaves the placeholders in the markdown; with the default
  `table_format: null` tables are inlined as markdown and no placeholder appears, which is why LiteOCR
  does not set `table_format`.
* **`document_annotation` is a JSON string, not an object** — double-encoded inside the response. It is
  `null` when no `document_annotation_format` was sent.
* **Document annotation only sees the first eight image bounding boxes** (the OCR markdown plus those
  images is what the vision model is shown), so `extract` is best on text-heavy documents. There is no
  documented page cap, but the vendor's own examples restrict `pages` to the first eight.
* **Strict schemas.** Mistral's `strict: true` follows the OpenAI convention: the schema must be closed
  (`additionalProperties: false`, every property `required`). LiteOCR only claims strictness when your
  schema already says so — otherwise the model is asked to follow the schema best-effort.
* **`extract` bills the "annotated page" rate** ($5 / 1 000 pages on OCR 4.x, $3 on OCR 3), and it runs
  a vision LLM *after* OCR, so it is markedly slower than `parse` on the same document.
* **No citations.** `ExtractResponse.fields` is always empty. Requesting `citations=True` sets
  `metadata.mistral_citations_unsupported = true` instead of failing.
* **Signed URLs are public-ish.** `GET /v1/files/{id}/url` returns an unauthenticated blob URL; LiteOCR
  requests the minimum one-hour expiry. Uploaded files stay in the workspace until deleted — LiteOCR
  does not delete them (a `DELETE /v1/files/{id}` sweep would race with retries).
* **`{"type":"file","file_id":"<uuid>"}` is also a valid `document`** (the spec's `FileChunk`), which
  would skip the signed-URL hop. LiteOCR uses the signed URL because that is the flow the guides
  document and exercise; pass a `FileChunk` yourself through `provider_options={"document": {...}}` if
  you already have a `file_id`.
* **`bbox_annotation_format` is not wired into a LiteOCR mode.** It annotates individual figures rather
  than the document, so it only makes sense as a passthrough (see §7); the annotations come back in
  `pages[].images[].image_annotation` and are visible with `include_raw=True`.

## 7. Useful `provider_options` passthrough

```python
# 1. Real paragraph blocks with per-block confidence (OCR 4.1 only).
liteocr.ocr("scan.pdf", model="mistral/ocr-4-1",
            provider_options={"confidence_scores_granularity": "block"})

# 2. Tables as separate HTML, and headers/footers split out of the body text.
liteocr.ocr("report.pdf", model="mistral/ocr-latest", include_raw=True,
            provider_options={"table_format": "html", "extract_header": True, "extract_footer": True})

# 3. Keep the cropped images (base64) in resp.raw, ignoring anything smaller than 200 px.
liteocr.ocr("figures.pdf", model="mistral/ocr-latest", include_raw=True,
            provider_options={"include_image_base64": True, "image_min_size": 200, "image_limit": 20})

# 4. Structured extraction with a prompt (instructions → document_annotation_prompt).
liteocr.extract("invoice.pdf", model="mistral/ocr-latest", schema=INVOICE_SCHEMA,
                instructions="Amounts are in EUR; ignore the shipping address.")

# 5. Caption every figure while parsing (bbox annotations land in resp.raw).
liteocr.ocr("paper.pdf", model="mistral/ocr-latest", include_raw=True,
            provider_options={"include_image_base64": True, "bbox_annotation_format": {
                "type": "json_schema",
                "json_schema": {"name": "bbox_annotation", "strict": True, "schema": {
                    "type": "object", "additionalProperties": False,
                    "properties": {"image_type": {"type": "string"}, "summary": {"type": "string"}},
                    "required": ["image_type", "summary"]}}}})
```

## 8. Links

* OCR processor guide: <https://docs.mistral.ai/capabilities/OCR/basic_ocr>
* Annotations guide: <https://docs.mistral.ai/capabilities/OCR/annotations>
* API reference (`POST /v1/ocr`): <https://docs.mistral.ai/api/endpoint/ocr> · full spec:
  <https://docs.mistral.ai/openapi.yaml>
* Files API: <https://docs.mistral.ai/api/> (tag `files`) — `POST /v1/files`, `GET /v1/files/{id}/url`
* Model cards: <https://docs.mistral.ai/models/ocr-4-1> · <https://docs.mistral.ai/models/ocr-4-0> ·
  <https://docs.mistral.ai/models/ocr-3-25-12>
* Pricing: <https://mistral.ai/pricing>
* Batch API (bulk OCR at 50% off): <https://docs.mistral.ai/capabilities/batch>
