# TypeScript / Node.js SDK

The `liteocr` npm package is the same Rust core as the Python SDK and the CLI, loaded into Node.js
as a native addon (N-API, built with [napi-rs](https://napi.rs)). Every call returns a `Promise`;
responses are plain camelCase objects typed by the bundled `index.d.ts`. No provider logic lives in
JavaScript, so a model behaves identically from Python, Node, Rust and the CLI.

Node.js 18+. Source: [`js/`](../../js/README.md) (wrapper and typings) and
[`crates/liteocr-node`](../../crates/liteocr-node/src/lib.rs) (the addon).

## Install

Prebuilt binaries are not published to npm yet, so build the addon from a clone (needs a Rust
toolchain):

```bash
git clone https://github.com/ajinkyashejul/liteocr && cd liteocr/js
npm install
npm run build          # cargo build --release of crates/liteocr-node -> liteocr.<platform>.node
npm test               # offline unit tests
```

Then depend on it by path (`npm install ../liteocr/js`) or `npm link`. Keys come from the same
environment variables as every other surface (`REDUCTO_API_KEY`, `LLAMA_API_KEY`, `EXTEND_API_KEY`,
...; see [Providers](/providers/)).

## First call

```ts
import { parse, ocr, extract } from 'liteocr'

const doc = await parse('invoice.pdf', { model: 'llamaparse/cost_effective' })
console.log(doc.markdown, doc.costUsd, doc.pages[0].blocks[0].bbox)

const text = await ocr('scan.png', { model: 'mistral/ocr-latest' })
console.log(text.pages[0].lines.length)

const inv = await extract<{ total: number }>('invoice.pdf', {
  model: 'reducto/extract',
  schema: { type: 'object', properties: { total: { type: 'number' } } },
  citations: true,
})
console.log(inv.data.total, inv.fields['/total']?.citations)
```

CommonJS works the same: `const liteocr = require('liteocr')`.

## Modes

As everywhere in LiteOCR, the mode decides the response type, and a model that does not serve the
mode you asked for rejects with `UnsupportedModelError` before any network call.

| Mode | Call | Resolves to |
|---|---|---|
| `parse` | `parse(doc, options?)` | `ParseResponse` — markdown + typed blocks with boxes |
| `ocr` | `ocr(doc, options?)` | `TextResponse` — plain text + line/word boxes |
| `extract` | `extract(doc, { schema, ... })` | `ExtractResponse<T>` — your schema's object + citations |

## Documents

`doc` is one of:

| Form | Example |
|---|---|
| Path string | `'invoice.pdf'` |
| URL string | `'https://example.com/invoice.pdf'` (passed to the provider as a remote URL) |
| `URL` | `new URL('https://...')` or `new URL('file:///tmp/a.pdf')` |
| Bytes | `Buffer`, `Uint8Array` or `ArrayBuffer` — pass `filename` too (it sets the document type) |
| Object | `{ url }`, `{ path }` or `{ data, filename }` |

Bytes are handed to the core as a `Buffer`, never base64-encoded through JSON.

## Options

The options mirror SPEC §4 and the Python keywords, in camelCase:

| Option | Default | Meaning |
|---|---|---|
| `model` | `'reducto'` | `"<provider>/<model>"`; a bare provider picks its default model for the mode. |
| `fallbacks` | `[]` | More models to try, in order, when `model` fails with a retryable error (a one-off ordered `Router`). |
| `filename` | — | Required with bytes. |
| `pages` | all | `'1-3,7'`, `2` or `[1, 2, 5]`, 1-based, forwarded best-effort. |
| `language` | — | Language hint when the provider supports one. |
| `output` | `'markdown'` | `parse` only: preferred block content, `'markdown'` or `'text'`. |
| `outputFormat` | unified | `parse` / `extract`: `'reducto'`, `'extend'` or `'llamaparse'` returns that vendor's JSON shape, verbatim ([compatibility](/project/compat/)). `'liteocr'` is the unified response. |
| `providerOptions` | — | Provider-specific options merged verbatim into the provider request. |
| `includeRaw` | `false` | Attach the provider payload as `response.raw`. |
| `timeout` | `300` | Whole-call deadline in **seconds** (upload + polling + download), as in Python and the CLI. |
| `maxRetries` | `2` | Retries on 429 / 5xx / network errors with exponential backoff. |
| `apiKey`, `baseUrl` | env | Override the key or the provider base URL. |
| `metadata` | `{}` | Echoed back in `response.metadata`. |
| `schema`, `instructions`, `citations` | — | `extract` only; `schema` is required and must be a JSON Schema object. |

Unknown option names are a `TypeError` that lists the accepted ones, so a snake_case typo such as
`provider_options` fails loudly instead of being ignored.

## Responses

The interfaces in `index.d.ts` follow [SPEC §5](/project/spec/) field for field, in camelCase.
Optional values are `null` (never missing), lists are always present, and three things are returned
exactly as received: `data` (your extraction), `metadata` (your keys plus `liteocr_*` keys such as
`liteocr_fallback_index` and `liteocr_derived_from`) and `raw`. `fields` keeps its JSON-pointer keys.

```ts
interface ParseResponse {
  id: string; provider: string; model: string; providerJobId: string | null
  pages: Page[]; markdown: string; text: string
  usage: { pages: number; credits: number | null; providerCostUsd: number | null }
  costUsd: number | null; latencyMs: number; createdAt: string
  metadata: Record<string, unknown>; raw: unknown
}
interface Page { pageNumber: number; width: number | null; height: number | null; markdown: string; text: string; blocks: Block[] }
interface Block { type: BlockType; content: string; text: string | null; bbox: BBox | null; confidence: number | null; pageNumber: number }
// TextResponse: same envelope + pages: TextPage[] ({ pageNumber, width, height, text, lines, words }) and text
// ExtractResponse<T>: same envelope + data: T and fields: Record<pointer, { confidence, citations }>
```

`BBox` is `{ x0, y0, x1, y1 }`, normalised 0..1 with the origin top-left.

## Router

```ts
import { Router } from 'liteocr'

const router = new Router({
  models: ['reducto/standard', 'llamaparse/agentic', 'extend/parse_light'],
  mode: 'parse',                 // default; every model must serve it
  strategy: 'ordered',           // or 'round_robin'
  fallbackOn: ['provider', 'rate_limit', 'timeout', 'network'],  // the default
})
const doc = await router.parse('contract.pdf')
router.stats()   // { 'reducto/standard': { successes, failures, totalLatencyMs, totalCostUsd, totalPages, avgLatencyMs }, ... }
router.plan()    // the order the next call would try
```

`fallbackOn` also accepts class names (`'ProviderError'`) or the classes themselves. Calling a
method for another mode (`router.ocr(...)` on a parse router) rejects with `InputError`. Per-call
options are the module-level ones minus `model` and `fallbacks`. When a fallback served the call,
`response.metadata.liteocr_fallback_index` says which.

## Async jobs and webhooks

`parse()` waits for the provider (polling job-queue providers for you). For long documents,
batches or webhook-driven pipelines, split the call in two and own the waiting yourself. Jobs are
`parse` mode only and need a provider with a job queue: `reducto`, `extend`, `llamaparse`.

```ts
import { submit, retrieve, handleWebhook, type Job } from 'liteocr'

const job: Job = await submit('200-pages.pdf', {
  model: 'reducto/standard',
  webhookUrl: 'https://example.com/hooks/liteocr',   // optional, see below
})
await queue.put(JSON.stringify(job))                  // a Job never holds an API key

// later, anywhere:
const result = await retrieve(JSON.parse(stored))     // the Job again while running, else ParseResponse
if ('jobId' in result) console.log('still running', result.jobId)
else console.log(result.markdown)
```

`submit(doc, options)` takes `parse()`'s options except `fallbacks` and `outputFormat`, plus
`webhookUrl`; `timeout` covers the upload and submission only. It resolves to a `Job`:

```ts
interface Job {
  provider: string; model: string; jobId: string; submittedAt: string
  output: 'markdown' | 'text'; includeRaw: boolean; baseUrl: string | null
  providerState: Record<string, unknown> | null   // non-secret options retrieve needs (Extend workspace_id)
  metadata: Record<string, unknown>
}
```

`retrieve(job, { apiKey?, baseUrl?, timeout = 120, maxRetries = 2, outputFormat? })` asks the
provider once. It resolves to the **same** `Job` object while the job is pending, or to the
`ParseResponse` (normalised exactly like `parse()`, `latencyMs` counted from submission; a
vendor shape with `outputFormat`) once it is done. A job the provider reports as failed rejects
with the typed `LiteOCRError`, `jobId` set, exactly like a failed `parse()`. The key is read from
the environment again unless you pass `apiKey`.

`webhookUrl` maps to Reducto `async.webhook` (direct mode) and LlamaParse `webhook_url`; Extend
has no per-job webhook (register an endpoint in its dashboard) and rejects it with `InputError`.
In your web handler, verify the provider's signature or your own secret first, then:

```ts
app.post('/hooks/liteocr', async (req, res) => {
  const result = await handleWebhook(req.body, { model: 'reducto' })   // Job | ParseResponse
  res.sendStatus(204)
})
```

`handleWebhook(payload, { model = 'reducto', apiKey?, baseUrl?, timeout?, maxRetries?,
outputFormat? })` accepts the parsed body, a JSON string or bytes. Bodies that carry the whole
result (a LlamaParse `webhook_url` push) are normalised directly; bodies that only name a finished
job (Reducto, Extend `parse_run.*`, LlamaCloud `parse.*` events) trigger one `retrieve()`; a
pending event resolves to its `Job`; a failure event rejects with the typed error. See SPEC §15
for every provider's body shape.

## Errors

Every provider or core failure rejects with a subclass of `LiteOCRError`, mapped from the core's
`ErrorKind`; argument mistakes are plain `TypeError`s thrown before anything else runs.

| Class | `kind` | When |
|---|---|---|
| `AuthenticationError` | `authentication` | 401/403, or no API key configured |
| `RateLimitError` | `rate_limit` | 429 after retries |
| `BadRequestError` | `bad_request` | other 4xx, or an unknown `outputFormat` |
| `ProviderError` | `provider` | 5xx, malformed payload, failed job |
| `TimeoutError` | `timeout` | the whole-call deadline passed |
| `UnsupportedModelError` | `unsupported_model` | unknown model, or one that does not serve the mode |
| `InputError` | `input` | unreadable file, bytes without `filename`, bad mode, router mode mismatch |
| `NetworkError` | `network` | network / TLS / DNS failure after retries |

Each carries `message` (the provider's own message, verbatim), `provider`, `statusCode`, `jobId`
and `retryable`; `toJSON()` returns all of them.

```ts
import { parse, AuthenticationError, LiteOCRError } from 'liteocr'

try {
  await parse('a.pdf', { model: 'reducto/standard' })
} catch (e) {
  if (e instanceof AuthenticationError) console.error('set REDUCTO_API_KEY')
  else if (e instanceof LiteOCRError) console.error(e.kind, e.statusCode, e.message)
  else throw e
}
```

## Models, pricing and scoring

```ts
import * as liteocr from 'liteocr'

liteocr.listModels()                 // every "<provider>/<model>"
liteocr.listModels('extract')        // only models serving extract
liteocr.resolveModel('reducto')      // 'reducto/standard'
liteocr.providers()                  // [{ name, displayName, envVar, baseUrl, docs, models }]
liteocr.estimateCost('reducto/standard', 100)          // USD, or null when unpriced
liteocr.setPricing({ 'reducto/standard': 0.012 })      // per page, mode defaults to 'parse'
liteocr.resetPricing()
liteocr.outputFormats()              // ['liteocr', 'reducto', 'extend', 'llamaparse']
liteocr.score(prediction, truth)     // { charSimilarity, cer, wer, wordF1, ..., tableScore }
liteocr.normalizeText(text)          // the normalisation applied before scoring
liteocr.initLogging('debug')         // core tracing on stderr
```

## Differences from the Python SDK

- Async only: every mode returns a `Promise` (there is no blocking variant); `submit` /
  `retrieve` / `handleWebhook` match Python's `asubmit` / `aretrieve` / `ahandle_webhook`, and
  `retrieve` / `handleWebhook` also take `outputFormat`.
- `fallbacks` on a single call is a JavaScript convenience over `Router`.
- Success/failure callbacks are not mirrored; wrap the promise instead.
- Responses are plain objects, so the Python conveniences (`.tables`, `.num_pages`,
  `.field_info()`) are one-liners over `pages` / `fields`.

## See also

- [Python SDK](/python/) — the same surface in Python.
- [Specification](/project/spec/) — the unified request, response and error model.
- [`js/README.md`](../../js/README.md) — building, testing and the release plan for prebuilt binaries.
