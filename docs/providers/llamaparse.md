# LlamaParse (LlamaCloud)

## 1. Summary

| | |
|---|---|
| Provider name | `llamaparse` (aliases accepted by `ModelRef`: `llama`, `llama_parse`, `llama-parse`, `llamacloud`, `llama_cloud`) |
| Base URL | `https://api.cloud.llamaindex.ai` (override: `base_url` on the request, or `LLAMA_BASE_URL`). EU region: `https://api.cloud.eu.llamaindex.ai` |
| API key | `LLAMA_API_KEY` (or `api_key` on the request) — sent as `Authorization: Bearer llx-…` |
| Docs | <https://developers.llamaindex.ai/llamaparse> (the old `docs.cloud.llamaindex.ai` host 308-redirects here) |
| API version | Path-versioned: LiteOCR uses `/api/v1/parsing/*`. Quality is selected by `tier` + a dated `version` (LiteOCR sends `version=latest`) |
| Verified | 2026-09-11, live against the North America host |
| Implementation | `crates/liteocr-core/src/providers/llamaparse.rs` |

A key is bound to one region; using the wrong host returns `401 "Invalid API Key. Please check your
region …"`. The OpenAPI spec is at `/api/openapi.json` (not `/openapi.json`); Swagger UI at `/docs`.

## 2. Models exposed by LiteOCR

| Model | Provider parameters LiteOCR sets | List price (`pricing.json`) |
|---|---|---|
| `llamaparse/fast` | `tier=fast`, `version=latest` | $0.00125 / page (1 credit) |
| `llamaparse/cost_effective` *(default)* | `tier=cost_effective`, `version=latest` | $0.00375 / page (3 credits) |
| `llamaparse/agentic` | `tier=agentic`, `version=latest` | $0.0125 / page (10 credits) |
| `llamaparse/agentic_plus` | `tier=agentic_plus`, `version=latest` | $0.05625 / page (45 credits) |

1 000 credits = $1.25 ⇒ 1 credit = $0.00125. Prices come from
<https://developers.llamaindex.ai/llamaparse/general/pricing/> and drive `OcrResponse.cost_usd`;
add-ons LiteOCR does not model include `extract_layout` (+3 credits/page) and enriched forms
(+10 credits per form page). The live per-tier version list is `GET /api/v2/parse/versions`.

## 3. Request flow LiteOCR uses

1. **Upload / create job — one call.** `POST {base}/api/v1/parsing/upload`, `multipart/form-data`, with
   `Authorization: Bearer …` and `accept: application/json`. The parts LiteOCR sends are:

   | Part | Value |
   |---|---|
   | `tier` | the model name (`fast` \| `cost_effective` \| `agentic` \| `agentic_plus`) |
   | `version` | `latest` |
   | `language` | the request's `language`, when set (single value; the field is repeatable for arrays) |
   | `target_pages` | from `pages`, **converted 1-based → 0-based**: `"1-3,5"` → `"0-2,4"`; an open range `"10-"` becomes `"9-100000"` |
   | `file` | the document bytes, with filename and guessed MIME type — **path / bytes input only** |
   | `input_url` | the URL string — **URL input only**, in place of `file` |

   Response: `{"id":"<uuid>","status":"PENDING","error_code":null,"error_message":null}`.
2. **Poll.** `GET {base}/api/v1/parsing/job/{job_id}` starting at 1 s, backing off ×1.5 to a maximum of
   5 s. Terminal statuses: `SUCCESS`, `PARTIAL_SUCCESS`, `ERROR`, `CANCELLED` (`PENDING` keeps polling).
   Both `SUCCESS` and `PARTIAL_SUCCESS` proceed to the result fetch.
3. **Result.** `GET {base}/api/v1/parsing/job/{job_id}/result/json` → `{"pages":[…],"job_metadata":{…}}`.
   LiteOCR always uses the JSON result (it carries per-page `md`, `text`, `items[]` and bboxes);
   `/result/markdown`, `/result/text` and the undocumented `/result/raw/markdown` are not used.

