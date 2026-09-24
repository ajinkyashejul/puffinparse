'use strict'

/**
 * LiteOCR for Node.js: one API for every OCR / document-parsing provider.
 *
 * This file is a thin, typed wrapper over the Rust core (the `liteocr-node` N-API addon loaded by
 * `native.js`). It validates arguments, builds the core's request JSON from camelCase options,
 * converts responses to camelCase objects (typed in `index.d.ts`, mirroring docs/SPEC.md §5) and
 * turns core errors into `LiteOCRError` subclasses. No provider logic lives here.
 */

const { fileURLToPath } = require('node:url')

const native = require('./native.js')

const CORE_ERROR_PREFIX = 'LITEOCR_CORE_ERROR:'

// ---- errors --------------------------------------------------------------------------------------

/** Base class for every error LiteOCR throws. `kind` is the core's `ErrorKind`. */
class LiteOCRError extends Error {
  constructor(message, options = {}) {
    super(message)
    this.name = new.target.name
    this.provider = options.provider ?? null
    this.statusCode = options.statusCode ?? null
    this.jobId = options.jobId ?? null
    this.retryable = Boolean(options.retryable)
  }

  get kind() {
    return 'error'
  }

  toJSON() {
    return {
      name: this.name,
      kind: this.kind,
      message: this.message,
      provider: this.provider,
      statusCode: this.statusCode,
      jobId: this.jobId,
      retryable: this.retryable,
    }
  }
}

function errorClass(name, kind) {
  const cls = {
    [name]: class extends LiteOCRError {
      get kind() {
        return kind
      }
    },
  }[name]
  cls.kind = kind
  return cls
}

const AuthenticationError = errorClass('AuthenticationError', 'authentication')
const RateLimitError = errorClass('RateLimitError', 'rate_limit')
const BadRequestError = errorClass('BadRequestError', 'bad_request')
const ProviderError = errorClass('ProviderError', 'provider')
const TimeoutError = errorClass('TimeoutError', 'timeout')
const UnsupportedModelError = errorClass('UnsupportedModelError', 'unsupported_model')
const InputError = errorClass('InputError', 'input')
const NetworkError = errorClass('NetworkError', 'network')

const ERROR_CLASSES = [
  AuthenticationError,
  RateLimitError,
  BadRequestError,
  ProviderError,
  TimeoutError,
  UnsupportedModelError,
  InputError,
  NetworkError,
]
const KIND_TO_CLASS = Object.fromEntries(ERROR_CLASSES.map((c) => [c.kind, c]))
const NAME_TO_KIND = Object.fromEntries(ERROR_CLASSES.map((c) => [c.name, c.kind]))

/** Build the typed error for a core error payload (`{kind, message, provider, ...}`). */
function errorFromCore(payload) {
  const Cls = KIND_TO_CLASS[payload.kind] ?? ProviderError
  return new Cls(String(payload.message ?? ''), {
    provider: payload.provider,
    statusCode: payload.status_code,
    jobId: payload.job_id,
    retryable: payload.retryable,
  })
}

/** Convert whatever the addon threw into a `LiteOCRError` (or a `TypeError` for bad arguments). */
function convertError(err, mode) {
  if (err instanceof LiteOCRError || err instanceof TypeError) return err
  const message = err && typeof err.message === 'string' ? err.message : String(err)
  if (message.startsWith(CORE_ERROR_PREFIX)) {
    let payload
    try {
      payload = JSON.parse(message.slice(CORE_ERROR_PREFIX.length))
    } catch {
      return new ProviderError(message.slice(CORE_ERROR_PREFIX.length))
    }
    return withModeHint(errorFromCore(payload), mode)
  }
  if (err && err.code === 'InvalidArg') return new TypeError(message)
  return new ProviderError(message)
}

/** Like the Python SDK: name the models that *do* serve the mode when a model cannot. */
function withModeHint(err, mode) {
  if (!(err instanceof UnsupportedModelError) || !mode || mode === 'parse') return err
  if (!err.message.includes(`'${mode}'`)) return err
  let available = []
  try {
    available = native.listModels(mode)
  } catch {
    available = []
  }
  let message = err.message
  const marker = `Models for '${mode}' from `
  const at = message.indexOf(marker)
  if (at !== -1 && message.slice(at).trimEnd().endsWith(':')) {
    message = message.slice(0, at).trimEnd().replace(/\.$/, '')
  }
  const hint = available.length
    ? available.join(', ')
    : `none yet - no provider in this build implements '${mode}' (listModels('${mode}') is empty)`
  return new UnsupportedModelError(`${message}. Models for mode '${mode}': ${hint}`, {
    provider: err.provider,
    statusCode: err.statusCode,
    jobId: err.jobId,
    retryable: err.retryable,
  })
}

