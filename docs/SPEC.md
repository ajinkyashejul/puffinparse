# LiteOCR Specification

> **One API for every OCR / document-parsing provider.** Rust core, Python SDK, CLI,
> and an open benchmark that ranks providers on accuracy, latency and cost.

Status: `v0.1` — providers: **Reducto**, **Extend**, **LlamaParse**.

---

## 1. Goals and non-goals

### Goals

1. **Single call, any provider.** `liteocr.ocr("invoice.pdf", model="reducto/standard")`
   returns the same `OcrResponse` shape whether the backend is Reducto, Extend,
   LlamaParse, or anything added later. Switching provider is a one-string change.
2. **Fast and lite.** The core is a Rust library (`liteocr-core`) with a small
   dependency set. Python only wraps it (PyO3). No provider SDKs are vendored;
   every provider is talked to over plain HTTPS with `reqwest`.
3. **Cost tracking.** Every response carries `usage` (pages, provider credits)
   and a computed `cost_usd` from an embedded, overridable pricing table.
4. **Reliability primitives.** Retries with backoff, per-call timeouts, and a
   `Router` with ordered fallbacks and simple load balancing.
5. **Open benchmark.** A reproducible harness + datasets + metrics that ranks
   providers, with results committed to the repo and published as a leaderboard.
6. **Open-source standards.** MIT license, CI (fmt, clippy, tests, Python lint +
   tests), semver, CHANGELOG, contributor docs, typed Python API, docstrings.

### Non-goals (v0.1)

- Hosting a proxy/gateway server (planned; the core is designed so a server
  crate can sit on top of `Router`).
- Running local/open-weight OCR models (Tesseract, PaddleOCR). The provider
  trait allows it later; v0.1 is API providers only.
- Structured extraction / classification products of the providers. LiteOCR
  covers *parsing* (document → text/markdown/blocks). Extraction is out of scope.

---

## 2. Architecture

```
┌────────────────────────────────────────────────────────────────────┐
│  Python SDK (python/liteocr)         CLI (crates/liteocr-cli)      │
│  liteocr.ocr / aocr / Router         liteocr parse | bench | ...   │
└───────────────┬────────────────────────────────┬───────────────────┘
                │ PyO3 (crates/liteocr-python)   │
┌───────────────▼────────────────────────────────▼───────────────────┐
│  liteocr-core (Rust)                                               │
│  ├─ types        OcrRequest / OcrResponse / Page / Block / Usage    │
│  ├─ providers    trait OcrProvider  { reducto, extend, llamaparse } │
│  ├─ router       fallbacks, retries, strategy (ordered/round-robin) │
│  ├─ pricing      embedded price table → cost_usd                    │
│  ├─ input        path | bytes | url  → DocumentInput                │
│  └─ bench        text normalisation + metrics (CER, WER, similarity)│
└────────────────────────────────────────────────────────────────────┘
```

Crates:

| Crate | Purpose |
|---|---|
| `crates/liteocr-core` | Library. All provider logic, types, router, pricing, benchmark metrics. `#![forbid(unsafe_code)]`. |
| `crates/liteocr-cli` | `liteocr` binary: `parse`, `providers`, `bench run`, `bench report`. |
| `crates/liteocr-python` | PyO3 extension module `liteocr._core`, built with maturin. |
| `python/liteocr` | Pure-Python public API, dataclasses, callbacks, typing. |
| `benchmark/` | Datasets, manifests, ground truth, results, leaderboard generator. |

---

## 3. Model naming

Like LiteLLM, the `model` string selects provider and mode: `"<provider>/<model>"`.

| Provider | Models (v0.1) | Maps to |
|---|---|---|
| `reducto` | `reducto/standard` (default), `reducto/hybrid`, `reducto/agentic` | `options.ocr_mode` = `standard` / `hybrid` / `agentic` |
| `extend` | `extend/parse` (default) | `POST /parse` with `config.target=MARKDOWN` |
| `llamaparse` | `llamaparse/fast`, `llamaparse/cost_effective` (default), `llamaparse/agentic`, `llamaparse/agentic_plus` | `tier` form field |

