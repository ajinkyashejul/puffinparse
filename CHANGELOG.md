# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
The workspace crates (`liteocr-core`, `liteocr-cli`, `liteocr-python`) and the
Python package `liteocr` share a single version.

## [Unreleased]

### Added

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
