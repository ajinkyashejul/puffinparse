# Landing AI — Agentic Document Extraction (ADE)

> **Status: docs-only.** Implemented from Landing AI's published API documentation and tested
> against fixture payloads built from it. It has not yet been run against the live API, so
> expect wire-format differences. Help verify it: [issue #10](https://github.com/ajinkyashejul/puffinparse/issues/10).

## 1. Summary

| | |
|---|---|
| Provider name | `landingai` |
| Base URL | `https://api.va.landing.ai` (override: `base_url`, or `LANDINGAI_BASE_URL`). EU: `https://api.va.eu-west-1.landing.ai` |
| API key | `LANDINGAI_API_KEY`, falling back to `VISION_AGENT_API_KEY` (the name Landing AI's own libraries use), or `api_key` on the request — sent as `Authorization: Bearer <key>` |
| Docs | <https://docs.landing.ai/ade/ade-overview> · <https://docs.landing.ai/ade/parse> · OpenAPI: <https://docs.landing.ai/ade/va_openapi_ade2.json> |
| Modes | `parse` (native), `ocr` (derived from `parse`), `extract` (parse → extract, two calls) |
| Checked against | 2026-09-11, **from documentation and the published OpenAPI spec only** — no key was available, so the `#[ignore]`d live test has not been run |
| Implementation | `crates/puffinparse-core/src/providers/landingai.rs` |

ADE parses a document into reading-order Markdown plus semantic **chunks**, each grounded to a page
and a normalised bounding box, with separate grounding entries for tables and individual table cells.
Field extraction is a *second* API that consumes the parse Markdown, so PuffinParse's `extract` mode is a
parse followed by an extract, with chunk references resolved back into page + box citations.

## 2. Models exposed by PuffinParse

| Model | Provider parameters PuffinParse sets | List price (`pricing.json`) |
|---|---|---|
| `landingai/dpt-2` *(default)* | `model=dpt-2-latest`, `split=page` | parse $0.03 / page · extract $0.04 / page |

Credits cost **$0.01** each on the Explore and Team plans. Parsing costs **3 credits/page**
(+1 credit/page with Zero Data Retention); spreadsheets are 1 credit/sheet plus 3 credits per embedded
image. Extraction is charged on characters — `(input chars ÷ 5 000) + (output chars ÷ 1 000)`, rounded
up to a tenth of a credit — which for a typical page of Markdown is around 1 credit, hence the
$0.04/page estimate for the two-call `extract` mode (3 credits parse + ~1 credit extract). Treat the
extract price as an order-of-magnitude estimate: it scales with document *length*, not page count.

Pin a snapshot with `provider_options={"model": "dpt-2-20260410"}`; available values are `dpt-2`,
`dpt-2-latest` and the dated snapshots (`dpt-2-20250919`, `-20251103`, `-20260302`, `-20260410`).
`dpt-1` and `dpt-2-mini` are **deprecated and rejected by the API**.

## 3. Request flow PuffinParse uses

1. **Parse.** `POST {base}/v1/ade/parse`, `multipart/form-data`:

   | field | value PuffinParse sends |
   |---|---|
   | `document` | the file part — **or** `document_url` with the URL when the input is a URL (ADE downloads it itself) |
   | `model` | `dpt-2-latest` (or the `provider_options.model` override) |
   | `split` | `page` — one split per page, which gives authoritative per-page Markdown |

   `provider_options` are appended as extra form fields and override defaults of the same name
   (`password`, `custom_prompts`, `model`, `split`, …). Two keys are consumed locally and never sent:
   `auth_scheme` (see §6) and `extract_model`.
2. **Extract (mode `extract` only).** `POST {base}/v1/ade/extract`, `multipart/form-data` with
   `markdown` = the **raw** parse Markdown (anchors included — the extractor's references point at
   them), `schema` = the request's JSON Schema serialised to a string, and `model` = `extract-latest`
   (override with `provider_options.extract_model`). `ExtractRequest.instructions`, if set, is written
   into the schema's top-level `description` (ADE has no separate instruction field).
3. **Normalise.** See §4.

`pages` is applied **client-side** — ADE Parse has no page-range parameter — and records
`metadata.landingai_pages_filtered_client_side`; `usage.pages` stays at the billed page count for the
whole document. `language` is not forwarded: ADE detects languages automatically and exposes no hint.

PuffinParse always uses the **synchronous** endpoint. Parse Jobs (`POST /v1/ade/parse/jobs` +
`GET /v1/ade/parse/jobs/{job_id}`) lift the limit from 100 pages to 6 000 pages / 1 GB and are the
obvious next addition; the sync endpoint's own gateway timeout is 475 s.

## 4. Response mapping

```json
{
  "markdown": "<a id='2831e56d-…'></a>\n\n# Hello PuffinParse\n\n…",
  "chunks": [
    { "markdown": "<a id='2831e56d-…'></a>\n\n# Hello PuffinParse", "type": "text",
      "id": "2831e56d-94f5-4ec4-b001-6e16e188119b",
      "grounding": { "box": { "left": 0.017, "top": 0.038, "right": 0.463, "bottom": 0.212 }, "page": 0 } }
  ],
  "splits": [ { "class": "page", "identifier": "page_0", "pages": [0], "markdown": "…", "chunks": ["2831e56d-…"] } ],
  "grounding": { "2831e56d-…": { "box": {…}, "page": 0, "type": "chunkText", "confidence": 0.97 },
                 "1-1": { "box": {…}, "page": 1, "type": "table" },
                 "1-4": { "box": {…}, "page": 1, "type": "tableCell", "position": { "row": 1, "col": 0, … } } },
  "metadata": { "filename": "…", "org_id": null, "page_count": 2, "duration_ms": 7861,
                "credit_usage": 6.0, "job_id": "job_…", "version": "dpt-2-20260410", "failed_pages": [] }
}
```

(The full fixtures are `crates/puffinparse-core/tests/fixtures/landingai_parse.json` and
`landingai_extract.json`.)

### parse / ocr

| ADE field | PuffinParse unified field | Notes |
|---|---|---|
| `chunks[]` | `Page.blocks[]` | Reading order preserved. |
| `chunks[].markdown` | `Block.content` | The `<a id='…'></a>` grounding anchors are stripped; table HTML (`<table id="0-1">…`) is kept verbatim. `output="text"` runs it through `markdown_to_text`. |
| `chunks[].type` | `Block.type` | Mapping below. |
| `chunks[].grounding.page` | `Block.page_number` | **Zero-indexed on the wire**, +1 in PuffinParse. |
| `chunks[].grounding.box` | `Block.bbox` | `{left, top, right, bottom}`, already normalised 0–1 with a top-left origin → `{x0, y0, x1, y1}`. |
| `grounding[<chunk id>].confidence` | `Block.confidence` | Only text-ish chunks carry one. |
| `splits[]` with `class == "page"` | `Page.markdown` / `Page.text` | Preferred over joining the chunks; `pages[0] + 1` is the page number. |
| `metadata.page_count` | `Usage.pages` | Falls back to the number of reconstructed pages. |
| `metadata.credit_usage` | `Usage.credits` | Only when > 0. |
| `metadata.job_id` | `provider_job_id` | |
| `metadata.version` | `metadata.landingai_version` | Resolved snapshot, e.g. `dpt-2-20260410`. |
| `metadata.failed_pages` | `metadata.landingai_failed_pages` | Converted to 1-based. Present on `206 Partial Content`. |
| `grounding[<table/cell id>]` | — | Table and cell boxes are not modelled by PuffinParse's `Block`; use `include_raw=True`. |
| — | `Page.width` / `Page.height` | **Never set**: ADE reports no page dimensions (boxes are already relative). |

Chunk types: `text` → `text`, unless the chunk's Markdown starts with a heading — `# ` → `title`,
`##`+ → `section_header`; `table` → `table`; `figure`, `logo`, `card`, `attestation` → `figure`;
`marginalia` (headers, footers, page numbers) and `scan_code` (barcode/QR) → `other`. Legacy names
(`title`, `caption`, `list`, `header`, `footer`, `footnote`, `equation`) are still mapped.

### extract

| ADE field | PuffinParse unified field | Notes |
|---|---|---|
| `extraction` | `ExtractResponse.data` | Exactly the object ADE returns, shaped by your schema. |
| `extraction_metadata.…{value, references}` | `ExtractResponse.fields["<json pointer>"]` | The metadata mirrors the schema; every leaf becomes one `FieldInfo` keyed by an RFC 6901 pointer into `data` (`/invoice/total`, `/items/0/description`). |
| `references[]` (chunk / table-cell ids) | `FieldInfo.citations[]` | Resolved through the **parse** response's `grounding` map → `{page_number (1-based), bbox}`; for chunk ids the chunk's Markdown is attached as `Citation.text`. Unresolvable ids are dropped. |
| parse `metadata.page_count` | `Usage.pages` | Extraction itself is not page-billed. |
| parse + extract `credit_usage` | `Usage.credits` | Summed across both calls. |
| `metadata.job_id` (extract) | `provider_job_id` | |
| `metadata.schema_violation_error` | `metadata.landingai_schema_violation_error` | Non-null means the output does not fully conform to the schema (HTTP 206). |
| `metadata.version` | `metadata.landingai_extract_version` | |

With `include_raw=True` the extract response's `raw` is `{"parse": <parse payload>, "extract": <extract payload>}`.

## 5. Errors, status codes, rate limits, timeouts

ADE is a FastAPI service: errors are `{"detail": "…"}` or `{"detail": [ValidationError, …]}`, which
`Error::from_http` surfaces directly.

| Status | Cause | PuffinParse `ErrorKind` |
|---|---|---|
| 200 | success | — |
| 206 | **partial content** — some pages failed; `metadata.failed_pages` lists them (zero-indexed) | success, with `metadata.landingai_failed_pages` |
| 400 | document download failed, unsupported/deprecated model version | `bad_request` |
| 401 | missing or invalid API key | `authentication` |
| 402 | **out of credits** | `bad_request` (not a fallback-eligible error for the router) |
| 422 | input validation failed (bad schema, missing `document`/`document_url`) | `bad_request` |
| 429 | pages-per-hour rate limit exceeded | `rate_limit` (retried) |
| 500 | all pages failed to process | `provider` (retried) |
| 504 | processing exceeded the 475 s gateway timeout | `provider` (retried) |

**Limits.** ADE Parse (sync): **100 pages** per PDF. Parse Jobs: 6 000 pages or 1 GB. Accepted:
PDF, JPEG/JPG/PNG/APNG and other images, Office documents and spreadsheets (XLSX/CSV are billed per
sheet). Rate limits are **pages per hour per organization**, by plan, spread evenly across the hour;
Extract Jobs have their own hourly budget where each job counts as one page-equivalent.

**Timeouts.** `timeout_secs` (default 300) is the whole-call deadline — for `extract` that covers
*both* HTTP calls — and also caps each individual request. Note the server-side 475 s ceiling on sync
parses: a large document needs `timeout` ≥ 500 or the Parse Jobs API.

## 6. Gotchas (documentation-derived; not yet live-verified)

* **Two generations of the API exist.** PuffinParse targets **ADE Gen1** (`api.va.landing.ai/v1/ade/*`,
  DPT-2), which is current and documented. A newer **Gen2** (`api.ade.landing.ai/v2/parse`, DPT-3,
  character-span grounding, a structure tree instead of chunks) is live with a different response
  shape; supporting it means a second model (`landingai/dpt-3`) and a second normaliser, not a base-URL
  switch. The *legacy* `v1/tools/agentic-document-analysis` endpoint and the `agentic-doc` library are
  deprecated and return errors.
* **Auth header ambiguity.** Every code sample in the docs uses `Authorization: Bearer <key>` (what
  PuffinParse sends), but the OpenAPI security scheme is named "Basic Auth" with
  `bearerFormat: Basic`, and the troubleshooting page mentions an `apikey` header. If a key is
  rejected with 401, try `provider_options={"auth_scheme": "basic"}`, which sends
  `Authorization: Basic <key>` verbatim (any other string is used as the scheme as-is).
* **Pages are zero-indexed everywhere** on the wire — `grounding.page`, `splits[].pages`,
  `failed_pages`, and the `page_0` identifiers. PuffinParse converts all of them to 1-based.
* **Chunk grounding is a single object, not a list.** The Gen1 schema is `grounding: {box, page}`;
  the legacy endpoint used a list of groundings with `{l, t, r, b}` keys. PuffinParse's deserialiser
  accepts both shapes and both key spellings.
* **Markdown carries anchors.** Every chunk's Markdown begins with `<a id='<chunk id>'></a>`, and
  table cells carry `id` attributes — that is how extraction references locations. PuffinParse strips the
  anchors from `content`/`markdown` but sends the *unstripped* Markdown to the extractor, which is
  what makes citations resolvable.
* **Tables come back as HTML**, not Markdown pipes, and cell ids are `"{page}-{base62}"` (page
  zero-indexed). `markdown_to_text` strips the tags for `Block.text` and `output="text"`.
* **There are no headings in the chunk vocabulary.** Titles and section headers are `text` chunks
  whose Markdown happens to start with `#`; PuffinParse maps those to `title` / `section_header`.
* **`marginalia` is lossy.** It merges what other providers split into `header`, `footer`,
  `page_number` and `footnote`, so it maps to `other`.
* **Extraction is length-priced, not page-priced**, and a very long document can cost far more to
  extract than to parse. `cost_usd` for `extract` is a per-page estimate and will drift.
* **`split=page` is PuffinParse's default**, because without it ADE returns a single `full` split and
  per-page Markdown would have to be stitched from chunk groundings. `page` is the only documented
  value, and `provider_options` nulls are skipped, so the default cannot be unset — that is
  deliberate. (The `split` *parameter* is unrelated to the ADE **Split API**, which classifies
  sub-documents.)
* **Password-protected files** are supported through `provider_options={"password": "…"}`.

## 7. Useful `provider_options` passthrough

```python
# 1. Pin a model snapshot so results do not move under you.
puffinparse.parse("doc.pdf", model="landingai/dpt-2",
              provider_options={"model": "dpt-2-20260410"})

# 2. Password-protected PDF.
puffinparse.parse("locked.pdf", model="landingai/dpt-2",
              provider_options={"password": "s3cret"})

# 3. Tell the figure captioner what you care about.
puffinparse.parse("chart.png", model="landingai/dpt-2",
              provider_options={"custom_prompts": '{"figure": "Describe axis labels in detail."}'})

# 4. Schema-driven extraction with citations (parse → extract under the hood).
puffinparse.extract("invoice.pdf", model="landingai/dpt-2", citations=True,
                schema={"type": "object", "properties": {
                    "invoice": {"type": "object", "properties": {
                        "number": {"type": "string"}, "total": {"type": "number"}}}}})

# 5. If a key is rejected with 401, switch the authorization scheme.
puffinparse.parse("doc.pdf", model="landingai/dpt-2",
              provider_options={"auth_scheme": "basic"})

# 6. EU data residency (key must come from the EU console).
puffinparse.parse("doc.pdf", model="landingai/dpt-2",
              base_url="https://api.va.eu-west-1.landing.ai")
```

## 8. Links

* Overview: <https://docs.landing.ai/ade/ade-overview> · docs index: <https://docs.landing.ai/llms.txt>
* Parse: <https://docs.landing.ai/ade/parse> · JSON response:
  <https://docs.landing.ai/ade/ade-json-response> · chunk types:
  <https://docs.landing.ai/ade/ade-chunk-types>
* Extract: <https://docs.landing.ai/ade/ade-extract> · response:
  <https://docs.landing.ai/ade/ade-extract-response>
* OpenAPI (Gen1): <https://docs.landing.ai/ade/va_openapi_ade2.json> ·
  (Gen2 / DPT-3): <https://docs.landing.ai/dpt3/openapi-adev2.json>
* Credits: <https://docs.landing.ai/ade/ade-credit-consumption> · plans:
  <https://docs.landing.ai/ade/ade-pricing> · rate limits:
  <https://docs.landing.ai/ade/ade-rate-limits>
* Parse Jobs (async, 6 000 pages): <https://docs.landing.ai/ade/ade-parse-async>