`model="reducto"` (no slash) selects the provider default. Unknown providers or
models raise `UnsupportedModelError` before any network call.

Provider-specific knobs that do not fit the common request are passed through
`provider_options` (a JSON object) and merged into the provider request body
verbatim. This is the escape hatch; it never changes the response shape.

---

## 4. Unified request

```python
liteocr.ocr(
    input,                       # str path | pathlib.Path | bytes | "https://..." URL
    model: str = "reducto",      # "<provider>/<model>"
    *,
    filename: str | None = None, # required when input is bytes
    pages: str | None = None,    # "1-3,7" 1-based page selection (best effort per provider)
    language: str | None = None, # BCP-47 hint, forwarded if provider supports it
    output: Literal["markdown", "text"] = "markdown",   # preferred `content` of blocks
    provider_options: dict | None = None,
    include_raw: bool = False,   # attach the provider's raw JSON to response.raw
    timeout: float = 300.0,      # seconds, whole call including polling
    max_retries: int = 2,        # on 429 / 5xx / network errors, exponential backoff
    api_key: str | None = None,  # overrides env var
    base_url: str | None = None, # overrides provider base URL
    metadata: dict | None = None # echoed back, useful for callbacks/logging
) -> OcrResponse
```

`aocr(...)` is the `async def` equivalent.

Input handling (`DocumentInput`):

- **Path** → read bytes, sniff MIME from extension (`mime_guess`), upload.
- **Bytes** → require `filename` (used for MIME + provider upload).
- **URL** (`http(s)://`) → passed to the provider as a remote URL when the
  provider supports it (all three do); otherwise downloaded and uploaded.

Supported document types are whatever the provider accepts; LiteOCR does not
pre-validate beyond a non-empty body.

---

## 5. Unified response

```python
@dataclass
class OcrResponse:
    id: str                    # liteocr-generated uuid
    provider: str              # "reducto"
    model: str                 # "reducto/standard"
    provider_job_id: str | None
    pages: list[Page]
    markdown: str              # whole document, pages joined by "\n\n"
    text: str                  # plain text
    usage: Usage
    cost_usd: float | None     # None if pricing unknown
    latency_ms: int            # wall-clock for the whole call, incl. polling
    created_at: str            # RFC 3339
    metadata: dict
    raw: Any | None            # provider payload if include_raw

@dataclass
class Page:
    page_number: int           # 1-based
    width: float | None        # points/pixels if provider reports it
    height: float | None
    markdown: str
    text: str
    blocks: list[Block]

@dataclass
class Block:
    type: BlockType            # text | title | section_header | list | table | figure |
                               # header | footer | footnote | caption | formula | other
    content: str               # markdown (tables as markdown/HTML per provider)
    text: str | None           # plain text if provider gives a separate one
    bbox: BBox | None          # normalised 0..1 {x0, y0, x1, y1}, origin top-left
    confidence: float | None   # 0..1 if provider reports one
    page_number: int

@dataclass
class Usage:
    pages: int                 # pages billed/processed
    credits: float | None      # provider-native credit units, if any
    provider_cost_usd: float | None   # if the provider reports $ directly
```

Rules:

- `markdown`/`text` at document level are derived from pages in order.
- A provider that only returns per-chunk (not per-page) content gets pages
  reconstructed from block page numbers; if that is impossible, a single page
  `page_number=1` is emitted and `Usage.pages` still reflects the billed count.
- Block types are mapped from each provider's vocabulary (see §8). Unknown → `other`.
- `bbox` is normalised so consumers can draw overlays without knowing the
  page size. Providers reporting absolute coordinates are divided by page dims.

---

## 6. Errors

All errors derive from `liteocr.LiteOCRError`:

| Error | When |
|---|---|
| `AuthenticationError` | 401/403 from provider or missing API key |
| `RateLimitError` | 429 (retried first; raised after `max_retries`) |
| `BadRequestError` | 4xx other than auth/rate limit |
| `ProviderError` | 5xx or provider-reported job failure |
| `TimeoutError` | overall timeout (upload + poll) exceeded |
| `UnsupportedModelError` | bad `model` string |
| `InputError` | unreadable file, bytes without filename, empty body |

