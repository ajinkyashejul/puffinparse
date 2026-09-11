# Extend

## 1. Summary

| | |
|---|---|
| Provider name | `extend` |
| Base URL | `https://api.extend.ai` (override: `base_url` on the request, or `EXTEND_BASE_URL`). Regional hosts: `https://api.us2.extend.app` (note `.app`), `https://api.eu1.extend.ai` |
| API key | `EXTEND_API_KEY` (or `api_key` on the request) — sent as `Authorization: Bearer <key>` |
| Docs | <https://docs.extend.ai> (append `.md` to any docs path for raw markdown; index at `llms.txt`) |
| API version | **`2026-02-09`**, pinned by LiteOCR in the mandatory `x-extend-api-version` header (`extend::API_VERSION`) |
| Verified | 2026-09-11, live against the production host |
| Implementation | `crates/liteocr-core/src/providers/extend.rs` |

Extend returns page chunks of markdown plus typed blocks with polygons, bounding boxes and OCR
confidence. LiteOCR always uses the **async** run API (`/parse_runs`), never the 5-minute sync `/parse`.

## 2. Models exposed by LiteOCR

| Model | Provider parameters LiteOCR sets | List price (`pricing.json`) |
|---|---|---|
| `extend/parse_performance` *(default)* | `config.engine = "parse_performance"` | $0.025 / page (2 credits) |
| `extend/parse_light` | `config.engine = "parse_light"` | $0.00625 / page (0.5 credits) |
| `extend/parse_auto` | `config.engine = "parse_auto"` | $0.025 / page (per-page: light pages bill 0.5 credits) |

Credits are the billing unit; pay-as-you-go is $0.0125/credit (Scale: $0.01). Prices above are the PAYG
list rate from <https://docs.extend.ai/credits>, used for `OcrResponse.cost_usd`. `parse_auto` is priced
at the performance rate, so its estimate is an upper bound. Surcharges LiteOCR does not model: agentic
text/table correction +1 credit per triggered page, priority parsing ×2, advanced Excel parsing
3 credits / 1 000 non-empty cells.

## 3. Request flow LiteOCR uses

Every request carries:

```
Authorization: Bearer $EXTEND_API_KEY
x-extend-api-version: 2026-02-09
x-extend-workspace-id: <provider_options.workspace_id>      # only when supplied
```

1. **Upload (only for path / bytes input).** `POST {base}/files/upload`, `multipart/form-data`, single
   part `file`. Response: a file object whose `id` (`file_…`) is used as `{"id": …}`.
   **URL inputs skip this step** — LiteOCR sends `{"url": "<url>", "name": "<filename>"}` and Extend
   downloads the file itself, creating a `file_…` record it returns in `run.file`.
2. **Create the run.** `POST {base}/parse_runs` with `Content-Type: application/json`. Body from
   `build_body()`:

   ```json
   {
     "file": { "id": "file_bJ2ZXxacw7o206UnS6eR0" },
     "config": {
       "target": "markdown",
       "chunkingStrategy": { "type": "page" },
       "engine": "parse_performance",
       "blockOptions": { "tables": { "targetFormat": "markdown" } }
     }
   }
   ```

   * `pages="1-3,7"` adds `config.advancedOptions.pageRanges = [{"start":1,"end":3},{"start":7,"end":750}]`
     — Extend requires an `end`, and 750 is its documented maximum page-range end.
   * `targetFormat: "markdown"` is deliberate: Extend's default is `"html"`.
   * The response echoes the **resolved** config (including `engineVersion`, e.g. `"2.0.0"`), which is
     what LiteOCR reads back to label the response model.
3. **Poll.** `GET {base}/parse_runs/{id}` starting at 1 s, backing off ×1.5 to a maximum of 10 s, until
   `status` is `PROCESSED` or `FAILED`. (`PENDING`/`PROCESSING` keep polling.) If the create call already
   came back terminal, the poll loop is skipped.
4. **Result.** Normally `run.output` is inline. If a run was created with `responseType=url`
   (via `provider_options`), `output` is `null` and LiteOCR fetches `run.outputUrl` with a plain `GET`
   and **no** auth header; the payload has exactly the shape of `output` (`{chunks, metadata, ocr?}`).
   Presigned output URLs expire in 15 minutes.

