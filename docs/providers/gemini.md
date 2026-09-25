# Google Gemini

## 1. Summary

| | |
|---|---|
| Provider name | `gemini` |
| Base URL | `https://generativelanguage.googleapis.com` (override: `base_url` on the request, or `GEMINI_BASE_URL`) |
| API key | `GEMINI_API_KEY` (or `api_key` on the request) — sent as the header `x-goog-api-key: AIza…` |
| Docs | <https://ai.google.dev/gemini-api/docs> |
| API version | Path-versioned; PuffinParse uses `/v1beta` (the Files API and the newest models live there). `/v1` serves the same `generateContent` method. |
| Modes | `parse`, `ocr` (derived from `parse`), `extract` |
| Verified | 2026-09-11 against the documented wire format; **the live call was not exercised** — see §6, "Unverified against a live key" |
| Implementation | `crates/puffinparse-core/src/providers/gemini.rs` |

Gemini is not a document-AI product but a general vision LLM: PuffinParse sends the file plus a
transcription prompt and pins the answer's shape with `generationConfig.response_schema`
(structured output), so the model must return one entry per page instead of a single markdown blob.
There is exactly **one** HTTP call per parse — no upload step, no polling — unless the document is
larger than ~14 MB, in which case the resumable Files API is used first.

The consequence of using an LLM is that **nothing geometric comes back**: no bounding boxes, no page
dimensions, no per-block confidence. Anything that needs overlays or coordinates should use a layout
provider (Reducto, Extend, LlamaParse, Azure, Textract) instead.

## 2. Models exposed by PuffinParse

The PuffinParse model name is the Gemini model id minus the `gemini-` prefix.

| Model | API model id | Token price (in / out, per 1M) | `pricing.json` per-page estimate |
|---|---|---|---|
| `gemini/2.5-flash` *(default)* | `gemini-2.5-flash` | $0.30 / $2.50 | $0.00245 |
| `gemini/2.5-pro` | `gemini-2.5-pro` | $1.25 / $10.00 (>200k prompt tokens: $2.50 / $15.00) | $0.009875 |
| `gemini/2.5-flash-lite` | `gemini-2.5-flash-lite` | $0.10 / $0.40 | $0.00047 |
| `gemini/3.5-flash` | `gemini-3.5-flash` | $1.50 / $9.00 | $0.00945 |
| `gemini/3.5-flash-lite` | `gemini-3.5-flash-lite` | $0.30 / $2.50 | $0.00245 |
| `gemini/3.8-flash` | `gemini-3.8-flash` | $0.75 / $3.75 *(introductory, through 2026-12-31; $1.50 / $7.50 after)* | $0.004125 |

All six support `parse`, `ocr` and `extract` — the mode is a prompt + schema, not an endpoint, so
every model serves every mode. The 2.5 trio is the safe default; the 3.x entries come from the
models page and the changelog (3.5 Flash GA 2026-05-19, 3.5 Flash-Lite GA 2026-07-21, 3.8 Flash GA
2026-09-02) and have not been exercised against a live key here.

**Cost is computed from tokens, not from pages.** Each response's `usageMetadata` is priced exactly
(`promptTokenCount × input + (candidatesTokenCount + thoughtsTokenCount) × output`) and lands in
`usage.provider_cost_usd`, which `puffinparse-core` prefers over the price table. The per-page numbers in
`pricing.json` are only a fallback for the case where a response carries no `usageMetadata` (and for
`puffinparse providers`-style estimates). They were derived, not measured:

```
per_page_usd = (1500 × input_price_per_1M + N × output_price_per_1M) / 1e6
N = 800 output tokens for parse/ocr, 300 for extract
```

so `gemini/2.5-flash` parse = (1500 × 0.30 + 800 × 2.50) / 1e6 = $0.00245/page, and the `extract`
column of `pricing.json` is the same formula with N = 300 ($0.0012/page).

1 500 input tokens/page is a deliberately conservative blend: a PDF page bills at a flat 258 tokens
plus the rendered-image tokens, while a full-page scan image is ~1 000–1 600 tokens. Thinking tokens
are not in the estimate; with a thinking budget left at its default they can double the output side.

