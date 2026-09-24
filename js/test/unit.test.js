'use strict'

// Offline unit tests: no provider is contacted. The end-to-end cases talk to a local HTTP server
// that replays the recorded Mistral OCR fixtures from crates/liteocr-core/tests/fixtures/.

const { test, describe, before, after } = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const http = require('node:http')
const path = require('node:path')

const liteocr = require('..')
const {
  LiteOCRError,
  AuthenticationError,
  BadRequestError,
  InputError,
  ProviderError,
  UnsupportedModelError,
  Router,
} = liteocr

const FIXTURES = path.join(__dirname, '..', '..', 'crates', 'liteocr-core', 'tests', 'fixtures')
const fixture = (name) => fs.readFileSync(path.join(FIXTURES, name), 'utf8')
const PNG = Buffer.from('89504e470d0a1a0a0000000d49484452', 'hex') // tiny, inlined as a data URL

describe('registry and helpers', () => {
  test('exports the version and modes', () => {
    assert.match(liteocr.VERSION, /^\d+\.\d+\.\d+/)
    assert.deepEqual(liteocr.modes(), ['parse', 'ocr', 'extract'])
    assert.deepEqual([...liteocr.MODES], liteocr.modes())
  })

  test('listModels lists qualified names and filters by mode', () => {
    const all = liteocr.listModels()
    assert.ok(all.includes('reducto/standard'))
    assert.ok(all.includes('llamaparse/cost_effective'))
    assert.ok(all.every((m) => /^[a-z_]+\/.+/.test(m)))
    const extract = liteocr.listModels('extract')
    assert.ok(extract.length > 0)
    assert.ok(extract.every((m) => all.includes(m)))
    assert.ok(!extract.includes('reducto/standard'), 'a parse-only model is not listed for extract')
  })

  test('listModels rejects an unknown mode with InputError', () => {
    assert.throws(
      () => liteocr.listModels('bogus'),
      (e) => e instanceof InputError && e.kind === 'input' && /unknown mode/.test(e.message),
    )
  })

  test('resolveModel canonicalises bare providers per mode', () => {
    assert.equal(liteocr.resolveModel('reducto'), 'reducto/standard')
    assert.equal(liteocr.resolveModel('reducto', 'extract'), 'reducto/extract')
  })

  test('outputFormats lists every accepted format', () => {
    assert.deepEqual(liteocr.outputFormats(), ['liteocr', 'reducto', 'extend', 'llamaparse'])
  })

  test('providers are camelCased metadata', () => {
    const reducto = liteocr.providers().find((p) => p.name === 'reducto')
    assert.equal(reducto.envVar, 'REDUCTO_API_KEY')
    assert.equal(reducto.displayName, 'Reducto')
    assert.ok(reducto.models.some((m) => m.model === 'standard' && m.modes.includes('parse')))
  })
})

describe('pricing', () => {
  after(() => liteocr.resetPricing())

  test('estimateCost multiplies the list price by pages', () => {
    const price = liteocr.pricing()['reducto/standard'].parse
    assert.equal(typeof price, 'number')
    assert.ok(Math.abs(liteocr.estimateCost('reducto/standard', 10) - price * 10) < 1e-12)
    // A bare provider resolves to its default model first.
    assert.equal(liteocr.estimateCost('reducto', 10), liteocr.estimateCost('reducto/standard', 10))
  })

  test('estimateCost is null when the model has no price in that mode', () => {
    assert.equal(liteocr.pricing()['reducto/standard'].extract, undefined)
    assert.equal(liteocr.estimateCost('reducto/standard', 3, 'extract'), null)
  })

  test('setPricing overrides one mode and resetPricing restores it', () => {
    const before = liteocr.estimateCost('reducto/standard', 4)
    liteocr.setPricing({ 'reducto/standard': 0.5 })
    assert.equal(liteocr.estimateCost('reducto/standard', 4), 2)
    assert.equal(liteocr.pricing()['reducto/standard'].source, 'user')
    liteocr.resetPricing()
    assert.equal(liteocr.estimateCost('reducto/standard', 4), before)
  })

  test('estimateCost validates its arguments', () => {
    assert.throws(() => liteocr.estimateCost('nope/x', 1), UnsupportedModelError)
    assert.throws(() => liteocr.estimateCost('reducto', -1), TypeError)
    assert.throws(() => liteocr.estimateCost('reducto', 1.5), TypeError)
    assert.throws(() => liteocr.estimateCost('reducto', 1, 'bogus'), InputError)
    assert.throws(() => liteocr.setPricing({ 'reducto/standard': 'free' }), TypeError)
  })
})

