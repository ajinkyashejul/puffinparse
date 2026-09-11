# Google Cloud Document AI

## 1. Summary

| | |
|---|---|
| Provider name | `google_documentai` |
| Base URL | `https://{location}-documentai.googleapis.com` — built from the configured location (override: `base_url`, or `GOOGLE_DOCUMENTAI_BASE_URL`) |
| Credential | an **OAuth 2.0 access token** in `GOOGLE_DOCUMENTAI_ACCESS_TOKEN`, `provider_options.access_token`, or `api_key` — sent as `Authorization: Bearer <token>` |
| Required config | `GOOGLE_DOCUMENTAI_PROJECT`, `GOOGLE_DOCUMENTAI_PROCESSOR_ID`, optional `GOOGLE_DOCUMENTAI_LOCATION` (`us` default, `eu`, …) — each overridable via `provider_options` |
| Docs | <https://cloud.google.com/document-ai/docs/reference/rest/v1/projects.locations.processors/process> |
| Modes | `ocr` (**native**), `parse`, `extract` (entities) |
| Verified | 2026-09-11, **from documentation only** — no credentials were available, so the `#[ignore]`d live test has not been run |
| Implementation | `crates/liteocr-core/src/providers/google_documentai.rs` |

Document AI is a fleet of *processors* you create in your own Google Cloud project. LiteOCR makes one
synchronous `:process` call against the processor named in your configuration; the LiteOCR model name
only selects **how the response is read**.

> **Service-account key exchange is out of scope.** LiteOCR does not sign JWTs or talk to
> `oauth2.googleapis.com`. Mint a token yourself — `export GOOGLE_DOCUMENTAI_ACCESS_TOKEN=$(gcloud auth print-access-token)`,
> a metadata-server token on GCE/Cloud Run, or your own service-account exchange — and remember that
> tokens expire after ~1 hour. The required IAM permission is
> `documentai.processors.processOnline` (scope `https://www.googleapis.com/auth/cloud-platform`).

## 2. Models exposed by LiteOCR

| Model | Expected processor type | Modes | List price (`pricing.json`) |
|---|---|---|---|
| `google_documentai/ocr` *(default)* | Enterprise Document OCR (`OCR_PROCESSOR`) | `parse`, `ocr` | $0.0015 / page |
| `google_documentai/layout` | Layout Parser (`LAYOUT_PARSER_PROCESSOR`) | `parse`, `ocr` | $0.01 / page |
| `google_documentai/form` | Form Parser (`FORM_PARSER_PROCESSOR`) | `parse`, `ocr`, `extract` | $0.03 / page |
| `google_documentai/prebuilt` | Invoice / W2 / Expense / Custom Extractor | `parse`, `ocr`, `extract` | $0.03 / page |

Prices from <https://cloud.google.com/document-ai/pricing>: Enterprise Document OCR **$1.50 / 1 000
pages** (first 1 000 pages/month free, $0.60 above 5 M/month); Layout Parser **$10 / 1 000 pages**;
Form Parser and Custom Extractor **$30 / 1 000 pages** ($20 above 1 M/month). Volume tiers and the
free allowance are not modelled — `cost_usd` uses the first paid tier.

The model **must match the processor you configured**: sending a Layout Parser id while asking for
`google_documentai/ocr` yields a document with no `pages[]`, and the parse falls back to
`documentLayout` if it is present. A bare `model="google_documentai"` resolves to `ocr` for `parse`
and `ocr`, and to `form` for `extract` (the first extract-capable entry in the registry).

## 3. Request flow LiteOCR uses

One call, no polling:

```
POST https://{location}-documentai.googleapis.com/v1/projects/{project}/locations/{location}/processors/{processorId}:process
Authorization: Bearer <access token>
Content-Type: application/json
```

With `provider_options.processor_version` the path becomes
`…/processors/{processorId}/processorVersions/{version}:process` (e.g. `pretrained-ocr-v2.0-2023-06-02`).

Body built by `build_body()`:

```json
{
  "rawDocument": { "content": "<base64 of the file>", "mimeType": "application/pdf" },
  "skipHumanReview": true,
  "processOptions": {
    "individualPageSelector": { "pages": [1, 2, 5] },
    "ocrConfig": { "hints": { "languageHints": ["de"] } }
  }
}
```

