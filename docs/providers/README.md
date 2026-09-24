# Provider reference

One page per provider, describing exactly what LiteOCR sends, what comes back, and how the two are
mapped onto the unified `OcrResponse`. Each page is verified against live API responses (2026-09-11)
and against the implementation in `crates/liteocr-core/src/providers/`.

| Provider | Doc | Implementation | Env var | Models | Status |
|---|---|---|---|---|---|
| Reducto | [`reducto.md`](reducto.md) | `providers/reducto.rs` | `REDUCTO_API_KEY` | `standard` *(default)*, `r-1`, `agentic` | live-verified |
| Extend | [`extend.md`](extend.md) | `providers/extend.rs` | `EXTEND_API_KEY` | `parse_performance` *(default)*, `parse_light`, `parse_auto` | live-verified |
| LlamaParse | [`llamaparse.md`](llamaparse.md) | `providers/llamaparse.rs` | `LLAMA_API_KEY` | `fast`, `cost_effective` *(default)*, `agentic`, `agentic_plus` | live-verified |
| Mistral | [`mistral.md`](mistral.md) | `providers/mistral.rs` | `MISTRAL_API_KEY` | `ocr-latest` *(default)*, `ocr-4-1`, `ocr-4-0`, `ocr-2512` | docs-only |
| Azure AI Document Intelligence | [`azure.md`](azure.md) | `providers/azure.rs` | `AZURE_DOCUMENT_INTELLIGENCE_KEY` + `AZURE_DOCUMENT_INTELLIGENCE_ENDPOINT` | `read` *(ocr default)*, `layout` *(parse default)*, `invoice` *(extract default)*, `receipt`, `id_document`, `tax_us_w2`, `custom` | docs-only |
| AWS Textract | [`textract.md`](textract.md) | `providers/textract.rs` | `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` (+ `AWS_SESSION_TOKEN`, `AWS_REGION`) | `detect-text` *(ocr default)*, `layout`, `queries` *(extract default)*, `forms` | docs-only (transport verified) |
| Google Gemini | [`gemini.md`](gemini.md) | `providers/gemini.rs` | `GEMINI_API_KEY` | `2.5-flash` *(default)*, `2.5-pro`, `2.5-flash-lite`, `3.5-flash`, `3.5-flash-lite`, `3.8-flash` | docs-only (wire tests) |
| OpenAI | [`openai.md`](openai.md) | `providers/openai.rs` | `OPENAI_API_KEY` | `gpt-5.6-luna` *(default)*, `gpt-5.6-terra`, `gpt-5.6-sol`, `gpt-6-astra` | docs-only |
| Anthropic | [`anthropic.md`](anthropic.md) | `providers/anthropic.rs` | `ANTHROPIC_API_KEY` | `claude-sonnet-5` *(default)*, `claude-haiku-4-5`, `claude-opus-5` | docs-only |
| Mathpix | [`mathpix.md`](mathpix.md) | `providers/mathpix.rs` | `MATHPIX_APP_ID` + `MATHPIX_APP_KEY` | `pdf` *(default)*, `text` | docs-only |
| Datalab (Marker) | [`datalab.md`](datalab.md) | `providers/datalab.rs` | `DATALAB_API_KEY` | `fast`, `balanced` *(default)*, `accurate` | docs-only |
| Unstructured | [`unstructured.md`](unstructured.md) | `providers/unstructured.rs` | `UNSTRUCTURED_API_KEY` | `hi_res` *(default)*, `fast`, `auto` | docs-only |
| Upstage | [`upstage.md`](upstage.md) | `providers/upstage.rs` | `UPSTAGE_API_KEY` | `document-parse` *(default)*, `document-parse-nightly` | docs-only |
| Landing AI (ADE) | [`landingai.md`](landingai.md) | `providers/landingai.rs` | `LANDINGAI_API_KEY` | `dpt-2` *(default)* | docs-only |
| Google Document AI | [`google-documentai.md`](google-documentai.md) | `providers/google_documentai.rs` | `GOOGLE_DOCUMENTAI_ACCESS_TOKEN` (+ `_PROJECT`, `_LOCATION`, `_PROCESSOR_ID`) | `ocr` *(default)*, `layout`, `form`, `prebuilt` | docs-only |
| Tesseract *(local)* | [`tesseract.md`](tesseract.md) | `providers/tesseract.rs` | none (`TESSERACT_CMD`, `PDFTOPPM_CMD`) | `default` | live-verified (local binary) |
| Docling *(self-hosted)* | [`docling.md`](docling.md) | `providers/docling.rs` | none (`DOCLING_BASE_URL`; optional `DOCLING_API_KEY`) | `default` | live-verified (local docling-serve 1.35) |
| PaddleOCR *(self-hosted)* | [`paddleocr.md`](paddleocr.md) | `providers/paddleocr.rs` | none (`PADDLEOCR_BASE_URL`, `PADDLEOCR_PARSE_BASE_URL`) | `default` | docs-only |