**Where `provider_options` are merged:** LlamaParse has no JSON body — everything is a multipart form
field — so `form_fields()` flattens `provider_options` (which must be a JSON object) into additional
text parts. Strings pass through; booleans become `"true"`/`"false"`; numbers are stringified; `null`
values are skipped; objects/arrays are serialised as JSON text. A key already present (e.g. `version`,
`tier`, `language`, `target_pages`) is **replaced**, so provider options override LiteOCR's defaults.

## 4. Response mapping

| LlamaParse field | LiteOCR unified field | Notes |
|---|---|---|
| upload/poll `id` | `OcrResponse.provider_job_id` | |
| `pages[].page` | `Page.page_number` | Already 1-based. |
| `pages[].md` | `Page.markdown` | Trimmed. With `output="text"` the page text is used instead. |
| `pages[].text` | `Page.text` | Trimmed; falls back to `markdown_to_text(md)` when blank. |
| `pages[].width` / `height` | `Page.width` / `Page.height` | The coordinate space `items[].bBox` lives in. |
| `pages[].items[]` | `Page.blocks[]` | In document order. |
| `items[].type` (+ `lvl`) | `Block.type` | See mapping below. |
| `items[].md` | `Block.content` | With `output="text"`, `items[].value` is preferred, else `markdown_to_text(md)`. |
| `items[].value` (string) | `Block.text` | `null` for non-string values. |
| `items[].bBox.{x,y,w,h}` | `Block.bbox` = `{x0,y0,x1,y1}` | **Absolute, in page units**; normalised by `pages[].width/height` and clamped to 0–1. No page dims ⇒ `bbox = None`. |
| `items[].bBox.confidence` | `Block.confidence` | 0–1. `pages[].confidence` and `layoutAwareBbox[]` are not mapped. |
| `job_metadata.job_pages` | `Usage.pages` | Falls back to the number of returned pages. |
| `job_metadata.job_credits_usage` (else `credits_used`) | `Usage.credits` | **Only kept when > 0** — see gotchas. |
| `job_metadata.job_is_cache_hit == true` | `metadata.llamaparse_cache_hit` | |
| job `status == "PARTIAL_SUCCESS"` | `metadata.llamaparse_partial_success` | |
| `pages[].images[]`, `charts`, `links`, `layout[]`, `printedPageNumber`, … | — | Not mapped; visible with `include_raw=True`. |

Item types: `heading` → `title` when `lvl == 1`, otherwise `section_header`; `text` → `text`;
`table` → `table`; `list`/`list_item` → `list`; `figure`/`image`/`chart` → `figure`;
`formula`/`equation` → `formula`; `header` → `header`; `footer` → `footer`; everything else → `other`.

Trimmed real response (`crates/liteocr-core/tests/fixtures/llamaparse_result_json.json`; two pages,
`images`/`layout`/`charts` and most page keys elided):

```json
{
  "pages": [
    {
      "page": 1,
      "text": "Hello LiteOCR\n\nInvoice #1234\nTotal: $56.78",
      "md": "# Hello LiteOCR\n\nInvoice #1234\n\nTotal: $56.78",
      "items": [
        { "type": "heading", "md": "# Hello LiteOCR", "value": "Hello LiteOCR", "lvl": 1,
          "bBox": { "x": 60.178, "y": 56.553, "w": 308.672, "h": 40.835,
                    "confidence": 0.84, "label": "doc_title" },
          "layoutAwareBbox": [ { "x": 60.178, "y": 56.553, "w": 308.672, "h": 40.835,
                                 "startIndex": 0, "endIndex": 14, "confidence": 0.84,
                                 "label": "doc_title" } ] },
        { "type": "text", "md": "Invoice #1234", "value": "Invoice #1234",
          "bBox": { "x": 59.731, "y": 138.867, "w": 217.678, "h": 28.367,
                    "confidence": 0.75, "label": "paragraph_title" } }
      ],
      "status": "OK", "width": 1000, "height": 1300, "confidence": 0.875,
      "parsingMode": "accurate", "originalOrientationAngle": 0,
      "pageHeaderMarkdown": "", "pageFooterMarkdown": "", "printedPageNumber": "",
      "noTextContent": false, "costOptimized": false
    },
    {
      "page": 2,
      "md": "## Line Items\n\n| Item     | Qty | Price  |\n| -------- | --- | ------ |\n| Widget   | 2   | $10.00 |",
      "items": [
        { "type": "heading", "md": "## Line Items", "value": "Line Items", "lvl": 2,
          "bBox": { "x": 60.159, "y": 55.671, "w": 235.113, "h": 44.238, "confidence": 0.84 } },
        { "type": "table",
          "md": "| Item     | Qty | Price  |\n| -------- | --- | ------ |\n| Widget   | 2   | $10.00 |",
          "html": "<table>…</table>",
          "rows": [["Item","Qty","Price"],["Widget","2","$10.00"]],
          "isPerfectTable": true,
          "csv": "\"Item\",\"Qty\",\"Price\"\n\"Widget\",\"2\",\"$10.00\"",
          "bBox": { "x": 57.919, "y": 138.705, "w": 843.873, "h": 242.652,
                    "confidence": 0.99, "label": "table" } }
      ],
      "width": 1000, "height": 1300
    }
  ],
  "job_metadata": {
    "credits_used": 0,
    "job_credits_usage": 0,
    "job_pages": 2,
    "job_auto_mode_triggered_pages": 0,
    "job_is_cache_hit": false
  }
}
```

