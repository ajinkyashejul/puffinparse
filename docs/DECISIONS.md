# Decision log

Architecture decision records, newest last. One entry per decision that someone might
otherwise re-litigate. Format: context → decision → consequences. Add an entry when you
change direction; do not edit old entries, supersede them.

## ADR-1: Rust core with a thin Python wrapper

**Context.** The brief asks for "fastest and litest" and a Python SDK. Provider calls are
HTTP + JSON; the benchmark needs Levenshtein over long documents.

**Decision.** All provider logic, normalisation, routing, pricing and metrics live in the
`liteocr-core` crate. Python (`python/liteocr`) is a typed wrapper over a PyO3 module
(`liteocr._core`) and holds no provider logic. The CLI is a separate crate on the same core.

**Consequences.** One implementation to test per provider; a future gateway server and a
TypeScript SDK reuse the core. Cost: Python contributors need a Rust toolchain
(`maturin develop`), and wheels must be built per platform (handled by `release.yml`).

## ADR-2: `"<provider>/<model>"` model strings, LiteLLM style

**Decision.** The `model` string selects provider and quality tier (`reducto/r-1`,
`llamaparse/agentic`). A bare provider name selects its default. Provider-specific knobs go in
`provider_options` and are merged verbatim into the provider request.

**Consequences.** Switching provider is a one-string change; the response shape never depends
on options. The registry in `crates/liteocr-core/src/model.rs` is the only place models are
declared; pricing is keyed by the same string.

## ADR-3: Normalised, top-left-origin bounding boxes

**Decision.** Every block bbox is `{x0, y0, x1, y1}` in `0..=1` of the page. Reducto already
reports normalised boxes; Extend and LlamaParse boxes are divided by the page dimensions they
report. Boxes are `None` when dimensions are unknown.

**Consequences.** Consumers can draw overlays without knowing page size or DPI. Precision of
Extend's DPI-scaled coordinates is preserved because the division uses the same units.

## ADR-4: Extend always via `/parse_runs`; Reducto sync by default

**Decision.** Extend's sync `/parse` has a 5-minute hard limit and its docs call it
"for onboarding"; LiteOCR always creates a run and polls. Reducto's sync `/parse` allows 15
minutes and returns inline, so it is the default; `provider_options.async = true` switches to
`/parse_async` + `/job/{id}`.

**Consequences.** One code path per provider is exercised by default; both are covered by the
same unified deadline (`timeout`), so polling never exceeds the caller's budget.

## ADR-5: Page reconstruction rules

**Decision.** Pages are the unit of the unified response. Reducto is asked for `chunk_mode=page`
and pages are derived from `blocks[].bbox.page`; Extend uses page chunks and
`metadata.page.number`; LlamaParse's JSON result is already per page. Chunk-level markdown from
the provider is preferred over re-joining blocks, so the page markdown is what the provider
itself renders.

**Consequences.** Benchmarks compare provider-rendered markdown, not a LiteOCR re-rendering.

## ADR-6: Cost is computed from a list-price table, not from provider credits

**Decision.** `cost_usd = pages × per_page_usd` from `pricing.json`, overridable at runtime.
Provider credits are passed through in `usage.credits` but not converted: Reducto credits are
null on new pricing plans, Extend credits depend on the plan's $/credit, and LlamaParse reports 0
credits until billing settles.

**Consequences.** Costs are comparable across providers and stable, but are list prices; the
benchmark labels them as such.

## ADR-7: Benchmark ground truth is exact by construction; metrics are deterministic

**Decision.** `synthetic-v1` renders documents from the same source as the truth markdown, so
there is no annotation noise. Metrics are text-only (char similarity, CER, WER, word F1, order,
table) computed in Rust. No LLM judge is required for the leaderboard.

**Consequences.** Fully reproducible and cheap to run, but synthetic documents are cleaner than
real scans; the combined open dataset (see TASKS) is the answer to that, not a looser scorer.

## ADR-8: Benchmark runs disable provider result caches

**Context.** LlamaParse caches results for 48 hours; a re-run returned in ~450 ms instead of
~9 s and would have misreported latency.

