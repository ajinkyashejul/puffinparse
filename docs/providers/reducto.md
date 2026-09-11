# Reducto

## 1. Summary

| | |
|---|---|
| Provider name | `reducto` |
| Base URL | `https://platform.reducto.ai` (override: `base_url` on the request, or `REDUCTO_BASE_URL`) |
| API key | `REDUCTO_API_KEY` (or `api_key` on the request) — sent as `Authorization: Bearer <key>` |
| Docs | <https://docs.reducto.ai> (append `.md` to any docs path for raw markdown; index at `llms.txt`) |
| API version | Unversioned — no version header. Server string observed in the published OpenAPI: `v1.12.12-114-g9808b824f237` |
| Verified | 2026-09-11, live against the production host |
| Implementation | `crates/liteocr-core/src/providers/reducto.rs` |

Reducto's Parse product returns Markdown chunks plus typed, bbox-carrying blocks. LiteOCR uses the
current **v3** request schema (`input` + `enhance`/`retrieval`/`formatting`/`spreadsheet`/`settings`),
not the legacy `document_url` schema.

## 2. Models exposed by LiteOCR

| Model | Provider parameters LiteOCR sets | List price (`pricing.json`) |
|---|---|---|
| `reducto/standard` *(default)* | none beyond the shared body — the account's default model (legacy Parse for most accounts) | $0.015 / page |
| `reducto/r-1` | `settings.model = "r-1"` | $0.010 / page |
| `reducto/agentic` | `enhance.agentic = [{"scope":"text"},{"scope":"table"}]` | $0.030 / page |