* **Input.** Path and bytes inputs are base64-encoded into `rawDocument`. Document AI accepts no
  http(s) URL, so a URL input is downloaded by LiteOCR and sent inline. (`gcsDocument` is reachable
  through `provider_options` if your file is already in Cloud Storage — pass
  `{"gcsDocument": {"gcsUri": "gs://…", "mimeType": "application/pdf"}}`; the `rawDocument` key stays
  in the body, so remove it via a full `provider_options` body override if Google rejects both.)
* **`pages`** → `processOptions.individualPageSelector.pages`, 1-based, expanded, sorted and
  de-duplicated. **Open-ended ranges (`"3-"`) are rejected** with an `input` error before any network
  call: the selector needs explicit page numbers (`fromStart`/`fromEnd` are reachable through
  `provider_options`).
* **`language`** → `processOptions.ocrConfig.hints.languageHints`, only for the `ocr` and `form`
  models: the Layout Parser returns an error if `ocrConfig` is set at all.
* **`provider_options`** are deep-merged into the body verbatim, after the five configuration keys
  (`project`, `location`, `processor_id`, `processor_version`, `access_token`) are removed. So
  `{"processOptions": {"ocrConfig": {"enableNativePdfParsing": true}}}` and `{"imagelessMode": true}`
  both work, and an explicit value always wins over LiteOCR's default.

## 4. Response mapping

The response is `{"document": {…}, "humanReviewStatus": {…}}`. `document.text` holds the whole
document's text; everything else points into it with `textAnchor.textSegments[{startIndex, endIndex}]`
(UTF-8 offsets, sent as **strings** because they are `int64`). LiteOCR slices `document.text` by byte
offset, falling back to character offsets when the indices are not byte boundaries.

Fixtures: `crates/liteocr-core/tests/fixtures/google_documentai_{ocr,layout,form}.json`.

### parse — `ocr`, `form`, `prebuilt`

| Document AI field | LiteOCR unified field | Notes |
|---|---|---|
| `pages[].paragraphs[]` | `Page.blocks[]` (type `text`) | Text via `layout.textAnchor`; empty paragraphs dropped. |
| `pages[].tables[]` | `Page.blocks[]` (type `table`) | Rendered as a Markdown table from `headerRows`/`bodyRows` cell anchors; pipes and newlines inside cells are escaped. Emitted *before* the page's paragraphs. |
| — | (paragraph suppression) | A paragraph whose box centre falls inside a table's box is skipped, so table text is not duplicated. |
| `layout.boundingPoly.normalizedVertices[]` | `Block.bbox` | Min/max of the vertices, already 0–1 with a top-left origin. `vertices[]` (absolute pixels) are divided by `dimension`; without dimensions, `bbox` is `None`. |
| `layout.confidence` | `Block.confidence` | 0–1. |
| `pages[].pageNumber` | `Block.page_number` / `Page.page_number` | 1-based; falls back to the array index + 1. |
| `pages[].dimension.{width,height}` | `Page.width` / `Page.height` | In `dimension.unit` (usually points). |
| `pages.len()` | `Usage.pages` | |
| `document.error` | error | A non-zero `error.code` becomes a `provider` error. |

### parse — `layout` (Layout Parser)

| Document AI field | LiteOCR unified field | Notes |
|---|---|---|
| `documentLayout.blocks[]` | `Page.blocks[]` | Flattened depth-first, reading order preserved. |
| `textBlock.type` | `Block.type` + Markdown prefix | `heading-1` → `title` (`# `), `heading-2`/`subtitle` → `section_header` (`## `), `heading-3/4/5` → `section_header` (`###`…), `header` → `header`, `footer` → `footer`, everything else → `text`. |
| `textBlock.blocks[]` | nested blocks | Children are emitted after their parent. |
| `tableBlock` | `Block` (type `table`) | Rendered as a Markdown table; `caption` is prepended when present. |
| `listBlock` | `Block` (type `list`) | `- item` / `1. item` per `listEntries`, depending on `type`. |
| `imageBlock` | `Block` (type `figure`) | Content is `imageText` (OCR/alt text). |
| `pageSpan.pageStart` | `Block.page_number` | 1-based. A block spanning pages is filed under its first page. |
| `boundingBox.normalizedVertices` | `Block.bbox` | |
| max `pageSpan.pageEnd` | `Usage.pages` | The Layout Parser returns no `pages[]`, so page count comes from the spans. |
| `chunkedDocument.chunks[]` | — | Not mapped; visible with `include_raw=True`. |
| — | `Page.width` / `Page.height` | Not available in `documentLayout`. |

### ocr (native)

