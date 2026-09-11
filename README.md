# LiteOCR

**One API for every OCR / document-parsing provider.** Rust core, Python SDK, CLI, and an open benchmark that ranks providers on accuracy, latency and cost.

```python
import liteocr

doc = liteocr.parse("invoice.pdf", model="reducto/standard")     # or "extend/parse_performance", "llamaparse/agentic", ...
print(doc.markdown)                                              # unified markdown, every provider
print(doc.pages[0].blocks[0].bbox, doc.usage.pages, doc.cost_usd)

text = liteocr.ocr("scan.png", model="llamaparse/fast")          # plain text + line/word boxes
print(text.text, text.pages[0].lines[0].bbox)
```

Switch providers by changing one string. Same request, same response shape, same errors.

[![CI](https://github.com/ajinkyashejul/liteocr/actions/workflows/ci.yml/badge.svg)](https://github.com/ajinkyashejul/liteocr/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Python 3.9+](https://img.shields.io/badge/python-3.9%2B-blue)](pyproject.toml)

## Why

Every document-parsing vendor has its own upload flow, polling loop, JSON layout, block vocabulary, coordinate system and billing unit. LiteOCR hides all of that behind one call, tracks cost per call, retries and falls back across providers, and ships a reproducible benchmark so you can pick a provider on evidence instead of marketing.

Two things make switching real rather than aspirational. **Modes**: every call names `parse`, `ocr` or `extract`, models declare the modes they serve, and a model that cannot serve the one you asked for fails before any network call — so a provider swap can never quietly change the shape of your answer. **Native-format compatibility**: if you are already integrated with Reducto, Extend or LlamaParse, `output_format="reducto"` (and friends) renders *any* provider's result into that vendor's own JSON, so you can re-point a request without touching your parsing code — [see below](#keep-your-reducto--extend--llamaparse-code).

| | |
|---|---|
| **Providers (v0.1)** | 15 providers · 57 models · 3 modes. [Reducto](https://reducto.ai), [Extend](https://extend.ai) and [LlamaParse](https://cloud.llamaindex.ai) are live-verified; 12 more (Mistral, Azure, Textract, Gemini, OpenAI, Anthropic, Mathpix, Datalab, Unstructured, Upstage, Landing AI, Google Document AI) ship from their API references — [full table](#model-names) |
| **Modes** | `parse` (markdown + blocks), `ocr` (plain text + boxes), `extract` (JSON from a schema) |
| **Core** | Rust (`liteocr-core`): `reqwest` + `tokio`, no vendor SDKs, `#![forbid(unsafe_code)]` |
| **SDK** | Python 3.9+ (`pip install liteocr`), sync + async, fully typed |
| **CLI** | `liteocr parse`, `liteocr ocr`, `liteocr extract`, `liteocr providers`, `liteocr bench` |
| **Reliability** | Retries with jittered backoff, whole-call deadlines, `Router` with ordered fallbacks / round-robin |
| **Compatibility** | `output_format` renders any provider's result in Reducto's, Extend's or LlamaParse's own JSON, so an existing integration keeps its parser ([`docs/COMPAT.md`](docs/COMPAT.md)) |
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

Every provider reads its own variable: [`.env.example`](.env.example) lists all of them, the
[model tables](#model-names) say which belongs to which provider, and `liteocr providers` shows
which ones are set in your shell.

From source (Rust stable + Python 3.9+):

```bash
git clone https://github.com/ajinkyashejul/liteocr && cd liteocr
python -m venv .venv && . .venv/bin/activate
pip install maturin && maturin develop --release     # builds the extension into the venv
cargo build --release -p liteocr-cli                 # ./target/release/liteocr
```

## Modes

Document AI vendors sell three different products, and they are not interchangeable: layout
**parsing**, plain-text **OCR**, and schema-driven **extraction**. A call picks one mode, and the
mode decides the response type:

| Mode | Call | Returns | Use it for |
|---|---|---|---|
| `parse` | `liteocr.parse(...)` | `ParseResponse` — `markdown`, `pages[].blocks[]` with types and boxes | RAG chunks, tables, document structure |
| `ocr` | `liteocr.ocr(...)` | `TextResponse` — `text`, `pages[].lines[]` / `words[]` with boxes | search indexes, redaction, overlays |
| `extract` | `liteocr.extract(..., schema)` | `ExtractResponse` — `data` shaped by your JSON Schema, plus per-field confidence and citations | invoices, forms, anything with fields |

**Providers are swappable only within a mode.** Every model declares the modes it serves, so a
model that cannot do what you asked raises `UnsupportedModelError` *before* any network call
instead of silently returning the wrong shape. `liteocr.list_models("ocr")` lists the candidates
for a mode; `liteocr providers --mode ocr` does the same on the command line. Providers without a
native OCR endpoint serve `ocr` from their parse output, flagged as
`resp.metadata["liteocr_derived_from"] == "parse"`.

> Which models serve which mode is a registry fact, not a guess — ask
> `liteocr.list_models("extract")` or `liteocr providers --mode extract`. Most providers serve
> `parse` and `ocr`; `extract` needs a model built for it (`reducto/extract`,
> `extend/extraction_light`, `azure/invoice`, the vision-LLM models, ...), and calling it with a
> parse-only model raises `UnsupportedModelError` naming the mode.

## Usage

### One call, any provider

```python
import liteocr

# path, URL, or bytes (+ filename)
doc = liteocr.parse("contract.pdf", model="extend/parse_performance")
doc = liteocr.parse("https://cdn.reducto.ai/samples/fidelity-example.pdf", model="reducto/r-1", pages="1-2")
doc = liteocr.parse(open("scan.png", "rb").read(), filename="scan.png", model="llamaparse/agentic")

doc.markdown            # whole document
doc.text                # plain text
doc.pages[0].markdown   # per page
for block in doc.pages[0].blocks:
    block.type           # text | title | section_header | list | table | figure | header | footer | ...
    block.content        # markdown
    block.bbox           # BBox(x0, y0, x1, y1) normalised 0..1, origin top-left (or None)
    block.confidence     # 0..1 when the provider reports one
doc.usage.pages, doc.usage.credits, doc.cost_usd, doc.latency_ms
```

### Plain text and boxes (`ocr`)

```python
text = liteocr.ocr("scan.png", model="reducto/standard")

text.text                     # whole document, pages joined by a blank line
page = text.pages[0]
page.text                     # plain text in reading order
for line in page.lines:       # Line(text, bbox, confidence)
    x0, y0, x1, y1 = line.bbox.to_pixels(page.width, page.height)
for word in page.words:       # Word(text, bbox, confidence)
    ...
```

### Structured extraction (`extract`)

```python
schema = {
    "type": "object",
    "properties": {"invoice_number": {"type": "string"}, "total": {"type": "number"}},
    "required": ["invoice_number", "total"],
}
result = liteocr.extract("invoice.pdf", schema, model="...", citations=True)

result.data                          # {"invoice_number": "INV-42", "total": 1280.5}
result.fields["/total"].confidence   # per-field confidence, keyed by JSON pointer
result.citations("/total")           # [Citation(page_number=2, bbox=..., text="Total due 1,280.50")]
```

### Async

```python
doc = await liteocr.aparse("contract.pdf", model="llamaparse/cost_effective")
text = await liteocr.aocr("scan.png", model="llamaparse/fast")
```

Runs on the Rust runtime; the event loop is never blocked.

### Model names

`"<provider>/<model>"`, like LiteLLM. A bare provider name picks that provider's default model
**for the mode you called** — marked `*` below. `liteocr providers` and
`liteocr.list_models(mode)` print the live list; the tables here are the built-in registry
(`crates/liteocr-core/src/model.rs` and `pricing.json`).

Prices are public pay-as-you-go **list prices, per page, per mode**, shown as
`parse · ocr · extract` with `—` where a model does not serve that mode. Override one mode at a
time with `liteocr.set_pricing({"reducto/standard": 0.012}, "parse")`, and estimate with
`liteocr.estimate_cost("reducto/standard", pages=1000, mode="ocr")`. The vision-LLM providers
(Gemini, OpenAI, Anthropic) bill tokens rather than pages, so their per-page numbers are
**estimates** — see the source lines in `pricing.json`.

**live-verified** means the provider's live tests have passed against the real API with a key.
**docs-only** means it was implemented from the official API reference with fixture-backed tests
and is waiting for a key to be promoted; `docs/providers/README.md` tracks the state and links one
reference page per provider.

**Reducto** · `REDUCTO_API_KEY` · live-verified — Layout parsing plus a schema extractor with citations; the default parse target.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `reducto/standard` | parse `*`, ocr `*` | $0.015 · $0.015 · — | Reducto Parse with account-default model (legacy standard) |
| `reducto/r-1` | parse, ocr | $0.01 · $0.01 · — | Reducto Parse with settings.model=r-1 (newest model, cheaper) |
| `reducto/agentic` | parse, ocr | $0.03 · $0.03 · — | Reducto Parse with agentic text+table enhancement (highest accuracy, 2x cost) |
| `reducto/extract` | extract `*` | — · — · $0.035 | Reducto Extract: POST /extract with a JSON schema, citations on request |
| `reducto/deep_extract` | extract | — · — · $0.055 | Reducto Deep Extract (settings.deep_extract = true) for long or complex documents |

**Extend** · `EXTEND_API_KEY` · live-verified — Parse engines that trade accuracy for cost per page, and two extraction processors.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `extend/parse_performance` | parse `*`, ocr `*` | $0.025 · $0.025 · — | Extend engine=parse_performance (highest accuracy) |
| `extend/parse_light` | parse, ocr | $0.00625 · $0.00625 · — | Extend engine=parse_light (fast, cheap, digital-native docs) |
| `extend/parse_auto` | parse, ocr | $0.025 · $0.025 · — | Extend engine=parse_auto (picks light or performance per page) |
| `extend/extraction_performance` | extract `*` | — · — · $0.0625 | Extend Extract, baseProcessor=extraction_performance (runs parse_performance) |
| `extend/extraction_light` | extract | — · — · $0.015 | Extend Extract, baseProcessor=extraction_light (runs parse_light) |

**LlamaParse (LlamaCloud)** · `LLAMA_API_KEY` · live-verified — Credit-priced tiers from plain text extraction to agentic parsing; extract from `cost_effective` up.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `llamaparse/fast` | parse, ocr | $0.00125 · $0.00125 · — | LlamaParse tier=fast (text extraction, no OCR of images) |
| `llamaparse/cost_effective` | parse `*`, ocr `*`, extract `*` | $0.00375 · $0.00375 · $0.01 | LlamaParse tier=cost_effective |
| `llamaparse/agentic` | parse, ocr, extract | $0.0125 · $0.0125 · $0.03125 | LlamaParse tier=agentic |
| `llamaparse/agentic_plus` | parse, ocr, extract | $0.05625 · $0.05625 · $0.11875 | LlamaParse tier=agentic_plus (highest accuracy) |

**Mistral Document AI** · `MISTRAL_API_KEY` · docs-only — One OCR model with pinned versions; annotations give it an extract mode.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `mistral/ocr-latest` | parse `*`, ocr `*`, extract `*` | $0.004 · $0.004 · $0.005 | Mistral OCR, latest alias (mistral-ocr-latest; currently OCR 4.1) |
| `mistral/ocr-4-1` | parse, ocr, extract | $0.004 · $0.004 · $0.005 | Mistral OCR 4.1 pinned (mistral-ocr-4-1; blocks + block confidence scores) |
| `mistral/ocr-4-0` | parse, ocr, extract | $0.004 · $0.004 · $0.005 | Mistral OCR 4.0 pinned (mistral-ocr-4-0; paragraph blocks, no block confidence) |
| `mistral/ocr-2512` | parse, ocr, extract | $0.002 · $0.002 · $0.003 | Mistral OCR 3 pinned (mistral-ocr-2512; cheaper, no paragraph blocks) |

**Azure AI Document Intelligence** · `AZURE_DOCUMENT_INTELLIGENCE_KEY` (+ `AZURE_DOCUMENT_INTELLIGENCE_ENDPOINT`) · docs-only — Prebuilt Document Intelligence models: `read` for OCR, `layout` for structure, fixed schemas for extraction.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `azure/read` | ocr `*` | — · $0.0015 · — | Azure prebuilt-read: native OCR, words/lines with confidence (cheapest) |
| `azure/layout` | parse `*`, ocr | $0.01 · $0.01 · — | Azure prebuilt-layout: markdown, paragraphs with roles, tables, polygons |
| `azure/invoice` | extract `*` | — · — · $0.01 | Azure prebuilt-invoice: fixed invoice schema |
| `azure/receipt` | extract | — · — · $0.01 | Azure prebuilt-receipt: fixed receipt schema |
| `azure/id_document` | extract | — · — · $0.01 | Azure prebuilt-idDocument: fixed ID document schema |
| `azure/tax_us_w2` | extract | — · — · $0.01 | Azure prebuilt-tax.us.w2: fixed W-2 schema |
| `azure/custom` | parse, ocr, extract | $0.03 · $0.03 · $0.03 | Azure custom model, id via provider_options.model_id |

**AWS Textract** · `AWS_ACCESS_KEY_ID` (+ `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`) · docs-only — One AWS API, four feature sets; SigV4-signed, no SDK.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `textract/detect-text` | ocr `*` | — · $0.0015 · — | Textract DetectDocumentText: raw OCR, lines + words with boxes (cheapest) |
| `textract/layout` | parse `*`, ocr | $0.015 · $0.015 · — | Textract AnalyzeDocument LAYOUT + TABLES: reading-order markdown + tables |
| `textract/queries` | extract `*` | — · — · $0.015 | Textract AnalyzeDocument QUERIES: one natural-language query per schema field |
| `textract/forms` | extract | — · — · $0.05 | Textract AnalyzeDocument FORMS: key-value pairs matched to schema fields |

**Google Gemini** · `GEMINI_API_KEY` · docs-only — Vision-LLM transcription. Prices are per-page **estimates** (~1500 in + ~800 out tokens); `cost_usd` uses them until the response reports real usage.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `gemini/2.5-flash` | parse `*`, ocr `*`, extract `*` | $0.00245 · $0.00245 · $0.0012 | Gemini 2.5 Flash: vision-LLM transcription, the price/quality default |
| `gemini/2.5-pro` | parse, ocr, extract | $0.009875 · $0.009875 · $0.004875 | Gemini 2.5 Pro: highest accuracy, ~4x the cost of Flash |
| `gemini/2.5-flash-lite` | parse, ocr, extract | $0.00047 · $0.00047 · $0.00027 | Gemini 2.5 Flash-Lite: cheapest and fastest, clean documents |
| `gemini/3.5-flash` | parse, ocr, extract | $0.00945 · $0.00945 · $0.00495 | Gemini 3.5 Flash: frontier Flash generation (GA 2026-05-19) |
| `gemini/3.5-flash-lite` | parse, ocr, extract | $0.00245 · $0.00245 · $0.0012 | Gemini 3.5 Flash-Lite: low-latency 3.x tier (GA 2026-07-21) |
| `gemini/3.8-flash` | parse, ocr, extract | $0.004125 · $0.004125 · $0.00225 | Gemini 3.8 Flash: newest Flash model (GA 2026-09-02, introductory pricing) |

**OpenAI** · `OPENAI_API_KEY` · docs-only — Vision transcription through the Responses API. Per-page prices are estimates, as for Gemini.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `openai/gpt-5.6-luna` | parse `*`, ocr `*`, extract `*` | $0.00114 · $0.00114 · $0.00114 | OpenAI Responses API, gpt-5.6-luna (cheapest current vision model) |
| `openai/gpt-5.6-terra` | parse, ocr, extract | $0.0114 · $0.0114 · $0.0114 | OpenAI Responses API, gpt-5.6-terra (balanced capability/price) |
| `openai/gpt-5.6-sol` | parse, ocr, extract | $0.02 · $0.02 · $0.02 | OpenAI Responses API, gpt-5.6-sol (flagship GPT-5.6) |
| `openai/gpt-6-astra` | parse, ocr, extract | $0.05 · $0.05 · $0.05 | OpenAI Responses API, gpt-6-astra (most capable, most expensive) |

**Anthropic (Claude)** · `ANTHROPIC_API_KEY` · docs-only — Vision transcription through the Messages API. Per-page prices are estimates.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `anthropic/claude-sonnet-5` | parse `*`, ocr `*`, extract `*` | $0.01 · $0.01 · $0.01 | Claude Messages API, claude-sonnet-5 (balanced vision transcription) |
| `anthropic/claude-haiku-4-5` | parse, ocr, extract | $0.005 · $0.005 · $0.005 | Claude Messages API, claude-haiku-4-5 (cheapest, 200K context) |
| `anthropic/claude-opus-5` | parse, ocr, extract | $0.025 · $0.025 · $0.025 | Claude Messages API, claude-opus-5 (highest accuracy) |

**Mathpix** · `MATHPIX_APP_KEY` (+ `MATHPIX_APP_ID`) · docs-only — Maths-first OCR: Mathpix Markdown with LaTeX, line and word polygons.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `mathpix/pdf` | parse `*`, ocr `*` | $0.005 · $0.005 · — | Mathpix v3/pdf document OCR (image inputs auto-routed to v3/text); MMD + line polygons |
| `mathpix/text` | parse, ocr | $0.002 · $0.002 · — | Mathpix v3/text single-image OCR (line + word polygons, per-image billing) |

**Datalab (Marker)** · `DATALAB_API_KEY` · docs-only — Marker's hosted API: three quality modes at two price points.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `datalab/fast` | parse, ocr | $0.004 · $0.004 · — | Datalab Convert mode=fast (lowest latency, digital-native documents) |
| `datalab/balanced` | parse `*`, ocr `*` | $0.004 · $0.004 · — | Datalab Convert mode=balanced (Datalab's recommended default) |
| `datalab/accurate` | parse, ocr | $0.01 · $0.01 · — | Datalab Convert mode=accurate (scans, dense layouts, complex tables) |

**Unstructured** · `UNSTRUCTURED_API_KEY` · docs-only — The partitioner behind many RAG pipelines; flat per-page price across strategies.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `unstructured/hi_res` | parse `*`, ocr `*` | $0.015 · $0.015 · — | Unstructured strategy=hi_res (layout model + OCR; coordinates, table HTML, confidence) |
| `unstructured/fast` | parse, ocr | $0.015 · $0.015 · — | Unstructured strategy=fast (text-layer extraction, no OCR, rejects images) |
| `unstructured/auto` | parse, ocr | $0.015 · $0.015 · — | Unstructured strategy=auto (routes each page to fast / hi_res / VLM) |

**Upstage Document Parse** · `UPSTAGE_API_KEY` · docs-only — Korean/English layout parsing to HTML or markdown.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `upstage/document-parse` | parse `*`, ocr `*` | $0.01 · $0.01 · — | Upstage Document Parse (layout to HTML/Markdown; 100 pages sync, 1000 async) |
| `upstage/document-parse-nightly` | parse, ocr | $0.01 · $0.01 · — | Upstage Document Parse nightly build (newest layout model, may change without notice) |

**Landing AI (Agentic Document Extraction)** · `LANDINGAI_API_KEY` · docs-only — Agentic Document Extraction (DPT-2): parse, then extract against your schema.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `landingai/dpt-2` | parse `*`, ocr `*`, extract `*` | $0.03 · $0.03 · $0.04 | Landing AI ADE DPT-2; extract runs parse then /v1/ade/extract with the JSON schema |

**Google Cloud Document AI** · `GOOGLE_DOCUMENTAI_ACCESS_TOKEN` (+ `GOOGLE_DOCUMENTAI_PROJECT`, `_LOCATION`, `_PROCESSOR_ID`) · docs-only — Processor-based: pick the processor id in `provider_options`; OCR, layout, forms and prebuilt extractors.

| Model | Modes | List price / page (parse · ocr · extract) | Notes |
|---|---|---|---|
| `google_documentai/ocr` | parse `*`, ocr `*` | $0.0015 · $0.0015 · — | Document OCR processor: native text + word boxes (processor id from config) |
| `google_documentai/layout` | parse, ocr | $0.01 · $0.01 · — | Layout Parser processor: documentLayout blocks (headings, tables, lists) |
| `google_documentai/form` | parse, ocr, extract `*` | $0.03 · $0.03 · $0.03 | Form Parser processor: paragraphs + tables, entities as extraction |
| `google_documentai/prebuilt` | parse, ocr, extract | $0.03 · $0.03 · $0.03 | Prebuilt or custom extractor (invoice, W2, ...): entities as extraction |

### Keep your Reducto / Extend / LlamaParse code

Already integrated with a vendor? Ask for its shape and LiteOCR renders the response into that
vendor's own JSON, whatever provider actually ran the call — switch the model string, keep your
parser:

```python
doc = liteocr.parse("invoice.pdf", model="extend/parse_light", output_format="reducto")
block = doc["result"]["chunks"][0]["blocks"][0]            # Reducto's shape, Extend's engine
draw(block["bbox"]["left"], block["bbox"]["top"], block["type"])
```

`output_format` accepts `reducto`, `extend`, `llamaparse`, or `liteocr` (the unified shape, and
the default). It returns a plain `dict` instead of a dataclass, and works on `parse`, `extract`,
their async variants, the `Router`, and the CLI (`--output-format <vendor>` with `--format json`).
An unknown name raises `BadRequestError` before any network call.

What is guaranteed is **structural fidelity** — key set and nesting, one chunk/page per page, the
content strings, the vendor's own block vocabulary and coordinate units, the billed page count —
not byte equality with what the vendor would have returned. Fields LiteOCR does not model
(presigned URLs, studio links, billing breakdowns, OCR word layers) are `null` or empty, and a few
block types are lossy. [`docs/COMPAT.md`](docs/COMPAT.md) enumerates all of it, per format;
`examples/switch_provider_keep_format.py` is a runnable version of the above.

### Provider-specific options

Anything the common request doesn't cover is passed through verbatim:

```python
liteocr.parse("doc.pdf", model="reducto/standard",
              provider_options={"settings": {"return_ocr_data": True}, "async": True})
liteocr.parse("doc.pdf", model="extend/parse_performance",
              provider_options={"blockOptions": {"figures": {"enabled": False}}})
liteocr.ocr("doc.pdf", model="llamaparse/agentic",
            provider_options={"take_screenshot": True, "version": "2026-08-19"})
```

`include_raw=True` attaches the provider's original payload as `resp.raw`.

### Router: fallbacks and load balancing

```python
router = liteocr.Router(
    ["reducto/standard", "llamaparse/agentic", "extend/parse_light"],
    mode="parse",                # the mode every model must serve; "ocr" and "extract" too
    strategy="ordered",          # or "round_robin"
)
resp = router.parse("doc.pdf")   # falls back on provider / rate-limit / timeout / network errors
router.stats()                   # per-model successes, failures, latency, cost, pages
```

A router is bound to one mode: `Router([...], mode="parse").ocr(...)` raises `InputError` rather
than quietly changing the answer's shape, and every model is validated against the mode when the
router is built. Auth, bad-request and input errors never trigger a fallback.

### Errors

All errors derive from `liteocr.LiteOCRError` and carry `provider`, `status_code`, `job_id` and `retryable`:

`AuthenticationError`, `RateLimitError`, `BadRequestError`, `ProviderError`, `TimeoutError`, `UnsupportedModelError`, `InputError`, `NetworkError`.

`UnsupportedModelError` also covers a model asked to do a mode it does not serve; the message
names the mode and the models that do serve it.

### Callbacks and logging

Callbacks fire for every mode, with that mode's response object:

```python
liteocr.success_callback.append(lambda r: print(r.model, r.usage.pages, r.cost_usd))
liteocr.failure_callback.append(lambda e: print("failed:", e))
liteocr.init_logging("debug")    # Rust core tracing on stderr (or LITEOCR_LOG=debug for the CLI)
```

### CLI

```bash
liteocr providers                                             # models, modes, prices, key status
liteocr providers --mode ocr                                  # only models that serve a mode
liteocr parse invoice.pdf -m extend/parse_light               # markdown to stdout
liteocr parse scan.png -m llamaparse/agentic -f json --raw    # full unified JSON (+ provider payload)
liteocr parse doc.pdf -m extend/parse_light -f json \
    --output-format reducto                                   # ... in Reducto's response shape
liteocr parse doc.pdf -m reducto/r-1 --pages 1-3 -f text
liteocr ocr scan.png -m reducto/standard                      # plain text to stdout
liteocr ocr scan.png -f json                                  # TextResponse: text + lines + words
liteocr extract invoice.pdf -s schema.json --citations        # JSON object from a schema
liteocr extract invoice.pdf -s '{"type":"object"}'            # inline schema also works
liteocr extract invoice.pdf -s schema.json --output-format extend   # Extend's extract_run shape
liteocr providers --json | jq '.output_formats'               # the vendor shapes this build renders
```

### Rust

```rust
use liteocr_core::{extract, ocr, parse, DocumentRequest, ExtractRequest};

let doc = parse(DocumentRequest::from_path("invoice.pdf").model("reducto/standard")).await?;
println!("{} pages, ${:.4}\n{}", doc.usage.pages, doc.cost_usd.unwrap_or(0.0), doc.markdown);

let text = ocr(DocumentRequest::from_path("scan.png").model("llamaparse/fast")).await?;
println!("{} lines on page 1", text.pages[0].lines.len());

let req = ExtractRequest::new(DocumentRequest::from_path("invoice.pdf").model("..."), schema);
let data = extract(req.citations(true)).await?.data;
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
- **Meta-benchmark.** Every vendor publishes a benchmark it wins (LlamaParse's ParseBench,
  Extend's RealDocBench, Reducto's LongExtractBench). LiteOCR will ship adapters that convert
  each open benchmark, plus olmOCR-bench and OmniDocBench, into the manifest format and run them
  all through the same harness, so one neutral leaderboard covers every provider on every
  public dataset, with per-dataset and combined scores.
- LLM-judge as an optional plug-in for metrics that need it (e.g. figure descriptions).
- Webhooks instead of polling for async providers.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Adding a provider is one Rust file plus a fixture test; the checklist is in the [new provider issue template](.github/ISSUE_TEMPLATE/new_provider.md).

## License

MIT. See [LICENSE](LICENSE).
