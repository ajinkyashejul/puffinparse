# Python SDK

`pip install liteocr` — Python 3.9+, fully typed (`py.typed`, `mypy --strict` clean). The package is
a thin, typed wrapper over the Rust core (`liteocr._core`); no provider logic lives in Python.

Everything below is the public surface exported from `liteocr.__all__`.

```python
import liteocr

liteocr.__version__          # version of the compiled core
liteocr.modes()              # ["parse", "ocr", "extract"]
```

## Modes

Every call names a **mode**, and the mode decides the response type. Providers can be swapped
freely *within* a mode; a model that does not serve the mode you asked for raises
`UnsupportedModelError` before any network call.

| Mode | Call | Async | Returns | Use it for |
|---|---|---|---|---|
| `parse` | `liteocr.parse` | `liteocr.aparse` | `ParseResponse` | Layout-aware markdown and typed blocks with boxes. |
| `ocr` | `liteocr.ocr` | `liteocr.aocr` | `TextResponse` | Plain text with line and word boxes — search indexes, redaction, overlays. |
| `extract` | `liteocr.extract` | `liteocr.aextract` | `ExtractResponse` | A JSON object shaped by your schema, with per-field citations. |

`Mode` is `Literal["parse", "ocr", "extract"]` and `MODES` is the same tuple as a constant.

```python
doc  = liteocr.parse("invoice.pdf", model="reducto/standard")     # markdown + blocks
text = liteocr.ocr("scan.png", model="llamaparse/fast")           # plain text + boxes
data = liteocr.extract("invoice.pdf", schema, model="reducto/standard")
```

## `parse`

```python
def parse(
    input: DocumentLike,
    model: str = "reducto",
    *,
    filename: Optional[str] = None,
    pages: Optional[str] = None,
    language: Optional[str] = None,
    output: Literal["markdown", "text"] = "markdown",
    provider_options: Optional[dict[str, Any]] = None,
    include_raw: bool = False,
    timeout: float = 300.0,
    max_retries: int = 2,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    metadata: Optional[dict[str, Any]] = None,
) -> ParseResponse
```

`DocumentLike` is `str | os.PathLike[str] | bytes | bytearray | memoryview`.

| Parameter | Meaning |
|---|---|
| `input` | A file path, an `http(s)://` URL, or raw bytes (then `filename` is required). |
| `model` | `"<provider>/<model>"`, e.g. `"reducto/standard"`. A bare provider name selects its default model *for this mode*. |
| `filename` | Required when `input` is bytes; used to infer the document type. |
| `pages` | 1-based page selection such as `"1-3,7"`, forwarded best-effort. |
| `language` | Language hint (ISO 639-1) when the provider supports it. |
| `output` | Preferred block content: `"markdown"` (default) or `"text"`. |
| `provider_options` | Provider-specific options merged verbatim into the provider request body. |
| `include_raw` | Attach the provider's raw payload as `response.raw`. |
| `timeout` | Whole-call deadline in seconds (upload + polling + download). |
| `max_retries` | Retries on 429 / 5xx / network errors with exponential backoff and full jitter. |
| `api_key` | Override the API key (otherwise read from `REDUCTO_API_KEY`, `EXTEND_API_KEY`, `LLAMA_API_KEY`). |
| `base_url` | Override the provider base URL. |
| `metadata` | Free-form dict echoed back in `response.metadata`. |

Passing bytes without `filename` raises `InputError` before any network call.

```python
resp = liteocr.parse("contract.pdf", model="extend/parse_performance")
resp = liteocr.parse("https://example.com/doc.pdf", model="reducto/r-1", pages="1-2")
resp = liteocr.parse(open("scan.png", "rb").read(), filename="scan.png", model="llamaparse/agentic")
```

`aparse` is the same call with `await`. It runs on the Rust runtime, so the event loop is never
blocked.

```python
resp = await liteocr.aparse("contract.pdf", model="llamaparse/cost_effective")
```

## `ocr`

```python
def ocr(
    input: DocumentLike,
    model: str = "reducto",
    *,
    filename: Optional[str] = None,
    pages: Optional[str] = None,
    language: Optional[str] = None,
    provider_options: Optional[dict[str, Any]] = None,
    include_raw: bool = False,
    timeout: float = 300.0,
    max_retries: int = 2,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    metadata: Optional[dict[str, Any]] = None,
) -> TextResponse
```

Same parameters as `parse` minus `output` (the mode implies plain text). Use it when you want text
and geometry rather than document structure; for markdown, tables and block types use `parse`.