**Where `provider_options` are merged:** `build_body()` removes `workspace_id` (it becomes a header),
then lifts any of `target`, `chunkingStrategy`, `engine`, `engineVersion`, `blockOptions`,
`advancedOptions` into `config` (deep-merged with an explicit `config` object if you passed one), and
deep-merges everything that remains at the **top level** of the body — which is how `metadata`,
`dataRetention` and even `file` overrides get through. So both spellings work:
`{"blockOptions": {...}}` and `{"config": {"blockOptions": {...}}}`.

## 4. Response mapping

| Extend field | LiteOCR unified field | Notes |
|---|---|---|
| `id` (`pr_…`) | `OcrResponse.provider_job_id` | |
| `config.engine` | `OcrResponse.model` | `extend/<engine>` read back from the resolved config; falls back to `parse_performance`. |
| `output.chunks[].content` | `Page.markdown` | Only for chunks with `type == "page"` and `pageRange.start == pageRange.end`. |
| `output.chunks[].blocks[]` | `Page.blocks[]` | Reading order preserved. |
| `blocks[].type` | `Block.type` | See mapping below. |
| `blocks[].content` | `Block.content` | Markdown; `output="text"` runs it through `markdown_to_text`. |
| `blocks[].metadata.page.number` | `Block.page_number` | Falls back to the chunk's `pageRange.start`. |
| `blocks[].metadata.page.{width,height}` | `Page.width` / `Page.height` | First value seen per page wins; these are the *raster* dimensions (e.g. 1241×1754 at 150 dpi), not PDF points. |
| `blocks[].boundingBox.{left,top,right,bottom}` | `Block.bbox` = `{x0,y0,x1,y1}` | **Absolute, top-left origin, page units.** Normalised by the page's `width`/`height` via `from_xywh(left, top, right-left, bottom-top, w, h)` and clamped to 0–1. No page dims for that page ⇒ `bbox = None`. |
| `blocks[].metadata.avgOcrConfidence` | `Block.confidence` | 0–1 float. `minOcrConfidence` and chunk-level confidences are ignored. |
| `metrics.pageCount` | `Usage.pages` | Rounded; falls back to the number of reconstructed pages. |
| `usage.credits` | `Usage.credits` | `null` for runs created before 2025-10-07. `totalCredits`/`breakdown` are not surfaced. |
| `metrics.processingTimeMs` | `metadata.extend_processing_time_ms` | |
| `blocks[].polygon`, `output.ocr.words[]`, `output.metadata.pages[]` | — | Not mapped; visible with `include_raw=True`. |

Block types: `text`, `key_value` → `text`; `heading` → `title`; `section_heading` → `section_header`;
`table`, `table_head`, `table_cell` → `table`; `figure` → `figure`; `formula` → `formula`;
`header` → `header`; `footer` → `footer`; everything else (`page_number`, `barcode`, …) → `other`.

Trimmed real response (`crates/liteocr-core/tests/fixtures/extend_parse_run.json`, one chunk with one
block shown; the file holds 2 pages, 5 blocks and 21 OCR words):