## 5. Errors, status codes, rate limits, timeouts

Every error is FastAPI-shaped: either `{"detail": "message"}` or, on 422, `{"detail": [ValidationError…]}`.
`Error::from_http` picks up `detail` (stringifying the array form) and classifies by status:

| Status | Trigger | LiteOCR `ErrorKind` |
|---|---|---|
| 400 | `tier` without `version`; no input source; malformed job id | `bad_request` |
| 401 | bad key (wrong region), or no `Authorization` header (`"Not authenticated"`) | `authentication` |
| 404 | unknown job, or `Result for Parsing Job <id> not found. Check job status…` | `bad_request` |
| 422 | invalid enum/type in the form body (e.g. an unknown `language`) | `bad_request` |
| 429 | rate limited | `rate_limit` (retried) |
| 5xx | server error | `provider` (retried) |

A job that reaches `ERROR` or `CANCELLED` is turned into an error whose message is
`job <STATUS>: <error_code> <error_message>` with the job id attached. The kind is `bad_request` when
`error_code` starts with `INVALID` (e.g. `INVALID_TIER_VERSION_COMBINATION`), otherwise `provider`.

**Retries.** `max_retries` (default 2), exponential backoff with full jitter, on rate-limit, network and
500/502/503/504 only.

**Rate limits.** `POST /api/v1/parsing/upload`: 50 QPS over a 10-second window, per **organization**.
`POST /api/v1/beta/files`: 50 QPS over 5 s, per project. Free-tier organizations: 20 requests/minute
overall. **No `Retry-After` and no rate-limit headers on any endpoint**, so backoff is blind.

**Timeouts.** `timeout_secs` (default 300) is the whole-call deadline (upload + polling + result fetch)
and caps each request. For reference, the official SDKs poll every 1 s with a 2 000 s ceiling; a 1-page
image on `cost_effective` finished in ~4.3 s in testing.

## 6. Gotchas (verified)

* **`tier` requires `version`.** Sending a tier alone is `400 "Must specify a version with a tier.
  Tier: cost_effective"`. LiteOCR always sends `version=latest`; pin a dated version through
  `provider_options` when you need reproducibility.
* **`tier` is not validated at upload.** An unknown tier returns `200 PENDING` and only fails later with
  `status: "ERROR"`, `error_code: "INVALID_TIER_VERSION_COMBINATION"`. LiteOCR validates the tier
  client-side, but a tier overridden via `provider_options` bypasses that check.
* **`PARTIAL_SUCCESS` is a real terminal status** (some pages failed within `page_error_tolerance`) and
  **results are retrievable**. LiteOCR treats it as success and flags
  `metadata.llamaparse_partial_success = true`.
* **Credits are 0 until billing settles.** `credits_used` and `job_credits_usage` were `0` on every
  observed job — including `agentic` on 2 pages with `job_is_cache_hit: false` — because usage is
  recorded asynchronously. LiteOCR therefore drops non-positive values and leaves `Usage.credits` as
  `None`; `cost_usd` comes from the price table instead. For real billing numbers use
  `GET /api/v1/beta/usage-metrics`.
