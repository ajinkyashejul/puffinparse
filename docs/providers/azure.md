# Azure AI Document Intelligence

## 1. Summary

| | |
|---|---|
| Provider name | `azure` |
| Base URL | **none built in** — Azure endpoints are per resource. `AZURE_DOCUMENT_INTELLIGENCE_ENDPOINT` (or `base_url` on the request), e.g. `https://<resource>.cognitiveservices.azure.com` or `https://<region>.api.cognitive.microsoft.com`. `base_url` *is* the endpoint. |
| API key | `AZURE_DOCUMENT_INTELLIGENCE_KEY` (or `api_key` on the request) — sent as the `Ocp-Apim-Subscription-Key` header |
| Docs | <https://learn.microsoft.com/azure/ai-services/document-intelligence/> |
| API version | **`2024-11-30`** (v4.0 GA), pinned by PuffinParse as the `api-version` query parameter (`azure::API_VERSION`) |
| Verified | 2026-09-11 — **against the published REST reference only**; no Azure resource was available when this provider was written, so the fixtures are built from the documented response schema, not captured from a live call. The `#[ignore]`d live tests in `providers/azure.rs` are the check to run once a key exists. |
| Implementation | `crates/puffinparse-core/src/providers/azure.rs` |
| Modes | `parse`, `ocr`, `extract` |

Azure is the only provider so far that serves all three PuffinParse modes: `prebuilt-layout` returns
markdown plus paragraphs/tables with polygons, `prebuilt-read` is a cheap native OCR endpoint with
words, lines and confidences, and the `prebuilt-*` extraction models return typed fields with
per-field confidence and bounding regions. Every model is reached through **one** endpoint shape, so
the whole provider is a single request + poll loop.

## 2. Models exposed by PuffinParse

| Model | Azure `modelId` | Modes | List price (`pricing.json`) |
|---|---|---|---|
| `azure/read` *(default for `ocr`)* | `prebuilt-read` | `ocr` | $0.0015 / page ($1.50 / 1 000) |
| `azure/layout` *(default for `parse`)* | `prebuilt-layout` | `parse`, `ocr` | $0.01 / page ($10 / 1 000) |
| `azure/invoice` *(default for `extract`)* | `prebuilt-invoice` | `extract` | $0.01 / page |
| `azure/receipt` | `prebuilt-receipt` | `extract` | $0.01 / page |
| `azure/id_document` | `prebuilt-idDocument` | `extract` | $0.01 / page |
| `azure/tax_us_w2` | `prebuilt-tax.us.w2` | `extract` | $0.01 / page |
| `azure/custom` | `provider_options.model_id` (required) | `parse`, `ocr`, `extract` | $0.03 / page ($30 / 1 000, custom extraction) |

Prices are the public S0 pay-as-you-go rates from
<https://azure.microsoft.com/pricing/details/ai-document-intelligence/> (the page renders its numbers
client-side; the values above were cross-checked against Microsoft's retail-price listings in
September 2026). Not modelled by `cost_usd`:

* **Volume tiers** — Read drops to $0.60 / 1 000 pages above 1 M pages/month; custom extraction drops
  to $20 / 1 000. Commitment tiers go lower still.
* **Add-on features** — `ocrHighResolution`, `formulas`, `barcodes`, `styleFont`, `languages` bill
  ~$6 / 1 000 pages *each* on top of the model, and `queryFields` ~$10 / 1 000. Turning them on
  through `provider_options` makes the real bill higher than `cost_usd`.
* **Free tier (F0)**: 500 pages/month, but it analyses **only the first two pages** of any request —
  a silent truncation, not an error.

`provider_options.model_id` overrides the `modelId` for *any* model name, which is also how you reach
prebuilt models PuffinParse does not list (`prebuilt-document`, `prebuilt-tax.us.1098`,
`prebuilt-healthInsuranceCard.us`, `prebuilt-contract`, …). Pricing then falls back to the price of
the PuffinParse model you named, so pick `azure/custom` for custom models and `azure/invoice` for other
prebuilt extraction models to keep the cost estimate honest.

## 3. Request flow PuffinParse uses

Every request carries `Ocp-Apim-Subscription-Key: $AZURE_DOCUMENT_INTELLIGENCE_KEY`.