function callNative(fn, mode) {
  try {
    return fn()
  } catch (e) {
    throw convertError(e, mode)
  }
}

async function callNativeAsync(fn, mode) {
  try {
    return await fn()
  } catch (e) {
    throw convertError(e, mode)
  }
}

// ---- argument handling ---------------------------------------------------------------------------

const MODES = Object.freeze(['parse', 'ocr', 'extract'])

const COMMON_OPTIONS = [
  'model',
  'filename',
  'pages',
  'language',
  'outputFormat',
  'providerOptions',
  'includeRaw',
  'timeout',
  'maxRetries',
  'apiKey',
  'baseUrl',
  'metadata',
  'fallbacks',
]
const ALLOWED_OPTIONS = {
  parse: new Set([...COMMON_OPTIONS, 'output']),
  ocr: new Set(COMMON_OPTIONS.filter((k) => k !== 'outputFormat')),
  extract: new Set([...COMMON_OPTIONS, 'schema', 'instructions', 'citations']),
  // A job is one provider call on one model: no fallbacks, and the result shape is chosen at
  // retrieve time.
  submit: new Set([
    ...COMMON_OPTIONS.filter((k) => k !== 'outputFormat' && k !== 'fallbacks'),
    'output',
    'webhookUrl',
  ]),
}
// A router owns its models, so per-call `model` / `fallbacks` make no sense there.
const ROUTER_EXCLUDED = new Set(['model', 'fallbacks'])

function isPlainObject(v) {
  if (v === null || typeof v !== 'object') return false
  const proto = Object.getPrototypeOf(v)
  return proto === Object.prototype || proto === null
}

function checkOptions(options, mode, where, excluded) {
  if (options === undefined || options === null) return {}
  if (!isPlainObject(options)) {
    throw new TypeError(`${where}: options must be a plain object, got ${describe(options)}`)
  }
  const allowed = [...ALLOWED_OPTIONS[mode]].filter((k) => !excluded || !excluded.has(k))
  const unknown = Object.keys(options).filter((k) => !allowed.includes(k))
  if (unknown.length) {
    throw new TypeError(
      `${where}: unknown option(s) ${unknown.map((k) => JSON.stringify(k)).join(', ')} (accepted: ${[...allowed]
        .sort()
        .join(', ')})`,
    )
  }
  return options
}

function describe(v) {
  if (v === null) return 'null'
  if (Array.isArray(v)) return 'an array'
  if (typeof v === 'object') return `an instance of ${v.constructor ? v.constructor.name : 'Object'}`
  return `a ${typeof v}`
}

function isBytes(v) {
  return v instanceof Uint8Array || v instanceof ArrayBuffer
}

function toBuffer(v) {
  if (Buffer.isBuffer(v)) return v
  if (v instanceof ArrayBuffer) return Buffer.from(v)
  return Buffer.from(v.buffer, v.byteOffset, v.byteLength)
}

const URL_RE = /^https?:\/\//i

/**
 * Normalise a document argument into the core's `DocumentInput` JSON plus optional bytes.
 * Accepts a path string, an `http(s)://` string, a `URL`, a Buffer / Uint8Array / ArrayBuffer
 * (then `filename` is required), or `{ url }`, `{ path }`, `{ data, filename }`.
 */