describe('score', () => {
  test('identical text scores perfectly, with camelCase metrics', () => {
    const m = liteocr.score('# Hello **world**', 'hello world')
    assert.equal(m.charSimilarity, 1)
    assert.equal(m.cer, 0)
    assert.equal(m.wer, 0)
    assert.equal(m.wordF1, 1)
    assert.equal(m.tableScore, null)
    assert.equal(m.tedsGrid, null)
    assert.equal(m.rulePassRate, null)
    assert.ok(!('char_similarity' in m))
  })

  test('differences lower the score and options are honoured', () => {
    const m = liteocr.score('hello there', 'hello world')
    assert.ok(m.charSimilarity < 1)
    assert.equal(m.wordRecall, 0.5)
    const strict = liteocr.score('Hello', 'hello', { caseInsensitive: false })
    assert.ok(strict.charSimilarity < 1)
    assert.equal(liteocr.normalizeText('# Hi  **there**'), 'hi there')
    assert.equal(liteocr.markdownToText('**bold**'), 'bold')
  })

  test('score validates its arguments', () => {
    assert.throws(() => liteocr.score(1, 'a'), TypeError)
    assert.throws(() => liteocr.score('a', 'b', { bogus: true }), TypeError)
  })
})

describe('errors and argument validation (no network)', () => {
  test('a bad model string rejects with UnsupportedModelError before any call', async () => {
    await assert.rejects(liteocr.parse('missing.pdf', { model: 'nope/x' }), (e) => {
      assert.ok(e instanceof UnsupportedModelError)
      assert.ok(e instanceof LiteOCRError)
      assert.ok(e instanceof Error)
      assert.equal(e.kind, 'unsupported_model')
      assert.equal(e.name, 'UnsupportedModelError')
      assert.equal(e.retryable, false)
      assert.match(e.message, /unknown provider 'nope'/)
      assert.deepEqual(Object.keys(e.toJSON()).sort(), [
        'jobId',
        'kind',
        'message',
        'name',
        'provider',
        'retryable',
        'statusCode',
      ])
      return true
    })
  })

  test('a model that does not serve the mode names the ones that do', async () => {
    await assert.rejects(
      liteocr.extract('missing.pdf', { model: 'reducto/standard', schema: { type: 'object' } }),
      (e) => e instanceof UnsupportedModelError && /Models for mode 'extract': .*reducto\/extract/.test(e.message),
    )
  })

  test('resolveModel throws synchronously for unknown models', () => {
    assert.throws(() => liteocr.resolveModel('reducto/nope'), UnsupportedModelError)
  })

  test('document arguments are validated', async () => {
    await assert.rejects(liteocr.parse(42), TypeError)
    await assert.rejects(liteocr.parse(''), InputError)
    await assert.rejects(liteocr.parse(Buffer.from('x')), (e) => e instanceof InputError && /filename/.test(e.message))
    await assert.rejects(liteocr.parse(new Uint8Array(0), { filename: 'a.pdf' }), InputError)
    await assert.rejects(liteocr.parse({ url: 'ftp://x/a.pdf' }), InputError)
    await assert.rejects(liteocr.parse({ nothing: true }), TypeError)
  })

  test('options are validated before any call', async () => {
    await assert.rejects(liteocr.parse('a.pdf', { provider_options: {} }), /unknown option\(s\) "provider_options"/)
    await assert.rejects(liteocr.parse('a.pdf', { timeout: -1 }), TypeError)
    await assert.rejects(liteocr.parse('a.pdf', { maxRetries: 1.5 }), TypeError)
    await assert.rejects(liteocr.parse('a.pdf', { pages: {} }), TypeError)
    await assert.rejects(liteocr.parse('a.pdf', { output: 'html' }), TypeError)
    await assert.rejects(liteocr.parse('a.pdf', 'reducto'), TypeError)
    await assert.rejects(liteocr.ocr('a.pdf', { outputFormat: 'reducto' }), TypeError)
    await assert.rejects(liteocr.extract('a.pdf', { model: 'reducto/extract' }), /schema must be a JSON Schema object/)
    await assert.rejects(liteocr.extract('a.pdf', { schema: [] }), TypeError)
    await assert.rejects(liteocr.parse('a.pdf', { fallbacks: 'llamaparse' }), TypeError)
  })

  test('an unknown outputFormat is a BadRequestError listing the choices', async () => {
    await assert.rejects(
      liteocr.parse('a.pdf', { outputFormat: 'docx' }),
      (e) => e instanceof BadRequestError && e.kind === 'bad_request' && /liteocr \| reducto/.test(e.message),
    )
  })

  test('a missing file is an InputError', async () => {
    await assert.rejects(
      liteocr.parse(path.join(__dirname, 'does-not-exist.pdf'), { model: 'mistral/ocr-latest', apiKey: 'k' }),
      InputError,
    )
  })
})