A model that is not in this table can be used without a registry change:
`provider_options={"model": "gemini-3.1-pro-preview"}` replaces the API model id verbatim (PuffinParse
then has no token prices for it and falls back to the registry model's per-page price).

## 3. Request flow PuffinParse uses

One call: `POST {base}/v1beta/models/{api_model}:generateContent`, headers `x-goog-api-key` and
`content-type: application/json`.

```json
{
  "contents": [{"role": "user", "parts": [
    {"inline_data": {"mime_type": "application/pdf", "data": "JVBERi0xLjQK…"}},
    {"text": "Transcribe the attached document to GitHub-flavoured Markdown, page by page. …"}
  ]}],
  "generationConfig": {
    "temperature": 0,
    "response_mime_type": "application/json",
    "response_schema": {
      "type": "object",
      "properties": {"pages": {"type": "array", "items": {
        "type": "object",
        "properties": {"page_number": {"type": "integer"}, "markdown": {"type": "string"}},
        "required": ["page_number", "markdown"],
        "propertyOrdering": ["page_number", "markdown"]}}},
      "required": ["pages"]
    }
  }
}
```

**Input handling.**

| Input | What PuffinParse does |
|---|---|
| Path / bytes ≤ 14 MB | base64 into `inline_data` |
| Path / bytes > 14 MB | resumable Files API upload, then `file_data: {file_uri, mime_type}` |
| URL | **downloaded first** (Gemini cannot fetch URLs), then treated as bytes |

The MIME type is sniffed from the magic bytes (`%PDF-`, PNG, JPEG, WebP, GIF) and only falls back to
the extension guess, because Gemini rejects a mismatched `mime_type` outright. PDFs are native input
(up to 1 000 pages / 50 MB); the accepted image types are `image/png`, `image/jpeg`, `image/webp`,
`image/heic` and `image/heif` — **not GIF**, which PuffinParse still labels correctly so that Gemini's
rejection names the real reason.

The 14 MB inline cut-off exists because a `generateContent` request is capped at ~20 MB and base64
inflates the payload by a third. The Files API path is
`POST {base}/upload/v1beta/files` with `X-Goog-Upload-Protocol: resumable` and
`X-Goog-Upload-Command: start` → the `x-goog-upload-url` response header → a second request with
`X-Goog-Upload-Command: upload, finalize` carrying the bytes → `{"file": {"uri", "name", "state"}}`;
if `state` is not `ACTIVE` PuffinParse polls `GET {base}/v1beta/files/{id}` (0.5 s, ×1.5, max 5 s) until
it is. Uploaded files expire after 48 hours.

**Per mode.**

* `parse` — the prompt above plus the `pages` schema. The model returns
  `{"pages": [{"page_number": 1, "markdown": "…"}, …]}`, so the page split is exact rather than
  guessed from a separator.
* `ocr` — not implemented natively; the trait default derives a `TextResponse` from `parse`
  (`metadata.puffinparse_derived_from = "parse"`). Lines come from the page text, words carry no boxes.
  Passing `output="text"` additionally tells the model to skip Markdown syntax.
* `extract` — the request schema is rewritten into Gemini's schema subset (see §4) and used as
  `response_schema`; the schema and `instructions` are also restated in the prompt, which measurably
  improves adherence. `ExtractResponse.data` is the parsed JSON. `citations=True` is accepted but
  cannot be honoured (`metadata.gemini_citations_unsupported = true`).

**Page selection.** Gemini always reads the whole file, so `pages` is a *prompt instruction*
("Transcribe ONLY these pages of the PDF: 2-3 … keep the original page numbers"), applied only when
the input is a PDF. It is best effort: the full document is still uploaded and still billed as input
tokens, and a model may ignore the restriction. For a non-PDF input `pages` is dropped and
`metadata.gemini_pages_ignored = true` is set.

**Where `provider_options` are merged:** the whole object is deep-merged into the request body after
PuffinParse builds it, so `generationConfig`, `safetySettings`, `systemInstruction`, `tools`, … can all be
set or overridden. Three keys are consumed by PuffinParse and never reach Gemini: `model` (API model id),
`prompt` (replaces the built-in instruction entirely) and `prompt_suffix` (appended to it).

## 4. Response mapping

```json
{
  "candidates": [{
    "content": {"parts": [{"text": "{\"pages\": [{\"page_number\": 1, \"markdown\": \"# …\"}]}"}],
                 "role": "model"},
    "finishReason": "STOP", "index": 0
  }],
  "usageMetadata": {
    "promptTokenCount": 1809, "candidatesTokenCount": 1060, "totalTokenCount": 2869,
    "promptTokensDetails": [{"modality": "DOCUMENT", "tokenCount": 1548},
                            {"modality": "TEXT", "tokenCount": 261}]
  },
  "modelVersion": "gemini-2.5-flash",
  "responseId": "HfPJaNXKO7OXm9IPk7P1kQc"
}
```

(The full payloads are `crates/puffinparse-core/tests/fixtures/gemini_parse_multipage.json` and
`gemini_extract_invoice.json`, which drive the normalisation tests.)

| Gemini field | PuffinParse unified field | Notes |
|---|---|---|
| `responseId` | `ParseResponse.provider_job_id` | Gemini has no job concept; this is the only per-call id. |
| `candidates[0].content.parts[].text` | — | All text parts are concatenated, then parsed as JSON (a stray ```` ```json ```` fence is stripped defensively). |
| `pages[].page_number` | `Page.page_number` | 1-based; falls back to the array index + 1 if the model omits it. |
| `pages[].markdown` | `Page.markdown` | Trimmed. With `output="text"` the markdown-to-text conversion is stored instead. |
| derived | `Page.text` | Always `markdown_to_text(markdown)`. |
| derived | `Page.blocks` | Exactly one `text` block per non-empty page, `content` = the page, `bbox: None`, `confidence: None`. |
| — | `Page.width` / `height` | Always `None` — **Gemini reports no geometry**. |
| number of returned pages | `Usage.pages` | For an inline PDF the `/Type /Page` count is compared against it (see below). |
| `usageMetadata` | `Usage.provider_cost_usd` | `prompt × input_price + (candidates + thoughts) × output_price`. |
| — | `Usage.credits` | Always `None`; Gemini bills tokens, not credits. |
| `usageMetadata.*` | `metadata.gemini_tokens` | `{prompt, candidates, thoughts, cached, total}`. |
| `modelVersion` | `metadata.gemini_model_version` | The exact served version behind an alias. |
| the API model id sent | `metadata.gemini_api_model` | Useful when `provider_options.model` overrode it. |
| `finishReason` ≠ `STOP` | `metadata.gemini_finish_reason` | |
| `promptFeedback.blockReason` | → error | `bad_request`, message `gemini blocked the prompt: <reason>`. |

For `extract`, the parsed JSON becomes `ExtractResponse.data` verbatim, `fields` stays empty (no
citations, no per-field confidence) and `usage.pages` is the PDF page count, or `1` for an image.

**PDF page-count sanity check.** For an inline PDF PuffinParse counts `/Type /Page` objects (ignoring
`/Type /Pages`) in the raw bytes and records it as `metadata.gemini_pdf_page_count`. If it differs
from the number of pages the model returned — a dropped or hallucinated page, or simply a
page-selection request — `metadata.gemini_page_count_mismatch = true` is set and a warning is logged.
`Usage.pages` still follows what the model returned. The count is best effort and **inline-only**:
PDFs whose page objects live in compressed object streams return no count, and neither does a
document that went through the Files API — in `extract` mode that means `usage.pages` falls back
to `1`.

**`sanitize_schema`** (public, unit-tested) rewrites a draft-2020-12 JSON Schema into the subset
Gemini's `response_schema` accepts:

* keeps only `type`, `format`, `title`, `description`, `nullable`, `enum`, `items`, `prefixItems`,
  `properties`, `required`, `propertyOrdering`, `minItems`, `maxItems`, `minimum`, `maximum`,
  `minLength`, `maxLength`, `pattern`, `anyOf` — everything else (`$schema`, `$id`,
  `additionalProperties`, `unevaluatedProperties`, `default`, `examples`, `allOf`, `not`, `if`/`then`,
  `patternProperties`, …) is dropped;
* `type: ["string", "null"]` → `type: "string"` + `nullable: true`;
* `oneOf` → `anyOf`; `const: X` → `enum: [X]` with the type inferred;
* local `$ref`s into `$defs` / `definitions` are inlined (depth-limited to 12; a recursive schema
  degrades gracefully instead of expanding forever, and an unresolvable `$ref` becomes
  `{"type": "object"}`);
* a node with `properties` but no `type` gets `type: "object"`, with `items` gets `type: "array"`.

## 5. Errors, status codes, rate limits, timeouts

Errors are `{"error": {"code": 400, "message": "…", "status": "INVALID_ARGUMENT", "details": [...]}}`;
`Error::from_http` lifts `error.message` and classifies by status:

| Status | `status` field | Trigger | PuffinParse `ErrorKind` |
|---|---|---|---|
| 400 | `INVALID_ARGUMENT` | malformed body, unsupported `mime_type`, **invalid API key**, schema Gemini rejects | `bad_request` |
| 400 | `FAILED_PRECONDITION` | free tier not available in the caller's country / billing required | `bad_request` |
| 403 | `PERMISSION_DENIED` | key lacks access to the model, or key restrictions (IP/referrer) | `authentication` |
| 404 | `NOT_FOUND` | unknown model id, or an expired Files API `file_uri` | `bad_request` |
| 429 | `RESOURCE_EXHAUSTED` | per-minute request/token quota | `rate_limit` (retried) |
| 500 / 503 | `INTERNAL` / `UNAVAILABLE` | server error, model overloaded | `provider` (retried) |
| 504 | `DEADLINE_EXCEEDED` | prompt too large to finish in time | `provider` (retried) |

Note the odd one: **an invalid API key is a 400, not a 401**, so it surfaces as `bad_request` rather
than `authentication`. A *missing* key is caught before the call and is `authentication`.

Failures that arrive with HTTP 200 are turned into errors too:

* `promptFeedback.blockReason` set, or a candidate with `finishReason: "SAFETY"` and no text →
  `bad_request` / `provider` with the reason in the message.
* `finishReason: "MAX_TOKENS"` → `provider`, "gemini hit the output token limit, so the JSON is
  truncated — raise `generationConfig.maxOutputTokens` … or parse fewer pages per call".
* Text that is not the promised JSON → `provider`, with the first 200 characters in the message.

**Retries.** `max_retries` (default 2) with exponential backoff and full jitter, on 429, 5xx and
network errors only (the shared `http::with_retry`). Gemini's 429 body sometimes carries a
`RetryInfo` detail with `retryDelay`; PuffinParse does not read it yet and backs off blind.

**Rate limits.** Counted per project and per model in three dimensions — requests/minute, *input*
tokens/minute and requests/day — and they depend on the usage tier, so the authoritative numbers are
the ones shown in Google AI Studio rather than any number quoted here. A single large document can
exhaust the token dimension in very few requests.

**Timeouts.** `timeout_secs` (default 300) is the whole-call deadline and caps every individual
request, including the Files API upload and the URL download.

## 6. Gotchas

* **Unverified against a live key.** Everything here follows the published wire format, and both the
  normalisation path (fixture tests) and the transport (`mod transport` — a loopback HTTP server that
  asserts the exact URL, headers, base64 `inline_data`, schema, 429 retry and 400 mapping) are
  covered by tests, but the `GEMINI_API_KEY` present in the development environment is rejected by
  Google with `400 API_KEY_INVALID` on every endpoint, so no call has ever reached the real service.
  The fixtures under `tests/fixtures/gemini_*.json` were therefore **constructed from the documented
  response schema**, not captured. Run
  `cargo test -p puffinparse-core gemini_live -- --ignored --nocapture` with a working key, then replace
  the fixtures with real payloads and fill in measured latency and cost here.
* **No geometry, ever.** `Block.bbox`, `Page.width`/`height` and `Block.confidence` are always
  `None`. `ocr` mode returns words without boxes. This is a property of the model, not of PuffinParse.
* **The page split comes from the model, not from the file.** Structured output makes it reliable in
  practice, but a model can still merge or drop a page. `metadata.gemini_page_count_mismatch` is the
  tripwire for PDFs; there is no equivalent check for multi-page TIFFs or images.
* **`pages` is a suggestion.** Whole-file upload, prompt-level selection, full input billing.
* **Thinking tokens are billed as output and are invisible in `candidatesTokenCount`.** On 2.5 models
  a page of dense tables can spend more on reasoning than on the transcript. Set
  `provider_options={"generationConfig": {"thinkingConfig": {"thinkingBudget": 0}}}` to turn thinking
  off on Flash / Flash-Lite (Pro cannot go below 128; Gemini 3 models use `thinkingLevel` instead).
* **Long documents hit `MAX_TOKENS` before they hit the page limit.** The 1 000-page ceiling is
  theoretical: the transcript of ~40 dense pages already approaches the 64k output budget. Split long
  PDFs with `pages`, or raise `generationConfig.maxOutputTokens`.
* **`temperature: 0` does not make the output deterministic.** Repeated runs differ slightly, which
  matters for benchmark reproducibility.
* **Gemini cannot fetch a URL.** PuffinParse downloads it first; a URL that needs authentication has to
  be fetched by the caller and passed as bytes.
* **`additionalProperties` is the classic 400.** Most schema generators (Pydantic, zod) emit it and
  Gemini rejects it; `sanitize_schema` strips it, along with `$schema`, `default` and `examples`.
  The current docs page lists `additionalProperties` as supported, but rejecting it has been the
  observed behaviour for long enough that stripping is the safe default.
* **The API is versioned in the path and the newest surface is moving.** Google's *Interactions API*
  (`POST /v1beta/interactions`, GA June 2026) is now the documented default and uses a different
  shape (`input`, `steps`, `response_format`). `generateContent` remains supported and is still the
  recommended path for stable deployments — PuffinParse targets it deliberately; expect the docs links
  below to show the Interactions shape.
* **Files API objects expire after 48 hours** and are scoped to the project, so a `file_uri` cannot be
  reused across keys. PuffinParse uploads per call and never reuses or deletes (files are free and
  capped at 20 GB per project).
* **Image vs. PDF tokenisation differ.** A PDF page is a flat 258 tokens plus image tokens; a
  standalone image is tiled at 768×768 (≈258 tokens per tile). A scan sent as PNG can therefore cost
  several times what the same page costs inside a PDF.
* **`x-goog-api-key` and `?key=` are equivalent**; PuffinParse uses the header so keys never land in
  request logs or URLs.

## 7. Useful `provider_options` passthrough

```python
# 1. Turn thinking off for cheap, fast transcription (Flash / Flash-Lite only).
puffinparse.parse("scan.png", model="gemini/2.5-flash",
              provider_options={"generationConfig": {"thinkingConfig": {"thinkingBudget": 0}}})

# 2. Raise the output budget for a long PDF.
puffinparse.parse("report.pdf", model="gemini/2.5-pro",
              provider_options={"generationConfig": {"maxOutputTokens": 65536}})

# 3. Reach a model that is not in the registry.
puffinparse.parse("doc.pdf", model="gemini/2.5-flash",
              provider_options={"model": "gemini-3.1-pro-preview"})

# 4. Steer the transcription without rewriting the whole prompt.
puffinparse.parse("statement.pdf", model="gemini/2.5-flash",
              provider_options={"prompt_suffix": "Keep every stamp and handwritten note, "
                                                 "and transcribe struck-through text as ~~text~~."})

# 5. Replace the instruction entirely (the pages schema still applies).
puffinparse.parse("form.pdf", model="gemini/2.5-flash-lite",
              provider_options={"prompt": "Return each page of this form as a Markdown table of "
                                          "field name and value, one row per field."})

# 6. Loosen safety filters for documents that trip them (medical, legal, incident reports).
puffinparse.parse("report.pdf", model="gemini/2.5-flash",
              provider_options={"safetySettings": [
                  {"category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "BLOCK_NONE"},
                  {"category": "HARM_CATEGORY_HARASSMENT", "threshold": "BLOCK_NONE"}]})

# 7. A system instruction, e.g. to pin the output language.
puffinparse.parse("brief.pdf", model="gemini/2.5-flash",
              provider_options={"systemInstruction": {"parts": [{"text": "Always answer in German."}]}})
```

## 8. Links

* Docs home: <https://ai.google.dev/gemini-api/docs>
* Document (PDF) understanding: <https://ai.google.dev/gemini-api/docs/document-processing>
* Image understanding: <https://ai.google.dev/gemini-api/docs/image-understanding>
* Structured output: <https://ai.google.dev/gemini-api/docs/structured-output>
* Models: <https://ai.google.dev/gemini-api/docs/models> · live list: `GET /v1beta/models`
* Pricing: <https://ai.google.dev/gemini-api/docs/pricing>
* Rate limits: <https://ai.google.dev/gemini-api/docs/rate-limits>
* Files API: <https://ai.google.dev/gemini-api/docs/files>
* `generateContent` reference: <https://ai.google.dev/api/generate-content>
* API versions: <https://ai.google.dev/gemini-api/docs/api-versions>
* Interactions API (the newer surface PuffinParse does *not* use):
  <https://ai.google.dev/gemini-api/docs/migrate-to-interactions>
