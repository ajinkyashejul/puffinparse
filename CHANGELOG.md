# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
The workspace crates (`liteocr-core`, `liteocr-cli`, `liteocr-python`) and the
Python package `liteocr` share a single version.

## [Unreleased]

### Added

- **Self-hosted engines, no key, $0/page.** `tesseract/default` (local `tesseract` binary, PDFs via
  `pdftoppm`; native OCR with word/line boxes and confidences), `docling/default` (docling-serve v1
  async API; layout, tables, OCR) and `paddleocr/default` (PaddleOCR/PaddleX serving: `/ocr` and
  PP-StructureV3 `/layout-parsing`; docs-only). Tesseract and Docling are live-verified locally.
  `liteocr providers` shows `local` for them; `--json` and Python `providers()` include
  `self_hosted`.
- README, docs site and landing page cover the TypeScript SDK, the gateway, jobs/webhooks,
  self-hosted engines and the new benchmark sources; the docs navigation gains the gateway,
  the self-hosted provider pages, the academic-benchmark survey and the adapter notes.
- **Async jobs API and webhooks.** Split a parse into submit and retrieve so the caller owns the
  waiting (long documents, batches, webhook-driven pipelines). Rust: `submit_parse`,
  `retrieve_parse` / `retrieve_parse_with`, `parse_webhook`, `resolve_webhook`, `JobHandle`
  (serialisable, never holds a key), `JobStatus`, and `DocumentRequest.webhook_url`. Python:
  `liteocr.submit` / `asubmit`, `retrieve` / `aretrieve`, `handle_webhook` / `ahandle_webhook` and
  `liteocr.Job`. Covers Reducto, Extend and LlamaParse (live-verified); `webhook_url` maps to
  Reducto `async.webhook` and the LlamaParse `webhook_url` field, and is rejected for Extend, which
  only has workspace-level webhooks. SPEC §15.
- **Benchmark viewer: verify every claim.** A per-document inspector at `#/<run>/<model>/<doc>` shows
  the page itself (pdf.js for PDFs, pinned with SRI, PNG fallback), every model's output side by
  side, a word diff against the truth, and a pass/fail rule checklist for rules documents that is
  checked against the recorded score. Layout-box overlays by block type appear when a run saved
  unified responses. The leaderboard ranks by `summary.headline` when present and adds p90 latency,
  a score-vs-cost/latency scatter with the Pareto frontier, and per-source and source × category
  breakdowns. Every view is a shareable link, with keyboard navigation (`j`/`k`, `m`, `1`–`4`,
  `d`, `o`, `?`), the product site's design and theme switch, and a mobile layout.
- `liteocr bench run --save-outputs` also writes the unified response as `<doc>.json` next to
  `<doc>.md`, which the viewer uses for its layout overlay.
- **More public benchmarks in the combined dataset.** `olmocr` (40 AI2 olmOCR-bench PDFs, 205
  rules, ODC-BY-1.0) and `omnidocbench` (40 pages across 10 document types, English and Chinese;
  index only — images and truth are fetched at a pinned revision by
  `python -m benchmark.adapters omnidocbench` because the data is research-only), joined with
  synthetic-v1 and ParseBench as `combined-v2` (159 documents; `combined-v1` is unchanged).
  Upstream tests that cannot be expressed faithfully (math, baseline, positional absences,
  vertical table neighbours) are skipped and counted in `conversion-stats.json`, never dropped
  silently. A dataset self-check test proves every converted rule is satisfiable. Survey of
  academic benchmarks with licences at pinned revisions: `docs/benchmarks/academic-benchmarks.md`.
- **Gateway server: `liteocr serve` (`crates/liteocr-server`).** An HTTP gateway in front of every
  provider, the LiteOCR equivalent of the LiteLLM proxy: `POST /v1/parse|ocr|extract` (JSON or
  multipart, `output_format`, `fallbacks`), `GET /v1/models`, `/v1/usage`, `/health` and
  Prometheus `/metrics`. A `liteocr.toml` config defines model aliases with ordered or round-robin
  fallback, provider keys as `env:` references, and virtual keys with model allow-lists, monthly
  USD budgets and per-minute rate limits. Every request is logged as one JSON line (never document
  content, provider error text or secrets), and all errors share one JSON body. Clients cannot
  override `api_key`/`base_url` or read local files. Ships a multi-stage distroless `Dockerfile`
  and `examples/server/liteocr.toml`; see [`docs/SERVER.md`](docs/SERVER.md).
