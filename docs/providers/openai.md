# OpenAI (Responses API)

> **Status: docs-only.** Implemented from OpenAI's published API documentation and tested
> against fixture payloads built from it. It has not yet been run against the live API, so
> expect wire-format differences. Help verify it: [issue #10](https://github.com/ajinkyashejul/puffinparse/issues/10).

## 1. Summary

| | |
|---|---|
| Provider name | `openai` |
| Base URL | `https://api.openai.com` (override: `base_url` on the request, or `OPENAI_BASE_URL`) |
| API key | `OPENAI_API_KEY` (or `api_key` on the request) — sent as `Authorization: Bearer sk-…` |
| Docs | <https://developers.openai.com/api/docs> (the old `platform.openai.com/docs/*` links 301 here) |
| Endpoint | `POST /v1/responses` — one synchronous call per document, no job id, no polling |
| Modes | `parse`, `ocr` (derived from `parse`), `extract` |
| Checked against | 2026-09-11 **against the published docs only** — no OpenAI key was available, so the fixtures are built from the documented response shape and the live tests are `#[ignore]` |
| Implementation | `crates/puffinparse-core/src/providers/openai.rs` |

This is not a document-parsing product: it is a general vision LLM asked, with a strict JSON schema,
to transcribe a document page by page. That buys layout-aware markdown of figures, handwriting and
messy scans, and costs you geometry — **there are no bounding boxes, no per-block types and no
confidences**. Every page comes back as a single `text` block whose `content` is the page markdown.

## 2. Models exposed by PuffinParse

| Model | Provider parameters PuffinParse sets | Estimated price (`pricing.json`) |
|---|---|---|
| `openai/gpt-5.6-luna` *(default)* | `model=gpt-5.6-luna`, `reasoning.effort=low` | ~$0.00114 / page |
| `openai/gpt-5.6-terra` | `model=gpt-5.6-terra`, `reasoning.effort=low` | ~$0.0114 / page |
| `openai/gpt-5.6-sol` | `model=gpt-5.6-sol`, `reasoning.effort=low` | ~$0.0200 / page |
| `openai/gpt-6-astra` | `model=gpt-6-astra`, `reasoning.effort=low` | ~$0.0500 / page |
| `openai/gpt-6-luna` | `model=gpt-6-luna`, `reasoning.effort=low` | ~$0.0005 / page |

**Pricing is per token, not per page**, so `pricing.json` holds an *estimate*: **1,500 input +
700 output tokens per page** (OpenAI bills a PDF page as extracted text *plus* a page image;
1.5k input tokens is the low end of the published 1,500–3,000 range, so a dense page costs more).
Rates per 1M tokens (<https://developers.openai.com/api/docs/pricing>, 2026-09-11):
luna $0.20/$1.20, terra $2/$12, sol $4/$20, astra $10/$50. `gpt-6-luna` (added 2026-10-08 from
<https://developers.openai.com/api/docs/models/gpt-6-luna>: text and image input, Responses API,
structured outputs, reasoning effort `none`…`max`) is $0.10/$0.50 short context ($0.20/$0.75 long
context, which `PRICES` does not model).

The authoritative number for a call is `response.usage.provider_cost_usd`, which PuffinParse computes
from the **actual** `usage.input_tokens`/`usage.output_tokens` and the per-token table embedded in
`openai.rs` (`PRICES`, dated `2026-09-11`). It always wins over the per-page estimate, so
`cost_usd` is exact even when the estimate is not. `metadata.openai_input_tokens` /
`openai_output_tokens` carry the raw counts.

Any other model can be reached without a registry change via
`provider_options={"model": "gpt-4.1-mini"}`; the price table covers the gpt-5.x, gpt-4.1 and
gpt-4o families as well, and cost falls back to `None` for a model it does not know.

## 3. Request flow PuffinParse uses

One call. `POST {base}/v1/responses`, `Authorization: Bearer …`, JSON body:

```jsonc
{
  "model": "gpt-5.6-luna",
  "input": [{
    "role": "user",
    "content": [
      // PDFs — the filename matters, the model sees it:
      {"type": "input_file", "filename": "invoice.pdf", "file_data": "data:application/pdf;base64,JVBER…"},
      // …or, for images: {"type": "input_image", "image_url": "data:image/png;base64,iVBOR…", "detail": "high"}
      {"type": "input_text", "text": "Transcribe this document. …"}
    ]
  }],
  "text": {"format": {"type": "json_schema", "name": "puffinparse_pages", "schema": { … }, "strict": true}},
  "max_output_tokens": 32000,
  "reasoning": {"effort": "low"},
  "store": false
}
```

* **Input handling.** Path and bytes inputs are read locally; a **URL input is downloaded by
  PuffinParse and inlined** as a `data:` URL (the API would only fetch public URLs, and the response is
  identical either way). MIME type comes from the URL's `Content-Type` when it is specific, else
  from the filename extension. `application/pdf` → `input_file`; `image/*` → `input_image`;
  anything else is an `input_error` before any network call.
* **`parse` schema** (`name: puffinparse_pages`): `{"pages": [{"page_number": <int>, "markdown": <string>}]}`.
* **`extract` schema** (`name: puffinparse_extraction`): the caller's JSON Schema, passed through
  [the strict sanitiser](#6-gotchas).
* **`pages` and `language` are prompt-level**, not API parameters: the prompt says
  "transcribe only pages 1-3,7, keeping the original page numbers" and "the document is mainly in
  de". Page selection is therefore best-effort — the whole document is still uploaded and billed.
* **`output="text"`** adds a rule telling the model to emit plain text instead of Markdown.
* **`store: false`** by default so the document is not retained for the response store; set
  `provider_options={"store": true}` if you want to fetch the response later.
* **`temperature: 0` is only sent to gpt-4.x models.** gpt-5.x and gpt-6 are reasoning models and
  return `400 Unsupported value: 'temperature' does not support 0 with this model` — PuffinParse sends
  `reasoning: {"effort": "low"}` to those instead (transcription is perception, not reasoning).
* **`provider_options` is merged into the body verbatim** (deep merge, so
  `{"reasoning": {"effort": "medium"}}` replaces only the effort). The key `strict` is consumed by
  PuffinParse and never forwarded.

## 4. Response mapping

| Responses API field | PuffinParse unified field | Notes |
|---|---|---|
| `id` (`resp_…`) | `ParseResponse.provider_job_id` | |
| `output[].content[].text` where `type == "output_text"` | parsed as JSON → the structured answer | Parts are concatenated in order; `reasoning` items are skipped. A `refusal` part becomes a `provider` error. If nothing is found, the SDK-style `output_text` aggregate is used as a fallback. |
| `pages[].page_number` | `Page.page_number` | Missing or `0` falls back to the array index + 1. |
| `pages[].markdown` | `Page.markdown` (trimmed) | With `output="text"` this is already plain text. |
| — | `Page.text` | `markdown_to_text(markdown)`, or the string itself in text mode. |
| — | `Page.blocks` | Exactly one `text` block per page, `content` = page markdown, `bbox: None`, `confidence: None`. |
| — | `Page.width` / `height` | Always `None` — the API reports no page geometry. |
| number of returned pages | `Usage.pages` | The API never reports a page count; extract mode reports `0`. |
| `usage.input_tokens` / `output_tokens` | `Usage.provider_cost_usd` (and `metadata.openai_*_tokens`) | Cost = tokens × the embedded per-token prices, so it is exact. |
| — | `Usage.credits` | Always `None` — OpenAI has no credit unit. |
| `status: "incomplete"` + `incomplete_details.reason` | `provider` error | Typically `max_output_tokens`. |
| `status: "failed"` + `error.message` | `provider` error | |
| everything else (`reasoning`, `annotations`, `output_tokens_details`, …) | — | Visible with `include_raw=True`. |

`extract` mode returns the tool-free JSON object as `ExtractResponse.data`. `ExtractResponse.fields`
is always empty: the Responses API has no per-field grounding, so `citations=True` is recorded as
`metadata.openai_citations_unsupported = true` rather than silently pretending to support it.

Trimmed response (`crates/puffinparse-core/tests/fixtures/openai_responses_parse.json`, built from the
documented shape):

```json
{
  "id": "resp_68f0a1b2c3d4e5f60123456789abcdef",
  "object": "response",
  "status": "completed",
  "model": "gpt-5.6-luna",
  "output": [
    { "id": "rs_…", "type": "reasoning", "summary": [] },
    { "id": "msg_…", "type": "message", "status": "completed", "role": "assistant",
      "content": [{ "type": "output_text", "annotations": [],
                    "text": "{\"pages\":[{\"page_number\":1,\"markdown\":\"# Hello PuffinParse\\n\\nInvoice #1234…\"}]}" }] }
  ],
  "text": { "format": { "type": "json_schema", "name": "puffinparse_pages", "strict": true } },
  "usage": { "input_tokens": 3120, "output_tokens": 412,
             "output_tokens_details": { "reasoning_tokens": 64 }, "total_tokens": 3532 },
  "store": false
}
```

## 5. Errors, status codes, rate limits, timeouts

The error envelope is `{"error": {"message", "type", "code", "param"}}`; `Error::from_http` picks up
the nested `error.message` and classifies by status:

| Status | Trigger | PuffinParse `ErrorKind` |
|---|---|---|
| 400 | invalid schema for `text.format`, unsupported parameter (`temperature` on a reasoning model), file too large / unreadable | `bad_request` |
| 401 | missing or revoked key | `authentication` |
| 403 | key/region or model not permitted for the org | `authentication` |
| 404 | unknown model id | `bad_request` |
| 429 | rate limit **or** insufficient quota (`code: "insufficient_quota"` — not actually retryable) | `rate_limit` (retried) |
| 5xx | `server_error`, `502`, `503` | `provider` (retried) |

**Retries.** `max_retries` (default 2), exponential backoff with full jitter, on rate-limit, network
and 500/502/503/504 only. 4xx is never retried.

**Rate limits.** Per-model RPM/TPM quotas by usage tier; responses carry `x-ratelimit-remaining-*`
and `retry-after` headers, which PuffinParse does **not** read yet — backoff is blind.

**Limits.** A file must stay under 50 MB, and all files in one request under 50 MB total; a PDF page
is billed as extracted text *plus* a page image, so context, not page count, is the practical limit.
`max_output_tokens` defaults to 32,000 (~20 dense pages); a longer document truncates with
`status: "incomplete"`, `reason: "max_output_tokens"`.

**Timeouts.** `timeout_secs` (default 300) is the whole-call deadline and also caps the single HTTP
request. Big documents on a reasoning model are slow: budget minutes, not seconds.

## 6. Gotchas

* **Strict structured outputs restrict JSON Schema.** With `strict: true`, every object must set
  `"additionalProperties": false` and list **every** property in `required`; there are no optional
  fields — express "may be absent" as a nullable type (`"type": ["string", "null"]`). The root must
  be an object. PuffinParse applies `sanitize_strict_schema()` to the caller's `extract` schema
  recursively (through `properties`, `items`, `prefixItems`, `$defs`/`definitions`, `anyOf`/`oneOf`/
  `allOf`, `if`/`then`/`else`, `not`, `contains`), rewriting `required` to the full property list and
  forcing `additionalProperties: false`. **This widens `required`**: fields you marked optional come
  back as `null` rather than missing.
* **Some keywords are still rejected in strict mode** (`minimum`/`maximum`, `minLength`/`maxLength`,
  `pattern`, recursive `$ref` beyond `#`). If the API answers `400 Invalid schema`, either drop those
  keywords or turn the feature off with `provider_options={"strict": false}` — PuffinParse then sends
  your schema untouched and asks for JSON by prompt instead.
* **`temperature` is not accepted by gpt-5.x / gpt-6.** See §3. If you force it through
  `provider_options`, expect a 400.
* **Reasoning tokens are billed as output tokens** and are invisible in the text. `effort: "low"`
  keeps them small; raise it with `provider_options={"reasoning": {"effort": "medium"}}` if a dense
  or handwritten document transcribes badly.
* **No geometry, ever.** `Block.bbox`, `Block.confidence` and `Page.width/height` are `None`, and
  every page holds exactly one `text` block. Anything that draws overlays needs a layout provider
  (Reducto, Extend, Azure, …). `ocr` mode is derived from `parse` by the default trait method, so
  its `words[]` carry no boxes either, and `metadata.puffinparse_derived_from = "parse"` says so.
* **`pages` is a prompt instruction, not an API parameter.** The whole document is uploaded and
  billed even when you ask for one page, and the model may ignore the restriction on a bad day.
  Split the PDF client-side if this matters.
* **Page numbers come from the model.** They are usually the document's own, but a model can repeat
  or skip one; PuffinParse falls back to the array position when the number is missing or `0`, and sorts
  pages by number when assembling the document markdown.
* **Hallucination is the failure mode.** A layout parser drops text it cannot read; an LLM can
  invent plausible text instead. The prompt forbids it ("transcribe verbatim, never invent"), but
  for high-stakes extraction prefer a provider that returns citations.
* **`store: false` is PuffinParse's default** — responses are not kept in OpenAI's response store, which
  also means `previous_response_id` chaining is unavailable unless you opt back in.
* **`detail: "high"` is set on image inputs** for legibility of small print; `{"detail": "low"}`
  through `provider_options` on the input block is not possible (the block is built by PuffinParse) —
  use the `openai/gpt-5.6-luna` model and a downsampled image instead if you need to save tokens.

## 7. Useful `provider_options` passthrough

```python
# 1. Longer documents: raise the output ceiling (the default 32k covers ~20 dense pages).
puffinparse.parse("contract.pdf", model="openai/gpt-5.6-luna",
              provider_options={"max_output_tokens": 64000})

# 2. Harder documents: more reasoning (billed as output tokens).
puffinparse.parse("handwritten.pdf", model="openai/gpt-5.6-terra",
              provider_options={"reasoning": {"effort": "medium"}})

# 3. A model that is not in the registry (pricing falls back to the embedded token table).
puffinparse.parse("scan.png", model="openai/gpt-5.6-luna",
              provider_options={"model": "gpt-4.1-mini", "temperature": 0})

# 4. Extraction with a schema that uses keywords strict mode rejects.
puffinparse.extract("invoice.pdf", schema=schema_with_patterns,
                model="openai/gpt-5.6-luna", provider_options={"strict": False})

# 5. Keep the response in OpenAI's response store, and tag it.
puffinparse.parse("report.pdf", model="openai/gpt-5.6-sol",
              provider_options={"store": True, "metadata": {"run": "bench-2026-09"}})
```

## 8. Links

* API reference: <https://developers.openai.com/api/docs/api-reference/responses>
* PDF / file inputs: <https://developers.openai.com/api/docs/guides/pdf-files>
* Images and vision: <https://developers.openai.com/api/docs/guides/images-vision>
* Structured outputs: <https://developers.openai.com/api/docs/guides/structured-outputs>
* Reasoning and `effort`: <https://developers.openai.com/api/docs/guides/reasoning>
* Models: <https://developers.openai.com/api/docs/models> · Pricing: <https://developers.openai.com/api/docs/pricing>
* Rate limits: <https://developers.openai.com/api/docs/guides/rate-limits>
