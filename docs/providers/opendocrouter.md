# OpenDocRouter

> **Status: docs-only.** Everything below comes from the OpenDocRouter docs
> (<https://www.opendocrouter.ai/docs>), its OpenAPI spec (`/v1/openapi.json`) and the public
> `GET /v1/models` listing, all read 2026-10-08, and from
> `crates/puffinparse-core/src/providers/opendocrouter.rs`. No live call has been made — this
> repository has no OpenDocRouter key. The `opendocrouter_*.json` fixtures are hand-built from the
> documented response schema. Mark this page **verified** only after the `#[ignore]`d live test in
> `providers/opendocrouter.rs` passes with a real key.
> Help verify it: [issue #10](https://github.com/ajinkyashejul/puffinparse/issues/10).

## 1. Summary

| | |
|---|---|
| Provider name | `opendocrouter` (aliases `odr`, `open_doc_router`, `open-doc-router`) |
| Base URL | `https://www.opendocrouter.ai` (override: `base_url` on the request, or `OPEN_DOC_ROUTER_BASE_URL`) |
| API key | `OPEN_DOC_ROUTER_API_KEY` (the name OpenDocRouter's own SDKs read; or `api_key` on the request) — sent as `Authorization: Bearer <key>` |
| Docs | <https://www.opendocrouter.ai/docs>, API reference <https://www.opendocrouter.ai/v1/reference> |
| API version | Path-versioned (`/v1`). Each response names the model's recipe version (`model_version`) and the price list it was billed on (`price_version`) |
| Checked against | **not live-verified** (see banner) — documentation and OpenAPI spec read 2026-10-08 |
| Implementation | `crates/puffinparse-core/src/providers/opendocrouter.rs` |

OpenDocRouter is LlamaIndex's hosted router for document parsing: one endpoint, `POST /v1/parse`,
runs a PDF or image through the model you name — frontier VLMs (Gemini, Claude, GPT) at their
providers' token prices, or open OCR models (MinerU, PaddleOCR-VL, dots.mocr, …) that it hosts.
Every page is parsed independently and comes back with its own status, token usage and charge.

## 2. Models exposed by PuffinParse

Model strings keep OpenDocRouter's own vendor-qualified id after the provider name:
`opendocrouter/<vendor>/<model>`. Only the **first** `/` separates provider from model (everywhere:
`ModelRef`, pricing, the CLI, the gateway and the Python / Node bindings), so
`opendocrouter/google/gemini-3-flash` sends `"model": "google/gemini-3-flash"`. Gateway key
allow-lists accept `opendocrouter/*` and narrower prefixes such as `opendocrouter/google/*`.

| Model | Modes | Price per 1M tokens (input / cached / output) | List estimate (`pricing.json`) |
|---|---|---|---|
| `opendocrouter/google/gemini-3.8-flash-low` *(default)* | `parse`, `ocr` (derived) | $0.75 / $0.08 / $3.75 | $0.005908 / page |
| `opendocrouter/google/gemini-3-flash` | `parse`, `ocr` (derived) | $0.50 / $0.05 / $3.00 | $0.019667 / page |
| `opendocrouter/anthropic/claude-opus-5-5` | `parse`, `ocr` (derived) | $4.00 / $0.20 / $20.00 | $0.04882 / page |
| `opendocrouter/anthropic/claude-haiku-5-5` | `parse`, `ocr` (derived) | $0.10 / $0.01 / $0.50 | $0.001225 / page |
| `opendocrouter/openai/gpt-5.6-terra` | `parse`, `ocr` (derived) | $2.00 / $0.20 / $12.00 | $0.019886 / page |
| `opendocrouter/openai/gpt-6-luna` | `parse`, `ocr` (derived) | $0.10 / $0.01 / $0.50 | $0.000798 / page |
| `opendocrouter/infly/infinity-parser2-flash` | `parse`, `ocr` (derived) | $0.24 / $0.24 / $1.16 | $0.004344 / page |
| `opendocrouter/opendatalab/mineru2.5-pro` | `parse`, `ocr` (derived) | $0.08 / $0.08 / $0.39 | $0.000861 / page |
| `opendocrouter/xingchen-agi/teleocr` | `parse`, `ocr` (derived) | $0.25 / $0.25 / $1.22 | $0.002702 / page |
| `opendocrouter/rednote-hilab/dots.mocr` | `parse`, `ocr` (derived) | $0.31 / $0.31 / $1.53 | $0.00397 / page |
| `opendocrouter/paddlepaddle/paddleocr-vl-1.6` | `parse`, `ocr` (derived) | $0.24 / $0.24 / $1.20 | $0.002111 / page |

**Pricing source.** <https://www.opendocrouter.ai/models> and `GET /v1/models`, **price version
2026-10-06**, read 2026-10-08. OpenDocRouter bills tokens, not pages, so the per-page numbers in
`pricing.json` are the site's own *average charge per page* (`avg_charge_per_page_usd`), without the
layout surcharge (**+$0.20 per 1M tokens** on pages whose layout comes back). They are only used when
a response carries no charge: every response does, and PuffinParse reports the **actual**
`charge_usd` as `Usage.provider_cost_usd` and therefore `cost_usd`. Credit is prepaid (top-ups from
$25 plus a 5% fee); failed, cached and blank pages are free.

The default is `google/gemini-3.8-flash-low`, the model OpenDocRouter's own quickstart uses.
The registry lists the 11 models published on 2026-10-08; a newer one can be reached with
`provider_options={"model": "<vendor>/<model>"}` on any registered model string (the option is
merged into the body), though the price estimate then belongs to the registered model.

**`ocr` mode** is derived from `parse` (`TextResponse::from_parse`, tagged
`metadata.puffinparse_derived_from = "parse"`), as for every provider without a native OCR
endpoint. OpenDocRouter returns no line or word geometry: with layout on, each derived line carries
the box of the **layout element** it came from; without layout, lines have no boxes. Use a native
OCR model (Textract, Azure Read, Google Document AI, Tesseract) when you need word boxes.

## 3. Request flow PuffinParse uses

**Body.** `POST {base}/v1/parse`, JSON, `Authorization: Bearer <key>`:

| Field | Value |
|---|---|
| `model` | the model id after `opendocrouter/` |
| `document` | see below |
| `layout` | `true` by default, so blocks get types and boxes (`provider_options={"layout": False}` turns it off and saves the layout surcharge) |
| `pages` | `pages` passed through (1-based, e.g. `"1-3,7"`), validated locally first |
| `mode`, `cache` | only for the async flow (below), or when set in `provider_options` |

`provider_options` are merged into the top level of the body (a `null` value removes a default such
as `layout`), except `upload`, which PuffinParse consumes, and `document`, which it always builds.

**Document.**

* URL input → `{"url": "<url>"}`. OpenDocRouter fetches it: public HTTPS on the default port only
  (`url_not_allowed` otherwise), up to 50 MB / 500 pages.
* Path or bytes up to **2.9 MB** → inline `{"data": "<base64>", "mime_type": …}`. The MIME type is
  sniffed from the magic bytes; anything other than PDF, PNG or JPEG is an `InputError` before any
  network call. (The docs say "about 3 MB" with a 4 MB cap on the JSON body; base64 adds a third.)
* Larger files, or `provider_options={"upload": True}` → `POST /v1/uploads` (no body) →
  `{upload_id, upload_url, max_bytes, expires_at}` → `PUT` the raw bytes to `upload_url` **without**
  the API key (the URL authorises itself) → `{"upload_id": …}`. A file over `max_bytes` (50 MB) is an
  `InputError`. `{"upload": False}` forces inline.

**Sync (default, up to 50 pages).** One `POST /v1/parse` → `200` with every page.

**Async (more than 50 pages).** OpenDocRouter refuses a sync request over the model's
`max_sync_pages` (50 for every listed model) with `413 too_large`, without charging it. PuffinParse
switches to async when:

* `provider_options={"mode": "async"}`; or
* a closed `pages` selection covers more than 50 pages, or a local PDF counts more than 50 pages
  (best-effort count of `/Type /Page` objects); or
* the sync request comes back `413` and no `mode` was given (URLs and compressed PDFs cannot be
  counted up front). An upload is single-use, so the file is uploaded again for the async request.

The async request is the same body with `"mode": "async", "cache": true` (async requires `cache`;
an explicit `provider_options={"cache": False}` is an `InputError` rather than a silent change) →
`202 {id, status: "processing"}` → poll `GET /v1/parse/{id}` every 1 s, backing off ×1.5 to 10 s,
until the status is no longer `processing` → `GET /v1/parse/{id}?expand=markdown,layout`
(`expand=markdown` without layout), following `cursor=<next_cursor>` while `has_more` (results over
4 MB come in parts). `provider_options={"mode": "sync"}` disables the automatic switch.

**Jobs** (`puffinparse.submit` / `retrieve`, `crate::submit_parse` / `retrieve_parse`): submit is the
async `POST` (always `mode: "async", cache: true`); each retrieve is one `GET /v1/parse/{id}`, plus
the expanded results once it has finished. The handle stores `provider_state = {"layout": bool}` so
retrieve asks for the layout only when it was requested. OpenDocRouter sends no webhooks:
`webhook_url` is rejected on submit, and `handle_webhook` / `parse_webhook` return an `InputError`.

## 4. Response mapping

| OpenDocRouter field | PuffinParse unified field | Notes |
|---|---|---|
| `id` | `ParseResponse.provider_job_id` | Also the job id on errors. |
| `pages[]` with `status: "ok"` | one `Page` each | Sorted by `page` (1-based, numbers of the original document). |
| `pages[].markdown` | `Page.markdown` | Trimmed; `Page.text` is `markdown_to_text` of it. Document markdown is the pages joined in order. |
| `pages[].layout.width/height` | `Page.width` / `Page.height` | PDF points, or pixels for images. `None` without layout. |
| `pages[].layout.elements[]` | `Page.blocks[]` | In reading order, then pictures the markdown does not mention. |
| element `lines: [first, last]` | `Block.content` | The page markdown split on `"\n"` **only** (as the docs require), lines `first..=last`, joined and trimmed. `null` (an unmentioned picture) → empty content, box kept. |
| element `boxes[]` | `Block.bbox` | Already fractions of the page from the top left; PuffinParse takes the box enclosing all pieces (a paragraph continued in the next column has two) and clamps to 0..1. For slanted text (`r`) the unrotated box is used. No boxes → `None`. |
| element `confidence` | `Block.confidence` | 0–1. |
| element `type` | `Block.type` | Table below. |
| `pages[]` with `status: "error"` | `metadata.opendocrouter_failed_pages` | `[{page, code, message, reason?}]`; see §5. |
| `pages[].layout.status == "error"` | `metadata.opendocrouter_layout_errors` | `[{page, code, message}]`; the page keeps its markdown as one text block with no box. |
| `pages[].cached == true` | `metadata.opendocrouter_cached_pages` | Served from OpenDocRouter's result cache, free. |
| number of ok pages | `Usage.pages` | Pages processed (cached and blank pages are free but counted). |
| `charge_usd` | `Usage.provider_cost_usd` → `cost_usd` | The request's actual charge; the sum of page charges when the total is still `null`. |
| `status` | `metadata.opendocrouter_status` | `completed` / `partial` (see §5). |
| `usage` | `metadata.opendocrouter_usage` | `{input_tokens, output_tokens}`. |
| `mode`, `model_version`, `price_version`, `page_count`, `results_expire_at` | `metadata.opendocrouter_*` | When present. |

A page without a usable layout (layout off, or layout failed) becomes one geometry-free `text`
block holding the page markdown, the same as the vision-LLM providers.

**Block types:**

| OpenDocRouter `type` | `BlockType` |
|---|---|
| `title` | `title` |
| `section_header` | `section_header` |
| `text`, `code`, `form`, `key_value` | `text` |
| `list_item` | `list` |
| `table` | `table` |
| `picture`, `chart` | `figure` |
| `formula` | `formula` |
| `caption` | `caption` |
| `footnote` | `footnote` |
| `page_header` | `header` |
| `page_footer` | `footer` |
| anything else | `other` |

## 5. Errors, status codes, rate limits, timeouts

Request errors are `{"error": {"code", "message"}}` and are never charged. PuffinParse keeps both
(`"insufficient_credits: Not enough credit …"`), appends `required_usd` / `available_usd` for 402,
the `Retry-After` value for 429 / 503, and the `X-Request-Id` header when present.

| Status | Code | PuffinParse `ErrorKind` |
|---|---|---|
| 400 | `invalid_request`, `url_not_allowed` | `bad_request` |
| 401 | `unauthorized` | `authentication` |
| 402 | `insufficient_credits` | `bad_request` — not retried and, by default, no router fallback (the same as Datalab and Landing AI). Add `BadRequestError` to `fallback_on` if you want a router to move on to another provider when credit runs out. |
| 403 | `account_paused` | `authentication` |
| 404 / 409 / 410 | `not_found`, `results_not_stored`, `gone` | `bad_request` |
| 413 | `too_large` | `bad_request` — except that a sync request without an explicit `mode` is retried as async once (§3) |
| 415 / 422 | `unsupported_type`, `unreadable_document` | `bad_request` |
| 429 | `rate_limited` | `rate_limit` — retried, waiting `Retry-After` (capped at 120 s and the deadline) |
| 500 | `internal_error` | `provider` (retried) |
| 503 | `at_capacity`, `model_starting` | `provider` — retried, waiting `Retry-After` (capped as above); `model_starting` asks for minutes, so raise `timeout_secs` / `max_retries` for cold open models |

**Per-page failures.** The response is `200` even when pages fail:

* `completed` — every page worked.
* `partial` — PuffinParse returns the successful pages and lists the others in
  `metadata.opendocrouter_failed_pages` (and logs a warning). Failed pages are **not** silently
  dropped from the record, but they are absent from `pages`, so check that key — or compare
  `Usage.pages` with the pages you asked for — before trusting a document as complete.
* `failed` (no page worked) — an error carrying every page's code and message:
  `rate_limit` when every page failed with `rate_limited` / `at_capacity`, `bad_request` when every
  page was `unreadable_page`, otherwise `provider` (retryable, fallback-eligible).
* `rejected` / `expired` (async) — an error mapped from `error_code` (`expired` → `provider`).

Page error codes: `timeout`, `rate_limited`, `provider_error`, `at_capacity`, `output_truncated`,
`content_filtered`, `repetitive_output`, `invalid_output`, `empty_output`, `response_too_large`,
`unreadable_page`, `not_processed`. OpenDocRouter already retries each page once on the transient
ones; a page that still fails is worth re-sending with `pages="<n>"` or another model.

**Limits.** 50 pages per sync request, 500 per async request; inline ~3 MB (body 4 MB); URLs and
uploads 50 MB; 10 concurrent requests and 5 concurrent async requests per account; 300
`POST /v1/parse` and 60 `GET /v1/parse/{id}` per minute. Pages still running after 270 s come back as
`timeout`; async requests wait up to 30 minutes for capacity.

**Timeouts.** `timeout_secs` (default 300) covers upload, submission, polling and result download. If
it expires while an async request is running, the `TimeoutError` carries the request id as `job_id`;
the request keeps running and is charged, so collect it with `retrieve` (a `JobHandle` with that id)
rather than parsing again. For long documents prefer `submit` / `retrieve` from the start.

## 6. Data retention

* **Sync without `cache`** (PuffinParse's default for ≤ 50 pages): the document is never stored and
  the markdown exists only in the POST response.
* **Async** (> 50 pages, `mode: "async"`, or jobs) requires `cache: true`: results are stored
  encrypted for **24 hours** after the request finishes (`results_expire_at`), and a later request for
  the same pages of the same document, model and `layout` is served from them for free. The document
  itself is kept only until parsing ends. PuffinParse does not delete stored results; call
  `DELETE /v1/parse/{id}` yourself if you need them gone sooner.
* Uploads are deleted once a parse request reads them.
* Setting `provider_options={"cache": True}` on a sync request opts into the same storage (and free
  repeats) — keep it off for latency benchmarks.

## 7. Gotchas

* **Model ids contain `/`.** `opendocrouter/google/gemini-3-flash` is provider `opendocrouter`,
  model `google/gemini-3-flash`. Do not confuse it with PuffinParse's own `gemini/…`, `openai/…` and
  `anthropic/…` providers, which call those vendors directly with your own keys.
* **`lines` counts `"\n"` only.** Splitting with a line splitter that also breaks on `\r`, ` `
  and friends shifts every block; PuffinParse splits on `"\n"`.
* **Layout is opt-in on the wire and costs extra.** PuffinParse turns it on for boxes; turn it off
  for cheaper markdown-only runs.
* **Auto-async stores results.** A document over 50 pages is parsed with `cache: true` (24-hour
  encrypted storage, see §6). Pin `provider_options={"mode": "sync"}` and select ≤ 50 `pages` if
  that is not acceptable.
* **Uploads are single-use.** PuffinParse re-uploads when it has to resend the same document.
* **Every page is billed by tokens.** Two documents with the same page count can cost very different
  amounts; `cost_usd` is always the real charge, the `pricing.json` figure is an average.
* **Rejected 413s count against nothing** (never charged), so the automatic async retry costs one
  extra round trip only.

## 8. Useful `provider_options` passthrough

```python
# 1. Markdown only (no layout boxes, no layout surcharge).
puffinparse.parse("doc.pdf", model="opendocrouter/opendatalab/mineru2.5-pro",
                  provider_options={"layout": False})

# 2. A 300-page report: async with stored results, or explicitly.
puffinparse.parse("report.pdf", model="opendocrouter/google/gemini-3-flash",
                  provider_options={"mode": "async"}, timeout_secs=1800)

# 3. Re-run a few failed pages on a stronger model.
puffinparse.parse("doc.pdf", model="opendocrouter/anthropic/claude-opus-5-5", pages="2,7")

# 4. Force the upload flow for a small file (or inline for a large one with False).
puffinparse.parse("scan.png", model="opendocrouter/paddlepaddle/paddleocr-vl-1.6",
                  provider_options={"upload": True})

# 5. A model newer than the registry.
puffinparse.parse("doc.pdf", model="opendocrouter/google/gemini-3-flash",
                  provider_options={"model": "google/gemini-4-flash"})
```

## 9. Links

* Docs: <https://www.opendocrouter.ai/docs> (Markdown: <https://www.opendocrouter.ai/docs.md>)
* API reference: <https://www.opendocrouter.ai/v1/reference>
* OpenAPI spec: <https://www.opendocrouter.ai/v1/openapi.json>
* Models and prices: <https://www.opendocrouter.ai/models> (`GET /v1/models`)
* SDKs: <https://github.com/run-llama/opendocrouter-py>, <https://github.com/run-llama/opendocrouter-ts>