function toDocument(doc, filename, where) {
  if (filename !== undefined && filename !== null && typeof filename !== 'string') {
    throw new TypeError(`${where}: filename must be a string, got ${describe(filename)}`)
  }
  if (typeof doc === 'string') {
    if (doc.trim() === '') throw new InputError(`${where}: document path is empty`)
    if (URL_RE.test(doc)) return { input: { kind: 'url', url: doc }, data: null }
    return { input: { kind: 'path', path: doc }, data: null }
  }
  if (doc instanceof URL) {
    if (doc.protocol === 'file:') return { input: { kind: 'path', path: fileURLToPath(doc) }, data: null }
    if (doc.protocol === 'http:' || doc.protocol === 'https:') {
      return { input: { kind: 'url', url: doc.href }, data: null }
    }
    throw new InputError(`${where}: unsupported URL scheme '${doc.protocol}' (use http(s):// or file://)`)
  }
  if (isBytes(doc)) return bytesDocument(toBuffer(doc), filename, where)
  if (isPlainObject(doc)) {
    if (typeof doc.url === 'string') {
      if (!URL_RE.test(doc.url)) throw new InputError(`${where}: { url } must be an http(s):// URL, got '${doc.url}'`)
      return { input: { kind: 'url', url: doc.url }, data: null }
    }
    if (typeof doc.path === 'string') return toDocument(doc.path, filename, where)
    if (isBytes(doc.data)) return bytesDocument(toBuffer(doc.data), doc.filename ?? filename, where)
    throw new TypeError(`${where}: a document object needs a 'url' string, a 'path' string, or 'data' bytes`)
  }
  throw new TypeError(
    `${where}: document must be a path, an http(s) URL, a Buffer/Uint8Array, or { url }; got ${describe(doc)}`,
  )
}

function bytesDocument(buf, filename, where) {
  if (!filename) {
    throw new InputError(`${where}: filename is required when passing bytes (used to infer the document type)`)
  }
  if (buf.length === 0) throw new InputError(`${where}: document bytes are empty`)
  return { input: { kind: 'bytes', data: '', filename }, data: buf }
}

function optString(options, key, where) {
  const v = options[key]
  if (v === undefined || v === null) return undefined
  if (typeof v !== 'string') throw new TypeError(`${where}: ${key} must be a string, got ${describe(v)}`)
  return v
}

function optObject(options, key, where) {
  const v = options[key]
  if (v === undefined || v === null) return undefined
  if (!isPlainObject(v)) throw new TypeError(`${where}: ${key} must be a plain object, got ${describe(v)}`)
  return v
}

function pagesOption(v, where) {
  if (v === undefined || v === null) return undefined
  if (typeof v === 'string') return v
  if (Number.isInteger(v) && v > 0) return String(v)
  if (Array.isArray(v) && v.length && v.every((p) => Number.isInteger(p) && p > 0)) return v.join(',')
  throw new TypeError(`${where}: pages must be a string like "1-3,7", a positive integer, or an array of them`)
}

/** Build the core request JSON (snake_case, `DocumentRequest` / `ExtractRequest`) from options. */
function buildRequest(doc, options, mode, where, model) {
  const { input, data } = toDocument(doc, options.filename, where)
  const req = { input, model, metadata: optObject(options, 'metadata', where) ?? {} }

  if (mode === 'ocr') {
    req.output = 'text'
  } else if (options.output !== undefined && options.output !== null) {
    if (options.output !== 'markdown' && options.output !== 'text') {
      throw new TypeError(`${where}: output must be 'markdown' or 'text', got ${JSON.stringify(options.output)}`)
    }
    req.output = options.output
  }

  const timeout = options.timeout ?? 300
  if (typeof timeout !== 'number' || !Number.isFinite(timeout) || timeout <= 0) {
    throw new TypeError(`${where}: timeout must be a positive number of seconds, got ${String(options.timeout)}`)
  }
  req.timeout_secs = timeout

  const maxRetries = options.maxRetries ?? 2
  if (!Number.isInteger(maxRetries) || maxRetries < 0) {
    throw new TypeError(`${where}: maxRetries must be a non-negative integer, got ${String(options.maxRetries)}`)
  }
  req.max_retries = maxRetries

  if (options.includeRaw !== undefined && options.includeRaw !== null) {
    if (typeof options.includeRaw !== 'boolean') throw new TypeError(`${where}: includeRaw must be a boolean`)
    req.include_raw = options.includeRaw
  }

  const pages = pagesOption(options.pages, where)
  if (pages !== undefined) req.pages = pages
  const language = optString(options, 'language', where)
  if (language !== undefined) req.language = language
  const apiKey = optString(options, 'apiKey', where)
  if (apiKey !== undefined) req.api_key = apiKey
  const baseUrl = optString(options, 'baseUrl', where)
  if (baseUrl !== undefined) req.base_url = baseUrl
  const providerOptions = optObject(options, 'providerOptions', where)
  if (providerOptions !== undefined) req.provider_options = providerOptions

  if (mode === 'extract') {
    const schema = options.schema
    if (!isPlainObject(schema)) {
      throw new TypeError(
        `${where}: schema must be a JSON Schema object, got ${describe(schema)}. ` +
          `Example: { type: 'object', properties: { total: { type: 'number' } } }`,
      )
    }
    req.schema = schema
    const instructions = optString(options, 'instructions', where)
    if (instructions !== undefined) req.instructions = instructions
    if (options.citations !== undefined && options.citations !== null && typeof options.citations !== 'boolean') {
      throw new TypeError(`${where}: citations must be a boolean`)
    }
    req.citations = Boolean(options.citations)
  }
  return { req, data }
}