```json
{
  "object": "parse_run",
  "id": "pr_xi5wEyAbYlYVBDDy8QDRg",
  "file": { "object": "file", "id": "file_bJ2ZXxacw7o206UnS6eR0", "name": "test_multi.pdf",
            "type": "PDF", "parentFileId": null, "metadata": { "pageCount": 2 },
            "dataRetention": { "mode": "workspace_default", "status": "available" },
            "createdAt": "2026-09-11T07:18:23.026Z", "updatedAt": "2026-09-11T07:18:37.514Z" },
  "status": "PROCESSED",
  "failureReason": null,
  "failureMessage": null,
  "metadata": null,
  "dataRetention": { "mode": "workspace_default", "status": "available" },
  "output": {
    "chunks": [
      {
        "object": "chunk", "id": "chunk_1_iSMg4N", "type": "page",
        "content": "# Hello LiteOCR\n\nInvoice #1234\nTotal: $56.78\nDate: 2026-09-11\n\n| Item | Amount |\n| --- | --- |\n| Widget | $56.78 |",
        "metadata": { "pageRange": { "start": 1, "end": 1 },
                      "minOcrConfidence": 0.905, "avgOcrConfidence": 0.986 },
        "blocks": [
          {
            "object": "block", "id": "block_1_YFFbD5", "type": "table",
            "content": "| Item | Amount |\n| --- | --- |\n| Widget | $56.78 |",
            "details": { "type": "table_details", "rowCount": 2, "columnCount": 2 },
            "metadata": { "page": { "number": 1, "width": 1241, "height": 1754 },
                          "minOcrConfidence": 0.905, "avgOcrConfidence": 0.973 },
            "polygon": [ { "x": 90.365, "y": 382.357 }, { "x": 704.950, "y": 382.357 },
                         { "x": 704.950, "y": 585.043 }, { "x": 90.365, "y": 585.043 } ],
            "boundingBox": { "left": 90.365, "top": 382.357, "right": 704.950, "bottom": 585.043 }
          }
        ]
      }
    ],
    "metadata": { "originalMimeType": "application/pdf", "finalMimeType": "application/pdf",
                  "pages": [ { "number": 1, "rotationApplied": 0, "originalPageWidth": 596,
                               "originalPageHeight": 842, "dpi": 150 } ] }
  },
  "outputUrl": null,
  "metrics": { "processingTimeMs": 4383, "pageCount": 2 },
  "config": { "target": "markdown", "chunkingStrategy": { "type": "page" },
              "engine": "parse_performance", "engineVersion": "2.0.0",
              "blockOptions": { "tables": { "targetFormat": "markdown" }, "figures": { "enabled": true } } },
  "batchId": null,
  "usage": { "credits": 4, "totalCredits": 4,
             "breakdown": [ { "object": "parse_run", "id": "pr_xi5wEyAbYlYVBDDy8QDRg", "credits": 4,
                              "charges": [ { "product": "parse_performance", "unit": "page",
                                             "quantity": 2, "credits": 4 } ] } ] }
}
```

## 5. Errors, status codes, rate limits, timeouts

Extend's error body is `{"code","message","requestId","retryable"}` (sometimes `docUrl`). LiteOCR's
`Error::from_http` uses `message` and classifies by HTTP status:

| Status | Typical `code` | LiteOCR `ErrorKind` |
|---|---|---|
| 400 | `INVALID_REQUEST` (missing version header, bad enum with a `Path: config.target` suffix), `INVALID_CONFIG_OPTIONS`, `UNABLE_TO_DOWNLOAD_FILE`, `FILE_TYPE_NOT_SUPPORTED`, `FILE_SIZE_TOO_LARGE` | `bad_request` |
| 401 | `UNAUTHORIZED` — "Invalid API key." | `authentication` |
| 403 | workspace lacks permission | `authentication` |
| 404 | `NOT_FOUND` — unknown file id, or an endpoint that does not exist | `bad_request` |
| 410 | `ENDPOINT_REMOVED` — e.g. `GET /parser_runs/{id}` | `bad_request` |
| 422 | corrupt / password-protected / conversion failure (sync `/parse` only) | `bad_request` |
| 429 | `RATE_LIMIT_EXCEEDED` (`retryable: true`) | `rate_limit` (retried) |
| 500 | `INTERNAL_ERROR`, OCR/chunking errors | `provider` (retried) |

On `/parse_runs`, only 400/401/403 happen at creation; **processing failures return HTTP 200** with
`status: "FAILED"`. LiteOCR maps `failureReason` itself:

| `failureReason` | LiteOCR `ErrorKind` |
|---|---|
| `OCR_ERROR`, `INTERNAL_ERROR` | `provider` |
| `OUT_OF_CREDITS` | `authentication` |
| anything else (`CORRUPT_FILE`, `PASSWORD_PROTECTED_FILE`, `FILE_TYPE_NOT_SUPPORTED`, `CHUNKING_ERROR`, `FAILED_TO_CONVERT_TO_PDF`, …) | `bad_request` |

The error message is `parse run failed: <reason>: <failureMessage>` and carries the run id as `job_id`.

**Retries.** `max_retries` (default 2), exponential backoff with full jitter, on rate-limit, network and
500/502/503/504 only.

**Rate limits.** Per organization, per category (independent GET / WRITE / RUN buckets), plus a
throughput cap in files/minute: PAYG 10 req/s and 80 files/min; Scale 25+ and 120+; Enterprise 75+ and
300+. Free-form `429`s are retryable. **No `X-RateLimit-*` headers are returned at all** — reactive 429
handling only, though `Retry-After` is sometimes present.

**Timeouts.** `timeout_secs` (default 300) is the whole-call deadline (upload + create + polling +
output download) and also caps each individual request. The async run itself has no server-side timeout;
Extend's sync `/parse` (which LiteOCR does not use) has a hard 5-minute one.

