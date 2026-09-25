# Native-format compatibility (`output_format`)

PuffinParse normalises every provider to one response shape. That is the right default, but it is a
migration cost for anyone already integrated with a vendor: code that reads
`result.chunks[].blocks[].bbox.left` has to be rewritten before a single request can be re-pointed
at another provider.

The compatibility layer removes that step. Ask for a vendor's shape and PuffinParse renders the unified
response into that vendor's own JSON, **whatever provider actually produced it**:

```text
any provider → ParseResponse (unified) → render_parse(Format::Reducto) → Reducto's parse JSON
```

Switch the model string, keep your parser.

---

## 1. Using it

Rust:

```rust
use puffinparse_core::{parse, DocumentRequest};

let resp = parse(DocumentRequest::from_path("invoice.pdf").model("extend/parse_performance")).await?;
let reducto_shaped = resp.to_format("reducto")?;   // serde_json::Value
```

or explicitly:

```rust
use puffinparse_core::compat::{render_parse, Format};

let value = render_parse(&resp, Format::Reducto);
```

A request can carry the choice so the SDK/CLI can apply it for you:

```rust
let req = DocumentRequest::from_path("invoice.pdf").model("llamaparse/agentic").output_format("reducto");
req.validate_output_format()?;   // fails fast on a typo, before any provider call
```

Accepted names (case-insensitive, `-` and `_` interchangeable):

| Value | Shape |
|---|---|
| `puffinparse` (also `unified`), or unset | PuffinParse's own `ParseResponse` JSON |
| `reducto` | Reducto `POST /parse` response (`response_type: "parse"`) |
| `extend` | Extend `parse_run` object (`GET /parse_runs/{id}`) |
| `llamaparse` (also `llama`, `llama_parse`) | LlamaParse `…/result/json` payload |

`output_format` is **independent of `output`**: `output` picks markdown vs plain text *inside*
block content; `output_format` picks the JSON envelope around it.

The core entry points (`parse`, `ocr`, `extract`) always return the unified structs — rendering is
a pure function on the result, so nothing about routing, retries, pricing or fallbacks changes.

---

## 2. What is guaranteed

**Structural fidelity, not semantic identity.**

Guaranteed:

- the **key set and nesting** of the vendor's payload, at the envelope, chunk/page and block/item
  levels;
- **one chunk/page per unified page**, and one block/item per unified block, in reading order;
- **content strings** (`content` / `md`) byte-identical to what the unified response carries;
- **block types** drawn only from the vendor's own vocabulary (tables in §4);
- **coordinates in the vendor's units and convention** (§5);
- the **billed page count** (`usage.num_pages` / `metrics.pageCount` / `job_metadata.job_pages`).

Not guaranteed:

- byte equality with what the vendor would have returned for the same document — a different
  engine produced the text;
- fields PuffinParse does not model. They are rendered as `null`, `[]`, `{}` or a stable constant
  (§3), never invented;
- vendor-specific enrichments (chart data, figure crops, OCR word layers, layout add-ons,
  studio links, presigned URLs). These are always `null`/empty;
- confidence semantics. Every vendor scores differently; the number is whatever the *source*
  provider reported, re-expressed in the target vendor's field.

If your integration depends on a field listed as always-null below, the compatibility layer will
not carry you — use the unified shape, or `include_raw=True` to get the source provider's own
payload alongside.

---

## 3. Per-format field map

### Reducto (`output_format="reducto"`)

Rendered envelope: every top-level key of a real `POST /parse` response.

| Field | Value |
|---|---|
| `response_type` | `"parse"` |
| `job_id` | source provider's job id, else PuffinParse's response `id` |
| `duration` | `latency_ms / 1000` (whole PuffinParse call, including polling) |
| `usage.num_pages` / `usage.credits` | unified `usage.pages` / `usage.credits` (`credits` is `null` when the source reports none) |
| `result.type` | `"full"` (the URL variant is never emitted) |
| `result.chunks[]` | one per page, `chunk_mode="page"` semantics; `content` and `embed` are both the page markdown |
| `…blocks[].bbox` | `{left, top, width, height, page, original_page}`, already normalised 0..1 |
| `…blocks[].confidence` | `"high"` when confidence ≥ 0.8, `"low"` below, `null` when unknown |
| `…blocks[].granular_confidence.parse_confidence` | the numeric confidence, or `null` |
| **Always `null`** | `pdf_url`, `studio_link`, `parse_mode`, `document_properties`, `usage.credit_breakdown`, `usage.page_billing_breakdown`, `usage.non_empty_cell_count`, `result.ocr`, `result.custom`, chunk `enriched`, block `image_url`, `chart_data`, `extra`, `granular_confidence.extract_confidence` |
| **Always `false`** | chunk `enrichment_success` |