function modelOption(options, where) {
  const model = options.model ?? 'reducto'
  if (typeof model !== 'string' || model.trim() === '') {
    throw new TypeError(`${where}: model must be a non-empty string like 'reducto/standard'`)
  }
  return model
}

function fallbacksOption(options, where) {
  const f = options.fallbacks
  if (f === undefined || f === null) return []
  if (!Array.isArray(f) || !f.every((m) => typeof m === 'string' && m.trim() !== '')) {
    throw new TypeError(`${where}: fallbacks must be an array of model strings`)
  }
  return f
}

/** Canonicalise `outputFormat` before any network call; `null` means the unified response. */
function outputFormatOption(options, where) {
  const f = options.outputFormat
  if (f === undefined || f === null) return null
  if (typeof f !== 'string') throw new TypeError(`${where}: outputFormat must be a string, got ${describe(f)}`)
  let canonical
  try {
    canonical = native.validateOutputFormat(f)
  } catch {
    throw new BadRequestError(
      `unknown outputFormat ${JSON.stringify(f)}: expected one of ${native.outputFormats().join(' | ')}, ` +
        `or leave it unset for LiteOCR's unified response`,
    )
  }
  return canonical === 'liteocr' ? null : canonical
}

// ---- response conversion (snake_case core JSON -> camelCase, SPEC §5) ----------------------------

const nul = (v) => (v === undefined ? null : v)

function bbox(d) {
  return d ? { x0: d.x0, y0: d.y0, x1: d.x1, y1: d.y1 } : null
}

function usage(d = {}) {
  return { pages: d.pages ?? 0, credits: nul(d.credits), providerCostUsd: nul(d.provider_cost_usd) }
}

function envelope(d) {
  return {
    id: d.id,
    provider: d.provider,
    model: d.model,
    providerJobId: nul(d.provider_job_id),
    usage: usage(d.usage),
    costUsd: nul(d.cost_usd),
    latencyMs: d.latency_ms ?? 0,
    createdAt: d.created_at ?? '',
    metadata: d.metadata ?? {},
    raw: nul(d.raw),
  }
}

function block(d) {
  return {
    type: d.type,
    content: d.content ?? '',
    text: nul(d.text),
    bbox: bbox(d.bbox),
    confidence: nul(d.confidence),
    pageNumber: d.page_number,
  }
}

function toParseResponse(d) {
  const pages = (d.pages ?? []).map((p) => ({
    pageNumber: p.page_number,
    width: nul(p.width),
    height: nul(p.height),
    markdown: p.markdown ?? '',
    text: p.text ?? '',
    blocks: (p.blocks ?? []).map(block),
  }))
  return { ...envelope(d), pages, markdown: d.markdown ?? '', text: d.text ?? '' }
}

function textItem(d) {
  return { text: d.text ?? '', bbox: bbox(d.bbox), confidence: nul(d.confidence) }
}

function toTextResponse(d) {
  const pages = (d.pages ?? []).map((p) => ({
    pageNumber: p.page_number,
    width: nul(p.width),
    height: nul(p.height),
    text: p.text ?? '',
    lines: (p.lines ?? []).map(textItem),
    words: (p.words ?? []).map(textItem),
  }))
  return { ...envelope(d), pages, text: d.text ?? '' }
}

function toExtractResponse(d) {
  const fields = {}
  for (const [pointer, info] of Object.entries(d.fields ?? {})) {
    fields[pointer] = {
      confidence: nul(info.confidence),
      citations: (info.citations ?? []).map((c) => ({
        pageNumber: c.page_number,
        bbox: bbox(c.bbox),
        text: nul(c.text),
      })),
    }
  }
  return { ...envelope(d), data: nul(d.data), fields }
}

const CONVERTERS = { parse: toParseResponse, ocr: toTextResponse, extract: toExtractResponse }