1. **Submit.** One call, no separate upload endpoint:

   ```
   POST {endpoint}/documentintelligence/documentModels/{modelId}:analyze
        ?api-version=2024-11-30
        &stringIndexType=unicodeCodePoint
        &outputContentFormat=markdown          # parse only; ocr/extract use `text`
        [&pages=1-3,7][&locale=en-US][&features=…][&queryFields=…][&output=…]
   Content-Type: application/json

   {"urlSource": "https://…"}          # URL input, Azure downloads it
   {"base64Source": "JVBERi0…"}        # path / bytes input, inlined as base64
   ```

   * `pages="1-3,7,10-"` → `pages=1-3,7,10-2000`: Azure's grammar
     (`^(\d+(-\d+)?)(,\s*(\d+(-\d+)?))*$`) has no open-ended range, so PuffinParse closes it at the S0
     page ceiling.
   * `language` → `locale`.
   * `stringIndexType=unicodeCodePoint` is deliberate: Azure's default is `textElements` (grapheme
     clusters), and PuffinParse slices `content` with Rust `char` indices, which *is* code points.
   * Query parameters are percent-encoded by hand — `reqwest`'s `query` feature is not enabled in
     this workspace.
2. **`202 Accepted`** with an empty body, an `Operation-Location` header
   (`{endpoint}/documentintelligence/documentModels/{modelId}/analyzeResults/{resultId}?api-version=…`)
   and usually `Retry-After: 1`. `{resultId}` becomes `provider_job_id`.
3. **Poll.** `GET {Operation-Location}` with the same key header. PuffinParse sleeps for `Retry-After`
   first, then polls starting at 2 s and backing off ×1.5 to a maximum of 10 s (Microsoft asks for at
   most one GET every 2 s per analyze request). `status` walks `notStarted` → `running` →
   `succeeded` | `failed`. Each poll is itself retried on 429 / 5xx / network errors, so a throttled
   poll does not fail the call.
4. **Result.** The succeeded payload carries the whole result inline under `analyzeResult`; there is
   no second fetch and no presigned URL indirection.