`original_page` is set equal to `page`: PuffinParse does not track pre-split page numbers.

### Extend (`output_format="extend"`)

| Field | Value |
|---|---|
| `object` | `"parse_run"` |
| `id` | source job id, else PuffinParse's response `id` |
| `status` | `"PROCESSED"` (a failure never reaches this code path — it is raised as an error) |
| `output.chunks[]` | one per page, `type: "page"`, ids `chunk_<page>` |
| `…blocks[]` | ids `block_<page>_<n>`, `details: {}` |
| `…blocks[].metadata.page` | `{number, width, height}` in page pixels (§5) |
| `…blocks[].boundingBox` | `{left, top, right, bottom}` in page pixels; `null` when the block has no box |
| `…blocks[].polygon` | the four corners of that box, clockwise from top-left; `[]` when there is no box |
| `…chunks[].metadata` | `pageRange {start, end}` (both the page number), plus `minOcrConfidence` / `avgOcrConfidence` over the page's blocks |
| `output.metadata.pages[]` | `{number, rotationApplied: 0, originalPageWidth, originalPageHeight, dpi: null}` |
| `metrics` | `{processingTimeMs: latency_ms, pageCount: usage.pages}` |
| `config` | `{target: "markdown", chunkingStrategy: {type: "page"}, engine: null}` |
| `usage` | `{credits, totalCredits: credits, breakdown: []}` |
| `metadata` | `null`, **or** `{"puffinparse_synthetic_page_dims": true}` when page sizes had to be synthesised (§5). The run-level `metadata` map is free-form in Extend's API, so this is a legal place to say so. |
| **Always `null`** | `file`, `failureReason`, `failureMessage`, `dataRetention`, `outputUrl`, `batchId`, `config.engine` |
| **Absent** | `output.ocr` (the word layer), `config.blockOptions`, `config.advancedOptions`, `block.details` contents |

### LlamaParse (`output_format="llamaparse"`)

| Field | Value |
|---|---|
| `pages[].page` / `text` / `md` | unified page number, text, markdown |
| `pages[].width` / `height` | page units (§5) |
| `pages[].items[]` | one per block: `{type, md, value, lvl, bBox, layoutAwareBbox: []}` |
| `…items[].type` | `heading` \| `text` \| `table` only |
| `…items[].lvl` | `1` for a title, `2` for a section header, `null` otherwise |
| `…items[].value` | the block's plain-text variant when the source provided one, else `null` |
| `…items[].bBox` | `{x, y, w, h, confidence}` in page units; `null` when the block has no box |
| `pages[].confidence` | mean of the page's block confidences, or `null` |
| `job_metadata` | `{credits_used, job_credits_usage, job_pages, job_auto_mode_triggered_pages: 0, job_is_cache_hit}` |
| **Always empty** | `images`, `charts`, `links`, `layoutAwareBbox` |
| **Always constant** | `status: "OK"`, `triggeredAutoMode: false`, `noStructuredContent: false`, `noTextContent: false`, `pageHeaderMarkdown`/`pageFooterMarkdown`/`printedPageNumber`: `""`, `parsingMode: null`, `structuredData: null` |
| **Absent** | page: `originalOrientationAngle`, `layout`, `costOptimized`, `slideSpeakerNotes`, `slideSectionName` (add-ons PuffinParse does not model); table items: `csv`, `html`, `rows`, `isPerfectTable` (restatements of the markdown table already in `md`) |

Figures have no LlamaParse item type (LlamaParse puts them in `images[]`/`charts[]`, which we
cannot synthesise), so a figure block is emitted as a `text` item carrying its markdown rather than
being dropped.

---

## 4. Block-type mapping

Unified → native. Cells marked **lossy** have no exact counterpart in that vendor's vocabulary.

