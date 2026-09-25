# LlamaParse (LlamaCloud)

## 1. Summary

| | |
|---|---|
| Provider name | `llamaparse` (aliases accepted by `ModelRef`: `llama`, `llama_parse`, `llama-parse`, `llamacloud`, `llama_cloud`) |
| Base URL | `https://api.cloud.llamaindex.ai` (override: `base_url` on the request, or `LLAMA_BASE_URL`). EU region: `https://api.cloud.eu.llamaindex.ai` |
| API key | `LLAMA_API_KEY` (or `api_key` on the request) — sent as `Authorization: Bearer llx-…` |
| Docs | <https://developers.llamaindex.ai/llamaparse> (the old `docs.cloud.llamaindex.ai` host 308-redirects here) |
| API version | Path-versioned: PuffinParse uses `/api/v1/parsing/*`. Quality is selected by `tier` + a dated `version` (PuffinParse sends `version=latest`) |
| Verified | 2026-09-11, live against the North America host |
| Implementation | `crates/puffinparse-core/src/providers/llamaparse.rs` |

A key is bound to one region; using the wrong host returns `401 "Invalid API Key. Please check your
region …"`. The OpenAPI spec is at `/api/openapi.json` (not `/openapi.json`); Swagger UI at `/docs`.

## 2. Models exposed by PuffinParse

| Model | Provider parameters PuffinParse sets | List price (`pricing.json`) |
|---|---|---|
| `llamaparse/fast` | `tier=fast`, `version=latest` | $0.00125 / page (1 credit) |
| `llamaparse/cost_effective` *(default)* | `tier=cost_effective`, `version=latest` | $0.00375 / page (3 credits) |
| `llamaparse/agentic` | `tier=agentic`, `version=latest` | $0.0125 / page (10 credits) |
| `llamaparse/agentic_plus` | `tier=agentic_plus`, `version=latest` | $0.05625 / page (45 credits) |

`cost_effective`, `agentic` and `agentic_plus` also serve `extract` (§5); `fast` does not — it is a
parse-only tier. Extract prices are per page, on top of the parse the extraction runs:

| Model | Extract parameters | List price (`pricing.json`, `extract`) |
|---|---|---|
| `llamaparse/cost_effective` *(default for `extract`)* | `configuration.tier=cost_effective` | $0.01 / page (5 extract + 3 parse credits) |
| `llamaparse/agentic` | `configuration.tier=agentic` | $0.03125 / page (15 + 10 credits) |
| `llamaparse/agentic_plus` | `configuration.tier=agentic_plus` | $0.11875 / page (50 + 45 credits) |

1 000 credits = $1.25 ⇒ 1 credit = $0.00125. Prices come from
<https://developers.llamaindex.ai/llamaparse/general/pricing/> and drive `OcrResponse.cost_usd`;
add-ons PuffinParse does not model include `extract_layout` (+3 credits/page) and enriched forms
(+10 credits per form page). The live per-tier version list is `GET /api/v2/parse/versions`.

## 3. Request flow PuffinParse uses

1. **Upload / create job — one call.** `POST {base}/api/v1/parsing/upload`, `multipart/form-data`, with
   `Authorization: Bearer …` and `accept: application/json`. The parts PuffinParse sends are:

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
   PuffinParse always uses the JSON result (it carries per-page `md`, `text`, `items[]` and bboxes);
   `/result/markdown`, `/result/text` and the undocumented `/result/raw/markdown` are not used.

**Where `provider_options` are merged:** LlamaParse has no JSON body — everything is a multipart form
field — so `form_fields()` flattens `provider_options` (which must be a JSON object) into additional
text parts. Strings pass through; booleans become `"true"`/`"false"`; numbers are stringified; `null`
values are skipped; objects/arrays are serialised as JSON text. A key already present (e.g. `version`,
`tier`, `language`, `target_pages`) is **replaced**, so provider options override PuffinParse's defaults.