## 6. Gotchas (verified)

* **`x-extend-api-version` is mandatory** for any key created after 2025-04-21 — omitting it is a hard
  `400 INVALID_REQUEST`. Keys older than that silently fall back to the legacy `2024-12-23` behaviour.
  LiteOCR always pins `2026-02-09`; the response echoes the same header back.
* **Endpoints from the old API are gone.** `POST /parse_async` → `404 NOT_FOUND`;
  `GET /parser_runs/{id}` → `410 ENDPOINT_REMOVED` ("Use GET /parse_runs/:id instead").
  Async is `POST /parse_runs`.
* **Enums are lowercase.** `"target": "MARKDOWN"` is a `400`; it must be `"markdown"` (or `"spatial"`).
* **The run object *is* the response** on `2026-02-09` — there is no `parserRun`/`success` envelope, and
  chunks live at `output.chunks`, not `chunks`.
* **`blockOptions.tables.targetFormat` defaults to `html`**, which surprises anyone expecting markdown
  end to end. LiteOCR sets `markdown` explicitly.
* **`details` is often the literal empty object `{}`** (text, heading, footer blocks) even though the
  docs describe a tagged union. Anything reading `details.type` must tolerate its absence.
* **`output.metadata.pages` is `null` for raw images** — only PDFs (and files converted to PDF) get page
  metadata. For images, block `metadata.page.width/height` are raw pixels, so LiteOCR still normalises
  boxes correctly.
* **Undocumented-but-always-present keys**: `dataRetention` on both file and run objects, `chunk.id`,
  `block.id`.
* **Coordinates are in the raster space at `dpi`**, not PDF points: a 596×842 pt page reports
  1241×1754 at 150 dpi. Normalising by `metadata.page.width/height` (what LiteOCR does) is correct in
  either space; converting back to source points needs `originalPageWidth × dpi/72` and the inverse of
  `rotationApplied`.
* **`file.metadata` is `{}` right after upload** even for PDFs; `pageCount` only appears once a run has
  processed the file.
* **`usage` is `null` for runs created before 2025-10-07**, and `totalCredits`/`breakdown` are missing on
  runs persisted before 2026-05-14 — all three are optional.
* **The docs site moved.** `https://docs.extend.ai/2025-04-21/developers/api-reference/…` URLs now 404;
  current docs live at the root.

## 7. Useful `provider_options` passthrough

```python
# 1. Organization-scoped API keys need a workspace; LiteOCR turns this into a header.
liteocr.ocr("doc.pdf", model="extend/parse_performance",
            provider_options={"workspace_id": "ws_…"})

# 2. Word-level OCR boxes + confidence in the raw payload.
liteocr.ocr("scan.pdf", model="extend/parse_performance", include_raw=True,
            provider_options={"advancedOptions": {"returnOcr": {"words": True}}})

# 3. Skip figure processing, and let the table agent fix messy tables.
liteocr.ocr("report.pdf", model="extend/parse_performance",
            provider_options={"blockOptions": {"figures": {"enabled": False},
                                               "tables": {"agentic": {"enabled": True}}}})

# 4. Password-protected PDF by URL, tagged for usage reporting, with no data retained.
liteocr.ocr("https://example.com/locked.pdf", model="extend/parse_light",
            provider_options={"file": {"settings": {"password": "…"}},
                              "metadata": {"extend:usage_tags": ["prod"]},
                              "dataRetention": {"mode": "zero"}})

# 5. Spreadsheets: advanced parsing, hidden content skipped.
liteocr.ocr("book.xlsx", model="extend/parse_performance",
            provider_options={"advancedOptions": {"excelParsingMode": "advanced",
                                                  "excelSkipHiddenContent": True}})
```

## 8. Links

* Docs home: <https://docs.extend.ai> · index: <https://docs.extend.ai/llms.txt> ·
  compact platform context: <https://docs.extend.ai/agents.md>
* Credits and pricing: <https://docs.extend.ai/credits>
* API versions in use: `2026-02-09` (current), `2025-04-21`, `2024-12-23`, `2024-11-14`, `2024-07-30`,
  `2024-02-01` — pin one with `x-extend-api-version`.
* Webhooks for `parse_run.processed` / `parse_run.failed` (HMAC-SHA256 over `v0:{timestamp}:{body}`)
  are the recommended alternative to polling at scale.
