# Getting started

PuffinParse gives you one call for every OCR / document-parsing provider. Install it, set one key,
and parse a document in under a minute.

## 1. Install

### Python

```bash
pip install puffinparse
```

Python 3.9+. The wheel bundles the Rust core — there is no toolchain to install and no provider
SDK to add.

### CLI

The `puffinparse` binary is built from the Rust workspace:

```bash
cargo install --git https://github.com/ajinkyashejul/liteocr puffinparse-cli
# or, from a clone:
cargo build --release -p puffinparse-cli     # ./target/release/puffinparse
```

### Rust

```toml
[dependencies]
puffinparse-core = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

### From source

```bash
git clone https://github.com/ajinkyashejul/liteocr && cd puffinparse
python -m venv .venv && . .venv/bin/activate
pip install maturin && maturin develop --release    # builds puffinparse._core into the venv
cargo build --release -p puffinparse-cli
```

## 2. Set a key

Each provider reads its own environment variable. Set only the ones you use.

| Provider | Environment variable | Get a key |
|---|---|---|
| Reducto | `REDUCTO_API_KEY` | [reducto.ai](https://reducto.ai) |
| Extend | `EXTEND_API_KEY` | [extend.ai](https://extend.ai) |
| LlamaParse | `LLAMA_API_KEY` (starts with `llx-`) | [cloud.llamaindex.ai](https://cloud.llamaindex.ai) |

```bash
export REDUCTO_API_KEY=...
export EXTEND_API_KEY=...
export LLAMA_API_KEY=llx-...
```

Keys can also be passed per call (`api_key=...` / `--api-key`), and base URLs overridden with
`REDUCTO_BASE_URL`, `EXTEND_BASE_URL`, `LLAMA_BASE_URL`. See [`.env.example`](https://github.com/ajinkyashejul/liteocr/blob/main/.env.example).

Check what is configured:

```bash
puffinparse providers
```

## 3. First call

### Python

```python
import puffinparse

resp = puffinparse.parse("invoice.pdf", model="reducto/standard")

print(resp.markdown)                 # unified markdown, identical shape for every provider
print(resp.usage.pages, resp.cost_usd, resp.latency_ms)
print(resp.pages[0].blocks[0].type, resp.pages[0].blocks[0].bbox)
```

Async is the same call with `await`:

```python
resp = await puffinparse.aparse("invoice.pdf", model="llamaparse/cost_effective")
```

### CLI

```bash
puffinparse parse invoice.pdf -m extend/parse_light            # markdown on stdout
puffinparse parse scan.png -m llamaparse/agentic -f json --raw # unified JSON + provider payload
```

### Rust

```rust
use puffinparse_core::{parse, DocumentRequest};

#[tokio::main]
async fn main() -> puffinparse_core::Result<()> {
    let resp = parse(DocumentRequest::from_path("invoice.pdf").model("reducto/standard")).await?;
    println!("{} pages, ${:.4}", resp.usage.pages, resp.cost_usd.unwrap_or(0.0));
    println!("{}", resp.markdown);
    Ok(())
}
```

## 4. Pick a mode

`parse` is one of three modes. Each has its own response type, and providers can only be swapped
*within* a mode.

| Mode | Python | CLI | You get |
|---|---|---|---|
| `parse` | `puffinparse.parse(...)` | `puffinparse parse doc.pdf` | Layout-aware markdown + typed blocks with boxes. |
| `ocr` | `puffinparse.ocr(...)` | `puffinparse ocr scan.png` | Plain text with line and word boxes. |
| `extract` | `puffinparse.extract(..., schema)` | `puffinparse extract doc.pdf -s schema.json` | A JSON object shaped by your schema, with citations. |

```python
text = puffinparse.ocr("scan.png", model="reducto/r-1")
text.text, text.pages[0].lines[0].bbox

data = puffinparse.extract(
    "invoice.pdf",
    {"type": "object", "properties": {"total": {"type": "number"}}},
    model="reducto/standard",
    citations=True,
)
data.data["total"], data.citations("/total")
```

`puffinparse providers --mode extract` lists the models that serve a mode; asking a model for a mode it
does not support raises `UnsupportedModelError` before any network call.

## 5. Switch providers

The model string is the only thing that changes. `"<provider>/<model>"`, like LiteLLM; a bare
provider name selects its default model.

```python
puffinparse.parse("doc.pdf", model="reducto/r-1")
puffinparse.parse("doc.pdf", model="extend/parse_performance")
puffinparse.parse("doc.pdf", model="llamaparse/agentic_plus")
puffinparse.parse("doc.pdf", model="reducto")               # -> reducto's default parse model
```

Unknown providers or models raise `UnsupportedModelError` before any network call.

## 6. Add fallbacks

```python
router = puffinparse.Router(
    ["reducto/standard", "llamaparse/agentic", "extend/parse_light"],
    mode="parse",             # a router is bound to one mode
    strategy="ordered",       # or "round_robin"
)
resp = router.parse("doc.pdf")
router.stats()                # successes, failures, latency, cost, pages per model
```

Provider, rate-limit, timeout and network errors fall through to the next model. Auth,
bad-request and input errors never do — they would fail on every provider.

## Where to next

- [Python SDK](/python/) — every function, dataclass and exception.
- [CLI](/cli/) — every subcommand and flag.
- [Rust](/rust/) — `puffinparse-core` crate usage.
- [Providers](/providers/) — exactly what PuffinParse sends and how the response is mapped.
- [Benchmark](/benchmark/) — how the leaderboard is produced, and its caveats.
- [Specification](/project/spec/) — the contract the implementations follow.