### Jobs API and webhooks (`submit_parse` / `retrieve_parse`, SPEC §15)

* **Submit** — the same `POST {base}/api/v1/parsing/upload`; the job `id` becomes
  `JobHandle.job_id`. `webhook_url` becomes the multipart field `webhook_url` (LlamaParse requires
  HTTPS, a domain name rather than an IP, and fewer than 200 characters).
* **Retrieve** — one `GET {base}/api/v1/parsing/job/{id}` (retried on 429/5xx); `SUCCESS` /
  `PARTIAL_SUCCESS` then fetch `result/json` exactly like `parse`; `ERROR` / `CANCELLED` →
  `JobStatus::Failed` with `error_code error_message` (`INVALID*` → `bad_request`, else
  `provider`); `PENDING` → `Pending`.
* **Webhook bodies** — two shapes reach a handler, and `parse_webhook` reads both:
  * the v1 `webhook_url` *result push*, `{"txt", "md", "json": [{"page", "text", "md", …}],
    "images"}`: this **is** the result (normalised through the `result/json` mapping, no geometry
    unless `items` are present) → `Succeeded`, but it carries no job id;
  * a LlamaCloud *event* (sent for jobs created with `webhook_configurations`, a v2 feature; that
    PuffinParse's v1 upload accepts it through `provider_options` is **not verified**),
    `{"event_id", "event_type": "parse.success", "timestamp", "data": {"job_id"}}`:
    `parse.pending` → `Pending`; `parse.success` / `partial_success` / `error` / `cancelled` →
    `Finished` (retrieve for the result or the error message). Events for other products
    (`extract.*`, …) are rejected. Verify the `LC-Signature` header when a signing secret is set.
* Verified live 2026-09-24 (`tests/live_jobs.rs::llamaparse_submit_retrieve_live`, `fast`, 1 page:
  3 status checks, ~4.4 s).

## 4. Response mapping (`parse` / `ocr`)

| LlamaParse field | PuffinParse unified field | Notes |
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

Trimmed real response (`crates/puffinparse-core/tests/fixtures/llamaparse_result_json.json`; two pages,
`images`/`layout`/`charts` and most page keys elided):

```json
{
  "pages": [
    {
      "page": 1,
      "text": "Hello PuffinParse\n\nInvoice #1234\nTotal: $56.78",
      "md": "# Hello PuffinParse\n\nInvoice #1234\n\nTotal: $56.78",
      "items": [
        { "type": "heading", "md": "# Hello PuffinParse", "value": "Hello PuffinParse", "lvl": 1,
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

## 5. Extract mode (`extract`) — LlamaExtract

Extraction is a different API surface from parsing: PuffinParse uses LlamaCloud's **v2 extract** endpoints,
not the v1 extraction-agent ones, so no agent has to be created and the schema travels with the
request. Implementation: `LlamaParse::extract` in `crates/puffinparse-core/src/providers/llamaparse.rs`.

1. **Upload.** `POST {base}/api/v1/beta/files`, `multipart/form-data` with parts `file` and
   `purpose=extract` → `201` `{"id": "<uuid>", "name": …, "expires_at": …}` (files expire after 48 h).
   Unlike the parse path there is **no URL input**: an `https://…` document is downloaded by PuffinParse
   and re-uploaded.
2. **Create.** `POST {base}/api/v2/extract`:

   ```json
   {
     "file_input": "<file uuid, or a parse job id>",
     "configuration": {
       "tier": "cost_effective",
       "data_schema": { "...the request's JSON Schema, verbatim..." },
       "cite_sources": true,
       "confidence_scores": true,
       "system_prompt": "…only when `instructions` is set…",
       "target_pages": "1-3,5"
     }
   }
   ```

   → `{"id": "ext-…", "status": "PENDING", "configuration": {…resolved defaults…}}`.
3. **Poll.** `GET {base}/api/v2/extract/{id}?expand=usage&expand=extract_metadata`, 1 s backing off
   ×1.5 to 5 s. **Both `expand` values matter**: without them `usage` and `extract_metadata` come back
   `null` even when citations were requested. Terminal statuses: `COMPLETED`, `FAILED`, `CANCELLED`
   (a failure carries `error_message`).

Mapping of the unified request:

* the schema is passed through **verbatim** (LlamaCloud accepts standard JSON Schema);
* `instructions` → `configuration.system_prompt`;
* `citations = true` → both `cite_sources` and `confidence_scores` (they are reported together under
  `extract_metadata`);
* `pages` → `configuration.target_pages`, which is **1-based** here — the opposite of the 0-based
  `target_pages` form field on the parsing endpoint;
* `provider_options` are merged into `configuration` key-by-key (`use_reasoning`, `extraction_target`,
  `parse_tier`, `disable_cache`, `max_pages`, `spreadsheet_mode`, …), and win over PuffinParse's defaults.

### Response mapping

| LlamaCloud field | PuffinParse unified field | Notes |
|---|---|---|
| `extract_result` | `ExtractResponse.data` | The schema shape; no wrapper to strip. |
| `extract_metadata.field_metadata.document_metadata` | `ExtractResponse.fields` | A **parallel tree** mirroring the data: objects keyed by field, arrays as lists indexed by position, and `{citation, confidence, extraction_confidence, parsing_confidence}` entries at the leaves. Walked into RFC 6901 pointers (`/sites/0/samples`). |
| leaf `confidence` (else `extraction_confidence`) | `FieldInfo.confidence` | 0–1. `parsing_confidence` is not surfaced. |
| leaf `citation[].page` | `Citation.page_number` | 1-based. |
| leaf `citation[].bounding_boxes[]` (`{x,y,w,h}`) | `Citation.bbox` | Normalised by the same entry's `page_dimensions` (PDF points). One `Citation` per box; a citation with no boxes still yields one with `bbox: None` (the `turbo` tier is text-only). |
| leaf `citation[].matching_text` | `Citation.text` | The matched **markdown** span, so it can contain `#`/`|`/`**`. |
| `usage.credits` | `Usage.credits` | Total: `extract_credits` + `parse_credits`. |
| — | `Usage.pages` | **Derived** — see below. |
| `extract_metadata.parse_job_id` / `parse_tier` | `metadata.llamaparse_parse_job_id` / `llamaparse_parse_tier` | The parse defaults to the extract tier. |
| `id` | `ExtractResponse.provider_job_id` | `ext-…` |

**Page count is derived.** LlamaExtract reports no page count anywhere in the job. PuffinParse uses the
highest page number seen in the citations; with citations off it divides `usage.extract_credits` by the
tier's published per-page rate (5 / 15 / 50 credits for cost_effective / agentic / agentic_plus, 35 for
turbo); failing both it reports 1. So `usage.pages` — and therefore `cost_usd` — is a best-effort
figure, exact when citations are on and every page contributes a cited field.

