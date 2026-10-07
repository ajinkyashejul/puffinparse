# Anthropic (Claude Messages API)

> **Status: docs-only.** Implemented from Anthropic's published API documentation and tested
> against fixture payloads built from it. It has not yet been run against the live API, so
> expect wire-format differences. Help verify it: [issue #10](https://github.com/ajinkyashejul/puffinparse/issues/10).

## 1. Summary

| | |
|---|---|
| Provider name | `anthropic` |
| Base URL | `https://api.anthropic.com` (override: `base_url` on the request, or `ANTHROPIC_BASE_URL`) |
| API key | `ANTHROPIC_API_KEY` (or `api_key` on the request) — sent as `x-api-key: sk-ant-…` |
| Docs | <https://platform.claude.com/docs> (the old `docs.anthropic.com/en/*` links 301 here) |
| Endpoint | `POST /v1/messages` with `anthropic-version: 2023-06-01` — one synchronous call, no job id, no polling |
| Modes | `parse`, `ocr` (derived from `parse`), `extract` |
| Checked against | 2026-09-11 **against the published docs only** — no Anthropic key was available, so the fixtures are built from the documented response shape and the live tests are `#[ignore]` |
| Implementation | `crates/puffinparse-core/src/providers/anthropic.rs` |

Like `openai`, this is a general vision LLM rather than a document-parsing product: Claude is given
the PDF (each page as text *plus* a page image) and asked, through a **forced tool call**, to return
one markdown transcription per page. You get layout-aware markdown of tables, figures and
handwriting, and you lose geometry — **no bounding boxes, no per-block types, no confidences**.

## 2. Models exposed by PuffinParse

| Model | Provider parameters PuffinParse sets | Estimated price (`pricing.json`) |
|---|---|---|
| `anthropic/claude-sonnet-5` *(default)* | `model=claude-sonnet-5`, `output_config.effort=low` | ~$0.0100 / page |
| `anthropic/claude-haiku-4-5` | `model=claude-haiku-4-5`, `temperature=0` | ~$0.0050 / page |
| `anthropic/claude-opus-5` | `model=claude-opus-5`, `output_config.effort=low` | ~$0.0250 / page |

**Pricing is per token, not per page**, so `pricing.json` holds an *estimate*: **1,500 input +
700 output tokens per page** — Anthropic's own guidance is 1,500–3,000 text tokens per page *plus*
the page image's visual tokens, so a dense page costs more than the estimate. Rates per 1M tokens
(<https://platform.claude.com/docs/en/about-claude/pricing>, 2026-09-11): Sonnet 5 $2/$10,
Haiku 4.5 $1/$5, Opus 5 $5/$25.

`response.usage.provider_cost_usd` is computed from the **actual** `usage.input_tokens` /
`usage.output_tokens` and the per-token table embedded in `anthropic.rs` (`PRICES`, dated
`2026-09-11`), so `cost_usd` is exact even where the per-page estimate is not.
`metadata.anthropic_input_tokens` / `anthropic_output_tokens` carry the raw counts.

Other model ids work without a registry change via `provider_options={"model": "claude-opus-4-8"}`;
the embedded table covers the Opus 4.6–5, Sonnet 4.6/5, Haiku 4.5 and Fable 5/5.1 ids, and cost
falls back to `None` for anything it does not know. **Claude Fable 5.1 / Mythos 5.1 do not work
here**: they reject forced `tool_choice` with a 400 (see §6).

## 3. Request flow PuffinParse uses

One call. `POST {base}/v1/messages` with `x-api-key`, `anthropic-version: 2023-06-01`,
`content-type: application/json`:

```jsonc
{
  "model": "claude-sonnet-5",
  "max_tokens": 32000,
  "system": "You are a precise document transcription and extraction engine. …",
  "messages": [{
    "role": "user",
    "content": [
      // PDFs (documents before text — Claude does better that way):
      {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBER…"}},
      // …or, for images: {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBOR…"}}
      {"type": "text", "text": "Transcribe this document. …"}
    ]
  }],
  "tools": [{"name": "emit_pages", "description": "…", "input_schema": { … }}],
  "tool_choice": {"type": "tool", "name": "emit_pages"},
  "output_config": {"effort": "low"}
}
```

* **Structured output is a forced tool call.** One tool is defined whose `input_schema` is the
  wanted shape, and `tool_choice` pins it, so the model must answer with a `tool_use` block whose
  `input` is the JSON. (`output_config.format` structured outputs would also work on these models;
  the tool route keeps `extract` and `parse` on one code path.)
  * `parse` → tool `emit_pages`, schema `{"pages": [{"page_number": <int>, "markdown": <string>}]}`.
  * `extract` → tool `record_extraction`, schema = the caller's JSON Schema.
* **Input handling.** Path and bytes inputs are read locally; a **URL input is downloaded by PuffinParse
  and inlined** as base64 (Claude also accepts `source: {"type": "url"}`, but only for publicly
  reachable URLs). `application/pdf` → `document` block; `image/jpeg|png|gif|webp` → `image` block;
  anything else is an `input_error` before any network call.
* **`pages` and `language` are prompt-level**, not API parameters: the prompt says "transcribe only
  pages 2-4, keeping the original page numbers" and "the document is mainly in fr". Page selection
  is best-effort — the whole document is still uploaded and billed.
* **`output="text"`** adds a rule telling the model to emit plain text instead of Markdown.
* **`temperature: 0` is only sent to Claude 4.5-era and older models** (`claude-haiku-4-5`,
  `claude-sonnet-4-5`, `claude-opus-4-5`, `claude-3*`). Claude 4.6 and later removed sampling
  parameters and return a 400 if you send them.
* **`output_config: {"effort": "low"}` is only sent to models that support effort** (Opus 4.6–5,
  Sonnet 4.6/5). Haiku 4.5 rejects it. Effort controls how much (billed) thinking Claude does;
  transcription is perception, not reasoning, so `low` is the default.
* **`thinking` is never sent.** Adaptive thinking (on by default on Sonnet 5 / Opus 5) is compatible
  with forced tool use; *manual* extended thinking (`thinking: {"type": "enabled"}`) is not and
  would break the forced call, so do not add it through `provider_options`.
* **`provider_options` is merged into the body verbatim** (deep merge). The key `strict` is consumed
  by PuffinParse and never forwarded.

## 4. Response mapping

| Messages API field | PuffinParse unified field | Notes |
|---|---|---|
| `id` (`msg_…`) | `ParseResponse.provider_job_id` | The `request-id` response header is not read. |
| `content[]` block with `type == "tool_use"` and the expected `name` → `.input` | the structured answer | Any `thinking` / `text` blocks before it are skipped. |
| `pages[].page_number` | `Page.page_number` | Missing or `0` falls back to the array index + 1. |
| `pages[].markdown` | `Page.markdown` (trimmed) | With `output="text"` this is already plain text. |
| — | `Page.text` | `markdown_to_text(markdown)`, or the string itself in text mode. |
| — | `Page.blocks` | Exactly one `text` block per page, `content` = page markdown, `bbox: None`, `confidence: None`. |
| — | `Page.width` / `height` | Always `None` — the API reports no page geometry. |
| number of returned pages | `Usage.pages` | The API never reports a page count; extract mode reports `0`. |
| `usage.input_tokens` / `output_tokens` | `Usage.provider_cost_usd` (and `metadata.anthropic_*_tokens`) | Cost = tokens × the embedded per-token prices, so it is exact. Cache and tier fields are not modelled. |
| — | `Usage.credits` | Always `None` — Anthropic has no credit unit. |
| `stop_reason: "max_tokens"` | `provider` error | "response truncated … raise provider_options.max_tokens". |
| `stop_reason: "refusal"` (+ `stop_details.category`) | `provider` error | |
| no `tool_use` block at all | `provider` error | The beginning of the model's prose answer is included in the message. |

`extract` mode returns the tool input as `ExtractResponse.data`. `ExtractResponse.fields` is always
empty: Claude's citations feature grounds *text* answers and cannot be combined with the forced tool
call, so `citations=True` is recorded as `metadata.anthropic_citations_unsupported = true`.

Trimmed response (`crates/puffinparse-core/tests/fixtures/anthropic_messages_parse.json`, built from the
documented shape):

```json
{
  "id": "msg_01XhT9Eq8bQ7vPz2kKcM4dLp",
  "type": "message",
  "role": "assistant",
  "model": "claude-sonnet-5",
  "content": [
    { "type": "tool_use", "id": "toolu_01A09q90qw90lq917835lq9", "name": "emit_pages",
      "input": { "pages": [ { "page_number": 1, "markdown": "# Hello PuffinParse\n\nInvoice #1234…" } ] } }
  ],
  "stop_reason": "tool_use",
  "stop_sequence": null,
  "usage": { "input_tokens": 3210, "cache_creation_input_tokens": 0,
             "cache_read_input_tokens": 0, "output_tokens": 389, "service_tier": "standard" }
}
```

## 5. Errors, status codes, rate limits, timeouts

The error envelope is `{"type": "error", "error": {"type", "message"}, "request_id"}`;
`Error::from_http` picks up the nested `error.message` and classifies by status:

| Status | Provider error type | PuffinParse `ErrorKind` |
|---|---|---|
| 400 | `invalid_request_error` — bad schema, `max_tokens` above the model's ceiling, `temperature` on a 4.6+ model, forced `tool_choice` on Fable 5.1 | `bad_request` |
| 401 | `authentication_error` | `authentication` |
| 402 | `billing_error` | `bad_request` |
| 403 | `permission_error` | `authentication` |
| 404 | `not_found_error` (unknown model id) | `bad_request` |
| 413 | `request_too_large` (the request is over 32 MB) | `bad_request` |
| 429 | `rate_limit_error` | `rate_limit` (retried) |
| 500 / 504 | `api_error` / `timeout_error` | `provider` (retried) |
| **529** | `overloaded_error` | `provider` (retried) — **reported as HTTP 503** |

**529 is rewritten to 503.** PuffinParse's shared retry policy only treats 500/502/503/504 as transient,
so `map_http_error()` reports an overloaded 529 as status `503` and prefixes the message with
`overloaded (HTTP 529):`. `Error.status_code` is therefore `503` for this case — the real status is
in the message.

**Retries.** `max_retries` (default 2), exponential backoff with full jitter, on rate-limit, network,
500/502/503/504 and (via the rewrite) 529. The `retry-after` header is **not** read yet.

**Limits.** 32 MB per request; 600 PDF pages per request (100 on models with a context window under
1M tokens); images up to 8000×8000 px and 10 MB each, JPEG/PNG/GIF/WebP only; no password-protected
PDFs. `max_tokens` defaults to 32,000 (~20 dense pages) — a longer document stops with
`stop_reason: "max_tokens"` and PuffinParse turns that into a provider error rather than returning half a
document.

**Timeouts.** `timeout_secs` (default 300) is the whole-call deadline and also caps the single HTTP
request. Anthropic recommends streaming or the Batch API beyond ~10 minutes; PuffinParse does neither,
so keep documents small enough to finish inside the deadline.

## 6. Gotchas

* **Forced tool use is not universal.** Claude Fable 5.1 and Mythos 5.1 reject
  `tool_choice: {"type": "tool"}` with `400 tool_choice: type "tool" and "any" are not supported for
  this model`, so they are deliberately absent from the registry. Manual extended thinking
  (`thinking: {"type": "enabled"}`) has the same restriction — do not add it via `provider_options`.
* **Sampling parameters are gone on Claude 4.6+.** `temperature`, `top_p` and `top_k` return a 400 on
  Sonnet 5 / Opus 5 and the 4.6+ family. PuffinParse only sends `temperature: 0` to the older models that
  still accept it; determinism on the newer ones comes from the schema and the prompt, not sampling.
* **`output_config.effort` is model-gated.** Haiku 4.5 and older reject it; PuffinParse sends it only to
  models in `EFFORT_MODELS`. Raise it (`{"output_config": {"effort": "medium"}}`) for hard scans.
* **Thinking tokens are billed as output tokens.** On Sonnet 5 / Opus 5 adaptive thinking is on by
  default (display omitted, so you never see it); `effort: "low"` keeps the bill down.
* **Strict tool use is opt-in here.** Unlike the OpenAI provider, PuffinParse does *not* set
  `strict: true` on the tool by default, because Claude accepts many JSON Schema keywords that strict
  mode rejects (`minimum`/`maximum`, `minLength`/`maxLength`, recursive schemas). Pass
  `provider_options={"strict": true}` to turn it on; PuffinParse then also sanitises your schema
  recursively — `additionalProperties: false` everywhere and **every** property moved into
  `required`, which means fields you marked optional come back as `null` instead of missing.
* **No geometry, ever.** `Block.bbox`, `Block.confidence` and `Page.width/height` are `None`, and
  every page holds exactly one `text` block. `ocr` mode is derived from `parse` by the default trait
  method (`metadata.puffinparse_derived_from = "parse"`), so its `words[]` carry no boxes either.
* **`pages` is a prompt instruction, not an API parameter.** The whole document is uploaded and
  billed even when you ask for one page. Split the PDF client-side if that matters.
* **Page numbers come from the model**, so a model can repeat or skip one; PuffinParse falls back to the
  array position when the number is missing or `0` and sorts pages by number.
* **Hallucination is the failure mode.** A layout parser drops what it cannot read; an LLM can invent
  plausible text. The prompt forbids it ("transcribe verbatim, never invent"), but do not use this
  provider for high-stakes extraction without review.
* **Claude will not identify people in images** and refuses documents that violate the AUP; such a
  response arrives as HTTP 200 with `stop_reason: "refusal"`, which PuffinParse maps to a provider error.
* **Prompt caching is not used.** Every call re-uploads the document; for repeated parses of the same
  file, add `cache_control` through `provider_options` yourself.

## 7. Useful `provider_options` passthrough

```python
# 1. Longer documents: raise the output ceiling (the default 32k covers ~20 dense pages).
puffinparse.parse("contract.pdf", model="anthropic/claude-sonnet-5",
              provider_options={"max_tokens": 64000})

# 2. Harder documents: more thinking (billed as output tokens).
puffinparse.parse("handwritten.pdf", model="anthropic/claude-opus-5",
              provider_options={"output_config": {"effort": "medium"}})

# 3. A model that is not in the registry (pricing falls back to the embedded token table).
puffinparse.parse("scan.png", model="anthropic/claude-sonnet-5",
              provider_options={"model": "claude-opus-4-8"})

# 4. Strict tool use for extraction (schema is sanitised: all fields become required).
puffinparse.extract("invoice.pdf", schema=invoice_schema,
                model="anthropic/claude-sonnet-5", provider_options={"strict": True})

# 5. Cache the document across repeated calls (5-minute ephemeral cache).
puffinparse.parse("handbook.pdf", model="anthropic/claude-haiku-4-5",
              provider_options={"cache_control": {"type": "ephemeral"}})
```

## 8. Links

* Messages API: <https://platform.claude.com/docs/en/api/messages>
* PDF support: <https://platform.claude.com/docs/en/build-with-claude/pdf-support>
* Vision (images, limits, visual tokens): <https://platform.claude.com/docs/en/build-with-claude/vision>
* Tool use / forcing a tool: <https://platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools>
* Structured outputs: <https://platform.claude.com/docs/en/build-with-claude/structured-outputs>
* Errors and request-size limits: <https://platform.claude.com/docs/en/api/errors>
* Models: <https://platform.claude.com/docs/en/models/overview> · Pricing: <https://platform.claude.com/docs/en/about-claude/pricing>
* Rate limits: <https://platform.claude.com/docs/en/api/rate-limits>