describe('Router construction', () => {
  test('validates models against its mode and exposes plan/stats', () => {
    const r = new Router({ models: ['reducto', 'llamaparse/agentic'], strategy: 'round_robin' })
    assert.deepEqual(r.models, ['reducto/standard', 'llamaparse/agentic'])
    assert.equal(r.mode, 'parse')
    assert.deepEqual(r.plan(), ['reducto/standard', 'llamaparse/agentic'])
    assert.deepEqual(r.plan(), ['llamaparse/agentic', 'reducto/standard'])
    assert.deepEqual(r.stats()['reducto/standard'], {
      successes: 0,
      failures: 0,
      totalLatencyMs: 0,
      totalCostUsd: 0,
      totalPages: 0,
      avgLatencyMs: null,
    })
    assert.match(String(r), /^Router\(models=/)
  })

  test('rejects bad configuration', async () => {
    assert.throws(() => new Router({ models: [] }), InputError)
    assert.throws(() => new Router({ models: ['reducto/standard'], mode: 'extract' }), UnsupportedModelError)
    assert.throws(() => new Router({ models: ['reducto'], strategy: 'random' }), InputError)
    assert.throws(() => new Router({ models: ['reducto'], fallbackOn: ['nonsense'] }), TypeError)
    assert.throws(() => new Router(['reducto']), TypeError)
    assert.throws(() => new Router({ models: ['reducto'], modle: 'x' }), TypeError)
    const r = new Router({ models: ['reducto'], fallbackOn: ['provider', 'RateLimitError', ProviderError] })
    await assert.rejects(r.ocr('a.pdf'), (e) => e instanceof InputError && /cannot serve 'ocr'/.test(e.message))
    await assert.rejects(r.parse('a.pdf', { model: 'x' }), TypeError)
  })
})

describe('end to end against a local mock provider', () => {
  let server
  let baseUrl
  let queue = []
  const requests = []

  before(async () => {
    server = http.createServer((req, res) => {
      let body = ''
      req.on('data', (c) => (body += c))
      req.on('end', () => {
        requests.push({ url: req.url, auth: req.headers.authorization, body: body ? JSON.parse(body) : null })
        const [status, payload] = queue.shift() ?? [500, '{"message":"mock queue empty"}']
        res.writeHead(status, { 'content-type': 'application/json' })
        res.end(payload)
      })
    })
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
    baseUrl = `http://127.0.0.1:${server.address().port}`
  })

  after(() => new Promise((resolve) => server.close(resolve)))

  const opts = (extra = {}) => ({ model: 'mistral/ocr-latest', apiKey: 'test-key', baseUrl, maxRetries: 0, ...extra })

  test('parse returns a camelCase ParseResponse', async () => {
    queue = [[200, fixture('mistral_ocr.json')]]
    const resp = await liteocr.parse(PNG, opts({ filename: 'scan.png', metadata: { my_key: 1 } }))
    const sent = requests.at(-1)
    assert.equal(sent.url, '/v1/ocr')
    assert.equal(sent.auth, 'Bearer test-key')
    assert.equal(resp.provider, 'mistral')
    assert.equal(resp.model, 'mistral/ocr-latest')
    assert.equal(resp.pages.length, 2)
    assert.equal(resp.pages[0].pageNumber, 1)
    assert.match(resp.markdown, /Invoice #1234/)
    assert.equal(typeof resp.latencyMs, 'number')
    assert.equal(typeof resp.createdAt, 'string')
    assert.equal(resp.usage.pages, 2)
    assert.ok(resp.costUsd > 0)
    assert.equal(resp.raw, null)
    assert.equal(resp.metadata.my_key, 1, 'metadata keys are returned verbatim')
    const block = resp.pages[0].blocks[0]
    assert.deepEqual(Object.keys(block).sort(), ['bbox', 'confidence', 'content', 'pageNumber', 'text', 'type'])
    for (const k of ['providerJobId', 'costUsd', 'latencyMs', 'createdAt', 'metadata', 'raw', 'usage']) {
      assert.ok(k in resp, `envelope has ${k}`)
    }
  })

  test('ocr returns a TextResponse and includeRaw attaches the payload', async () => {
    queue = [[200, fixture('mistral_ocr.json')]]
    const resp = await liteocr.ocr({ data: PNG, filename: 'scan.png' }, opts({ includeRaw: true }))
    assert.equal(resp.pages[0].pageNumber, 1)
    assert.ok(Array.isArray(resp.pages[0].lines))
    assert.ok(Array.isArray(resp.pages[0].words))
    assert.match(resp.text, /Invoice #1234/)
    assert.ok(resp.raw && Array.isArray(resp.raw.pages), 'raw is the untouched provider JSON')
  })

  test('extract returns data verbatim and an empty fields map', async () => {
    queue = [[200, fixture('mistral_annotation.json')]]
    const schema = { type: 'object', properties: { invoice_number: { type: 'string' } } }
    const resp = await liteocr.extract(PNG, opts({ filename: 'scan.png', schema, instructions: 'be exact' }))
    assert.equal(requests.at(-1).body.document_annotation_prompt, 'be exact')
    assert.ok(resp.data && typeof resp.data === 'object')
    assert.ok(Object.keys(resp.data).some((k) => k.includes('_')), 'extracted keys are not camelCased')
    assert.deepEqual(resp.fields, {})
  })

  test('outputFormat renders the vendor shape', async () => {
    queue = [[200, fixture('mistral_ocr.json')]]
    const resp = await liteocr.parse(PNG, opts({ filename: 'scan.png', outputFormat: 'reducto' }))
    assert.ok(resp.result && Array.isArray(resp.result.chunks), 'Reducto-shaped response')
  })

  test('HTTP 401 maps to AuthenticationError and keeps the provider message', async () => {
    queue = [[401, '{"message":"Invalid API key from mock"}']]
    await assert.rejects(liteocr.parse(PNG, opts({ filename: 'scan.png' })), (e) => {
      assert.ok(e instanceof AuthenticationError)
      assert.equal(e.kind, 'authentication')
      assert.equal(e.statusCode, 401)
      assert.equal(e.provider, 'mistral')
      assert.equal(e.message, 'Invalid API key from mock')
      return true
    })
  })

  test('HTTP 500 maps to a retryable ProviderError', async () => {
    queue = [[500, '{"detail":"boom"}']]
    await assert.rejects(liteocr.parse(PNG, opts({ filename: 'scan.png' })), (e) => {
      assert.ok(e instanceof ProviderError)
      assert.equal(e.statusCode, 500)
      assert.equal(e.retryable, true)
      assert.equal(e.message, 'boom')
      return true
    })
  })

  test('fallbacks move on after a retryable failure and say so in metadata', async () => {
    queue = [
      [503, '{"message":"overloaded"}'],
      [200, fixture('mistral_ocr.json')],
    ]
    const resp = await liteocr.parse(PNG, opts({ filename: 'scan.png', fallbacks: ['mistral/ocr-4-1'] }))
    assert.equal(resp.model, 'mistral/ocr-4-1')
    assert.equal(resp.metadata.liteocr_fallback_index, 1)
  })

  test('Router records per-model stats', async () => {
    queue = [[200, fixture('mistral_ocr.json')]]
    const router = new Router({ models: ['mistral/ocr-latest'], mode: 'ocr' })
    const resp = await router.ocr(PNG, { filename: 'scan.png', apiKey: 'test-key', baseUrl, maxRetries: 0 })
    assert.equal(resp.model, 'mistral/ocr-latest')
    const s = router.stats()['mistral/ocr-latest']
    assert.equal(s.successes, 1)
    assert.equal(s.totalPages, 2)
    assert.equal(typeof s.avgLatencyMs, 'number')
  })
})