* **Re-parsing the same file within 48 hours is a free cache hit**, which makes latency and cost
  benchmarks meaningless. `liteocr bench` therefore injects
  `{"do_not_cache": true, "invalidate_cache": true}` for every `llamaparse/*` model
  (`crates/liteocr-cli/src/bench.rs::cache_busting_options`) unless caching is explicitly allowed.
  `metadata.llamaparse_cache_hit` surfaces a hit when it happens.
* **The docs moved** from `docs.cloud.llamaindex.ai` to `developers.llamaindex.ai` (308), and most old
  deep links 404. The OpenAPI spec is at `/api/openapi.json`.
* **`result_type` is not an API parameter** — it is SDK-only and silently ignored by the server. The
  result flavour is chosen by which `/result/...` endpoint you call.
* **`error_code` / `error_message` are omitted (not null)** on `GET /job/{id}` for successful jobs, but
  present-and-null on the upload response.
* **Three coordinate spaces in one response**: `items[].bBox` and `images[]` are in page units
  (`pages[].width/height` — LiteOCR normalises against these), `pages[].layout[].bbox` is already
  normalised 0–1, and `images[].ocr[]` is in that image's own `original_width`×`original_height` pixels.
* **Mixed casing.** `bBox`, `layoutAwareBbox`, `isPerfectTable`, `noTextContent`, `originalOrientationAngle`
  are camelCase while `job_metadata`, `original_width` are snake_case — no blanket rename rule works.
* **`target_pages` is 0-based** while LiteOCR's `pages` is 1-based; the conversion happens in
  `form_fields()`. The document-level markdown is exactly `page_separator.join(pages[].md)` with a
  default separator of `"\n\n---\n\n"`.
* **The `fast` tier degrades tables noticeably** (misaligned columns on a table `agentic` got right).
  Avoid it for anything structured. Also note `output_tables_as_HTML` (capital HTML) only affects the
  rendered markdown — `items[].html` is present either way.

## 7. Useful `provider_options` passthrough

```python
# 1. Pin a dated parser version instead of `latest` (reproducible output).
liteocr.ocr("doc.pdf", model="llamaparse/cost_effective",
            provider_options={"version": "2026-08-19"})

# 2. Defeat the 48-hour result cache (what the benchmark does).
liteocr.ocr("doc.pdf", model="llamaparse/agentic",
            provider_options={"do_not_cache": True, "invalidate_cache": True})

# 3. Layout blocks and a full-page screenshot in the raw payload (+3 credits/page for layout).
liteocr.ocr("scan.png", model="llamaparse/agentic", include_raw=True,
            provider_options={"extract_layout": True, "take_screenshot": True})

# 4. Prompt steering and table tuning.
liteocr.ocr("statement.pdf", model="llamaparse/agentic_plus",
            provider_options={"parsing_instruction": "Preserve every table column.",
                              "merge_tables_across_pages_in_markdown": True,
                              "output_tables_as_HTML": True})

# 5. Skip OCR on a digital-native PDF, hide running headers/footers, tolerate bad pages.
liteocr.ocr("contract.pdf", model="llamaparse/fast",
            provider_options={"disable_ocr": True, "hide_headers": True, "hide_footers": True,
                              "page_error_tolerance": 0.1, "replace_failed_page_mode": "raw_text"})
```

## 8. Links

* Docs home: <https://developers.llamaindex.ai/llamaparse>
* Tiers: <https://developers.llamaindex.ai/llamaparse/parse/guides/tiers/>
* Pricing: <https://developers.llamaindex.ai/llamaparse/general/pricing/> · <https://www.llamaindex.ai/pricing>
* Rate limits: <https://developers.llamaindex.ai/llamaparse/general/rate_limits>
* Regions: <https://developers.llamaindex.ai/python/cloud/general/regions>
* OpenAPI spec: <https://api.cloud.llamaindex.ai/api/openapi.json> · Swagger UI:
  <https://api.cloud.llamaindex.ai/docs>
* Live per-tier version list: `GET https://api.cloud.llamaindex.ai/api/v2/parse/versions`
* Supported input extensions: `GET /api/v1/parsing/supported_file_extensions` (~130 extensions)