const camelKey = (k) => k.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase())

function camelize(v) {
  if (Array.isArray(v)) return v.map(camelize)
  if (v && typeof v === 'object') {
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [camelKey(k), camelize(x)]))
  }
  return v
}

const OPTIONAL_METRICS = ['orderScore', 'tableScore', 'tedsGrid', 'rulePassRate', 'rulesPassed', 'rulesTotal']

function toMetrics(d) {
  const m = camelize(d)
  for (const k of OPTIONAL_METRICS) if (m[k] === undefined) m[k] = null
  return m
}

// ---- the three modes -----------------------------------------------------------------------------

async function run(mode, doc, options, where) {
  options = checkOptions(options, mode, where)
  const model = modelOption(options, where)
  const fallbacks = fallbacksOption(options, where)
  const outputFormat = mode === 'ocr' ? null : outputFormatOption(options, where)
  const { req, data } = buildRequest(doc, options, mode, where, model)

  let raw
  if (fallbacks.length) {
    // `fallbacks` is sugar for a one-off ordered Router over [model, ...fallbacks].
    const router = callNative(() => new native.NativeRouter([model, ...fallbacks], mode, 'ordered'), mode)
    raw = await callNativeAsync(() => router[mode](req, data), mode)
  } else {
    raw = await callNativeAsync(() => native[mode](req, data), mode)
  }
  return finish(raw, mode, outputFormat)
}

function finish(raw, mode, outputFormat) {
  if (outputFormat) {
    const render = mode === 'extract' ? native.renderExtract : native.renderParse
    return callNative(() => render(raw, outputFormat), mode)
  }
  return CONVERTERS[mode](raw)
}

/** Parse a document into markdown + typed blocks (`parse` mode). */
function parse(doc, options) {
  return run('parse', doc, options, 'parse()')
}

/** Read a document as plain text with line/word boxes (`ocr` mode). */
function ocr(doc, options) {
  return run('ocr', doc, options, 'ocr()')
}

/** Pull a JSON object shaped by `options.schema` out of a document (`extract` mode). */
function extract(doc, options) {
  return run('extract', doc, options, 'extract()')
}

// ---- asynchronous jobs (SPEC §15) ----------------------------------------------------------------

/** Core `JobHandle` JSON -> camelCase `Job`. `providerState` and `metadata` are kept verbatim. */
function toJob(d) {
  return {
    provider: d.provider,
    model: d.model,
    jobId: d.job_id,
    submittedAt: d.submitted_at ?? '',
    output: d.output === 'text' ? 'text' : 'markdown',
    includeRaw: Boolean(d.include_raw),
    baseUrl: nul(d.base_url),
    providerState: nul(d.provider_state),
    metadata: d.metadata ?? {},
  }
}

/** `Job` (or its JSON round trip) -> the core's `JobHandle` JSON. */
function jobToCore(job, where) {
  if (
    !isPlainObject(job) ||
    typeof job.provider !== 'string' ||
    typeof job.model !== 'string' ||
    typeof job.jobId !== 'string' ||
    job.jobId === ''
  ) {
    throw new TypeError(`${where}: job must be a Job from submit() (provider, model and jobId strings), got ${describe(job)}`)
  }
  const d = {
    provider: job.provider,
    model: job.model,
    job_id: job.jobId,
    submitted_at: typeof job.submittedAt === 'string' ? job.submittedAt : '',
    output: job.output === 'text' ? 'text' : 'markdown',
    include_raw: Boolean(job.includeRaw),
    metadata: isPlainObject(job.metadata) ? job.metadata : {},
  }
  if (typeof job.baseUrl === 'string') d.base_url = job.baseUrl
  if (job.providerState !== undefined && job.providerState !== null) d.provider_state = job.providerState
  return d
}

const RETRIEVE_OPTIONS = ['apiKey', 'baseUrl', 'timeout', 'maxRetries', 'outputFormat']