| Unified | Reducto | Extend | LlamaParse |
|---|---|---|---|
| `title` | `Title` | `heading` | `heading` (`lvl: 1`) |
| `section_header` | `Section Header` | `section_heading` | `heading` (`lvl: 2`) |
| `text` | `Text` | `text` | `text` |
| `list` | `List Item` | `text` **lossy** | `text` **lossy** |
| `table` | `Table` | `table` | `table` |
| `figure` | `Figure` | `figure` | `text` **lossy** |
| `header` | `Header` | `header` | `text` **lossy** |
| `footer` | `Footer` | `footer` | `text` **lossy** |
| `footnote` | `Text` **lossy** | `text` **lossy** | `text` **lossy** |
| `caption` | `Text` **lossy** | `text` **lossy** | `text` **lossy** |
| `formula` | `Text` **lossy** | `formula` | `text` **lossy** |
| `other` | `Text` **lossy** | `text` **lossy** | `text` **lossy** |

Two consequences worth stating plainly:

- **The mapping is not injective, so it is not invertible.** Extend's `key_value` normalises to
  `text` and comes back as `text`, not `key_value`. Round-trip tests therefore compare types *up
  to the provider's own forward mapping* (§7).
- **Reducto's render deliberately uses a reduced vocabulary.** It emits only
  `Title`, `Section Header`, `Text`, `List Item`, `Table`, `Figure`, `Header`, `Footer`. Reducto
  itself also emits `Footnote`, `Caption`, `Formula`, `Page Number` and others
  (`docs/providers/reducto.md` §4); a Reducto → Reducto round trip over a document containing
  those blocks will see them arrive as `Text`. Widening the reverse map is a one-line change in
  `crates/puffinparse-core/src/compat/reducto.rs::block_type` if that fidelity is wanted.

---

## 5. Coordinates

The unified `BBox` is `{x0, y0, x1, y1}` normalised to 0..1 with a top-left origin (ADR-3). Each
renderer converts to the vendor's convention:

| Format | Units | Conversion |
|---|---|---|
| Reducto | normalised 0..1, `left/top/width/height` | `left = x0`, `top = y0`, `width = x1 - x0`, `height = y1 - y0` — no page size needed |
| Extend | page pixels, `left/top/right/bottom` | multiply by the page's width/height |
| LlamaParse | page units, `x/y/w/h` | multiply by the page's width/height |

**The synthetic-page rule.** Extend and LlamaParse express boxes in page units, so a page size is
required. When the unified response carries `Page.width`/`height` (Extend and LlamaParse report
them; Reducto and the vision-LLM providers do not) those are used verbatim. When it does not, the
renderer assumes a **1000 × 1000** page. That keeps the numbers readable and makes the original
normalised coordinates exactly recoverable by dividing by 1000 — but they are not real page
dimensions, and the Extend render says so via `metadata.puffinparse_synthetic_page_dims = true`.
The constant is `puffinparse_core::compat::SYNTHETIC_PAGE_DIM`.

A block with no box at all renders as `{left: 0, top: 0, width: 0, height: 0, page, original_page}`
for Reducto (whose `bbox` is not nullable in practice) and as `null` for Extend (`boundingBox`)
and LlamaParse (`bBox`), matching each vendor's own optionality.

---

## 6. Migration examples

### Extend → Reducto

You read Reducto's shape today and want Extend's engine.

```python
# before
resp = reducto_client.parse.run(document_url=url)
for chunk in resp.result.chunks:
    for block in chunk.blocks:
        draw(block.bbox.left, block.bbox.top, block.type)

# after — same parsing code, Extend doing the work
doc = puffinparse.parse(url, model="extend/parse_performance", output_format="reducto")
for chunk in doc["result"]["chunks"]:
    for block in chunk["blocks"]:
        draw(block["bbox"]["left"], block["bbox"]["top"], block["type"])
```

What changes: `pdf_url`, `studio_link` and the billing breakdowns are `null`; Extend's `key_value`
blocks arrive as `Text`; boxes are Extend's pixel boxes divided by the page size, so they line up
with the same page image.

### Reducto → Extend

```python
doc = puffinparse.parse(path, model="reducto/standard", output_format="extend")
assert doc["object"] == "parse_run" and doc["status"] == "PROCESSED"
for chunk in doc["output"]["chunks"]:
    text = chunk["content"]
    page = chunk["metadata"]["pageRange"]["start"]
```

