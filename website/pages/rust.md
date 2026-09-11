# Rust

`liteocr-core` is the whole implementation: unified types, every provider, the router, pricing and
the benchmark metrics. The Python SDK and the CLI are thin wrappers over it.

`#![forbid(unsafe_code)]`, no vendor SDK crates — every provider is spoken to over plain HTTPS with
`reqwest` and `tokio`.

API reference: [docs.rs/liteocr-core](https://docs.rs/liteocr-core) *(published on the first crates.io release)*.
Until then, `cargo doc -p liteocr-core --open` from a clone.

## Install

```toml
[dependencies]
liteocr-core = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

From the repository while it is pre-release:

```toml
liteocr-core = { git = "https://github.com/ajinkyashejul/liteocr" }
```

## First call

```rust
use liteocr_core::{parse, DocumentRequest};

#[tokio::main]
async fn main() -> liteocr_core::Result<()> {
    let resp = parse(DocumentRequest::from_path("invoice.pdf").model("reducto/standard")).await?;
    println!("{} pages, ${:.4}", resp.usage.pages, resp.cost_usd.unwrap_or(0.0));
    println!("{}", resp.markdown);
    Ok(())
}
```

Each entry point resolves the model, builds the provider, times the call, fills in `provider`,
`model`, `latency_ms` and `cost_usd`, merges request metadata into the response, and drops `raw`
unless `include_raw` was set.

## Modes

A call has a **mode**, and each mode has its own request and response type. Providers can only be
swapped within a mode — a model that does not support the requested mode is rejected before any
network call.

| Mode | Entry point | Request | Response | What you get |
|---|---|---|---|---|
| `Mode::Parse` | `parse` | `DocumentRequest` | `ParseResponse` | Layout-aware markdown and typed blocks with boxes. |
| `Mode::Ocr` | `ocr` | `DocumentRequest` | `TextResponse` | Plain text with line and word boxes, no layout semantics. |
| `Mode::Extract` | `extract` | `ExtractRequest` | `ExtractResponse` | A JSON object shaped by your schema, with per-field citations. |

```rust
pub async fn parse(request: DocumentRequest) -> Result<ParseResponse>;
pub async fn ocr(request: DocumentRequest) -> Result<TextResponse>;
pub async fn extract(request: ExtractRequest) -> Result<ExtractResponse>;

/// Blocking wrapper around `parse`; builds a small current-thread runtime per call.
pub fn parse_blocking(request: DocumentRequest) -> Result<ParseResponse>;
```

`Mode` is `Parse | Ocr | Extract`, with `Mode::ALL`, `as_str()` and a `Display` impl.

## `DocumentRequest` builder

Every setter takes `self` and returns `Self`, so requests chain.

```rust
use liteocr_core::{DocumentRequest, OutputFormat};

let req = DocumentRequest::from_path("doc.pdf")
    .model("extend/parse_performance")
    .pages("1-3,7")
    .language("en")
    .output(OutputFormat::Markdown)
    .provider_options(serde_json::json!({ "blockOptions": { "figures": { "enabled": false } } }))
    .include_raw(true)
    .timeout_secs(120.0)
    .max_retries(3)
    .api_key(std::env::var("EXTEND_API_KEY").unwrap())
    .base_url("https://api.extend.ai");
```

### Constructors

| Constructor | Input |
|---|---|
| `DocumentRequest::from_path(path)` | Local file. |
| `DocumentRequest::from_bytes(data, filename)` | In-memory bytes; the filename infers the type. |
| `DocumentRequest::from_url(url)` | Remote document, passed to the provider where supported. |
| `DocumentRequest::from_str_input(s)` | URL if `s` starts with `http://` / `https://`, else a path. |
| `DocumentRequest::new(DocumentInput)` | The general form. |

`DocumentInput` is an enum of `Path { path }`, `Bytes { data, filename }` and `Url { url }`, with
`filename()`, `mime_type()` and `describe()` helpers.

### Fields and defaults

| Field | Type | Default |
|---|---|---|
| `input` | `DocumentInput` | — |
| `model` | `String` | `"reducto"` |
| `pages` | `Option<String>` | `None` |
| `language` | `Option<String>` | `None` |
| `output` | `OutputFormat` | `Markdown` |
| `provider_options` | `Option<serde_json::Value>` | `None` |
| `include_raw` | `bool` | `false` |
| `timeout_secs` | `f64` | `300.0` |
| `max_retries` | `u32` | `2` |
| `api_key` | `Option<String>` | `None` (falls back to the provider's env var) |
| `base_url` | `Option<String>` | `None` (falls back to `<PROVIDER>_BASE_URL`, then the built-in) |
| `metadata` | `BTreeMap<String, Value>` | empty |

`req.option("key")` reads a single key back out of `provider_options`.

## `ParseResponse`

```rust
pub struct ParseResponse {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub pages: Vec<Page>,
    pub markdown: String,
    pub text: String,
    pub usage: Usage,
    pub latency_ms: u64,
    pub created_at: String,
    pub provider_job_id: Option<String>,
    pub cost_usd: Option<f64>,
    pub metadata: BTreeMap<String, serde_json::Value>,
    pub raw: Option<serde_json::Value>,
}
```

`Page { page_number, markdown, text, blocks, width, height }`,
`Block { type, content, page_number, text, bbox, confidence }`,
`BBox { x0, y0, x1, y1 }` normalised 0..1 with a top-left origin, and
`Usage { pages, credits, provider_cost_usd }`.

Everything derives `Serialize` / `Deserialize`, so a response round-trips through JSON unchanged —
that is exactly what `liteocr parse -f json` prints.

Helpers in `types`: `ParseResponse::from_pages`, `page_count`, `join_pages`, `pages_from_blocks`,
`strip_html_tags`, `markdown_to_text`.

## `TextResponse` (ocr mode)

Same envelope — `id`, `provider`, `model`, `provider_job_id`, `usage`, `cost_usd`, `latency_ms`,
`created_at`, `metadata`, `raw` — with `text` for the whole document and `pages: Vec<TextPage>`.

```rust
pub struct TextPage {
    pub page_number: u32,
    pub text: String,
    pub lines: Vec<Line>,
    pub words: Vec<Word>,
    pub width: Option<f64>,
    pub height: Option<f64>,
}
```

`Line` and `Word` are both `{ text, bbox: Option<BBox>, confidence: Option<f64> }`.

## `ExtractRequest` / `ExtractResponse` (extract mode)

```rust
use liteocr_core::{extract, DocumentRequest, ExtractRequest};

let req = ExtractRequest::new(
    DocumentRequest::from_path("invoice.pdf").model("reducto/standard"),
    serde_json::json!({
        "type": "object",
        "properties": { "total": { "type": "number" }, "vendor": { "type": "string" } },
    }),
)
.instructions("Totals are inclusive of tax.")
.citations(true);

let resp = extract(req).await?;
println!("{}", resp.data);                      // the object your schema describes
for (pointer, info) in &resp.fields {           // "/total" -> confidence + citations
    println!("{pointer}: {:?} {:?}", info.confidence, info.citations);
}
```

`ExtractRequest` flattens a `DocumentRequest` (so every builder setter above still applies) and adds
`schema` (a JSON Schema 2020-12 subset), optional `instructions`, and `citations: bool`.
`ExtractResponse` carries `data`, plus `fields: BTreeMap<String, FieldInfo>` keyed by JSON pointer,
where `FieldInfo { confidence: Option<f64>, citations: Vec<Citation> }` and
`Citation { page_number, bbox: Option<BBox>, text: Option<String> }`.

## Router

```rust
use liteocr_core::{DocumentRequest, Mode, Router, RouterConfig, Strategy};

let router = Router::new(
    RouterConfig::new(vec!["reducto/standard".into(), "llamaparse/agentic".into()])
        .mode(Mode::Parse),               // every model must support this mode
)?;

let resp = router.parse(&DocumentRequest::from_path("doc.pdf")).await?;
for (model, s) in router.stats() {
    println!("{model}: {} ok, {} failed, avg {:?} ms", s.successes, s.failures, s.avg_latency_ms());
}
```

`RouterConfig::new(models)` fills in `Mode::Parse`, `Strategy::Ordered` and the default
`fallback_on`: `Provider`, `RateLimit`, `Timeout`, `Network`. Set `strategy` to
`Strategy::RoundRobin` to rotate the starting model per call. The request's own `model` is ignored
— the router chooses. `router.models()` returns the canonicalised list, `router.mode()` the mode it
is pinned to, and `router.plan()` the order the next call will try (advancing the round-robin
cursor). `Strategy` parses from a string (`"ordered"` / `"fallback"`, `"round_robin"` /
`"roundrobin"`).

The router has one method per mode: `parse`, `ocr` and `extract`. Calling one whose mode does not
match the router's configured mode is an error.

`ModelStats` carries `successes`, `failures`, `total_latency_ms`, `total_cost_usd`, `total_pages`
and `avg_latency_ms()`.

## Errors

```rust
pub enum ErrorKind {
    Authentication, RateLimit, BadRequest, Provider,
    Timeout, UnsupportedModel, Input, Network,
}
```

`Error` carries the `kind`, the provider message, and optional `provider`, `status_code` and
`job_id`. `type Result<T> = std::result::Result<T, Error>`. The Python exception hierarchy is a
1:1 mirror of these kinds.

```rust
match parse(req).await {
    Ok(resp) => println!("{}", resp.markdown),
    Err(e) if e.kind == liteocr_core::ErrorKind::RateLimit => { /* back off */ }
    Err(e) => eprintln!("{e}"),
}
```

## Models and pricing

```rust
use liteocr_core::{list_models, list_models_for, model_info, Mode, ModelRef, PROVIDERS};

for p in PROVIDERS {                       // name, display_name, env_var, base_url, docs, models
    for m in p.models {
        println!("{} {:?} {}", m.qualified(), m.modes, if m.default { "*" } else { "" });
    }
}

list_models();                             // every "<provider>/<model>"
list_models_for(Mode::Extract);            // only the models that serve this mode
model_info("reducto", "r-1");              // -> Option<&ModelInfo>

let ModelRef { provider, model } = ModelRef::parse("reducto")?;           // -> reducto / standard
let checked = ModelRef::parse_for("reducto", Mode::Ocr)?;                 // also checks the mode
let usd = liteocr_core::pricing::estimate_cost("reducto/standard", Mode::Parse, 12);
let table = liteocr_core::pricing::all_prices();
```

`ModelInfo` is `{ provider, model, description, default, modes }`; `default` marks the provider's
default model *within each mode it supports*. `ModelRef::parse` / `parse_for` are the validation
gate: unknown providers, unknown models, or a model that does not serve the requested mode produce
`ErrorKind::UnsupportedModel` before any network call. Aliases `llama`, `llama_parse` and
`llamacloud` resolve to `llamaparse`.

## Benchmark module

`liteocr_core::bench` is the deterministic scoring used by `liteocr bench` and
`liteocr.score` — no LLM judge, no network.

```rust
use liteocr_core::bench::{normalize, score, summarize, Metrics, NormalizeOptions, Summary};

let opts = NormalizeOptions {
    case_insensitive: true,
    strip_markdown: true,
    strip_punctuation: false,
};

let m: Metrics = score(&prediction, &truth, opts);
println!("{:.3} char sim, {:.3} CER, {:.3} WER", m.char_similarity, m.cer, m.wer);

let s: Summary = summarize(&[Some(m), None]);   // None counts as a failed document
println!("overall {:.2} over {} docs ({} failed)", s.overall, s.documents, s.failed);
```

| Item | Description |
|---|---|
| `normalize(s, opts)` | NFKC, markdown stripped, quotes/dashes straightened, whitespace collapsed. |
| `score(pred, truth, opts)` | `Metrics { char_similarity, cer, wer, word_recall, word_precision, word_f1, order_score, table_score, pred_chars, truth_chars }`. |
| `summarize(&[Option<Metrics>])` | `Summary { documents, failed, char_similarity, cer, wer, word_f1, order_score, table_score, overall }`. |
| `levenshtein(a, b)` | The generic edit distance used underneath. |

See [Benchmark](/benchmark/) for the definition of each metric.

## Module map

| Module | Contents |
|---|---|
| `types` | `Mode`, `DocumentRequest`, `ParseResponse`, `TextResponse`, `TextPage`, `Line`, `Word`, `ExtractRequest`, `ExtractResponse`, `Citation`, `FieldInfo`, `Page`, `Block`, `BBox`, `Usage`, `DocumentInput`, `OutputFormat`. |
| `error` | `Error`, `ErrorKind`, `Result`. |
| `model` | `ModelRef`, `ModelInfo`, `ProviderInfo`, `PROVIDERS`, `list_models`, `list_models_for`, `model_info`. |
| `pricing` | Embedded, overridable per-page price table. |
| `http` | Shared client: retry/backoff with jitter, deadline, polling helper. |
| `provider` | `Provider` trait plus key / base-URL / multipart helpers. |
| `providers` | `reducto`, `extend`, `llamaparse`, and `build()`. |
| `router` | `Router` (one method per mode), `RouterConfig`, `Strategy`, `ModelStats`. |
| `bench` | Normalisation, metrics, summaries. |
| `util` | `deep_merge`, page-range parsing. |

## Adding a provider

One file under `crates/liteocr-core/src/providers/`, implementing `Provider`, registered in
`providers/mod.rs` and `model::PROVIDERS`, plus `pricing.json` entries, a fixture-backed
normalisation test with a real redacted payload, and a page under [Providers](/providers/).
The full checklist is in [Contributing](/project/contributing/).

## See also

- [Specification](/project/spec/) — the contract all three surfaces implement
- [CLI](/cli/) · [Python SDK](/python/)