`mode="ocr"` on `ocr`, `form` and `prebuilt` reads geometry straight off the page:
`pages[].lines[]` → `TextPage.lines` (text, box, confidence), `pages[].tokens[]` → `TextPage.words`
(trimmed, so trailing `detectedBreak` whitespace does not leak into the word), and the page's own
`layout.textAnchor` → `TextPage.text` (falling back to the joined lines). For `layout` there are no
lines or tokens, so `ocr` is derived from the parsed blocks (`liteocr_derived_from=parse`).

### extract — `form`, `prebuilt`

| Document AI field | LiteOCR unified field | Notes |
|---|---|---|
| `entities[].type` | key in `ExtractResponse.data` | Google's own names (`invoice_id`, `total_amount`, `line_item/description`). |
| `entities[].properties[]` | nested object | Recursively, keyed by the child's `type`. |
| repeated `type` | JSON array | Two `line_item` entities become `data["line_item"] == [ {...}, {...} ]`. |
| `normalizedValue` | value | `booleanValue`/`integerValue`/`floatValue`/`signatureValue` are used as-is; `moneyValue`/`dateValue`/`datetimeValue`/`addressValue` are kept as objects with `normalizedValue.text` merged in; otherwise `normalizedValue.text`, else `mentionText`. |
| `entities[].confidence` | `FieldInfo.confidence` | Keyed by JSON pointer (`/invoice_id`, `/line_item/1`). |
| `entities[].pageAnchor.pageRefs[]` | `FieldInfo.citations[]` | `page` is a **0-based index into `document.pages`** (and is omitted when 0) → `page_number = page + 1`; `boundingPoly` → `bbox`; `mentionText` → `Citation.text`. |
| `pages.len()` | `Usage.pages` | |
| — | `metadata.google_documentai_schema_source = "processor"` | See the gotcha below. |

**`ExtractRequest.schema` is not sent.** A Document AI processor extracts the schema it was trained
on; there is no request-time JSON Schema. LiteOCR returns every entity the processor found and leaves
the schema as documentation of intent — filter or rename on your side. (`processOptions.schemaOverride`
exists but takes Google's `DocumentSchema` proto, not JSON Schema; it is reachable through
`provider_options` if your processor version supports it.)

## 5. Errors, status codes, rate limits, timeouts

Google's standard envelope — `{"error": {"code", "message", "status", "details": []}}` — is picked up
by `Error::from_http` (it reports `error.message`).

| Status | `status` | Typical cause | LiteOCR `ErrorKind` |
|---|---|---|---|
| 400 | `INVALID_ARGUMENT` / `FAILED_PRECONDITION` | page limit exceeded, unsupported MIME type, `ocrConfig` on a Layout Parser, bad page selector | `bad_request` |
| 401 | `UNAUTHENTICATED` | missing / **expired** access token | `authentication` |
| 403 | `PERMISSION_DENIED` | no `documentai.processors.processOnline`, API not enabled, wrong project | `authentication` |
| 404 | `NOT_FOUND` | wrong processor id, or a processor in another location | `bad_request` |
| 429 | `RESOURCE_EXHAUSTED` | per-project QPS / pages-per-minute quota | `rate_limit` (retried) |
| 500/503 | `INTERNAL` / `UNAVAILABLE` | transient backend failure | `provider` (retried) |

A 200 response can still carry `document.error` (a `google.rpc.Status`); a non-zero code becomes a
`provider` error.

**Limits.** Online `:process` accepts **40 MB** per request (batch: 1 GB) and, for almost every
processor, **15 pages** — 30 with `imagelessMode: true`, and only when the pages are contiguous from
page 1. Identity/driver-licence processors cap at 2 pages, Expense at 10. Images are capped at
40 megapixels. Bigger documents need `batchProcess` (async, Cloud Storage in and out), which LiteOCR
does not implement: an obvious follow-up.

**Timeouts.** `timeout_secs` (default 300) covers the whole call and caps the single HTTP request.
Because there is no polling, a document that is too large fails fast with a 400 rather than hanging.

## 6. Gotchas (documentation-derived; not yet live-verified)

* **Access tokens expire in about an hour.** A long-running process must refresh
  `GOOGLE_DOCUMENTAI_ACCESS_TOKEN` (or pass `provider_options.access_token` per call); a stale token is
  a plain 401.
* **Location is part of the hostname *and* the resource path.** `us` and `eu` are separate endpoints;
  a processor created in `us` is `NOT_FOUND` on `eu-documentai.googleapis.com`.
