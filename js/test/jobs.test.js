'use strict'

// Jobs API (submit / retrieve / handleWebhook) against a local server that emulates Reducto's
// POST /parse_async + GET /job/{id}, replaying the recorded parse fixture. No provider is contacted.

const { test, describe, before, after, beforeEach } = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const http = require('node:http')
const path = require('node:path')

const puffinparse = require('..')
const { AuthenticationError, InputError, ProviderError, UnsupportedModelError } = puffinparse

const FIXTURES = path.join(__dirname, '..', '..', 'crates', 'puffinparse-core', 'tests', 'fixtures')
const REDUCTO_RESULT = JSON.parse(fs.readFileSync(path.join(FIXTURES, 'reducto_parse.json'), 'utf8'))
const DOC = { url: 'https://example.com/invoice.pdf' } // Reducto takes URLs as-is: no upload step

const job = (status, extra = {}) => [200, JSON.stringify({ status, ...extra })]

describe('async jobs against a mock Reducto', () => {
  let server
  let baseUrl
  let queue = []
  let seen = []

  before(async () => {
    server = http.createServer((req, res) => {
      let body = ''
      req.on('data', (c) => (body += c))
      req.on('end', () => {
        let parsed = null
        try {
          parsed = body ? JSON.parse(body) : null
        } catch {
          parsed = body
        }
        seen.push({ method: req.method, url: req.url, auth: req.headers.authorization, body: parsed })
        const [status, payload] = queue.shift() ?? [500, '{"detail":"mock queue empty"}']
        res.writeHead(status, { 'content-type': 'application/json' })
        res.end(payload)
      })
    })
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
    baseUrl = `http://127.0.0.1:${server.address().port}`
  })

  after(() => new Promise((resolve) => server.close(resolve)))

  beforeEach(() => {
    queue = []
    seen = []
  })

  const submitOpts = (extra = {}) => ({ model: 'reducto/standard', apiKey: 'test-key', baseUrl, maxRetries: 0, ...extra })

  test('submit returns a camelCase Job and forwards the webhook', async () => {
    queue = [[200, '{"job_id":"rj-42"}']]
    const j = await puffinparse.submit(
      DOC,
      submitOpts({ webhookUrl: 'https://hooks.example.com/x', metadata: { batch_id: 7 } }),
    )
    assert.deepEqual(Object.keys(j).sort(), [
      'baseUrl',
      'includeRaw',
      'jobId',
      'metadata',
      'model',
      'output',
      'provider',
      'providerState',
      'submittedAt',
    ])
    assert.equal(j.provider, 'reducto')
    assert.equal(j.model, 'reducto/standard')
    assert.equal(j.jobId, 'rj-42')
    assert.equal(j.baseUrl, baseUrl)
    assert.equal(j.output, 'markdown')
    assert.deepEqual(j.metadata, { batch_id: 7 }, 'metadata keys are kept verbatim')
    assert.match(j.submittedAt, /^\d{4}-\d{2}-\d{2}T/)
    assert.ok(!JSON.stringify(j).includes('test-key'), 'a Job never holds the key')

    const sent = seen[0]
    assert.equal(sent.method, 'POST')
    assert.equal(sent.url, '/parse_async')
    assert.equal(sent.auth, 'Bearer test-key')
    assert.equal(sent.body.input, DOC.url)
    assert.deepEqual(sent.body.async.webhook, { mode: 'direct', url: 'https://hooks.example.com/x' })
  })

  test('retrieve returns the same Job while pending, then the ParseResponse', async () => {
    queue = [[200, '{"job_id":"rj-1"}'], job('Pending'), job('Completed', { result: REDUCTO_RESULT })]
    const j = await puffinparse.submit(DOC, submitOpts({ metadata: { run: 'r1' } }))
    const pending = await puffinparse.retrieve(j, { apiKey: 'test-key' })
    assert.equal(pending, j)
    // A Job survives a JSON round trip (e.g. stored in a queue between processes).
    const done = await puffinparse.retrieve(JSON.parse(JSON.stringify(j)), { apiKey: 'test-key' })
    assert.equal(done.model, 'reducto/standard')
    assert.equal(done.provider, 'reducto')
    assert.equal(done.providerJobId, REDUCTO_RESULT.job_id)
    assert.match(done.markdown, /^# Hello LiteOCR/)
    assert.equal(done.pages[0].pageNumber, 1)
    assert.equal(done.metadata.run, 'r1')
    assert.ok(done.costUsd > 0)
    assert.equal(done.raw, null)
    assert.deepEqual(
      seen.slice(1).map((s) => [s.method, s.url, s.auth]),
      [
        ['GET', '/job/rj-1', 'Bearer test-key'],
        ['GET', '/job/rj-1', 'Bearer test-key'],
      ],
    )
  })

  test('retrieve renders a vendor shape with outputFormat', async () => {
    queue = [job('Completed', { result: REDUCTO_RESULT })]
    const j = { provider: 'reducto', model: 'reducto/standard', jobId: 'rj-2', submittedAt: '', baseUrl }
    const resp = await puffinparse.retrieve(j, { apiKey: 'k', outputFormat: 'llamaparse' })
    assert.ok(Array.isArray(resp.pages), 'LlamaParse-shaped response')
    assert.equal(resp.pages[0].page, 1)
  })

  test('a failed job rejects with a typed error carrying the job id and provider message', async () => {
    queue = [job('Failed', { reason: 'Password-protected document' })]
    const j = { provider: 'reducto', model: 'reducto/standard', jobId: 'rj-9', submittedAt: '', baseUrl }
    await assert.rejects(puffinparse.retrieve(j, { apiKey: 'k' }), (e) => {
      assert.ok(e instanceof ProviderError)
      assert.equal(e.jobId, 'rj-9')
      assert.equal(e.provider, 'reducto')
      assert.match(e.message, /Password-protected document/)
      return true
    })
  })

  test('HTTP errors on retrieve map like parse', async () => {
    queue = [[401, '{"detail":"Invalid API key from mock"}']]
    const j = { provider: 'reducto', model: 'reducto/standard', jobId: 'rj-3', submittedAt: '', baseUrl }
    await assert.rejects(puffinparse.retrieve(j, { apiKey: 'bad', maxRetries: 0 }), (e) => {
      assert.ok(e instanceof AuthenticationError)
      assert.equal(e.statusCode, 401)
      assert.equal(e.message, 'Invalid API key from mock')
      return true
    })
  })

  test('handleWebhook: pending, finished (one retrieve) and failed bodies', async () => {
    const pending = await puffinparse.handleWebhook({ event_type: 'parse.pending', data: { job_id: 'j1' } }, { model: 'llamaparse' })
    assert.equal(pending.jobId, 'j1')
    assert.equal(pending.provider, 'llamaparse')

    queue = [job('Completed', { result: REDUCTO_RESULT })]
    const done = await puffinparse.handleWebhook(JSON.stringify({ status: 'Completed', job_id: 'rj-7' }), {
      apiKey: 'k',
      baseUrl,
    })
    assert.match(done.markdown, /Hello LiteOCR/)
    assert.deepEqual([seen[0].method, seen[0].url], ['GET', '/job/rj-7'])

    const failed = {
      eventType: 'parse_run.failed',
      payload: { object: 'parse_run_status', id: 'pr_9', status: 'FAILED', failureReason: 'OUT_OF_CREDITS', failureMessage: 'No credits left.' },
    }
    await assert.rejects(
      puffinparse.handleWebhook(failed, { model: 'extend' }),
      (e) => e instanceof AuthenticationError && /No credits left/.test(e.message) && e.jobId === 'pr_9',
    )

    const push = { txt: 'Hello', md: '# Hello', json: [{ page: 1, text: 'Hello', md: '# Hello' }], images: [] }
    const pushed = await puffinparse.handleWebhook(push, { model: 'llamaparse/agentic' })
    assert.equal(pushed.model, 'llamaparse/agentic')
    assert.equal(pushed.markdown, '# Hello')
    assert.equal(seen.length, 1, 'only the finished body made a network call')
  })

  test('arguments are validated before any call', async () => {
    await assert.rejects(puffinparse.submit(DOC, { fallbacks: ['llamaparse'] }), /unknown option\(s\) "fallbacks"/)
    await assert.rejects(puffinparse.submit(DOC, { outputFormat: 'reducto' }), TypeError)
    await assert.rejects(puffinparse.submit(DOC, { webhookUrl: 42 }), TypeError)
    await assert.rejects(puffinparse.submit(DOC, { model: 'mistral/ocr-latest', apiKey: 'k' }), UnsupportedModelError)
    await assert.rejects(
      puffinparse.submit(DOC, { model: 'extend/parse_light', apiKey: 'k', webhookUrl: 'https://h.example.com/x' }),
      (e) => e instanceof InputError && /webhook/i.test(e.message),
    )
    await assert.rejects(puffinparse.retrieve({ jobId: 'x' }), TypeError)
    await assert.rejects(puffinparse.retrieve('rj-1'), TypeError)
    const j = { provider: 'reducto', model: 'reducto/standard', jobId: 'rj-1', submittedAt: '' }
    await assert.rejects(puffinparse.retrieve(j, { timeout: 0 }), TypeError)
    await assert.rejects(puffinparse.retrieve(j, { webhookUrl: 'x' }), TypeError)
    await assert.rejects(puffinparse.handleWebhook({ unexpected: true }, { model: 'reducto' }), InputError)
    await assert.rejects(puffinparse.handleWebhook('not json'), InputError)
    await assert.rejects(puffinparse.handleWebhook([1]), TypeError)
    assert.equal(seen.length, 0)
  })
})