The three self-hosted engines need no API key and are priced at $0/page (`liteocr providers` shows
`local` in the Key column); they are the open baselines in the benchmark. Their shared helpers are
in `providers/local.rs`.

Every page follows the same structure: summary → models → request flow → response mapping → errors and
limits → gotchas → `provider_options` examples → links.

## At a glance

| | Reducto | Extend | LlamaParse |
|---|---|---|---|
| Upload method | `POST /upload` (multipart) → `reducto://<uuid>` id; URLs passed through as `input` | `POST /files/upload` (multipart) → `file_…` id; URLs passed through as `{"url", "name"}` | one multipart call: `file` part, or `input_url` field |
| Sync / async | Sync `POST /parse` by default; async `POST /parse_async` + `GET /job/{id}` with `provider_options={"async": true}` | Always async: `POST /parse_runs` + `GET /parse_runs/{id}` | Always async: `POST /api/v1/parsing/upload` + `GET …/job/{id}` + `GET …/job/{id}/result/json` |
| Page info source | `blocks[].bbox.page` (chunks carry no page number); LiteOCR pins `chunk_mode: "page"` | `chunk.metadata.pageRange` + `block.metadata.page.number`; LiteOCR pins `chunkingStrategy: {"type":"page"}` | `pages[].page` (native per-page objects with `md` and `text`) |
| Page dimensions | not reported — `Page.width`/`height` are `None` | `block.metadata.page.width/height` (raster pixels at the run's dpi) | `pages[].width/height` |
| Bbox units on the wire | already normalised 0–1, top-left origin | absolute `left/top/right/bottom` in page units | absolute `bBox {x,y,w,h}` in page units |
| Bbox after normalisation | clamped as-is | divided by page width/height | divided by page width/height |
| Confidence type | `granular_confidence.parse_confidence` (0–1), else coarse `"high"`/`"low"` → 0.9 / 0.5 | `metadata.avgOcrConfidence` (0–1) per block | `items[].bBox.confidence` (0–1) per item |
| Credits reported? | yes — `usage.credits` (`null` on new per-product pricing) | yes — `usage.credits` (`null` for runs before 2025-10-07) | effectively no — `job_credits_usage` is 0 until billing settles, so LiteOCR reports `None` |
| Large-result indirection | `result.type == "url"` → presigned JSON fetched without auth | `outputUrl` (15-min presigned) when `responseType=url` | none — result endpoints return inline JSON |
| Job id surfaced as | `job_id` | run `id` (`pr_…`) | job `id` (UUID) |

`Usage.pages` comes from `usage.num_pages` (Reducto), `metrics.pageCount` (Extend) and
`job_metadata.job_pages` (LlamaParse), falling back to the number of reconstructed pages.
`cost_usd` is always `pages × per_page_usd` from `crates/liteocr-core/src/pricing.json` — no provider
reports dollar cost directly.

## Shared behaviour

These are implemented once, in `crates/liteocr-core/src/{http,error,provider}.rs`, and apply to all
three providers:

* **API key**: request `api_key` first, then the provider's env var; missing ⇒ `authentication` error.
* **Base URL**: request `base_url`, then `<PROVIDER>_BASE_URL`, then the built-in default.
* **Deadline**: `timeout_secs` (default 300) covers upload, submission, polling and result download, and
  also caps each individual HTTP request.
* **Retries**: `max_retries` (default 2) with exponential backoff and full jitter, only on rate-limit,
  network, and 500/502/503/504 errors. 4xx is never retried.
* **Error kinds**: 401/403 → `authentication`, 429 → `rate_limit`, other 4xx → `bad_request`,
  5xx / malformed payload / failed job → `provider`, deadline → `timeout`.
* **None of the three returns rate-limit headers** (`X-RateLimit-*`), so backoff is always blind.

## How to add a provider

See [`CONTRIBUTING.md`](../../CONTRIBUTING.md), section **"3. Adding a provider"** — one file under
`crates/liteocr-core/src/providers/`, registered in `providers/mod.rs` and `model::PROVIDERS`, plus
`pricing.json` entries, a fixture-backed normalisation test, and a doc page here following the same
eight sections as the pages above.

**Status:** *live-verified* means the provider's `#[ignore]` live tests have passed with a real key. *docs-only* means it was implemented from the official API reference with fixtures shaped from documented responses; run `cargo test -p liteocr-core <provider> -- --ignored` with a key to promote it.
