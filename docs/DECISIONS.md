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

*Update 2026-09-11:* done. The old `main` and the session branch were deleted and `main-clean`
was renamed to `main`; all development now happens on `main`.

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

## ADR-13: Native-format compatibility is a renderer over the unified response

**Context.** The unified response is the product, but it is also the migration cost: a team already
parsing Reducto's `result.chunks[].blocks[].bbox.left` cannot try another provider without
rewriting the code that reads the result. The one thing that would make switching free is getting
answers back in the shape they already parse.

**Decision.** Add `crates/liteocr-core/src/compat/`: a pure, infallible renderer
`render_parse(&ParseResponse, Format) -> serde_json::Value` (plus a best-effort `render_extract`)
with one module per vendor shape — `Format::{Liteocr, Reducto, Extend, LlamaParse}`. It is applied
*after* a call, not inside it: `parse`/`ocr`/`extract` keep returning the unified structs,
`DocumentRequest.output_format` only records and validates the caller's choice
(`validate_output_format`), and the SDK/CLI call `ParseResponse::to_format(&str)` on the result.
So routing, retries, pricing, fallbacks and every existing test are untouched.

What is promised is **structural fidelity, not semantic identity**: the vendor's key set, nesting,
chunk/page and block counts, content strings, block-type vocabulary and coordinate units. Fields
LiteOCR does not model are rendered `null`/empty and enumerated in `docs/COMPAT.md`, never invented.
Extend and LlamaParse need a page size for their unit boxes; when the source provider reports none
(Reducto, vision LLMs) the renderer assumes a 1000×1000 page and, for Extend, records
`metadata.liteocr_synthetic_page_dims = true` in the run's free-form metadata map.

The claim is enforced rather than asserted: for each provider, `compat/roundtrip.rs` runs
`fixture → provider::normalize → render_parse(same format)` and compares against the original
fixture with a `skeleton_diff` (key sets, counts, contents, types up to the provider's own forward
mapping, boxes within 1e-6, billed pages), plus cross-format and no-geometry cases. The tests live
in the crate because the `normalize` functions are `pub(crate)`.