* **The model is a *reading strategy*, not a processor selector.** Both come from your configuration:
  point `GOOGLE_DOCUMENTAI_PROCESSOR_ID` at a Layout Parser and use `google_documentai/layout`; point
  it at an Invoice Parser and use `google_documentai/prebuilt`. Mismatches produce empty output rather
  than an error (LiteOCR logs a warning when `extract` finds no entities).
* **15 pages online.** This is the single biggest practical limit; Document AI is the only provider
  here whose sync ceiling is that low.
* **`int64` fields are JSON strings.** `startIndex`, `endIndex` and `pageRefs[].page` arrive as
  `"14"`, not `14` — and a value of `0` is **omitted entirely** (proto3 default), which is why
  `pageRefs[]` without a `page` means *page 1*.
* **Text offsets are UTF-8 byte offsets** in the proto sense. LiteOCR slices bytes when the indices
  land on char boundaries and falls back to character slicing otherwise, so non-ASCII documents do not
  panic or truncate mid-codepoint. Worth re-checking against a real CJK document.
* **The Document OCR processor returns no tables** — `pages[].tables` is a Form Parser (and
  specialised processor) feature, so `parse` with `google_documentai/ocr` yields paragraphs only.
* **`normalizedVertices` vs `vertices`.** Most processors emit both; a few emit only absolute
  `vertices`, which are only convertible with `dimension`. The Layout Parser's `boundingBox` has no
  page dimensions at all, so absolute vertices there yield `bbox = None`.
* **`skipHumanReview` is documented as deprecated** but is still the field on `ProcessRequest`;
  LiteOCR sends `true` so a human-review-enabled processor does not silently queue work.
* **Prices differ by 20× across processors** ($1.50 vs $30 per 1 000 pages), so the model string is a
  cost decision, not just an output-shape decision.

## 7. Useful `provider_options` passthrough

```python
# 1. Everything by configuration, nothing in the environment.
liteocr.parse("scan.pdf", model="google_documentai/ocr",
              provider_options={"project": "my-proj", "location": "eu",
                                "processor_id": "1a2b3c4d5e6f7890",
                                "access_token": token})

# 2. Pin a processor version for reproducible output.
liteocr.parse("scan.pdf", model="google_documentai/ocr",
              provider_options={"processor_version": "pretrained-ocr-v2.0-2023-06-02"})

# 3. Better text from digital-born PDFs, plus image-quality diagnostics.
liteocr.parse("report.pdf", model="google_documentai/ocr",
              provider_options={"processOptions": {"ocrConfig": {
                  "enableNativePdfParsing": True, "enableImageQualityScores": True}}})

# 4. Push the online page limit from 15 to 30 (contiguous pages from page 1).
liteocr.parse("long.pdf", model="google_documentai/layout",
              provider_options={"imagelessMode": True})

# 5. Layout Parser chunking, for RAG pipelines (chunks land in `raw`).
liteocr.parse("handbook.pdf", model="google_documentai/layout", include_raw=True,
              provider_options={"processOptions": {"layoutConfig": {"chunkingConfig": {
                  "chunkSize": 1000, "includeAncestorHeadings": True}}}})

# 6. Entities from a prebuilt Invoice Parser, with citations.
liteocr.extract("invoice.pdf", model="google_documentai/prebuilt", citations=True,
                schema={"type": "object", "properties": {"invoice_id": {"type": "string"}}})

# 7. A file already in Cloud Storage.
liteocr.parse("placeholder.pdf", model="google_documentai/ocr",
              provider_options={"gcsDocument": {"gcsUri": "gs://bucket/doc.pdf",
                                                "mimeType": "application/pdf"}})
```

## 8. Links

* `processors.process` REST reference:
  <https://cloud.google.com/document-ai/docs/reference/rest/v1/projects.locations.processors/process>
* `Document` (response shape):
  <https://cloud.google.com/document-ai/docs/reference/rest/v1/Document> ·
  `ProcessOptions`: <https://cloud.google.com/document-ai/docs/reference/rest/v1/ProcessOptions>
* Processor catalogue: <https://cloud.google.com/document-ai/docs/processors-list> ·
  Layout Parser: <https://cloud.google.com/document-ai/docs/layout-parse-chunk>
* Pricing: <https://cloud.google.com/document-ai/pricing> · limits:
  <https://cloud.google.com/document-ai/limits> · quotas:
  <https://cloud.google.com/document-ai/quotas>
* Authentication: <https://cloud.google.com/docs/authentication/rest> —
  `gcloud auth print-access-token`.
* Not implemented here: `batchProcess` (async, >15 pages, Cloud Storage in/out) and
  service-account token exchange.
