# LiteOCR

**One API for every OCR / document-parsing provider.** Rust core, Python SDK, CLI, and an open benchmark that ranks providers on accuracy, latency and cost.

```python
import liteocr

resp = liteocr.ocr("invoice.pdf", model="reducto/standard")      # or "extend/parse_performance", "llamaparse/agentic", ...
print(resp.markdown)                                             # unified markdown, every provider
print(resp.pages[0].blocks[0].bbox, resp.usage.pages, resp.cost_usd)
```

Switch providers by changing one string. Same request, same response shape, same errors.

[![CI](https://github.com/ajinkyashejul/liteocr/actions/workflows/ci.yml/badge.svg)](https://github.com/ajinkyashejul/liteocr/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Python 3.9+](https://img.shields.io/badge/python-3.9%2B-blue)](pyproject.toml)

## Why

Every document-parsing vendor has its own upload flow, polling loop, JSON layout, block vocabulary, coordinate system and billing unit. LiteOCR hides all of that behind one call, tracks cost per call, retries and falls back across providers, and ships a reproducible benchmark so you can pick a provider on evidence instead of marketing.

| | |
|---|---|
| **Providers (v0.1)** | [Reducto](https://reducto.ai), [Extend](https://extend.ai), [LlamaParse](https://cloud.llamaindex.ai) — 10 models total |
| **Core** | Rust (`liteocr-core`): `reqwest` + `tokio`, no vendor SDKs, `#![forbid(unsafe_code)]` |
| **SDK** | Python 3.9+ (`pip install liteocr`), sync + async, fully typed |
| **CLI** | `liteocr parse`, `liteocr providers`, `liteocr bench` |
| **Reliability** | Retries with jittered backoff, whole-call deadlines, `Router` with ordered fallbacks / round-robin |
| **Cost** | Embedded, overridable price table → `cost_usd` on every response |
| **Benchmark** | Deterministic text metrics (char similarity, CER, WER, word F1, reading order, tables), latency p50/p95, $/1k pages |

## Install

```bash
pip install liteocr
```

Set the keys for the providers you use:

```bash
export REDUCTO_API_KEY=...
export EXTEND_API_KEY=...
export LLAMA_API_KEY=llx-...      # LlamaCloud / LlamaParse
```

From source (Rust stable + Python 3.9+):

```bash
git clone https://github.com/ajinkyashejul/liteocr && cd liteocr
python -m venv .venv && . .venv/bin/activate
pip install maturin && maturin develop --release     # builds the extension into the venv
cargo build --release -p liteocr-cli                 # ./target/release/liteocr
```

## Usage

### One call, any provider

```python
import liteocr

# path, URL, or bytes (+ filename)
resp = liteocr.ocr("contract.pdf", model="extend/parse_performance")
resp = liteocr.ocr("https://cdn.reducto.ai/samples/fidelity-example.pdf", model="reducto/r-1", pages="1-2")
resp = liteocr.ocr(open("scan.png", "rb").read(), filename="scan.png", model="llamaparse/agentic")

resp.markdown            # whole document
resp.text                # plain text
resp.pages[0].markdown   # per page
for block in resp.pages[0].blocks:
    block.type           # text | title | section_header | list | table | figure | header | footer | ...
    block.content        # markdown
    block.bbox           # BBox(x0, y0, x1, y1) normalised 0..1, origin top-left (or None)
    block.confidence     # 0..1 when the provider reports one
resp.usage.pages, resp.usage.credits, resp.cost_usd, resp.latency_ms
```

### Async

```python
resp = await liteocr.aocr("contract.pdf", model="llamaparse/cost_effective")
```

Runs on the Rust runtime; the event loop is never blocked.

### Model names

`"<provider>/<model>"`, like LiteLLM. A bare provider name picks its default (`*`).

| Model | List price / page | Notes |
|---|---:|---|
| `reducto/standard` `*` | $0.015 | Reducto Parse, account-default model |
| `reducto/r-1` | $0.010 | Reducto's newest model (`settings.model = "r-1"`) |
| `reducto/agentic` | $0.030 | Agentic text + table enhancement |
| `extend/parse_performance` `*` | $0.025 | Highest accuracy engine |
| `extend/parse_light` | $0.00625 | Fast, cheap, digital-native documents |
| `extend/parse_auto` | $0.025 | Picks light or performance per page |
| `llamaparse/fast` | $0.00125 | Text extraction only |
| `llamaparse/cost_effective` `*` | $0.00375 | |
| `llamaparse/agentic` | $0.0125 | |
| `llamaparse/agentic_plus` | $0.05625 | Highest accuracy tier |

Prices are public pay-as-you-go list prices at the time of the last update (see `crates/liteocr-core/src/pricing.json`). Override them with `liteocr.set_pricing({"reducto/standard": 0.012})`.

### Provider-specific options

Anything the common request doesn't cover is passed through verbatim:

```python
liteocr.ocr("doc.pdf", model="reducto/standard",
            provider_options={"settings": {"return_ocr_data": True}, "async": True})
liteocr.ocr("doc.pdf", model="extend/parse_performance",
            provider_options={"blockOptions": {"figures": {"enabled": False}}})
liteocr.ocr("doc.pdf", model="llamaparse/agentic",
            provider_options={"take_screenshot": True, "version": "2026-08-19"})
```

`include_raw=True` attaches the provider's original payload as `resp.raw`.

### Router: fallbacks and load balancing

```python
router = liteocr.Router(
    ["reducto/standard", "llamaparse/agentic", "extend/parse_light"],
    strategy="ordered",          # or "round_robin"
)
resp = router.ocr("doc.pdf")     # falls back on provider / rate-limit / timeout / network errors
router.stats()                   # per-model successes, failures, latency, cost, pages
```

Auth, bad-request and input errors never trigger a fallback.

### Errors

All errors derive from `liteocr.LiteOCRError` and carry `provider`, `status_code`, `job_id` and `retryable`:

`AuthenticationError`, `RateLimitError`, `BadRequestError`, `ProviderError`, `TimeoutError`, `UnsupportedModelError`, `InputError`, `NetworkError`.

### Callbacks and logging

```python
liteocr.success_callback.append(lambda r: print(r.model, r.usage.pages, r.cost_usd))
liteocr.failure_callback.append(lambda e: print("failed:", e))
liteocr.init_logging("debug")    # Rust core tracing on stderr (or LITEOCR_LOG=debug for the CLI)
```

### CLI

```bash
liteocr providers                                             # models, prices, key status
liteocr parse invoice.pdf -m extend/parse_light               # markdown to stdout
liteocr parse scan.png -m llamaparse/agentic -f json --raw    # full unified JSON (+ provider payload)
liteocr parse doc.pdf -m reducto/r-1 --pages 1-3 -f text
```

### Rust

```rust
use liteocr_core::{ocr, OcrRequest};

let resp = ocr(OcrRequest::from_path("invoice.pdf").model("reducto/standard")).await?;
println!("{} pages, ${:.4}\n{}", resp.usage.pages, resp.cost_usd.unwrap_or(0.0), resp.markdown);
```

## Benchmark

LiteOCR ships an open, reproducible benchmark. Ground truth is exact by construction (the documents are rendered from the same source as the truth files), metrics are deterministic text comparisons, and every run records the dataset hash, model, latency and cost.

```bash
python benchmark/generate_synthetic.py                       # regenerate the dataset (byte-reproducible)
liteocr bench run --dataset benchmark/datasets/synthetic-v1 \
    --models reducto/standard extend/parse_performance llamaparse/cost_effective
liteocr bench report benchmark/results/*.json > benchmark/LEADERBOARD.md
liteocr bench score prediction.md truth.md                   # metrics for one pair, no network
```

Metrics (after NFKC + markdown stripping + whitespace collapsing, case-insensitive by default):

- **Overall** = `100 × mean(char_similarity)`, where `char_similarity = 1 − levenshtein / max(len)`
- **CER**, **WER**, **word F1** (bag-of-words precision / recall)
- **Order**: Kendall-τ-style agreement of shared line order (reading order)
- **Table**: character similarity restricted to markdown table rows
- **Latency** p50 / p95 and ms per page; **$/1k pages** from the price table

The current leaderboard is in [`benchmark/LEADERBOARD.md`](benchmark/LEADERBOARD.md). Dataset details are in [`benchmark/datasets/synthetic-v1/README.md`](benchmark/datasets/synthetic-v1/README.md). Adapters for public sets (olmOCR-bench, OmniDocBench) are on the roadmap; see [`docs/SPEC.md`](docs/SPEC.md).

## How it maps providers

| | Reducto | Extend | LlamaParse |
|---|---|---|---|
| Upload | `POST /upload` → `reducto://` id (URLs passed directly) | `POST /files/upload` → `file_…` (URLs passed directly) | multipart `file` or `input_url` |
| Parse | `POST /parse` (sync) or `/parse_async` + `GET /job/{id}` | `POST /parse_runs` + `GET /parse_runs/{id}` | `POST /api/v1/parsing/upload` + `GET …/job/{id}` + `…/result/json` |
| Pages | `chunk_mode=page`, blocks grouped by `bbox.page` | page chunks, blocks by `metadata.page.number` | `pages[].md` / `items[]` |
| Boxes | already normalised | divided by `metadata.page.width/height` | `bBox` divided by page `width/height` |
| Usage | `usage.num_pages`, `usage.credits` | `metrics.pageCount`, `usage.credits` | `job_metadata.job_pages` |

Full details, including the exact wire formats verified against live responses, are in [`docs/SPEC.md`](docs/SPEC.md).

## Project layout

```
crates/liteocr-core     Rust library: types, providers, router, pricing, benchmark metrics
crates/liteocr-cli      `liteocr` binary
crates/liteocr-python   PyO3 extension (liteocr._core)
python/liteocr          Python package (typed public API)
benchmark/              dataset generator, datasets, results, leaderboard
docs/SPEC.md            specification
```

## Roadmap

- More providers: Mistral OCR, Azure Document Intelligence, AWS Textract, Google Document AI, Gemini / GPT vision, Mathpix, local Tesseract / PaddleOCR.
- Hosted gateway (`liteocr serve`) with keys, budgets and logging, built on `Router`.
- Benchmark adapters for olmOCR-bench and OmniDocBench; LLM-judge as an optional plug-in.
- Webhooks instead of polling for async providers.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Adding a provider is one Rust file plus a fixture test; the checklist is in the [new provider issue template](.github/ISSUE_TEMPLATE/new_provider.md).

## License

MIT. See [LICENSE](LICENSE).