Trimmed real response (`crates/puffinparse-core/tests/fixtures/llamaparse_extract_job.json`, a 2-page PDF):

```json
{
  "id": "ext-4mlkajo0l99nprnzarbjmt5x6143",
  "status": "COMPLETED",
  "extract_result": { "title": "A Short History of the Harbor",
                      "sites": [ { "site": "Hazel Bend", "samples": 205 } ] },
  "extract_metadata": {
    "field_metadata": {
      "document_metadata": {
        "title": {
          "citation": [ { "page": 1, "matching_text": "# A Short History of the Harbor",
                          "bounding_boxes": [ { "x": 36.86, "y": 38.94, "w": 341.68, "h": 24.56 } ],
                          "page_dimensions": { "width": 595.2, "height": 841.92 } } ],
          "confidence": 0.9425, "extraction_confidence": 0.9425, "parsing_confidence": 1.0
        },
        "sites": [ { "site": { "citation": [ { "page": 2, "…": "…" } ], "confidence": 0.9476 },
                     "samples": { "…": "…" } } ]
      },
      "page_metadata": null, "row_metadata": null
    },
    "parse_job_id": "pjb-12ue2qidroaofurlc4mfqgfgli1j",
    "parse_tier": "agentic"
  },
  "usage": { "credits": 50.0, "extract_credits": 30.0, "parse_credits": 20.0 }
}
```

