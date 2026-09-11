# Provider reference

One page per provider, describing exactly what LiteOCR sends, what comes back, and how the two are
mapped onto the unified `OcrResponse`. Each page is verified against live API responses (2026-09-11)
and against the implementation in `crates/liteocr-core/src/providers/`.

| Provider | Doc | Implementation | Env var | Models |
|---|---|---|---|---|
| Reducto | [`reducto.md`](reducto.md) | `providers/reducto.rs` | `REDUCTO_API_KEY` | `standard` *(default)*, `r-1`, `agentic` |
| Extend | [`extend.md`](extend.md) | `providers/extend.rs` | `EXTEND_API_KEY` | `parse_performance` *(default)*, `parse_light`, `parse_auto` |
| LlamaParse | [`llamaparse.md`](llamaparse.md) | `providers/llamaparse.rs` | `LLAMA_API_KEY` | `fast`, `cost_effective` *(default)*, `agentic`, `agentic_plus` |

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
