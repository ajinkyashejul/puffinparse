# LiteOCR documentation

Start here if you are joining the project. Everything a contributor or agent needs to work
without asking around is linked from this page; if something is missing, add it here.

| Document | What it is for |
|---|---|
| [`../README.md`](../README.md) | User-facing overview, install, usage, model table. |
| [`SPEC.md`](SPEC.md) | Product and architecture specification: goals, unified request/response, errors, router, provider mappings, benchmark design, quality bar. Update it when behaviour changes. |
| [`TASKS.md`](TASKS.md) | **Live task board.** Claim work here before starting; move items as they progress. |
| [`DECISIONS.md`](DECISIONS.md) | Architecture decision records. Read before proposing a change to something listed there; add an ADR when you change direction. |
| [`COMPAT.md`](COMPAT.md) | Native-format compatibility (`output_format="reducto"\|"extend"\|"llamaparse"`): what the vendor-shaped renders guarantee, which fields are always null, the coordinate-units rule, and migration examples. |
| [`SERVER.md`](SERVER.md) | The HTTP gateway (`liteocr serve`): config file, virtual keys, budgets, rate limits, API, errors, metrics, Docker. |
| [`providers/`](providers/README.md) | Per-provider reference: endpoints, request flow, response mapping, errors, gotchas, passthrough options. |
| [`benchmarks/`](benchmarks/) | Survey of public OCR benchmarks, adapter notes for the combined dataset, and [`findings.md`](benchmarks/findings.md) (what committed runs taught us: scorer v2, provider quirks). |
| [`../benchmark/README.md`](../benchmark/README.md) | Benchmark methodology, metrics, how to run, dataset format. |
| [`../benchmark/LEADERBOARD.md`](../benchmark/LEADERBOARD.md) | Generated leaderboard. Do not edit by hand; regenerate with `liteocr bench report`. |
| [`../benchmark/site/README.md`](../benchmark/site/README.md) | The static results viewer (GitHub Pages) and how to build it. |
| [`../CONTRIBUTING.md`](../CONTRIBUTING.md) | Setup, checks, how to add a provider or a dataset, PR checklist. |
| [`../CHANGELOG.md`](../CHANGELOG.md) | Keep-a-Changelog; add a line under Unreleased with every user-visible change. |
| [`../CLAUDE.md`](../CLAUDE.md) | Working agreement for AI agents (and humans): branch, identity, checks, what not to do. |

## Working in parallel

1. Pick or add a task in [`TASKS.md`](TASKS.md); mark it `[~]` with your name and date.
2. Work on `main` (see ADR-9). Small, reviewable commits; run `make lint test` before pushing.
3. Anything that changes an interface (unified types, model names, manifest format, result JSON)
   is a spec change: update `SPEC.md` in the same commit and add an ADR if it reverses a decision.
4. Provider facts go in `providers/<name>.md`, never only in code comments or chat.
5. When done, move the task to Done with the commit hash, and add a CHANGELOG line.

## Layout cheat sheet

```
crates/liteocr-core/src/
  types.rs        unified request/response, markdown→text, page grouping
  error.rs        Error + ErrorKind (mirrored by python/liteocr/exceptions.py)
  model.rs        provider + model registry (the only place models are declared)
  pricing.rs/.json list prices, overridable
  http.rs         shared client, retry/backoff, deadline, polling helper
  jobs.rs         async jobs API types: JobHandle, JobStatus, webhook events (SPEC §15)
  provider.rs     OcrProvider trait + helpers (keys, base URLs, multipart)
  providers/      reducto.rs, extend.rs, llamaparse.rs (+ mod.rs build()); vlm.rs = shared
                  helpers of the vision-LLM providers (gemini, openai, anthropic)
  testutil.rs     test-only loopback HTTP server for provider wire tests
  compat/         render a unified response in a vendor's native JSON shape (docs/COMPAT.md)
  router.rs       ordered / round-robin fallbacks, stats
  bench.rs        normalisation + metrics + summaries
  util.rs         deep_merge, page-range parsing
crates/liteocr-cli/src/   main.rs (parse, providers, serve), bench.rs (run, report, score)
crates/liteocr-server/src/ api.rs (routes, auth, fallback loop), config.rs, usage.rs, metrics.rs, log.rs
crates/liteocr-python/    PyO3 module `liteocr._core`
python/liteocr/           public API, types, exceptions; python/tests/
crates/liteocr-node/      napi-rs addon behind the npm package
js/                       npm package `liteocr`: index.js + hand-written index.d.ts; js/test/
benchmark/                generate_synthetic.py, datasets/, results/, site/, adapters/ (parsebench, olmocr, omnidocbench, dpbench, combined)
```