**Consequences.** Adding a fourth shape is one module plus one enum variant. Lossy edges are real
and documented: type mappings are not injective (Extend's `key_value` returns as `text`), the
Reducto render uses a reduced vocabulary (`footnote`/`caption`/`formula`/`other` → `Text`), and
`render_extract` is explicitly weaker than the parse path until extract fixtures exist for all
three providers. Because the renderer only reads the unified types, any provider added later gets
all three native shapes for free — and any unified field a new provider cannot fill shows up as a
`null` in someone's vendor-shaped payload, which is the honest outcome.

## ADR-14: The Node.js SDK is a napi-rs addon over the same core

**Context.** A TypeScript SDK was on the roadmap once the Python surface stabilised. The options
were a WASM build, a wrapper around the CLI, or a native N-API addon.

**Decision.** `crates/liteocr-node` exposes the core through napi 3; the `js/` package is a thin
layer. The addon only converts values: requests and responses cross as the core's serde JSON,
errors as a prefixed JSON payload (`LITEOCR_CORE_ERROR:<json>`) that the JS layer rebuilds into
typed `LiteOCRError` subclasses. The JS layer converts to camelCase with explicit per-type
converters; `data`, `metadata` and `raw` are never renamed, and `index.d.ts` is hand-written
against SPEC §5 (the napi-generated `native.d.ts` is internal). The crate is a normal workspace
member: napi's `dyn-symbols` keeps `cargo test --workspace` free of Node. It uses
`deny(unsafe_code)` because napi's macro expansion is incompatible with `forbid`, and declares
`rust-version = "1.88"` (napi 3) while the rest of the workspace stays at 1.80. `timeout` is in
seconds, as in the other SDKs, and `LiteOCRError.kind` uses the ErrorKind serde values.

**Consequences.** One implementation behind three surfaces (Python, Node, CLI). No provider logic
in JS. Prebuilt binaries per platform are required for `npm install` without a Rust toolchain;
the release matrix builds them but publishing is not wired yet.

## ADR-15: The gateway is a thin axum layer over the core, TOML-configured, with no database

**Context.** LiteLLM's proxy is what teams actually deploy: one endpoint, central keys, budgets and
logs. LiteOCR needed the same without growing a second implementation of providers.

**Decision.** `crates/liteocr-server` (axum + tower-http, which are HTTP frameworks, not provider
SDKs) calls `liteocr_core::{parse, ocr, extract}` and runs its own fallback loop, because the core
`Router` cannot give each target its own credentials or base URL. Configuration is TOML (already
idiomatic in Rust, lighter than YAML). Usage and budget state live in memory with an optional
JSON state file; a database is out of scope. Clients may not send `api_key` or `base_url`, so they
cannot redirect the gateway's credentials, and local file paths are refused. Provider credential
failures map to 502, not 401, because the caller's own key was valid.

**Consequences.** Single binary, no infrastructure to run. Budgets can be overshot by requests in
flight, and key changes need a restart. A database, HTTP key management, async job endpoints and
metrics auth are follow-ups, not blockers.

## ADR-16: Vendor only what the licence permits; index the rest

**Context.** The combined benchmark (ADR-10) pulls in public datasets whose licences differ:
olmOCR-bench is ODC-BY-1.0, OmniDocBench has no licence and is marked research-only /
non-commercial.

**Decision.** A dataset is vendored into the repo (with attribution) only when its licence permits
redistribution. Otherwise the repo holds a manifest with upstream paths, a pinned revision and
image/truth hashes, and the adapter materialises the data locally. Combined datasets are
versioned and never rewritten once results exist (`combined-v2` supersedes `combined-v1` for new
runs). Upstream tests that cannot be expressed faithfully in the shared rule schema are skipped
and counted, never weakened silently; the one relaxed mapping (olmOCR "left/right of" → same row)
is counted as relaxed.

**Consequences.** Anyone can reproduce every score, but index-only sources need a fetch step before
a run (their documents carry a `fetch-required` tag). Stats files make the coverage of each
conversion auditable.

## ADR-17: The benchmark results viewer stays vanilla JS

**Context.** TASKS listed an open choice for `benchmark/site/` between a React app on Extend UI
(PDF viewer and layout overlays out of the box) and the zero-build vanilla viewer. The viewer has
to be where every benchmark claim can be checked: page rendering, side-by-side outputs, diffs,
rule checklists, bbox overlays, charts, deep links.

**Decision.** Keep static HTML/CSS/ES2018 in `benchmark/site/src/`: no framework, no bundler, no
npm install; the build stays stdlib Python (Pillow optional). The one third-party runtime
dependency is pdf.js, loaded lazily from cdnjs at a pinned version with SRI, only when a PDF is
opened, with the build-time PNG as fallback. Overlays and charts are inline SVG. The per-document
rule checklist uses a JS port of `liteocr-core`'s `score_rules`, and every rules page compares its
count with the recorded Rust score and flags any disagreement.

**Consequences.** Vercel and Pages build the site with one `uv run` command and nothing to audit.
The data contract (`data/index.json`, `data/runs/`, `data/outputs/…/<doc>.{md,json}`) is
independent of the front-end, so this can be revisited without touching the build. The JS port
must follow scorer changes in `bench.rs`; the mismatch badge makes drift visible.

## ADR-18: Webhooks are exposed as primitives, not received

**Context.** TASKS asked for "webhooks instead of polling". An SDK cannot host an HTTP endpoint,
and providers differ: Reducto and LlamaParse accept a per-job webhook URL, Extend only has
workspace-level webhook endpoints.

**Decision.** LiteOCR offers `submit_parse` / `retrieve_parse` plus a pure `parse_webhook` /
`resolve_webhook` that normalises a provider's webhook body into `JobStatus` (doing one retrieve
when the body only names the job). `JobHandle` is serialisable and secret-free; credentials are
resolved again at retrieve time. `DocumentRequest.webhook_url` maps to per-job provider webhooks
where they exist and is rejected with an input error where they don't. Only `parse` mode has jobs
for now.

**Consequences.** Users wire LiteOCR into their own web handler; the gateway (ADR-15) can later add
job endpoints on top of the same primitives. Webhook body shapes come from vendor docs until real
deliveries are captured.