Providers without a native OCR endpoint serve this mode from their parse output. The response then
carries `metadata["liteocr_derived_from"] == "parse"`, so you can tell the difference.

```python
text = liteocr.ocr("scan.png", model="reducto/r-1")
text.text                                    # whole document
for line in text.pages[0].lines:
    line.text, line.bbox, line.confidence
text = await liteocr.aocr("scan.png")        # async
```

## `extract`

```python
def extract(
    input: DocumentLike,
    schema: dict[str, Any],
    *,
    model: str = "reducto",
    instructions: Optional[str] = None,
    citations: bool = False,
    filename: Optional[str] = None,
    pages: Optional[str] = None,
    language: Optional[str] = None,
    provider_options: Optional[dict[str, Any]] = None,
    include_raw: bool = False,
    timeout: float = 300.0,
    max_retries: int = 2,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    metadata: Optional[dict[str, Any]] = None,
) -> ExtractResponse
```

| Parameter | Meaning |
|---|---|
| `schema` | A JSON Schema **object** describing the fields you want. |
| `model` | Must support `extract`; a parse-only model raises `UnsupportedModelError` before any network call. |
| `instructions` | Optional natural-language guidance, forwarded to providers that accept it. |
| `citations` | Ask for per-field citations (page, box, source text) where the provider supports them. |

Everything else matches `parse`. `aextract` is the async variant.

```python
schema = {
    "type": "object",
    "properties": {
        "vendor": {"type": "string"},
        "total": {"type": "number"},
    },
}

resp = liteocr.extract("invoice.pdf", schema, model="reducto/standard",
                       instructions="Totals are inclusive of tax.", citations=True)

resp.data["total"]                      # the object your schema asked for
resp.citations("/total")                # [Citation(page_number=1, bbox=..., text="...")]
resp.field_info("/total").confidence
```

## `Router`

```python
class Router:
    def __init__(
        self,
        models: list[str],
        *,
        mode: Mode = "parse",
        strategy: Literal["ordered", "round_robin"] = "ordered",
        fallback_on: Optional[list[str]] = None,
    ) -> None
```

A router is bound to **one mode** at construction: every model must support it, and calling a method
for another mode raises `InputError`. That keeps fallbacks honest — a parse-only model can never
quietly answer an extraction.

| Member | Type | Description |
|---|---|---|
| `models` | `list[str]` (property) | The canonicalised model list. |
| `mode` | `Mode` (property) | The mode this router serves. |
| `plan()` | `list[str]` | The order models would be tried for the next call. Advances the round-robin cursor. |
| `stats()` | `dict[str, dict[str, Any]]` | Per model: `successes`, `failures`, `total_latency_ms`, `total_cost_usd`, `total_pages`. |
| `parse(input, **kw)` / `aparse` | `ParseResponse` | Same keyword arguments as `liteocr.parse` except `model`. |
| `ocr(input, **kw)` / `aocr` | `TextResponse` | Same, minus `output`. |
| `extract(input, schema, *, instructions, citations, **kw)` / `aextract` | `ExtractResponse` | Same as `liteocr.extract` except `model`. |

`fallback_on` is a list of error-kind names; the default is `provider`, `rate_limit`, `timeout`,
`network`. Authentication, bad-request, unsupported-model and input errors never trigger a
fallback. Unknown keyword arguments raise `TypeError` and the message lists what is accepted.

```python
router = liteocr.Router(["reducto/standard", "llamaparse/agentic"], strategy="round_robin")
resp = router.parse("doc.pdf", pages="1-5", timeout=120)
router.stats()["reducto/standard"]["successes"]

text_router = liteocr.Router(["reducto/r-1", "extend/parse_light"], mode="ocr")
text_router.ocr("scan.png").text
```

## Response types

All response dataclasses mirror the Rust structs in `liteocr-core` 1:1 and live in `liteocr.types`.
`Response` is the union `ParseResponse | TextResponse | ExtractResponse`.

Every response carries the same envelope: `id`, `provider`, `model`, `provider_job_id`, `usage`,
`cost_usd`, `latency_ms`, `created_at`, `metadata`, `raw`, plus `to_dict()` and a `from_dict()`
classmethod. `cost_usd` is `pages × per_page_usd` for the mode from the embedded price table (no
provider reports dollars directly), and `raw` is `None` unless the call passed `include_raw=True`.

### `ParseResponse`