What changes: Reducto reports no page dimensions, so the render uses a 1000 × 1000 page and sets
`metadata.puffinparse_synthetic_page_dims = true`. `boundingBox` values are therefore *relative*
coordinates × 1000, not PDF points. If you overlay boxes on a rendered page image, scale by
`your_image_size / 1000` — or scale by `boundingBox / page.width`, which is correct in both cases
and is the recommended form.

### LlamaParse → Reducto

```python
doc = puffinparse.parse(path, model="llamaparse/agentic", output_format="reducto")
pages = {b["bbox"]["page"] for c in doc["result"]["chunks"] for b in c["blocks"]}
```

What changes: LlamaParse items become Reducto blocks (`heading` + `lvl:1` → `Title`, `lvl:2` →
`Section Header`, `table` → `Table`); boxes are divided by LlamaParse's real page size, so they
land in Reducto's normalised 0..1 space; `job_metadata.credits_used` becomes `usage.credits`
(`null` when LlamaParse has not settled billing yet).

---

## 7. Extract mode (best effort)

`ExtractResponse::to_format(...)` / `compat::render_extract(...)` render the extract envelope of
each vendor. This is explicitly **weaker** than the parse renderers: extract surfaces differ far
more between vendors (schemas, citation objects, per-field metadata placement), and there are no
captured extract fixtures for all three providers yet, so only the documented envelope and the
citation/confidence placement are reproduced.

| Format | Envelope |
|---|---|
| `reducto` | `{response_type: "v3_extract", job_id, usage: {num_pages, num_fields, credits}, result, studio_link: null}` — `result` rebuilds Reducto's `{value, citations}` leaf wrappers from the unified pointer-keyed `fields`, the exact inverse of the unwrapping done when normalising |
| `extend` | `{object: "extract_run", id, status: "PROCESSED", output: {value, metadata: {<field>: {confidence, citations}}}, usage}` |
| `llamaparse` | `{data, extraction_metadata: {field_metadata: {<field>: {confidence, citations}}, job_id}}` (LlamaExtract) |

Caveat: an extract response carries no page dimensions, so citation boxes stay in the **unified
normalised 0..1 space** for the Extend and LlamaExtract renders instead of being scaled to page
units. Field keys come from the unified JSON pointers (`/invoice/total`) with the leading slash
stripped; LlamaExtract uses dots (`invoice.total`).

---

## 8. How this is validated

`crates/puffinparse-core/src/compat/roundtrip.rs` runs, for each of the three providers:

```text
real fixture → provider::normalize → ParseResponse → render_parse(same format) → compare
```

The comparison is a `skeleton_diff` that reports the first mismatch with a path
(`unit[1].block[0].content differs: …`) and checks:

1. top-level key set, exactly;
2. chunk/page and block/item key sets (no key the fixture has may be missing, except the
   documented absences in §3);
3. billed page count;
4. chunk/page count, then block/item count per unit;
5. `content` / `md` strings, trimmed;
6. block types, compared after mapping both sides through the provider's own forward map
   (so `key_value` → `text` counts as faithful);
7. boxes, converted back to the unified 0..1 space and compared within `1e-6`.

Relaxations, and why:

- **Trimmed string comparison.** `providers::llamaparse::normalize` trims page markdown, so the
  fixture's trailing newlines do not survive. Content is compared trimmed.
- **Block types up to the forward map.** The mapping is not injective (§4).
- **`originalPageWidth` is not compared.** Extend reports both a PDF-point page size in
  `output.metadata.pages[]` (596 × 842) and a rasterised pixel size in
  `blocks[].metadata.page` (1241 × 1754). The unified `Page` keeps the one the boxes are in
  (pixels), so the render emits that in both places.
- **Cross-format tests do not compare block types**, since vendors share no vocabulary.

Alongside those, the suite asserts that every render **deserialises with the target provider's own
wire types** (`WireParseResponse`, `ParseRun`, `JsonResult`), that re-normalising a render and
rendering again is a fixed point, that every `Format::ALL` renders a response whose pages have no
dimensions and whose blocks have no boxes without panicking, and that `skeleton_diff` itself
detects each class of mismatch it claims to.
