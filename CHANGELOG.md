# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
The workspace crates (`liteocr-core`, `liteocr-cli`, `liteocr-python`) and the
Python package `liteocr` share a single version.

## [Unreleased]

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