/** Validate retrieve-style options; returns the core `RetrieveOptions` JSON and the output format. */
function retrieveOptions(options, where, extra = []) {
  if (options === undefined || options === null) options = {}
  if (!isPlainObject(options)) throw new TypeError(`${where}: options must be a plain object, got ${describe(options)}`)
  const allowed = [...RETRIEVE_OPTIONS, ...extra]
  const unknown = Object.keys(options).filter((k) => !allowed.includes(k))
  if (unknown.length) {
    throw new TypeError(
      `${where}: unknown option(s) ${unknown.map((k) => JSON.stringify(k)).join(', ')} (accepted: ${allowed
        .sort()
        .join(', ')})`,
    )
  }
  const timeout = options.timeout ?? 120
  if (typeof timeout !== 'number' || !Number.isFinite(timeout) || timeout <= 0) {
    throw new TypeError(`${where}: timeout must be a positive number of seconds, got ${String(options.timeout)}`)
  }
  const maxRetries = options.maxRetries ?? 2
  if (!Number.isInteger(maxRetries) || maxRetries < 0) {
    throw new TypeError(`${where}: maxRetries must be a non-negative integer, got ${String(options.maxRetries)}`)
  }
  const opts = { timeout_secs: timeout, max_retries: maxRetries }
  const apiKey = optString(options, 'apiKey', where)
  if (apiKey !== undefined) opts.api_key = apiKey
  const baseUrl = optString(options, 'baseUrl', where)
  if (baseUrl !== undefined) opts.base_url = baseUrl
  return { opts, outputFormat: outputFormatOption(options, where) }
}

/**
 * Upload a document and start a `parse` job without waiting for it. Takes `parse()`'s options
 * (minus `fallbacks` / `outputFormat`) plus `webhookUrl`; resolves to a `Job` for `retrieve()`.
 */
async function submit(doc, options) {
  const where = 'submit()'
  options = checkOptions(options, 'submit', where)
  const model = modelOption(options, where)
  const { req, data } = buildRequest(doc, options, 'parse', where, model)
  const webhookUrl = optString(options, 'webhookUrl', where)
  if (webhookUrl !== undefined) req.webhook_url = webhookUrl
  const raw = await callNativeAsync(() => native.submit(req, data), 'parse')
  return toJob(raw)
}

async function retrieveWith(job, core, opts, outputFormat) {
  const status = await callNativeAsync(() => native.retrieve(core, opts), 'parse')
  if (status.status === 'succeeded') return finish(status.result, 'parse', outputFormat)
  return job
}

/**
 * Check a submitted job once: resolves to the same `Job` while the provider is working, or the
 * `ParseResponse` (vendor shape with `outputFormat`) once done. A failed job rejects with the
 * typed error, `jobId` set. Credentials come from the environment unless `apiKey` is given.
 */
function retrieve(job, options) {
  const where = 'retrieve()'
  try {
    const core = jobToCore(job, where)
    const { opts, outputFormat } = retrieveOptions(options, where)
    return retrieveWith(job, core, opts, outputFormat)
  } catch (e) {
    return Promise.reject(e)
  }
}

function webhookPayload(payload, where) {
  if (typeof payload === 'string' || isBytes(payload)) {
    const text = typeof payload === 'string' ? payload : toBuffer(payload).toString('utf8')
    try {
      return JSON.parse(text)
    } catch (e) {
      throw new InputError(`${where}: webhook body is not JSON: ${e.message}`)
    }
  }
  if (payload === null || typeof payload !== 'object' || Array.isArray(payload)) {
    throw new TypeError(`${where}: payload must be the parsed JSON body (an object), a string or bytes`)
  }
  return payload
}

/**
 * Turn the body a provider POSTed to your webhook into a `Job` (still running) or a result.
 * `model` names the provider (`'reducto'`, `'extend'`, `'llamaparse'`) or one of its models.
 * Bodies that only say a job finished trigger one `retrieve()`; a failure rejects with the typed
 * error. Verify the provider's signature (or your own secret) before calling this.
 */
async function handleWebhook(payload, options) {
  const where = 'handleWebhook()'
  const body = webhookPayload(payload, where)
  if (options !== undefined && options !== null && !isPlainObject(options)) {
    throw new TypeError(`${where}: options must be a plain object, got ${describe(options)}`)
  }
  const { model: givenModel, ...rest } = options ?? {}
  const model = modelOption({ model: givenModel }, where)
  const { opts, outputFormat } = retrieveOptions(rest, where)
  const event = callNative(() => native.parseWebhook(model, body), 'parse')
  const status = event.status ?? {}
  if (status.status === 'failed') throw errorFromCore(status.result ?? {})
  if (status.status === 'succeeded') return finish(status.result, 'parse', outputFormat)
  if (!event.job) throw new InputError(`${where}: the webhook payload names no job id`)
  const job = toJob(event.job)
  if (status.status === 'finished') return retrieveWith(job, jobToCore(job, where), opts, outputFormat)
  return job
}