- **Node.js / TypeScript SDK.** The `liteocr` npm package in `js/` runs on a napi-rs addon over the
  same Rust core (`crates/liteocr-node`): async `parse` / `ocr` / `extract` (with `fallbacks`),
  `Router`, camelCase typed responses (`index.d.ts`), `LiteOCRError` subclasses mapped from the core
  `ErrorKind`, and the pricing, model and scoring helpers. CI builds the addon and runs the
  typecheck and `node:test` suite; the release workflow builds prebuilt `.node` binaries as
  artifacts (npm publishing not wired yet, so build from source for now). Docs at `/docs/typescript/`.
- **`liteocr bench run --resume`**: every finished call is appended to `<out>.partial.jsonl`, so an
  interrupted or partly failed run continues where it stopped and only re-runs missing or failed
  (model, document) pairs. `--dry-run` prints the plan (calls, pages, list-price estimate) without
  calling any provider; `--max-cost <usd>` aborts before the first call when the estimate is higher;
  `--retries N` re-issues documents after retryable errors (off by default, since a retried job may
  be billed twice). Result documents record `provider_job_id`, `cache_hit`, `attempts`,
  `started_at` and `error_kind`, and the run ends with a summary line (calls, failures, cost, wall time).
- **Native-format compatibility (`output_format`).** A response can be rendered in a
  provider's own JSON shape instead of the unified one, so an integration already written
  against Reducto, Extend or LlamaParse can switch the underlying provider without
  rewriting its parsing code: `ParseResponse::to_format("reducto")` in Rust
  (`liteocr_core::compat::{Format, render_parse, render_extract}`), with
  `DocumentRequest.output_format` carrying and validating the choice. What is guaranteed
  is structural fidelity — key set, chunk/page and block counts, content strings, block-type
  vocabulary, coordinate units and billed pages — not byte equality; the always-null fields
  and the lossy type mappings are enumerated in [`docs/COMPAT.md`](docs/COMPAT.md), and each
  provider's fixture is round-tripped through its own renderer in the test suite. Extract-mode
  rendering is best effort. See ADR-13.
- **`output_format` in the Python SDK and the CLI.** `liteocr.parse(..., output_format="reducto")`
  (and `aparse`, `extract`, `aextract`, plus the matching `Router` methods) returns the vendor's
  own JSON as a `dict` instead of the dataclass; `output_format=None` (the default) or
  `"liteocr"` keeps the unified shape. The value is validated in the core before any network call
  — an unknown name raises `BadRequestError` listing `liteocr | reducto | extend | llamaparse` —
  the rendering happens in Rust (`liteocr._core.render_parse` / `render_extract`), and success
  callbacks still receive the dataclass. Overloads type the return (`None` → dataclass, `str` →
  `dict`), `liteocr.output_formats()` lists the accepted values, and
  `examples/switch_provider_keep_format.py` shows a provider swap with the parsing code untouched.
  The CLI gains `--output-format <vendor>` on `parse` (with `--format json`; ignored with a
  warning on stderr otherwise) and on `extract`, and `liteocr providers --json` now emits
  `{"providers": [...], "output_formats": [...]}` instead of a bare array.

### Changed

- Internal: the vision-LLM providers (Gemini, OpenAI, Anthropic) share `providers/vlm.rs`; behaviour
  and request bodies are unchanged.
- Adapter HTML→markdown table conversion no longer doubles backslashes, so LaTeX in table cells reaches
  the scorer as a parser would print it.