**Decision.** `bench run` passes cache-busting options per provider (`do_not_cache` +
`invalidate_cache` for LlamaParse) unless `--allow-cache` is given.

**Consequences.** Latency in the leaderboard reflects real processing. Repeated runs cost real
credits.

## ADR-9: Development happens on `main-clean`

**Context.** The first three commits were pushed to `main` with a different author email. The
owner wants every commit attributed to `ajinkyashejul <ajinkyashejul4195@gmail.com>`. History
rewriting was blocked in the automated environment, so the same four commits were rebuilt with
`git commit-tree` under the correct identity and pushed as `main-clean`.

**Decision.** All work continues on `main-clean`. A repo admin will make it the default branch,
delete `main` (and the session mirror branch), and rename `main-clean` to `main`; until then,
references to `main` in workflows and docs are intentional and describe the post-rename state.

**Consequences.** Commits on `main-clean` never carry the old email. Dependabot PRs opened
against the old `main` will be re-created once the default branch changes.

## ADR-10: Combined open benchmark instead of a new vendor benchmark

**Context.** Each vendor publishes a benchmark it wins (ParseBench, RealDocBench,
LongExtractBench).

**Decision.** LiteOCR will not author a competing "vendor-neutral" opinion benchmark. It will
run every public benchmark through one harness with one scoring pipeline and publish per-dataset
and combined scores, with every input, truth and output inspectable online. Datasets whose
license permits redistribution are vendored in the repo; others are downloaded by an adapter at
run time with the upstream revision pinned in the manifest.

**Consequences.** The site is the verification surface; the CLI is the reproduction surface.
Some benchmarks (olmOCR-bench) need their own scorer implemented rather than transcript
similarity.

## ADR-11: Modes — providers are only interchangeable within a mode

**Context.** "OCR provider" covers different products: plain text recognition (Textract
DetectDocumentText, Azure Read, Google Document OCR), layout-aware parsing to markdown and typed
blocks (Reducto, Extend, LlamaParse, Mistral OCR, Datalab, Unstructured, Upstage, Landing AI),
and schema-driven structured extraction (Reducto Extract, Extend Extract, LlamaExtract, Azure
prebuilt models, Textract Queries/Forms, vision LLMs with structured output). Swapping a parse
model for an extract model is not a like-for-like switch.

**Decision.** The core defines `Mode::{Parse, Ocr, Extract}` with its own request/response
type per mode (`DocumentRequest → ParseResponse`, `DocumentRequest → TextResponse`,
`ExtractRequest → ExtractResponse`) and its own entry point (`parse`, `ocr`, `extract`).
Every registry model declares the modes it supports; resolution (`ModelRef::parse_for`) and
the `Router` reject models outside the requested mode. Pricing is per model *and* mode.
A layout provider serves `ocr` by deriving lines/words from its parse output (marked with
`liteocr_derived_from = "parse"`), so plain-text callers can still use it, but native OCR
endpoints override that when they exist.

**Consequences.** The Python API becomes `liteocr.parse` / `liteocr.ocr` / `liteocr.extract`
(the pre-release `liteocr.ocr` that meant parse is renamed; nothing was published). Vision
LLMs (Gemini, OpenAI, Anthropic) fit as parse/extract models without geometry, which the
response makes explicit by returning blocks without boxes. Adding a fourth mode (classify,
split) is additive: a new enum variant, types, trait method and entry point.

## ADR-12: Provider fan-out rules

**Decision.** Each provider lives in one file (`crates/liteocr-core/src/providers/<name>.rs`),
is declared up front in `providers/mod.rs`, and is implemented independently against the
`Provider` trait. Shared registration points (`model.rs` registry, `pricing.json`,
`build()`, README table, `.env.example`) are edited only by the integrator after a provider
lands, from snippets in the provider's hand-off. Providers without a key in the environment are
implemented from official docs with fixtures built from documented responses and `#[ignore]`
live tests; the task board records which ones have been verified live.

**Consequences.** Many providers can be built in parallel without merge conflicts; the cost is a
short integration step per provider and, for unverified providers, a "docs-only" label until
someone with a key runs the live tests.