// ---- router --------------------------------------------------------------------------------------

function fallbackKinds(list) {
  if (list === undefined || list === null) return undefined
  if (!Array.isArray(list)) throw new TypeError('Router: fallbackOn must be an array of error kinds')
  return list.map((k) => {
    if (typeof k === 'function' && k.kind) return k.kind
    if (typeof k === 'string') return NAME_TO_KIND[k] ?? k
    throw new TypeError(`Router: fallbackOn entries must be error kinds or classes, got ${describe(k)}`)
  })
}

/** Route calls across several models with ordered fallbacks or round-robin, bound to one mode. */
class Router {
  #inner

  constructor(config) {
    if (!isPlainObject(config)) {
      throw new TypeError(`Router: expected { models, mode?, strategy?, fallbackOn? }, got ${describe(config)}`)
    }
    const known = new Set(['models', 'mode', 'strategy', 'fallbackOn'])
    const unknown = Object.keys(config).filter((k) => !known.has(k))
    if (unknown.length) throw new TypeError(`Router: unknown option(s) ${unknown.join(', ')}`)
    const { models, mode = 'parse', strategy = 'ordered', fallbackOn } = config
    if (!Array.isArray(models) || !models.every((m) => typeof m === 'string')) {
      throw new TypeError('Router: models must be an array of model strings')
    }
    if (typeof mode !== 'string') throw new TypeError('Router: mode must be a string')
    if (typeof strategy !== 'string') throw new TypeError('Router: strategy must be a string')
    const kinds = fallbackKinds(fallbackOn)
    this.#inner = callNative(() => new native.NativeRouter(models, mode, strategy, kinds), mode)
  }

  /** Canonical model strings, in fallback order. */
  get models() {
    return this.#inner.models()
  }

  /** The mode this router serves. */
  get mode() {
    return this.#inner.mode()
  }

  /** The order models would be tried for the next call (advances round-robin). */
  plan() {
    return this.#inner.plan()
  }