- **Modes.** Every call now names a mode — `parse` (markdown + typed blocks), `ocr`
  (plain text with line/word boxes) or `extract` (a JSON object from a schema, with
  per-field confidence and citations) — and providers can only be swapped within a
  mode. The Python SDK exposes `parse`/`aparse`, `ocr`/`aocr` (now plain text, not
  markdown) and `extract`/`aextract`, the new `TextResponse` / `ExtractResponse`
  dataclasses, a mode-bound `Router(models, mode=...)`, and mode arguments on
  `list_models`, `resolve_model`, `set_pricing` and `estimate_cost`; pricing is now
  per page *per mode*. The CLI gains `liteocr ocr` and `liteocr extract`, and
  `liteocr providers` shows modes, per-mode prices and a `--mode` filter. The old
  `liteocr.ocr` (which returned markdown) is now `liteocr.parse`, and `OcrResponse`
  is now `ParseResponse`. Which models serve which mode is reported by
  `liteocr.list_models(mode)` and `liteocr providers --mode <mode>`.
- **README model table.** The "Model names" section is regenerated from the registry and
  `pricing.json`: every provider group carries its env var and verification status
  (live-verified vs docs-only), every model its modes, the default per mode and the list price
  per page for `parse · ocr · extract`.

### Fixed

- Extend `provider_options={"responseType": "url"}` is now sent as the `GET /parse_runs/{id}` query
  parameter; it used to go in the request body, so the presigned-output path never triggered.
  Reducto and Extend url-typed results are now covered by live-captured fixtures and loopback tests.

## [0.1.0] - 2026-09-11

Initial release.

### Added

- **Rust core (`liteocr-core`)** — a unified request/response contract for
  document parsing: `OcrRequest` accepting a path, bytes or URL, and an
  `OcrResponse` with `pages`, `blocks` (normalised block types and 0..1
  bounding boxes), document-level `markdown`/`text`, `usage`, `cost_usd` and
  `latency_ms`, identical across every provider. `#![forbid(unsafe_code)]`.
- **Providers** — Reducto, Extend and LlamaParse, each talked to over plain
  HTTPS with no vendored SDK, addressed by `"<provider>/<model>"` model
  strings (`reducto/standard`, `extend/parse_performance`,
  `llamaparse/cost_effective`, …) with a per-provider default and a
  `provider_options` pass-through escape hatch.
- **Router** — ordered fallbacks and round-robin load balancing across models,
  with per-model success/failure and latency stats; only provider, rate-limit
  and timeout errors trigger fallback.
- **Pricing and cost tracking** — an embedded, overridable `pricing.json`
  price table mapping model → per-page (or per-credit) USD, used to compute
  `cost_usd` on every response.
- **Reliability primitives** — retries with exponential backoff on 429/5xx and
  network errors, per-call timeouts, and whole-call deadlines that cover
  upload plus polling.
- **Benchmark metrics** — character similarity (the primary score), CER, WER,
  word-level F1/recall, reading-order agreement and a table-restricted score,
  all computed in Rust over NFKC-normalised text.
- **CLI (`liteocr`)** — `parse` (markdown/text/json output, optional raw
  provider payload), `providers` (models, pricing and API-key status), and
  `bench run` / `bench report` / `bench score` for running the benchmark,
  scoring results and generating the leaderboard.
- **Python SDK (`liteocr`)** — `ocr()`, the async `aocr()`, `Router`, typed
  dataclasses (`OcrResponse`, `Page`, `Block`, `BBox`, `Usage`), the full error
  hierarchy, `set_pricing()`, `list_models()`, and success/failure callbacks.
  Fully typed, ships `py.typed`, distributed as abi3 wheels for CPython 3.9+.
- **`synthetic-v1` benchmark dataset** (v1.1.0) — 39 deterministically generated
  documents across 13 categories (plain, headings, invoice, table, two-column,
  noisy scan, low resolution, multi-page PDF, skewed, dense, faded, receipt,
  complex table) with exact markdown ground truth, plus the generator that
  produces them byte-for-byte.
- **First leaderboard** (`benchmark/LEADERBOARD.md`) from a run over seven models
  across the three providers; `bench run` disables provider result caches by
  default (`--allow-cache` to opt out) so latency reflects real work.

[Unreleased]: https://github.com/ajinkyashejul/liteocr/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ajinkyashejul/liteocr/releases/tag/v0.1.0