```python
@dataclass
class ParseResponse:
    id: str
    provider: str
    model: str
    pages: list[Page]
    markdown: str
    text: str
    usage: Usage
    latency_ms: int
    created_at: str
    provider_job_id: Optional[str] = None
    cost_usd: Optional[float] = None
    metadata: dict[str, Any] = field(default_factory=dict)
    raw: Any = None
```

| Member | Description |
|---|---|
| `num_pages` (property) | `len(self.pages)`. |
| `blocks` (property) | Every block from every page, in reading order. |
| `tables` (property) | Blocks whose `type` is `"table"`. |
| `__str__` | Returns `markdown`. |

### `Page` and `Block`

```python
@dataclass
class Page:
    page_number: int
    markdown: str
    text: str
    blocks: list[Block] = field(default_factory=list)
    width: Optional[float] = None
    height: Optional[float] = None

@dataclass
class Block:
    type: BlockType
    content: str
    page_number: int
    text: Optional[str] = None
    bbox: Optional[BBox] = None
    confidence: Optional[float] = None
```

`Page.blocks_of(*types)` filters by block type; `Page.tables` is shorthand for
`blocks_of("table")`. `width` / `height` are `None` for providers that do not report page
dimensions (Reducto).

`BlockType` is a `Literal` of `"text"`, `"title"`, `"section_header"`, `"list"`, `"table"`,
`"figure"`, `"header"`, `"footer"`, `"footnote"`, `"caption"`, `"formula"`, `"other"`.

### `TextResponse`, `TextPage`, `Line`, `Word`

```python
@dataclass
class TextResponse:
    id: str
    provider: str
    model: str
    pages: list[TextPage]
    text: str
    usage: Usage
    latency_ms: int
    created_at: str
    provider_job_id: Optional[str] = None
    cost_usd: Optional[float] = None
    metadata: dict[str, Any] = field(default_factory=dict)
    raw: Any = None

@dataclass
class TextPage:
    page_number: int
    text: str
    lines: list[Line] = field(default_factory=list)
    words: list[Word] = field(default_factory=list)
    width: Optional[float] = None
    height: Optional[float] = None
```

`Line` and `Word` are both `{ text: str, bbox: Optional[BBox], confidence: Optional[float] }`.
`TextResponse.lines` and `.words` flatten across pages, `num_pages` is the page count, and
`__str__` returns `text`.

### `ExtractResponse`, `FieldInfo`, `Citation`

```python
@dataclass
class ExtractResponse:
    id: str
    provider: str
    model: str
    data: Any
    usage: Usage
    latency_ms: int
    created_at: str
    provider_job_id: Optional[str] = None
    fields: dict[str, FieldInfo] = field(default_factory=dict)
    cost_usd: Optional[float] = None
    metadata: dict[str, Any] = field(default_factory=dict)
    raw: Any = None
```

`fields` is keyed by **JSON pointer** into `data` (`"/invoice/total"`).
`field_info(pointer)` returns the `FieldInfo` or `None`; `citations(pointer)` returns its citation
list, or an empty list when the provider reported none. `__str__` returns `str(data)`.

```python
@dataclass
class FieldInfo:
    confidence: Optional[float] = None
    citations: list[Citation] = field(default_factory=list)

@dataclass
class Citation:
    page_number: int                 # 1-based
    bbox: Optional[BBox] = None
    text: Optional[str] = None       # source text the value was read from
```

### `BBox` and `Usage`

```python
@dataclass(frozen=True)
class BBox:
    x0: float
    y0: float
    x1: float
    y1: float

@dataclass
class Usage:
    pages: int = 0
    credits: Optional[float] = None
    provider_cost_usd: Optional[float] = None
```

Boxes are normalised to 0..1 relative to page size, origin top-left. `BBox.width` and `.height` are
properties; `to_pixels(page_width, page_height)` returns absolute `(x0, y0, x1, y1)`.

### `Metrics`

Returned by `liteocr.score`.

```python
@dataclass
class Metrics:
    char_similarity: float
    cer: float
    wer: float
    word_recall: float
    word_precision: float
    word_f1: float
    pred_chars: int
    truth_chars: int
    order_score: Optional[float] = None
    table_score: Optional[float] = None
```

## Exceptions

Every error raised by LiteOCR derives from `LiteOCRError`.