  /** Per-model counters keyed by model string. */
  stats() {
    const out = {}
    for (const [model, s] of Object.entries(this.#inner.stats())) {
      out[model] = {
        successes: s.successes,
        failures: s.failures,
        totalLatencyMs: s.total_latency_ms,
        totalCostUsd: s.total_cost_usd,
        totalPages: s.total_pages,
        avgLatencyMs: s.successes ? s.total_latency_ms / s.successes : null,
      }
    }
    return out
  }

  async #run(mode, doc, options) {
    const where = `Router.${mode}()`
    options = checkOptions(options, mode, where, ROUTER_EXCLUDED)
    const outputFormat = mode === 'ocr' ? null : outputFormatOption(options, where)
    // The router replaces `model` with each of its own models; this placeholder is never called.
    const { req, data } = buildRequest(doc, options, mode, where, this.#inner.models()[0])
    const raw = await callNativeAsync(() => this.#inner[mode](req, data), mode)
    return finish(raw, mode, outputFormat)
  }

  parse(doc, options) {
    return this.#run('parse', doc, options)
  }

  ocr(doc, options) {
    return this.#run('ocr', doc, options)
  }

  extract(doc, options) {
    return this.#run('extract', doc, options)
  }

  toString() {
    return `Router(models=${JSON.stringify(this.models)}, mode=${JSON.stringify(this.mode)})`
  }
}

// ---- registry, pricing, helpers ------------------------------------------------------------------

function checkMode(mode, where) {
  if (typeof mode !== 'string') throw new TypeError(`${where}: mode must be a string, got ${describe(mode)}`)
  return mode
}

/** All `"<provider>/<model>"` strings, or only those serving `mode`. */
function listModels(mode) {
  if (mode === undefined || mode === null) return native.listModels()
  checkMode(mode, 'listModels()')
  return callNative(() => native.listModels(mode))
}

/** The modes LiteOCR knows: `['parse', 'ocr', 'extract']`. */
function modes() {
  return native.modes()
}

/** Provider metadata: name, env var, base URL, docs and models with the modes they serve. */
function providers() {
  return camelize(native.providers())
}

/** Validate and canonicalise a model string (`'reducto'` -> `'reducto/standard'`). */
function resolveModel(model, mode) {
  if (typeof model !== 'string') throw new TypeError(`resolveModel(): model must be a string, got ${describe(model)}`)
  if (mode !== undefined && mode !== null) checkMode(mode, 'resolveModel()')
  return callNative(() => native.resolveModel(model, mode ?? null), mode ?? 'parse')
}

/** The active price table: model -> `{ parse?, ocr?, extract?, source?, updated? }` (USD per page). */
function pricing() {
  return native.pricing()
}

/** Override per-page USD prices for one mode, e.g. `setPricing({ 'reducto/standard': 0.012 })`. */
function setPricing(prices, mode = 'parse') {
  if (!isPlainObject(prices)) throw new TypeError(`setPricing(): prices must be an object, got ${describe(prices)}`)
  for (const [model, price] of Object.entries(prices)) {
    if (typeof price !== 'number' || !Number.isFinite(price) || price < 0) {
      throw new TypeError(`setPricing(): price for '${model}' must be a non-negative number`)
    }
  }
  checkMode(mode, 'setPricing()')
  callNative(() => native.setPricing(prices, mode))
}

/** Restore the embedded price table, discarding every `setPricing` override. */
function resetPricing() {
  native.resetPricing()
}

/** Estimated USD cost for `pages` pages at `model`'s list price in `mode`; `null` when unpriced. */
function estimateCost(model, pages, mode = 'parse') {
  if (!Number.isInteger(pages) || pages < 0 || pages > 0xffffffff) {
    throw new TypeError(`estimateCost(): pages must be a non-negative integer, got ${String(pages)}`)
  }
  checkMode(mode, 'estimateCost()')
  const qualified = resolveModel(model)
  return callNative(() => native.estimateCost(qualified, mode, pages))
}

/** Every value `outputFormat` accepts: `['liteocr', 'reducto', 'extend', 'llamaparse']`. */
function outputFormats() {
  return native.outputFormats()
}

function normalizeOptions(options, where) {
  if (options === undefined || options === null) return [null, null, null]
  if (!isPlainObject(options)) throw new TypeError(`${where}: options must be a plain object`)
  const keys = ['caseInsensitive', 'stripMarkdown', 'stripPunctuation']
  const unknown = Object.keys(options).filter((k) => !keys.includes(k))
  if (unknown.length) throw new TypeError(`${where}: unknown option(s) ${unknown.join(', ')}`)
  return keys.map((k) => {
    const v = options[k]
    if (v === undefined || v === null) return null
    if (typeof v !== 'boolean') throw new TypeError(`${where}: ${k} must be a boolean`)
    return v
  })
}

/** Benchmark metrics (character similarity, CER, WER, word F1, ...) for a prediction. */
function score(prediction, truth, options) {
  if (typeof prediction !== 'string' || typeof truth !== 'string') {
    throw new TypeError('score(): prediction and truth must be strings')
  }
  const [ci, sm, sp] = normalizeOptions(options, 'score()')
  return toMetrics(native.score(prediction, truth, ci, sm, sp))
}

/** The normalisation applied before scoring (NFKC, markdown stripped, whitespace collapsed). */
function normalizeText(text, options) {
  if (typeof text !== 'string') throw new TypeError('normalizeText(): text must be a string')
  const [ci, sm, sp] = normalizeOptions(options, 'normalizeText()')
  return native.normalizeText(text, ci, sm, sp)
}

/** Strip markdown syntax to plain text. */
function markdownToText(markdown) {
  if (typeof markdown !== 'string') throw new TypeError('markdownToText(): markdown must be a string')
  return native.markdownToText(markdown)
}

/** Enable the Rust core's tracing output on stderr (`'debug'` shows every HTTP step). */
function initLogging(level = 'info') {
  if (typeof level !== 'string') throw new TypeError('initLogging(): level must be a string')
  callNative(() => native.initLogging(level))
}

module.exports = {
  VERSION: native.version(),
  MODES,
  parse,
  ocr,
  extract,
  submit,
  retrieve,
  handleWebhook,
  Router,
  listModels,
  modes,
  providers,
  resolveModel,
  pricing,
  setPricing,
  resetPricing,
  estimateCost,
  outputFormats,
  score,
  normalizeText,
  markdownToText,
  initLogging,
  LiteOCRError,
  AuthenticationError,
  RateLimitError,
  BadRequestError,
  ProviderError,
  TimeoutError,
  UnsupportedModelError,
  InputError,
  NetworkError,
}