Prices are public pay-as-you-go list prices (source: <https://docs.reducto.ai/reference/credit-usage>),
used only to fill `OcrResponse.cost_usd = per_page_usd × usage.pages`. Reducto bills "complex" pages a
surcharge credit on top of the base page credit, so the estimate is a floor for `standard`/`agentic`.

## 3. Request flow LiteOCR uses

All requests carry `Authorization: Bearer $REDUCTO_API_KEY`. There is no version or workspace header.

1. **Upload (only for path / bytes input).** `POST {base}/upload`, `multipart/form-data`, single part
   `file` with the filename and guessed MIME type. Response: `{"file_id":"reducto://<uuid>.pdf",
   "presigned_url":null}`; `file_id` is used verbatim as `input`.
   **URL inputs skip this step** — the URL string is passed straight through as `input`, and Reducto
   downloads it server-side.
2. **Create.** `POST {base}/parse` (default) with `Content-Type: application/json`. The body is built by
   `build_body()`:

   ```json
   {
     "input": "reducto://<uuid>.pdf",
     "retrieval": { "chunking": { "chunk_mode": "page" } },
     "formatting": { "table_output_format": "md" },
     "settings": {}
   }
   ```

   * `reducto/r-1` adds `"settings": {"model": "r-1"}`.
   * `reducto/agentic` adds `"enhance": {"agentic": [{"scope":"text"},{"scope":"table"}]}`.
   * `pages="1-3,7"` becomes `settings.page_range = [{"start":1,"end":3},{"start":7}]` (1-based,
     matching Reducto's own indexing).
   * `chunk_mode: "page"` is deliberate: Reducto's default is `"disabled"`, which returns the whole
     document as one chunk and gives LiteOCR no page structure.
3. **Async variant.** When `provider_options.async == true`, LiteOCR posts the *same* body to
   `POST {base}/parse_async` → `{"job_id":"..."}`, then polls `GET {base}/job/{job_id}` (Bearer header
   required) starting at 1 s and backing off ×1.5 to a maximum of 8 s. Terminal statuses are
   `Completed`, `Failed`, `Cancelled`; anything else (`Pending`, `Idle`, `InProgress`, `Completing`, …)
   keeps polling. A completed job's `result` is deserialised as the same `ParseResponse` the sync call
   returns — i.e. the payload lives at `job.result.result.chunks`.
4. **Result.** If `result.type == "url"` (large results, or `settings.force_url_result`), LiteOCR issues
   a plain `GET` on the presigned URL **without** the Authorization header and expects the *whole*
   `FullResult` object back (`{"type":"full","chunks":[…]}`), not a bare chunk array. A second `url`
   result is an error.

**Where `provider_options` are merged:** `build_body()` clones `provider_options`, removes the LiteOCR-only
key `async`, and deep-merges the rest into the body above (objects merge recursively; scalars and arrays
replace). So `provider_options` keys are top-level Reducto request keys — `settings`, `retrieval`,
`formatting`, `enhance`, `spreadsheet`, `queue_priority`, `async` (the Reducto object of that name is
*not* forwarded; only the boolean switch is consumed).

## 4. Response mapping

| Reducto field | LiteOCR unified field | Notes |
|---|---|---|
| `job_id` | `OcrResponse.provider_job_id` | |
| `result.chunks[].content` | `Page.markdown` | Only when every block in the chunk is on one page; multiple such chunks on a page are joined with a blank line. |
| `result.chunks[].blocks[]` | `Page.blocks[]` | Grouped by page, reading order preserved. |
| `blocks[].type` | `Block.type` | See mapping below. |
| `blocks[].content` | `Block.content` | Markdown; `output="text"` runs it through `markdown_to_text`. |
| `blocks[].bbox.page` | `Block.page_number` | 1-based; missing bbox ⇒ page 1. `original_page` is not used. |
| `blocks[].bbox.{left,top,width,height}` | `Block.bbox` = `{x0,y0,x1,y1}` | **Already normalised 0–1**, origin top-left. `from_normalized_ltwh` clamps and converts to `x1=left+width`, `y1=top+height` — no page dimensions needed. `Page.width`/`height` stay `None`. |
| `blocks[].granular_confidence.parse_confidence` | `Block.confidence` | Preferred, 0–1. |
| `blocks[].confidence` (`"high"`/`"low"`) | `Block.confidence` | Fallback only: `high → 0.9`, `low → 0.5`, anything else `None`. |
| `usage.num_pages` | `Usage.pages` | Falls back to the number of reconstructed pages when 0/absent. |
| `usage.credits` | `Usage.credits` | `null` on accounts migrated to per-product pricing → `None`. |
| `duration` | `metadata.reducto_duration_s` | |
| `studio_link` | `metadata.reducto_studio_link` | |
| — | `Usage.provider_cost_usd` | Never set; `cost_usd` comes from `pricing.json`. |

Block types: `Text`, `Key Value`, `Comment` → `text`; `Title` → `title`; `Section Header` →
`section_header`; `List Item` → `list`; `Table` → `table`; `Figure` → `figure`; `Header` → `header`;
`Footer` → `footer`; `Footnote` → `footnote`; `Caption` → `caption`; `Formula`/`Equation` → `formula`;
everything else (including `Page Number`, `Signature`, `Checkbox`) → `other`.

Trimmed real response (`crates/liteocr-core/tests/fixtures/reducto_parse.json`, two of three blocks elided):

```json
{
  "response_type": "parse",
  "job_id": "04fe7cf4-1f40-4a03-b1a0-460bd8d6a89a",
  "duration": 0.7815132141113281,
  "pdf_url": "https://example-storage.invalid/converted.pdf?X-Amz-Signature=REDACTED",
  "studio_link": "https://studio.reducto.ai/job/04fe7cf4-1f40-4a03-b1a0-460bd8d6a89a",
  "usage": {
    "num_pages": 1,
    "credits": 1.0,
    "credit_breakdown": { "page": 1.0 },
    "page_billing_breakdown": { "1": ["page"] },
    "non_empty_cell_count": null
  },
  "result": {
    "type": "full",
    "chunks": [
      {
        "content": "# Hello LiteOCR\n\nInvoice #1234\nTotal: $56.78\n\nAcme Corporation, 123 Main Street",
        "embed": "# Hello LiteOCR\n\nInvoice #1234\nTotal: $56.78\n\nAcme Corporation, 123 Main Street",
        "enriched": null,
        "enrichment_success": false,
        "blocks": [
          {
            "type": "Title",
            "bbox": { "left": 0.119281045751634, "top": 0.06818181818181818,
                      "width": 0.24754901960784315, "height": 0.021464646464646464,
                      "page": 1, "original_page": 1 },
            "content": "Hello LiteOCR",
            "image_url": null,
            "chart_data": null,
            "confidence": "high",
            "granular_confidence": { "extract_confidence": null, "parse_confidence": 0.9461241155862807 },
            "extra": null
          }
        ]
      }
    ],
    "ocr": null,
    "custom": null
  },
  "parse_mode": null,
  "document_properties": null
}
```

## 5. Errors, status codes, rate limits, timeouts

Every non-2xx body goes through `Error::from_http`, which pulls a message out of `message` / `detail` /
`error` and classifies by status:

| Status | Reducto body | LiteOCR `ErrorKind` |
|---|---|---|
| 400 | `{"error":{"code":400,"name":"INVALID_CONFIG",…},"detail":"…"}` — bad config, source download failure, bad page range | `bad_request` |
| 401 | `{"error":{"code":401,"name":"AUTH_ERROR","message":"Invalid access token"},…}` | `authentication` |
| 403 | **nginx HTML page** when the `Authorization` header is missing entirely; also expired/inaccessible presigned source | `authentication` |
| 404 | File not found | `bad_request` |
| 413 | Image over 50 MP / 15 000 px per axis | `bad_request` |
| 415 | Conversion error / corrupt PDF | `bad_request` |
| 422 | Pydantic validation array (`{"detail":[…]}`) | `bad_request` |
| 429 | `{"message":"[CODE 1000] rate limit exceeded, retry with exponential backoff"}` | `rate_limit` (retried) |
| 442 | Password-protected document — Reducto's own non-standard code | `bad_request` |
| 500 | Content/citation extraction error — not retriable upstream | `provider` |
| 502/503/504 | LLM error, overload, timeout | `provider` (retried) |

`reducto::map_status()` spells 442 out explicitly; the live path reaches the same verdict through
`Error::from_http`'s generic `400..=499 → bad_request` arm. An async job that ends `Failed`/`Cancelled`
becomes a `provider` error carrying `reason` (or `error.message`) and the `job_id`.

**Retries.** `max_retries` (default 2) with exponential backoff + full jitter, on rate-limit, network,
and 500/502/503/504 errors only. 4xx is never retried.

**Rate limits.** 1 000 req/s per key across all endpoints (`[CODE 1000]`), 200 req/s on
`GET /job/{id}` (`[CODE 2000]`, "use webhooks instead of polling"). A separate concurrency throttle
(200/350/500+ concurrent pages by plan) does **not** return 429 — it just queues, so the symptom is
latency. **No rate-limit headers of any kind** are returned (no `X-RateLimit-*`, no `Retry-After`), so
backoff is blind.

**Timeouts.** `timeout_secs` (default 300) is a whole-call deadline covering upload, parse, polling and
result download; it is also the per-request timeout, shrinking as the budget is spent. Reducto's own
sync `/parse` ceiling is 900 s — use `provider_options={"async": true}` for anything longer.

## 6. Gotchas (verified)

* **`input`, not `document_url`.** The body is a three-way server-side union: `ParseConfigNew`,
  legacy `ParseConfig` (`document_url` + `options`/`advanced_options`/`experimental_options`), and the
  current `SyncParseConfig` (`input` + option groups). Legacy still works on sync `/parse`, but mixing
  the two (`document_url` + `retrieval`) yields `400 INVALID_CONFIG`. Do not push `document_url` through
  `provider_options`.
* **One bad field produces several validation errors**, most of them complaints about a schema you never
  used; the `SyncParseConfig`-scoped entry is the relevant one.
* **Error text always names `document_url`** even when you sent `input` — never parse error strings.
* **`result.type == "url"`** appears for large results (~6 MB inline limit). The fetched body is the full
  `{"type":"full","chunks":[…]}` object, not a bare array — the vendor's own snippet gets this wrong.
  LiteOCR handles both variants; force the URL path deterministically with
  `provider_options={"settings":{"force_url_result":true}}`.
* **442 is a real status code** (password-protected document), not a typo for 422. Supply
  `settings.document_password`.
* **No rate-limit headers**, no `Retry-After`, even on 429.
* **Missing auth header → 403 with an HTML body**, invalid token → 401 with JSON. LiteOCR's message
  extraction falls back to the raw (truncated) body for the HTML case.
* **Chunks have no page number.** Page identity comes only from `blocks[].bbox.page`; with
  `chunk_mode: "variable"` chunks straddle pages, which is why LiteOCR pins `chunk_mode: "page"` and
  still derives page numbers from blocks rather than array position.
* **Undocumented keys on the wire**: `response_type`, `parse_mode`, `document_properties`,
  `usage.credit_breakdown`, `usage.page_billing_breakdown` (1-based page numbers as *string* keys),
  `usage.non_empty_cell_count`. LiteOCR ignores all but `credit_breakdown`, which it deserialises but
  does not surface.
* **`usage.credits` can be `null`** on accounts on the new per-product pricing (they get
  `usage_breakdown` instead) — `Usage.credits` is then `None` and `cost_usd` still comes from the table.
* **Uploaded files expire after 24 h**, and results expire after 24 h unless
  `settings.persist_results` is set.
* **`settings.return_images: ["page"]` does not populate `block.image_url`** (it stays `null`); the URL
  shows up under `block.extra.page_image_url`. LiteOCR surfaces neither.

## 7. Useful `provider_options` passthrough

```python
# 1. Long documents: submit asynchronously and poll (the `async` key is consumed by LiteOCR).
liteocr.ocr("200-page.pdf", model="reducto/standard", provider_options={"async": True})

# 2. Word- and line-level OCR boxes in the raw payload (+$2 / 1k pages).
liteocr.ocr("scan.pdf", model="reducto/standard", include_raw=True,
            provider_options={"settings": {"return_ocr_data": True}})

# 3. HTML tables instead of LiteOCR's markdown default, and merge tables split across pages.
liteocr.ocr("report.pdf", model="reducto/r-1",
            provider_options={"formatting": {"table_output_format": "html", "merge_tables": True}})

# 4. Cheap bulk queue: 12-hour completion guarantee, 20% usage discount.
liteocr.ocr("batch.pdf", model="reducto/r-1",
            provider_options={"async": True, "queue_priority": "batch"})

# 5. Password-protected PDF, and always take the presigned-URL result path.
liteocr.ocr("locked.pdf", model="reducto/standard",
            provider_options={"settings": {"document_password": "…", "force_url_result": True}})
```

## 8. Links

* Docs home: <https://docs.reducto.ai> · agent guide: <https://docs.reducto.ai/agent-guide.md> ·
  index: <https://docs.reducto.ai/llms.txt>
* Parse API reference: <https://docs.reducto.ai/api-reference/endpoint/parse>
* Legacy parse schema: <https://docs.reducto.ai/api-reference/legacy/parse>
* Credit usage / pricing: <https://docs.reducto.ai/reference/credit-usage> · <https://reducto.ai/pricing>
* Error codes: <https://docs.reducto.ai/reference/error-codes>
* Studio (per-job inspector): <https://studio.reducto.ai>
