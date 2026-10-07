# Upstage Document Parse

> **Status: docs-only.** Implemented from Upstage's published API documentation and tested
> against fixture payloads built from it. It has not yet been run against the live API, so
> expect wire-format differences. Help verify it: [issue #10](https://github.com/ajinkyashejul/puffinparse/issues/10).

## 1. Summary

| | |
|---|---|
| Provider name | `upstage` |
| Base URL | `https://api.upstage.ai` (override: `base_url` on the request, or `UPSTAGE_BASE_URL`) |
| API key | `UPSTAGE_API_KEY` (or `api_key` on the request) — sent as `Authorization: Bearer <key>` |
| Docs | <https://console.upstage.ai/docs/capabilities/document-digitization/document-parsing> · agent-oriented dump: <https://console.upstage.ai/api/docs/for-agents/raw> |
| Modes | `parse` (native), `ocr` (derived from `parse`) |
| Checked against | 2026-09-11, **from documentation only** — no key was available, so the `#[ignore]`d live test has not been run |
| Implementation | `crates/puffinparse-core/src/providers/upstage.rs` |

Document Parse turns a document into layout **elements** (`paragraph`, `heading1`, `table`, `figure`,
`chart`, …) with HTML, Markdown and plain text per element, plus four relative corner coordinates.
PuffinParse uses the synchronous endpoint by default and the async batch endpoint on request.

## 2. Models exposed by PuffinParse

| Model | Provider parameters PuffinParse sets | List price (`pricing.json`) |
|---|---|---|
| `upstage/document-parse` *(default)* | `model=document-parse` | $0.01 / page |
| `upstage/document-parse-nightly` | `model=document-parse-nightly` | $0.01 / page (billed as standard) |

Prices are the public per-page rates from <https://www.upstage.ai/pricing>: Document Parse
**standard $0.01/page**, **enhanced $0.03/page**. PuffinParse does not set `mode`, so the account default
(`standard`) applies; `provider_options={"mode": "enhanced"}` or `"auto"` changes the engine **and the
price**, which `cost_usd` will then under-report. The response's `usage.enhanced` list is surfaced as
`metadata.upstage_enhanced_pages` so an enhanced-mode run is visible after the fact.

The separate Document **OCR** product (`model=ocr`, $0.0015/page, word boxes) is *not* wired up:
PuffinParse's `ocr` mode for this provider is derived from the parse blocks. See §6.

## 3. Request flow PuffinParse uses

Every request carries `Authorization: Bearer $UPSTAGE_API_KEY` and `accept: application/json`.

1. **Load the document.** Path and bytes inputs are uploaded as-is. Document Parse has **no URL
   input**, so a URL input is downloaded by PuffinParse first and then uploaded.
2. **Submit.** `POST {base}/v1/document-digitization`, `multipart/form-data`:

   | field | value PuffinParse sends |
   |---|---|
   | `document` | the file part (filename + guessed MIME) |
   | `model` | `document-parse` or `document-parse-nightly` |
   | `output_formats` | `["html","markdown","text"]` — all three, so blocks get Markdown *and* a provider plain text |
   | `ocr` | `auto` (digital-born PDFs keep their embedded text; images are OCR'd) |
   | `coordinates` | `true` |

   Anything in `provider_options` is appended as extra form fields and **overrides** a default with
   the same name. Non-string values are serialised: booleans/numbers as text, arrays/objects as JSON
   (`base64_encoding=["table"]`). The key `async` is consumed locally and never sent.
3. **Async (opt-in).** With `provider_options={"async": true}` the same multipart body goes to
   `POST {base}/v1/document-digitization/async` → `{"request_id": …}`. PuffinParse then polls
   `GET {base}/v1/document-digitization/requests/{request_id}` (2 s, backing off ×1.5 to 15 s) until
   `status` is `completed` or `failed`, and finally downloads every `batches[].download_url`
   (plain `GET`, **no auth header**, 15-minute expiry) in `batches[].id` order. Each batch payload has
   exactly the shape of the sync response, so they are concatenated into one result.
4. **Normalise.** See §4.

`pages` is applied **client-side**: Document Parse has no page-range parameter, so PuffinParse parses the
whole document and then drops the pages outside the selection, recording
`metadata.upstage_pages_filtered_client_side`. `usage.pages` still reports the whole billed document.
`language` is ignored (Document Parse auto-detects; there is no language parameter).

## 4. Response mapping

```json
{
  "apiVersion": "1.1",
  "model": "document-parse-260630",
  "elements": [
    { "id": 0, "category": "heading1", "page": 1,
      "content": { "html": "<h1 id='0'>Hello PuffinParse</h1>", "markdown": "# Hello PuffinParse", "text": "Hello PuffinParse" },
      "coordinates": [ {"x":0.125,"y":0.0525}, {"x":0.425,"y":0.0525}, {"x":0.425,"y":0.1052}, {"x":0.125,"y":0.1052} ] }
  ],
  "content": { "html": "…", "markdown": "…", "text": "…" },
  "usage": { "pages": 2, "standard": [1, 2] }
}
```

(The full fixture is `crates/puffinparse-core/tests/fixtures/upstage_document_parse.json`.)

| Upstage field | PuffinParse unified field | Notes |
|---|---|---|
| `elements[]` | `Page.blocks[]` | Grouped into pages by `element.page`, reading order preserved. |
| `elements[].category` | `Block.type` | Mapping below. |
| `elements[].content.markdown` | `Block.content` | `output="text"` uses `content.text` instead (falling back to `markdown_to_text`). Empty Markdown falls back to `text`, then `html`. |
| `elements[].content.text` | `Block.text` | |
| `elements[].coordinates[]` | `Block.bbox` | Four corner points, **already relative 0–1, top-left origin**; PuffinParse takes min/max x/y and clamps. |
| `elements[].page` | `Block.page_number` | 1-based on the wire and in PuffinParse. |
| joined element Markdown | `Page.markdown` / `Page.text` | Per page, blank-line separated (`pages_from_blocks`). |
| `usage.pages` | `Usage.pages` | Falls back to the number of reconstructed pages. |
| `model` | `metadata.upstage_model_version` | Resolved snapshot, e.g. `document-parse-260630`. |
| `usage.enhanced` | `metadata.upstage_enhanced_pages` | Only when non-empty (`mode=auto`/`enhanced`). |
| async `request_id` | `provider_job_id` | `None` for sync calls — the sync response carries no id. |
| `content.{html,markdown,text}` | — | Only used as a fallback when `elements` is empty; otherwise page content is rebuilt from elements. |
| `elements[].sub_category`, `base64_encoding`, `apiVersion` | — | Visible with `include_raw=True`. |
| — | `Page.width` / `Page.height` | **Never set**: Document Parse reports no page dimensions (coordinates are already relative). |
| — | `Block.confidence` | **Never set**: Document Parse reports no per-element confidence. |

Block types: `paragraph` → `text`; `heading1` → `title`; `table` → `table`; `figure`, `chart` →
`figure`; `caption` → `caption`; `list` → `list`; `header` → `header`; `footer` → `footer`;
`footnote` → `footnote`; `equation` → `formula`; `code` → `text` (it is still text; the fenced block
is kept in `content`); `index` and anything new → `other`.

## 5. Errors, status codes, rate limits, timeouts

Error body: `{"error": {"message": …, "type": …, "code": …}}` — `Error::from_http` picks up
`error.message`.

| Status | Cause | PuffinParse `ErrorKind` |
|---|---|---|
| 400 | malformed request, unknown model, no document | `bad_request` |
| 401 | invalid API key | `authentication` |
| 403 | **insufficient credit** (also an expired async `download_url`) | `authentication` |
| 404 | wrong path | `bad_request` |
| 405 | `http://` instead of `https://` | `bad_request` |
| 413 | file over 50 MB (async) | `bad_request` |
| 415 | unsupported file format | `bad_request` |
| 422 | corrupted / damaged document | `bad_request` |
| 429 | rate limit | `rate_limit` (retried) |
| 500/502/503/504 | server error | `provider` (retried) |

Async failures come back as HTTP 200 with `status: "failed"` (plus `failure_message`), or with a
per-batch `"status": "failed"`; PuffinParse raises `provider` errors carrying the `request_id` as `job_id`.

**Limits.** 50 MB per file, 200 megapixels per page, PDF/JPEG/PNG/BMP/TIFF/HEIC/DOCX/PPTX/XLSX/HWP/HWPX.
Sync: **100 pages** (pages beyond 100 are silently dropped). Async: **1 000 pages**, processed in
10-page batches; results are stored 30 days, each `download_url` expires after ~15 minutes.

**Rate limits (tier 0).** Document Parse sync 1 RPS / 300 pages-per-minute; async 2 RPS / 1 200 PPM.
Limits rise with the commitment tier. No `X-RateLimit-*` headers, so backoff is blind. There is no
batch endpoint for multiple documents — send them one at a time.

**Timeouts.** `timeout_secs` (default 300) covers download + upload + polling + batch fetches and caps
each individual request. The async queue can hold a job for **up to 72 hours** at peak, so async runs
need a `timeout` far beyond the default (or a webhook-style poll of your own).

## 6. Gotchas (documentation-derived; not yet live-verified)

* **No page selection.** There is no `pages`/`page_range` parameter, so PuffinParse filters pages after
  the fact and you are billed for the whole document.
* **No page dimensions and no confidences.** Coordinates are relative, which is what PuffinParse wants,
  but `Page.width`/`height` and `Block.confidence` stay `None` for this provider.
* **Async page numbering is assumed global.** Batches cover 10-page ranges (`start_page`/`end_page`).
  PuffinParse trusts `element.page` as a document-level page number, but if a batch numbers its own pages
  from 1 while starting later in the document, the offset (`start_page - 1`) is added. Worth
  re-checking against a real 20+ page async run.
* **`download_url` expires in ~15 minutes** and returns 403 afterwards; PuffinParse fetches it right after
  polling, so this only bites very slow clients. Re-fetching the request status mints a fresh URL and
  costs nothing.
* **`ocr=auto` vs `force`.** Digital-born PDFs keep their embedded text layer with `auto`; a scanned
  PDF that still contains a bad text layer needs `provider_options={"ocr": "force"}`.
* **`mode` changes the price** (standard $0.01 → enhanced $0.03) without changing the response shape.
* **Charts degrade to figures.** With `chart_recognition` on (the default), a recognised chart comes
  back as `category: "chart"` with a Markdown table; when recognition fails it silently becomes a
  `figure` with OCR text only. Both map to `figure` in PuffinParse.
* **Equations are LaTeX in `markdown`/`html` but raw (often wrong) OCR in `text`** — prefer
  `output="markdown"` for documents with formulas.
* **`content.markdown` is not the same as joining the elements**: PuffinParse rebuilds page Markdown from
  elements so that pages, blocks and boxes stay consistent. Use `include_raw=True` if you want the
  provider's own whole-document HTML.
* **The separate Document OCR product is cheaper** ($0.0015 vs $0.01 per page) and returns word-level
  `boundingBox.vertices` in pixels. Adding it as a native `ocr` model (e.g. `upstage/ocr`) is an
  obvious follow-up; today `mode="ocr"` on `upstage/document-parse` derives lines from parse blocks.
* **File names matter**: ≤ 900 chars (≤ 300 for Korean), no path components, extension must match the
  real format, or documents can hang in `started`.

## 7. Useful `provider_options` passthrough

```python
# 1. Scanned PDFs: force OCR instead of trusting an embedded text layer.
puffinparse.parse("scan.pdf", model="upstage/document-parse",
              provider_options={"ocr": "force"})

# 2. Complex tables / charts / low-quality scans (enhanced mode is $0.03/page).
puffinparse.parse("report.pdf", model="upstage/document-parse",
              provider_options={"mode": "enhanced"})

# 3. Long documents (up to 1 000 pages) through the async batch API.
puffinparse.parse("book.pdf", model="upstage/document-parse", timeout=3600,
              provider_options={"async": True})

# 4. Tables merged across page breaks, plus cropped table images in the raw payload.
puffinparse.parse("financials.pdf", model="upstage/document-parse", include_raw=True,
              provider_options={"merge_multipage_tables": True, "base64_encoding": ["table"]})

# 5. Word-level OCR boxes alongside the layout elements (raw only).
puffinparse.parse("form.png", model="upstage/document-parse", include_raw=True,
              provider_options={"words": True})
```

## 8. Links

* Document Parse: <https://console.upstage.ai/docs/capabilities/document-digitization/document-parsing>
* Full API reference for agents (single markdown file):
  <https://console.upstage.ai/api/docs/for-agents/raw>
* Pricing: <https://www.upstage.ai/pricing> · rate limits:
  <https://console.upstage.ai/docs/guides/rate-limits>
* Document OCR (word boxes, $0.0015/page): `model=ocr` on the same endpoint.
* Information Extraction (`POST /v1/information-extraction`) is a separate product and is **not** used
  by PuffinParse's `extract` mode for this provider.