Every error carries `provider`, `status_code` (if any), `message`, and
`request_id`/`job_id` when available.

---

## 7. Router

```python
router = liteocr.Router(
    models=["reducto/standard", "llamaparse/agentic", "extend/parse"],
    strategy="ordered",          # "ordered" (fallback order) | "round_robin"
    max_retries=2,
    fallback_on=("ProviderError", "RateLimitError", "TimeoutError"),
)
resp = router.ocr("doc.pdf")     # tries each in turn / rotates
```

Semantics: `ordered` → try `models[0]`, on a fallback-eligible error move on.
`round_robin` → rotate the starting index per call, then fallback in order.
The router records per-model success/failure counts and average latency,
exposed as `router.stats()`. Auth/BadRequest/Input errors never trigger fallback.

---

## 8. Provider mapping

### 8.1 Reducto

- Base: `https://platform.reducto.ai`, header `Authorization: Bearer <key>`.
- Flow: `POST /upload` (multipart `file`) → `{file_id}` (`reducto://…`) →
  `POST /parse` `{document_url, options:{ocr_mode}, advanced_options, …}`.
  For URLs, `document_url` is the URL directly.
- Sync parse is used; async (`/parse_async` + `/job/{id}`) is used when
  `provider_options.async = true` or the file exceeds a size threshold.
- Response: `result.chunks[].blocks[]` with `type`, `bbox{left,top,width,height,page,original_page}`
  (normalised 0..1), `content`, `confidence`. `usage.num_pages`, `usage.credits`.
  If `result.type == "url"`, fetch `result.url` to get the full result.
- Block type mapping: `Title→title`, `Section Header→section_header`,
  `Text→text`, `List Item→list`, `Table→table`, `Figure→figure`,
  `Header→header`, `Footer→footer`, `Footnote→footnote`, `Caption→caption`,
  `Formula→formula`, `Page Number→other`.
- Page reconstruction: group blocks by `bbox.page`.

### 8.2 Extend

- Base: `https://api.extend.ai`, headers `Authorization: Bearer <key>`,
  `x-extend-api-version: <date>`.
- Flow: `POST /files/upload` (multipart) → `fileId` → `POST /parse`
  `{file:{fileId | fileUrl}, config:{target:"MARKDOWN", …}}` (sync) or
  `/parse_async` + `GET /parser_runs/{id}`.
- Response: `parserRun.output.chunks[].blocks[]` with `type`, `content`,
  `pageNumber`, `boundingBox`, `confidence`; `parserRun.metrics.numPages`.
- Page reconstruction: group blocks by `pageNumber`.

### 8.3 LlamaParse

- Base: `https://api.cloud.llamaindex.ai`, header `Authorization: Bearer <key>`.
- Flow: `POST /api/v1/parsing/upload` (multipart `file` or `input_url`, form
  fields `tier`, `version`, `language`, `page_separator`, …) → `{id}`; poll
  `GET /api/v1/parsing/job/{id}` until `SUCCESS`/`ERROR`; then
  `GET /api/v1/parsing/job/{id}/result/json` for pages + items + metadata.
- Response: `pages[].{page, text, md, items[], width, height}`;
  `job_metadata.{job_pages, job_credits_usage, credits_used}`.
- Block type mapping: `heading→section_header` (level 1 → `title`),
  `text→text`, `table→table`; items carry `bBox{x,y,w,h}` in page units.

Exact field names are verified against live responses captured in
`crates/liteocr-core/tests/fixtures/`.

---

## 9. Pricing

`crates/liteocr-core/pricing.json` (embedded via `include_str!`) maps
`"<provider>/<model>"` → `{ "per_page_usd": float, "source": url, "updated": date }`.
`cost_usd = usage.pages * per_page_usd` unless the provider reports credits with
a known credit price, in which case `credits * per_credit_usd` is used.
Users can override with `liteocr.set_pricing({"reducto/standard": 0.01})`.
Prices are best-effort public list prices; the benchmark reports them as such.

---

## 10. Benchmark

### 10.1 Principles