**Verified live** on 2026-09-11: `invoice_001.png` on `cost_effective` returned
`{invoice_number: "INV-9865", total: "$14,667.43", date: "2024-08-03", vendor: "Cedar Ridge Supply"}`
with `usage.credits = 8` (5 extract + 3 parse) and every field cited on page 1; the 2-page
`multipage_001.pdf` on `agentic` returned 7 table rows with cell-level citations on page 2 and
`credits = 50` for 2 pages, matching 15 + 10 credits per page exactly. Tests:
`providers::llamaparse::tests::live_extract` (`#[ignore]`) and `normalizes_extract_fixture`.

### Extract gotchas

* **`expand` is not optional.** `GET /api/v2/extract/{id}` without `expand=usage&expand=extract_metadata`
  returns `usage: null` and `extract_metadata: null`, which looks exactly like "citations were not
  produced".
* **`target_pages` flips base** between the two APIs: 0-based on `/api/v1/parsing/upload`, 1-based on
  the v2 extract configuration.
* **No URL input and no page count** — both are handled by PuffinParse (download + re-upload, derived
  pages).
* **`fast` is not an extract tier**; `turbo` exists on the API (35 credits/page, text-only citations)
  but is not registered as a PuffinParse model, so `llamaparse/turbo` resolves to an unsupported-model
  error even though the provider code accepts the tier.
* **Results are cached**: an identical file + configuration returns in a couple of seconds and may not
  be billed again. Pass `provider_options={"disable_cache": true}` for benchmarking.
* **`matching_text` is markdown**, taken from the parse output rather than the raw page text, and the
  boxes are the parse block's, not a tight box around the value.
* Uploaded files carry `expires_at` (48 h) and `purpose=extract`; a file uploaded for parsing is not
  reusable here.

## 6. Errors, status codes, rate limits, timeouts

Every error is FastAPI-shaped: either `{"detail": "message"}` or, on 422, `{"detail": [ValidationError…]}`.
`Error::from_http` picks up `detail` (stringifying the array form) and classifies by status:

| Status | Trigger | PuffinParse `ErrorKind` |
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

## 7. Gotchas (verified)

* **`tier` requires `version`.** Sending a tier alone is `400 "Must specify a version with a tier.
  Tier: cost_effective"`. PuffinParse always sends `version=latest`; pin a dated version through
  `provider_options` when you need reproducibility.
* **`tier` is not validated at upload.** An unknown tier returns `200 PENDING` and only fails later with
  `status: "ERROR"`, `error_code: "INVALID_TIER_VERSION_COMBINATION"`. PuffinParse validates the tier
  client-side, but a tier overridden via `provider_options` bypasses that check.
* **`PARTIAL_SUCCESS` is a real terminal status** (some pages failed within `page_error_tolerance`) and
  **results are retrievable**. PuffinParse treats it as success and flags
  `metadata.llamaparse_partial_success = true`.
* **Credits are 0 until billing settles.** `credits_used` and `job_credits_usage` were `0` on every
  observed job — including `agentic` on 2 pages with `job_is_cache_hit: false` — because usage is
  recorded asynchronously. PuffinParse therefore drops non-positive values and leaves `Usage.credits` as
  `None`; `cost_usd` comes from the price table instead. For real billing numbers use
  `GET /api/v1/beta/usage-metrics`.
* **Re-parsing the same file within 48 hours is a free cache hit**, which makes latency and cost
  benchmarks meaningless. `puffinparse bench` therefore injects
  `{"do_not_cache": true, "invalidate_cache": true}` for every `llamaparse/*` model
  (`crates/puffinparse-cli/src/bench.rs::cache_busting_options`) unless caching is explicitly allowed.
  `metadata.llamaparse_cache_hit` surfaces a hit when it happens.