**`provider_options`** map to query parameters (Azure's request body only holds the document):

| Option | Effect |
|---|---|
| `model_id` | replaces the `modelId` path segment (required for `azure/custom`) |
| `features` | `features=` — `ocrHighResolution`, `languages`, `barcodes`, `formulas`, `keyValuePairs`, `styleFont`, `queryFields` |
| `query_fields` | `queryFields=` (and adds `queryFields` to `features` automatically) |
| `output` | `output=` — `pdf` (searchable PDF) or `figures` (cropped figure images) |
| `locale` | `locale=`, overriding `language` |
| `output_content_format` | `markdown` or `text`, overriding the per-mode default |
| `string_index_type` | overrides `unicodeCodePoint` — **only** do this if you also stop reading `Page.markdown` (see §6) |
| `api_version` | pins a different `api-version` |

Lists may be given as a JSON array or a single string.

## 4. Response mapping

### 4.1 `parse` (`prebuilt-layout`)

| Azure field | PuffinParse unified field | Notes |
|---|---|---|
| `{resultId}` from `Operation-Location` | `ParseResponse.provider_job_id` | |
| `analyzeResult.content` sliced by `pages[].spans` | `Page.markdown` | The page's slice of the document-level markdown, with the `<!-- PageBreak -->` marker stripped and the edges trimmed. Offsets are code points. With no spans (or no content) the page markdown falls back to its blocks joined by blank lines. |
| — | `Page.text` | Derived from `Page.markdown`: `<!-- PageHeader="…" -->` / `PageFooter` / `PageNumber` comments are unwrapped to their text, other HTML comments dropped, then `markdown_to_text` flattens headings and the HTML table. |
| `pages[].pageNumber` | `Page.page_number` | 1-based, as Azure reports it. |
| `pages[].width` / `height` | `Page.width` / `height` | **In `pages[].unit`: `inch` for PDFs, `pixel` for images.** PuffinParse passes the numbers through unchanged, so a PDF page is `8.5 × 11`, not `612 × 792`. |
| `paragraphs[]` | `Page.blocks[]` | One block per paragraph, ordered by span offset within the page. |
| `paragraphs[].role` | `Block.type` | `title`→`title`, `sectionHeading`→`section_header`, `pageHeader`→`header`, `pageFooter`→`footer`, `footnote`→`footnote`, `formulaBlock`→`formula`, `pageNumber`→`other`, no role→`text`. |
| `paragraphs[].content` | `Block.content` | Markdown; `output="text"` runs it through `markdown_to_text`. |
| `paragraphs[].boundingRegions[0]` | `Block.page_number`, `Block.bbox` | First region only. Multi-page paragraphs keep their first page. |
| `pages[].words[].confidence` | `Block.confidence` | Mean confidence of the words whose span falls inside the block's spans; `None` when the input has no words (Office/HTML). |
| `tables[]` | `Page.blocks[]` of type `table` | Cells are re-rendered as a **markdown** table (`| Item | Amount |` …) from `rowIndex`/`columnIndex`, with `columnHeader`/`stubHead` cells in row 0 becoming the header row and a `caption` prepended. Merged cells are placed at their origin cell; `rowSpan`/`columnSpan` are not replicated. |
| `pages[].{angle, selectionMarks}`, `styles`, `languages`, `sections`, `figures`, `keyValuePairs` | — | Not mapped in `parse`; visible with `include_raw=True`. |
| `pages.len()` | `Usage.pages` | Azure never reports credits or dollars, so `credits` and `provider_cost_usd` are `None` and `cost_usd` is `pages × per_page_usd`. |
| `analyzeResult.contentFormat` | `metadata.azure_content_format` | |
| resolved `modelId` | `metadata.azure_model_id` | Useful with `provider_options.model_id`. |

**Bounding boxes.** Azure gives `polygon: [x1,y1,x2,y2,x3,y3,x4,y4]` — a *quadrilateral* that follows
the text rotation. PuffinParse takes its axis-aligned hull (min/max of the x and y components) and divides
by the page `width`/`height`, so `BBox` stays the unified 0–1, top-left-origin rectangle. The polygon
unit and the page unit are always the same, so no unit conversion is needed.

**Table paragraph de-duplication.** Azure emits table cell text *both* in `tables[].cells[]` and as
ordinary `paragraphs[]`. PuffinParse drops paragraphs whose span starts inside a table's span, so cell
text appears exactly once — in the table block.

### 4.2 `ocr` (`prebuilt-read`, also `prebuilt-layout`)

`Provider::ocr` is overridden, so this is a native mapping and not derived from `parse`:

| Azure field | PuffinParse unified field |
|---|---|
| `pages[].lines[].content` | `Line.text` |
| `pages[].lines[].polygon` | `Line.bbox` (normalised) |
| mean of the line's words' `confidence` | `Line.confidence` |
| `pages[].words[].content` / `polygon` / `confidence` | `Word.text` / `bbox` / `confidence` |
| lines joined by `\n` (or the page's `content` slice when there are no lines) | `TextPage.text` |
| `pages.len()` | `Usage.pages` |

`prebuilt-read` is 6–7× cheaper than layout and returns the same word geometry, which is why it is
the `ocr` default; use `azure/layout` for `ocr` only when you want layout and text from one call.

### 4.3 `extract` (`prebuilt-*`, custom models)

`documents[0].fields` is flattened into `ExtractResponse.data`:

* Scalars come from `valueString` / `valueNumber` / `valueInteger` / `valueBoolean` / `valueDate` /
  `valueTime` / `valuePhoneNumber` / `valueCountryRegion` / `valueSelectionMark` / `valueSignature`.
* Composite values are passed through as objects: `valueCurrency` → `{"amount": 56.78,
  "currencyCode": "USD", "currencySymbol": "$"}`, `valueAddress` → Azure's address object.
* `valueArray` / `valueObject` recurse, so `Items[0].Description` is a normal nested JSON value.
* A field with no typed value falls back to its `content` string; a field with neither is `null`.
* `ExtractResponse.fields` is keyed by JSON pointer (`/InvoiceTotal`, `/Items/0/Amount`) and carries
  `confidence` plus `citations` built from `boundingRegions` (page number + normalised box) with the
  field's `content` as the citation text.
* `documents[0].docType` → `metadata.azure_doc_type`; `documents[0].confidence` is not surfaced
  (it is the document-type confidence, not a field confidence) — read it from `raw`.
* **Fallback**: if the model returned no `documents` but the response has `keyValuePairs` (a custom
  model, or `prebuilt-layout` with `features=keyValuePairs`), each pair becomes
  `data[key.content] = value.content` with the pair's confidence and the value's box. With neither,
  the call fails with a `provider` error.

**The request schema does not change what Azure extracts.** Prebuilt models have a fixed field set
(see the per-model field tables in the Azure docs) and custom models have the schema you trained.
PuffinParse therefore uses `ExtractRequest.schema` only to **select and rename**: each key of
`schema.properties` is matched against the returned field names ignoring case and non-alphanumeric
characters (`invoice_total` ≡ `InvoiceTotal`), and matches are emitted under the *schema's* spelling,
with `fields` pointers renamed to match. If nothing matches, the full Azure field set is returned
unchanged. `metadata.azure_schema_selected_fields` says which of the two happened. `instructions` is
ignored; to ask for a field Azure does not model, use `provider_options.query_fields`
(billed separately).

### 4.4 Trimmed fixture

`crates/puffinparse-core/tests/fixtures/azure_layout.json` (2 pages, 12 paragraphs, 1 table, 20 words; the three
`azure_*.json` fixtures follow the documented `2024-11-30` schema exactly and drive the unit tests):

```json
{
  "status": "succeeded",
  "createdDateTime": "2026-09-11T07:21:04Z",
  "lastUpdatedDateTime": "2026-09-11T07:21:09Z",
  "analyzeResult": {
    "apiVersion": "2024-11-30",
    "modelId": "prebuilt-layout",
    "stringIndexType": "unicodeCodePoint",
    "contentFormat": "markdown",
    "content": "<!-- PageHeader=\"PuffinParse sample\" -->\n\n# Hello PuffinParse\n\nInvoice #1234\n\n…\n\n<table>\n<tr><th>Item</th><th>Amount</th></tr>\n<tr><td>Widget</td><td>$56.78</td></tr>\n</table>\n\n\n<!-- PageFooter=\"Page 1\" -->\n\n<!-- PageBreak -->\n\n## Page Two\n\nReference: ABC-9876",
    "pages": [
      {
        "pageNumber": 1, "angle": 0.0, "width": 8.5, "height": 11.0, "unit": "inch",
        "words": [ { "content": "Hello", "polygon": [1.0, 1.0, 1.8, 1.0, 1.8, 1.5, 1.0, 1.5],
                     "span": { "offset": 40, "length": 5 }, "confidence": 0.99 } ],
        "lines": [ { "content": "Hello PuffinParse", "polygon": [1.0, 1.0, 3.5, 1.0, 3.5, 1.3, 1.0, 1.3],
                     "spans": [ { "offset": 40, "length": 13 } ] } ],
        "selectionMarks": [],
        "spans": [ { "offset": 0, "length": 249 } ]
      }
    ],
    "paragraphs": [
      { "spans": [ { "offset": 17, "length": 14 } ],
        "boundingRegions": [ { "pageNumber": 1, "polygon": [1.0, 0.5, 3.5, 0.5, 3.5, 1.0, 1.0, 1.0] } ],
        "content": "PuffinParse sample", "role": "pageHeader" },
      { "spans": [ { "offset": 40, "length": 13 } ],
        "boundingRegions": [ { "pageNumber": 1, "polygon": [1.0, 1.0, 3.5, 1.0, 3.5, 1.5, 1.0, 1.5] } ],
        "role": "title", "content": "Hello PuffinParse" }
    ],
    "tables": [
      { "rowCount": 2, "columnCount": 2,
        "cells": [ { "kind": "columnHeader", "rowIndex": 0, "columnIndex": 0, "content": "Item",
                     "boundingRegions": [ { "pageNumber": 1, "polygon": [1.0, 4.0, 2.8, 4.0, 2.8, 4.3, 1.0, 4.3] } ],
                     "spans": [ { "offset": 104, "length": 4 } ] } ],
        "boundingRegions": [ { "pageNumber": 1, "polygon": [1.0, 3.9, 4.8, 3.9, 4.8, 4.8, 1.0, 4.8] } ],
        "spans": [ { "offset": 104, "length": 94 } ] }
    ],
    "figures": [], "sections": [ { "spans": [], "elements": ["/paragraphs/0"] } ], "styles": []
  }
}
```

## 5. Errors, status codes, rate limits, timeouts

Azure's error body is `{"error": {"code", "message", "target"?, "details"?, "innererror"?}}`, which
`Error::from_http` reads out of the box (it picks up the nested `message`).

| Status | Typical `error.code` | PuffinParse `ErrorKind` |
|---|---|---|
| 400 | `InvalidRequest` (+ `innererror.code` such as `InvalidContent`, `InvalidContentDimensions`, `InvalidArgument`), `NotSupportedApiVersion` | `bad_request` |
| 401 | `401` / `Unauthorized` — "Access denied due to invalid subscription key or wrong API endpoint." | `authentication` |
| 403 | `PermissionDenied`, disabled resource, network rules | `authentication` |
| 404 | `ModelNotFound` — unknown `modelId`, or the wrong endpoint/region for that custom model | `bad_request` |
| 405/415 | wrong verb or `Content-Type` | `bad_request` |
| 429 | `429` — over the TPS quota (`Retry-After` is usually present) | `rate_limit` (retried) |
| 500/503 | `InternalServerError`, `ServiceUnavailable` | `provider` (retried) |

Failures **after** the 202 come back as `200 OK` with `status: "failed"` and the same error object.
PuffinParse maps those itself: codes starting with `Invalid` / `Unsupported` / `NotSupported`, plus
`ContentSourceNotAccessible`, `ContentSourceTimeout` and `ContentSourceSizeExceeded`, become
`bad_request`; anything else is a `provider` error. The message is
`analysis failed: <code>: <message> (<innererror.code>: <innererror.message>)`, and the error carries
the `resultId` as `job_id`.

**Missing configuration.** No key ⇒ `authentication` ("set `AZURE_DOCUMENT_INTELLIGENCE_KEY`"); no
endpoint ⇒ `authentication` ("set `AZURE_DOCUMENT_INTELLIGENCE_ENDPOINT` … or pass `base_url`") —
both before any network call. `azure/custom` without `provider_options.model_id` ⇒
`unsupported_model`.

**Rate limits (S0 defaults, adjustable by support ticket):** 15 analyze transactions/second, 50 GET
operations/second, 5 model-management/second, 10 list/second. Free F0 is 1/second for each. There are
no `X-RateLimit-*` headers; 429 carries `Retry-After` and is retried with PuffinParse's own jittered
backoff.

**Service limits:** 500 MB and 2 000 pages per document on S0 (4 MB / 2 pages on F0); max 500 MB of
JSON response. PDF, JPEG/JPG, PNG, BMP, TIFF and HEIF work with every model; DOCX/PPTX/XLS(X) and HTML
only with Read and Layout.

**Timeouts.** `timeout_secs` (default 300) is the whole-call deadline — submit, `Retry-After` sleep,
every poll and the result download — and also caps each individual HTTP request. Azure imposes no
server-side ceiling on how long an analysis may run; a 2 000-page PDF can take several minutes, so
raise `timeout_secs` for large documents.

## 6. Gotchas (from the REST reference; re-verify the ⚠ ones against a live resource)

* **The endpoint is not a constant.** Every Document Intelligence resource has its own host, so
  `base_url` is mandatory in the same sense an API key is. PuffinParse raises an authentication error
  rather than guessing a region. Keep the `/` -free form: `https://<resource>.cognitiveservices.azure.com`.
* **Two auth schemes, one supported.** Azure also accepts Microsoft Entra ID bearer tokens
  (`Authorization: Bearer`, scope `https://cognitiveservices.azure.com/.default`). PuffinParse only sends
  `Ocp-Apim-Subscription-Key`; a managed identity / AAD setup is not reachable through `api_key`.
* **Markdown tables are HTML.** In `2024-11-30` the markdown `content` renders tables as
  `<table><tr><th>…` (to express merged cells), *not* as pipe tables, and selection marks as ☒ / ☐
  rather than `:selected:`. `Page.markdown` therefore contains HTML fragments, while `Block.content`
  for a table block is a pipe table PuffinParse rendered from `tables[].cells`. `Page.text` flattens both.
* **Page headers, footers and page numbers are HTML comments** in markdown
  (`<!-- PageHeader="…" -->`), and pages are separated by `<!-- PageBreak -->`. PuffinParse splits pages
  by `pages[].spans` (exact) and only strips the page-break marker; the comments stay in
  `Page.markdown`, while `Page.text` unwraps `PageHeader`/`PageFooter`/`PageNumber` to their text.
* ⚠ **Offsets depend on `stringIndexType`.** PuffinParse pins `unicodeCodePoint` so `content` can be
  sliced with `char` indices. Overriding it with `textElements` (Azure's default) or `utf16CodeUnit`
  will mis-slice `Page.markdown` for documents containing emoji, combining marks or non-BMP script —
  everything else (blocks, boxes, fields) is unaffected.
* **Page dimensions are in inches for PDFs.** `Page.width = 8.5` is not a bug; check `pages[].unit`
  in `raw` if you need to know which. Normalised `BBox` values are unaffected.
* **Office and HTML inputs have no geometry.** For DOCX/PPTX/XLS(X)/HTML, v4.0 reports no `angle`,
  no `width`/`height`/`unit`, no polygons and no `lines`. Blocks then carry `bbox: None`, pages carry
  `width: None`, and `ocr` mode falls back to the page's slice of `content` with an empty `lines`
  list. Word/HTML pages are counted in blocks of 3 000 characters, XLSX per worksheet, PPTX per slide.
* **Figures are not blocks.** `figures[]` (with `output=figures`, croppable via
  `/analyzeResults/{resultId}/figures/{figureId}`) is not mapped; the figure's caption usually also
  appears as a paragraph, so the text is not lost. Read `raw` for figure geometry.
* **`prebuilt-layout` is the only model with roles.** `prebuilt-read` returns paragraphs without
  `role` (everything maps to `text`) and no `tables`, which is why `read` is registered for `ocr`
  only.
* ⚠ **F0 silently truncates to 2 pages.** A 10-page document on the free tier returns
  `usage.pages = 2` and a 2-page result with no warning anywhere in the payload.
* **Add-on features cost extra per page** and are off by default; `ocrHighResolution` also makes the
  analysis noticeably slower. `keyValuePairs` is the replacement for the retired `prebuilt-document`
  model.
* **Analyze is always async**, even for a one-page PNG: there is no synchronous endpoint, so the
  minimum latency is one POST plus one GET.
* **`Retry-After` is honoured once**, before the first poll; afterwards PuffinParse uses its own 2 s → 10 s
  backoff. A throttled poll (429) is retried inside the poll loop instead of failing the call.

## 7. Useful `provider_options` passthrough

```python
# 1. Custom (trained) model — `model_id` is required for azure/custom and overrides any model name.
puffinparse.extract("po.pdf", model="azure/custom", schema=schema,
                provider_options={"model_id": "purchase-orders-v3"})

# 2. Fine print / low-quality scans: high-resolution OCR (+$6 / 1k pages).
puffinparse.parse("fine-print.pdf", model="azure/layout",
              provider_options={"features": ["ocrHighResolution"]})

# 3. Formulas as LaTeX and barcodes as markdown images, plus a searchable PDF of the result.
puffinparse.parse("paper.pdf", model="azure/layout",
              provider_options={"features": ["formulas", "barcodes"], "output": ["pdf"]})

# 4. Ask a prebuilt model for fields it does not model (+$10 / 1k pages); `features` gets
#    `queryFields` added automatically.
puffinparse.extract("receipt.png", model="azure/receipt", schema=schema,
                provider_options={"query_fields": ["StoreNumber", "CashierName"]})

# 5. General key/value pairs out of an unstructured form, via the layout model.
puffinparse.extract("form.pdf", model="azure/layout", schema={},
                provider_options={"model_id": "prebuilt-layout", "features": ["keyValuePairs"]})

# 6. A prebuilt model PuffinParse does not list, with a locale hint.
puffinparse.extract("card.jpg", model="azure/invoice", schema=schema,
                provider_options={"model_id": "prebuilt-healthInsuranceCard.us", "locale": "en-US"})
```

(Example 5 pairs `azure/layout` with `extract`; that combination is only reachable because
`provider_options.model_id` bypasses the model→`modelId` table — the registry itself lists `layout`
for `parse` and `ocr`.)

## 8. Links

* Analyze Document REST reference (2024-11-30):
  <https://learn.microsoft.com/rest/api/aiservices/document-models/analyze-document?view=rest-aiservices-v4.0%20(2024-11-30)>
* Get Analyze Result (the poll target):
  <https://learn.microsoft.com/rest/api/aiservices/document-models/get-analyze-result?view=rest-aiservices-v4.0%20(2024-11-30)>
* Layout model and its JSON: <https://learn.microsoft.com/azure/ai-services/document-intelligence/prebuilt/layout>
* Markdown output elements: <https://learn.microsoft.com/azure/ai-services/document-intelligence/concept/markdown-elements>
* Read model: <https://learn.microsoft.com/azure/ai-services/document-intelligence/prebuilt/read>
* Prebuilt extraction models and their field lists:
  <https://learn.microsoft.com/azure/ai-services/document-intelligence/model-overview>
* Add-on capabilities: <https://learn.microsoft.com/azure/ai-services/document-intelligence/concept/add-on-capabilities>
* Service quotas and limits: <https://learn.microsoft.com/azure/ai-services/document-intelligence/service-limits>
* Pricing: <https://azure.microsoft.com/pricing/details/ai-document-intelligence/>