1. **Reproducible**: every run records provider, model, dataset hash, options,
   timestamp, and the raw provider outputs (optionally, gitignored).
2. **Machine-checkable ground truth**: text-based metrics that don't need an LLM.
   An optional LLM-judge is a plug-in, never required for the leaderboard.
3. **Three axes**: accuracy, latency (p50/p95 per page), cost per 1k pages.
4. **Open datasets only**: synthetic documents generated by the repo (so the
   ground truth is exact) plus adapters for public sets (olmOCR-bench,
   OmniDocBench) that users download themselves.

### 10.2 Dataset format

```
benchmark/datasets/<name>/
  manifest.json          # {name, version, description, license, documents:[…]}
  docs/<id>.<ext>        # input file
  truth/<id>.md          # expected markdown (or .txt for text-only docs)
```

`manifest.documents[]`: `{id, file, truth, pages, tags:[...], category}`.
Categories in the built-in `synthetic-v1` set: `plain`, `invoice`, `table`,
`two_column`, `noisy_scan`, `handwriting_like`, `low_res`, `rotated`.

### 10.3 Metrics (computed in Rust, `liteocr_core::bench`)

Given predicted `P` and truth `T` after **normalisation** (NFKC, collapse
whitespace, strip markdown emphasis, lowercase for the `case_insensitive` variant):

- `char_similarity = 1 - levenshtein(P, T) / max(|P|, |T|)` (primary score)
- `cer = levenshtein(P, T) / |T|`
- `wer = word_levenshtein(P_words, T_words) / |T_words|`
- `word_recall` = fraction of truth word tokens present in prediction (bag-of-words)
- `table_score` (docs tagged `table`): char_similarity restricted to table lines
- `order_score`: Kendall-τ–like agreement of shared line order (reading order)

Per document all metrics are recorded; aggregate = mean over docs, plus per
category. **Overall score** = `100 * mean(char_similarity)`.

### 10.4 Runner and outputs

```
liteocr bench run --dataset benchmark/datasets/synthetic-v1 \
    --models reducto/standard extend/parse llamaparse/agentic \
    --out benchmark/results/<date>-synthetic-v1.json
liteocr bench report benchmark/results/*.json --format markdown > benchmark/LEADERBOARD.md
```

Result JSON: `{run_id, created_at, dataset:{name, version, sha256}, models:[{model,
docs:[{id, metrics, latency_ms, pages, cost_usd, error}], summary:{…}}]}`.

`LEADERBOARD.md` is regenerated from committed results and links to each run.

---

## 11. Python SDK details

- `python/liteocr/__init__.py` exports `ocr`, `aocr`, `Router`, `OcrResponse`,
  `Page`, `Block`, `BBox`, `Usage`, errors, `set_pricing`, `list_models`,
  `register_callback`.
- Callbacks: `liteocr.success_callback: list[Callable[[OcrResponse], None]]`,
  `liteocr.failure_callback` — invoked after each call (sync callbacks run
  inline; async ones scheduled on the loop).
- Logging: `LITEOCR_LOG=debug` enables tracing in the core; Python uses
  `logging.getLogger("liteocr")`.
- Typing: fully typed, `py.typed` shipped.
- Build: maturin, `abi3-py39` wheels, `pip install liteocr`.

---

## 12. CLI

```
liteocr parse <file|url> [--model reducto/standard] [--format markdown|text|json] [--raw]
liteocr providers                          # lists providers, models, pricing, key status
liteocr bench run|report|generate          # see §10
```

Exit code 0 on success, 1 on provider error, 2 on usage/config error.

---

## 13. Quality bar

- Rust: `cargo fmt --check`, `cargo clippy -D warnings`, unit tests with
  fixtures for each provider's parser, no network in tests (live tests are
  `#[ignore]` and run only when keys are present).
- Python: `ruff`, `mypy --strict` on the package, `pytest` with a fake core
  for unit tests; live tests skipped without keys.
- CI: GitHub Actions on push/PR (Linux; wheels build matrix on tags).
- Versioning: semver, single workspace version, `CHANGELOG.md` (Keep a Changelog).