* **The docs moved** from `docs.cloud.llamaindex.ai` to `developers.llamaindex.ai` (308), and most old
  deep links 404. The OpenAPI spec is at `/api/openapi.json`.
* **`result_type` is not an API parameter** — it is SDK-only and silently ignored by the server. The
  result flavour is chosen by which `/result/...` endpoint you call.
* **`error_code` / `error_message` are omitted (not null)** on `GET /job/{id}` for successful jobs, but
  present-and-null on the upload response.
* **Three coordinate spaces in one response**: `items[].bBox` and `images[]` are in page units
  (`pages[].width/height` — PuffinParse normalises against these), `pages[].layout[].bbox` is already
  normalised 0–1, and `images[].ocr[]` is in that image's own `original_width`×`original_height` pixels.
* **Mixed casing.** `bBox`, `layoutAwareBbox`, `isPerfectTable`, `noTextContent`, `originalOrientationAngle`
  are camelCase while `job_metadata`, `original_width` are snake_case — no blanket rename rule works.
* **`target_pages` is 0-based** while PuffinParse's `pages` is 1-based; the conversion happens in
  `form_fields()`. The document-level markdown is exactly `page_separator.join(pages[].md)` with a
  default separator of `"\n\n---\n\n"`.
* **The `fast` tier degrades tables noticeably** (misaligned columns on a table `agentic` got right).
  Avoid it for anything structured. Also note `output_tables_as_HTML` (capital HTML) only affects the
  rendered markdown — `items[].html` is present either way.

## 8. Useful `provider_options` passthrough

```python
# 1. Pin a dated parser version instead of `latest` (reproducible output).
puffinparse.ocr("doc.pdf", model="llamaparse/cost_effective",
            provider_options={"version": "2026-08-19"})

# 2. Defeat the 48-hour result cache (what the benchmark does).
puffinparse.ocr("doc.pdf", model="llamaparse/agentic",
            provider_options={"do_not_cache": True, "invalidate_cache": True})

# 3. Layout blocks and a full-page screenshot in the raw payload (+3 credits/page for layout).
puffinparse.ocr("scan.png", model="llamaparse/agentic", include_raw=True,
            provider_options={"extract_layout": True, "take_screenshot": True})

# 4. Prompt steering and table tuning.
puffinparse.ocr("statement.pdf", model="llamaparse/agentic_plus",
            provider_options={"parsing_instruction": "Preserve every table column.",
                              "merge_tables_across_pages_in_markdown": True,
                              "output_tables_as_HTML": True})

# 5. Skip OCR on a digital-native PDF, hide running headers/footers, tolerate bad pages.
puffinparse.ocr("contract.pdf", model="llamaparse/fast",
            provider_options={"disable_ocr": True, "hide_headers": True, "hide_footers": True,
                              "page_error_tolerance": 0.1, "replace_failed_page_mode": "raw_text"})
```

## 9. Links

* Docs home: <https://developers.llamaindex.ai/llamaparse>
* Tiers: <https://developers.llamaindex.ai/llamaparse/parse/guides/tiers/>
* Pricing: <https://developers.llamaindex.ai/llamaparse/general/pricing/> · <https://www.llamaindex.ai/pricing>
* Rate limits: <https://developers.llamaindex.ai/llamaparse/general/rate_limits>
* Regions: <https://developers.llamaindex.ai/python/cloud/general/regions>
* OpenAPI spec: <https://api.cloud.llamaindex.ai/api/openapi.json> · Swagger UI:
  <https://api.cloud.llamaindex.ai/docs>
* Live per-tier version list: `GET https://api.cloud.llamaindex.ai/api/v2/parse/versions`
* Supported input extensions: `GET /api/v1/parsing/supported_file_extensions` (~130 extensions)