```
LiteOCRError                 kind = "error"
├── AuthenticationError      "authentication_error"    401/403, or no API key configured
├── RateLimitError           "rate_limit_error"        429 after retries were exhausted
├── BadRequestError          "bad_request_error"       other 4xx — the request itself is wrong
├── ProviderError            "provider_error"          5xx, malformed payload, failed job
├── TimeoutError             "timeout_error"           the whole-call deadline was exceeded
├── UnsupportedModelError    "unsupported_model_error" unknown model, or one that cannot serve the mode
├── InputError               "input_error"             unreadable input, bytes without filename
└── NetworkError             "network_error"           network / TLS / DNS failure after retries
```

Every instance carries:

| Attribute | Type | Description |
|---|---|---|
| `message` | `str` | The provider's message, never swallowed. |
| `kind` | `str` (class attribute) | Stable machine-readable kind, as above. |
| `provider` | `Optional[str]` | Provider the call was routed to. |
| `status_code` | `Optional[int]` | HTTP status when there was one. |
| `job_id` | `Optional[str]` | Provider job / run id when there was one. |
| `retryable` | `bool` | Whether a retry could plausibly succeed. |
| `to_dict()` | `dict` | All of the above as a plain dict. |

```python
try:
    resp = liteocr.parse("doc.pdf", model="reducto/standard")
except liteocr.RateLimitError as e:
    print(e.provider, e.status_code, e.retryable)
except liteocr.LiteOCRError as e:
    print(e.to_dict())
```

`liteocr.exceptions.from_core(exc)` converts a raw `liteocr._core.CoreError` into the typed
exception; the SDK applies it for you.

## Callbacks

Two module-level lists, called after every call in every mode, including `Router` calls. Callbacks
may be plain functions or coroutines; exceptions raised inside a callback are logged to the
`liteocr` logger and never propagate to the caller.

```python
liteocr.success_callback: list[Callable[[Response], None | Awaitable[None]]]
liteocr.failure_callback: list[Callable[[LiteOCRError], None | Awaitable[None]]]
```

```python
liteocr.success_callback.append(lambda r: print(r.model, r.usage.pages, r.cost_usd))
liteocr.failure_callback.append(lambda e: print("failed:", e))
```

## Models and pricing

```python
liteocr.modes() -> list[str]
liteocr.list_models(mode: Optional[Mode] = None) -> list[str]
liteocr.providers() -> list[dict[str, Any]]        # name, env var, base URL, docs, models + modes
liteocr.resolve_model(model: str, mode: Optional[Mode] = None) -> str
liteocr.pricing() -> dict[str, dict[str, Any]]     # model -> per-mode $/page, source, updated
liteocr.set_pricing(prices: dict[str, float], mode: Mode = "parse") -> None
liteocr.reset_pricing() -> None
liteocr.estimate_cost(model: str, pages: int, mode: Mode = "parse") -> Optional[float]
```

```python
liteocr.list_models("extract")                     # only models that serve extract
liteocr.resolve_model("reducto", "ocr")            # provider default *for that mode*
liteocr.set_pricing({"reducto/standard": 0.012})   # your negotiated parse rate
liteocr.estimate_cost("reducto/standard", 1000)    # 12.0
liteocr.reset_pricing()
```

`resolve_model` raises `UnsupportedModelError` for an unknown string, or for a model that does not
serve the requested mode — which makes it a cheap validator for user input. `estimate_cost` returns
`None` when a model has no published price for that mode.

## Scoring helpers

The benchmark metrics are exposed directly — deterministic, offline, no LLM judge.

```python
liteocr.score(
    prediction: str,
    truth: str,
    *,
    case_insensitive: bool = True,
    strip_markdown: bool = True,
    strip_punctuation: bool = False,
) -> Metrics

liteocr.normalize_text(text, *, case_insensitive=True, strip_markdown=True,
                       strip_punctuation=False) -> str

liteocr.markdown_to_text(markdown: str) -> str
```

```python
m = liteocr.score(resp.markdown, open("truth.md").read())
print(m.char_similarity, m.cer, m.wer, m.word_f1, m.order_score, m.table_score)
```

See [Benchmark](/benchmark/) for what each metric means.

## Logging

```python
liteocr.init_logging(level: str = "info") -> None
```

Enables the Rust core's `tracing` output on stderr; `"debug"` shows every HTTP step. The CLI uses
the `LITEOCR_LOG` environment variable for the same thing. Python-side messages (such as a raising
callback) go to the standard `logging` logger named `liteocr`.

## See also

- [Getting started](/getting-started/)
- [Specification](/project/spec/) — the unified request/response contract these types implement
- [Providers](/providers/) — per-provider mapping, supported modes and `provider_options`
