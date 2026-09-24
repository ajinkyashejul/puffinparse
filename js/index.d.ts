/**
 * LiteOCR for Node.js: one API for every OCR / document-parsing provider.
 *
 * Hand-written to match `index.js` and the unified types in docs/SPEC.md §4–6 (camelCase here,
 * snake_case in the core's JSON). Keep it in sync with `crates/liteocr-core/src/types.rs`.
 */

/// <reference types="node" />

// ---- modes, inputs, options ----------------------------------------------------------------------

/** What a call asks a provider to do. Providers can only be swapped within a mode. */
export type Mode = 'parse' | 'ocr' | 'extract'

/** `['parse', 'ocr', 'extract']`. */
export declare const MODES: readonly Mode[]

/** Version of the compiled Rust core. */
export declare const VERSION: string

/** Values `outputFormat` accepts (aliases such as `'llama'` are canonicalised by the core). */
export type OutputFormat = 'liteocr' | VendorFormat

/** The vendor shapes a response can be rendered in (docs/COMPAT.md). */
export type VendorFormat = 'reducto' | 'extend' | 'llamaparse'

/**
 * A document: a local path, an `http(s)://` URL string, a `URL` (http(s) or file), raw bytes
 * (then pass `filename`), or an object form.
 */
export type DocumentInput =
  | string
  | URL
  | Uint8Array
  | ArrayBuffer
  | { url: string }
  | { path: string }
  | { data: Uint8Array | ArrayBuffer; filename?: string }

/** Options every mode accepts (SPEC §4.1). */
export interface CommonOptions {
  /** `"<provider>/<model>"`, e.g. `'reducto/standard'`; a bare provider picks its default for the mode. Default `'reducto'`. */
  model?: string
  /** Extra models tried in order when `model` fails with a retryable error (a one-off ordered Router). */
  fallbacks?: string[]
  /** Required when the document is bytes; used to infer the document type. */
  filename?: string
  /** 1-based page selection: `'1-3,7'`, `2`, or `[1, 2, 5]` (forwarded best-effort). */
  pages?: string | number | number[]
  /** Language hint (ISO 639-1 / BCP-47) when the provider supports it. */
  language?: string
  /** Provider-specific options merged verbatim into the provider request. */
  providerOptions?: Record<string, unknown>
  /** Attach the provider's raw payload as `response.raw`. Default `false`. */
  includeRaw?: boolean
  /** Whole-call deadline in **seconds** (upload + polling + download). Default `300`. */
  timeout?: number
  /** Retries on 429 / 5xx / network errors with exponential backoff. Default `2`. */
  maxRetries?: number
  /** Override the API key (otherwise read from `REDUCTO_API_KEY` etc.). */
  apiKey?: string
  /** Override the provider base URL. */
  baseUrl?: string
  /** Free-form object echoed back in `response.metadata`. */
  metadata?: Record<string, unknown>
}

export interface ParseOptions extends CommonOptions {
  /** Preferred block content. Default `'markdown'`. */
  output?: 'markdown' | 'text'
  /**
   * Return a vendor's own JSON shape instead of the unified response. `'liteocr'` (or unset)
   * gives the unified camelCase `ParseResponse`. See docs/COMPAT.md.
   */
  outputFormat?: OutputFormat | (string & {}) | null
}

export type OcrOptions = CommonOptions

export interface ExtractOptions extends CommonOptions {
  /** A JSON Schema **object** describing the fields you want. */
  schema: Record<string, unknown>
  /** Extra natural-language guidance, forwarded when the provider accepts it. */
  instructions?: string
  /** Ask for per-field citations (page, box, source text) where supported. Default `false`. */
  citations?: boolean
  /** Vendor-shaped extract JSON instead of the unified response (best effort, COMPAT §7). */
  outputFormat?: OutputFormat | (string & {}) | null
}

/** Per-call options on a Router: the router owns `model` / `fallbacks`. */
export type RouterParseOptions = Omit<ParseOptions, 'model' | 'fallbacks'>
export type RouterOcrOptions = Omit<OcrOptions, 'model' | 'fallbacks'>
export type RouterExtractOptions = Omit<ExtractOptions, 'model' | 'fallbacks'>

/** A vendor-shaped response (`outputFormat: 'reducto' | 'extend' | 'llamaparse'`), returned verbatim. */
export type NativeFormatResponse = Record<string, unknown>

// ---- responses (SPEC §5) -------------------------------------------------------------------------

/** Normalised bounding box: 0..1 relative to the page, origin top-left. */
export interface BBox {
  x0: number
  y0: number
  x1: number
  y1: number
}

export type BlockType =
  | 'text'
  | 'title'
  | 'section_header'
  | 'list'
  | 'table'
  | 'figure'
  | 'header'
  | 'footer'
  | 'footnote'
  | 'caption'
  | 'formula'
  | 'other'

export interface Usage {
  /** Pages billed / processed. */
  pages: number
  /** Provider-native credit units, if any. */
  credits: number | null
  /** Cost in USD if the provider reports it directly. */
  providerCostUsd: number | null
}

/** Fields shared by every mode's response. */
export interface ResponseEnvelope {
  /** LiteOCR-generated UUID. */
  id: string
  /** e.g. `'reducto'`. */
  provider: string
  /** Fully-qualified model, e.g. `'reducto/standard'`. */
  model: string
  providerJobId: string | null
  usage: Usage
  /** `null` when pricing is unknown. */
  costUsd: number | null
  /** Wall-clock for the whole call, including polling. */
  latencyMs: number
  /** RFC 3339. */
  createdAt: string
  /** Your `metadata`, plus `liteocr_*` keys (e.g. `liteocr_fallback_index`, `liteocr_derived_from`). Keys are not camelCased. */
  metadata: Record<string, unknown>
  /** Provider payload when `includeRaw: true`, else `null`. Returned verbatim. */
  raw: unknown
}

export interface Block {
  type: BlockType
  /** Markdown (tables as markdown/HTML per provider). */
  content: string
  /** Plain text if the provider gives a separate one. */
  text: string | null
  bbox: BBox | null
  /** 0..1 if the provider reports one. */
  confidence: number | null
  pageNumber: number
}

export interface Page {
  /** 1-based. */
  pageNumber: number
  width: number | null
  height: number | null
  markdown: string
  text: string
  blocks: Block[]
}

/** `parse` mode: layout-aware markdown and typed blocks. */
export interface ParseResponse extends ResponseEnvelope {
  pages: Page[]
  /** Whole document, pages joined by `"\n\n"`. */
  markdown: string
  text: string
}

/** A recognised word or line with its box and confidence. */
export interface Word {
  text: string
  bbox: BBox | null
  confidence: number | null
}

export type Line = Word

export interface TextPage {
  pageNumber: number
  width: number | null
  height: number | null
  /** Plain text in reading order, lines separated by `"\n"`. */
  text: string
  lines: Line[]
  words: Word[]
}

/** `ocr` mode: plain text with line/word geometry, no layout semantics. */
export interface TextResponse extends ResponseEnvelope {
  pages: TextPage[]
  text: string
}

export interface Citation {
  pageNumber: number
  bbox: BBox | null
  /** Source text the value was read from, if reported. */
  text: string | null
}

export interface FieldInfo {
  confidence: number | null
  citations: Citation[]
}

/** `extract` mode: the object your schema asked for, plus provenance. */
export interface ExtractResponse<T = unknown> extends ResponseEnvelope {
  /** The extracted object, shaped by the request schema. Returned verbatim (not camelCased). */
  data: T
  /** Keyed by JSON pointer into `data`, e.g. `'/invoice/total'`. */
  fields: Record<string, FieldInfo>
}

export type Response = ParseResponse | TextResponse | ExtractResponse

/** Benchmark metrics between a prediction and a ground truth (SPEC §10.3). */
export interface Metrics {
  charSimilarity: number
  cer: number
  wer: number
  wordRecall: number
  wordPrecision: number
  wordF1: number
  predChars: number
  truthChars: number
  /** Reading-order agreement; `null` with fewer than 2 shared lines. */
  orderScore: number | null
  /** Similarity on markdown table lines; `null` when the truth has no tables. */
  tableScore: number | null
  /** `passed / total` of a rule-scored document; `null` for transcript documents. */
  rulePassRate: number | null
  rulesPassed: number | null
  rulesTotal: number | null
}

export interface ScoreOptions {
  /** Default `true`. */
  caseInsensitive?: boolean
  /** Default `true`. */
  stripMarkdown?: boolean
  /** Default `false`. */
  stripPunctuation?: boolean
}

// ---- calls ---------------------------------------------------------------------------------------

/** Parse a document into markdown + typed blocks (`parse` mode). */
export declare function parse(doc: DocumentInput, options?: ParseOptions & { outputFormat?: 'liteocr' | null }): Promise<ParseResponse>
export declare function parse(doc: DocumentInput, options: ParseOptions & { outputFormat: VendorFormat }): Promise<NativeFormatResponse>
export declare function parse(doc: DocumentInput, options: ParseOptions): Promise<ParseResponse | NativeFormatResponse>

/** Read a document as plain text with line/word boxes (`ocr` mode). */
export declare function ocr(doc: DocumentInput, options?: OcrOptions): Promise<TextResponse>

/** Pull a structured JSON object out of a document with a schema (`extract` mode). */
export declare function extract<T = unknown>(
  doc: DocumentInput,
  options: ExtractOptions & { outputFormat?: 'liteocr' | null },
): Promise<ExtractResponse<T>>
export declare function extract(doc: DocumentInput, options: ExtractOptions & { outputFormat: VendorFormat }): Promise<NativeFormatResponse>
export declare function extract<T = unknown>(
  doc: DocumentInput,
  options: ExtractOptions,
): Promise<ExtractResponse<T> | NativeFormatResponse>

// ---- router (SPEC §7) ----------------------------------------------------------------------------

export type Strategy = 'ordered' | 'round_robin'

export interface RouterConfig {
  models: string[]
  /** Default `'parse'`. Every model must serve it. */
  mode?: Mode
  /** Default `'ordered'`. */
  strategy?: Strategy
  /**
   * Error kinds that move on to the next model. Accepts kinds (`'provider'`), class names
   * (`'ProviderError'`) or classes. Default: provider, rate_limit, timeout, network.
   */
  fallbackOn?: Array<ErrorKind | ErrorClassName | (abstract new (...args: any[]) => LiteOCRError)>
}

export interface ModelStats {
  successes: number
  failures: number
  totalLatencyMs: number
  totalCostUsd: number
  totalPages: number
  /** `null` until the model has served a call. */
  avgLatencyMs: number | null
}

/** Route calls across several models with ordered fallbacks or round-robin, bound to one mode. */
export declare class Router {
  constructor(config: RouterConfig)
  /** Canonical model strings, in fallback order. */
  readonly models: string[]
  readonly mode: Mode
  /** The order models would be tried for the next call (advances round-robin). */
  plan(): string[]
  /** Per-model counters keyed by model string. */
  stats(): Record<string, ModelStats>
  parse(doc: DocumentInput, options?: RouterParseOptions & { outputFormat?: 'liteocr' | null }): Promise<ParseResponse>
  parse(doc: DocumentInput, options: RouterParseOptions & { outputFormat: VendorFormat }): Promise<NativeFormatResponse>
  parse(doc: DocumentInput, options: RouterParseOptions): Promise<ParseResponse | NativeFormatResponse>
  ocr(doc: DocumentInput, options?: RouterOcrOptions): Promise<TextResponse>
  extract<T = unknown>(
    doc: DocumentInput,
    options: RouterExtractOptions & { outputFormat?: 'liteocr' | null },
  ): Promise<ExtractResponse<T>>
  extract(doc: DocumentInput, options: RouterExtractOptions & { outputFormat: VendorFormat }): Promise<NativeFormatResponse>
  extract<T = unknown>(doc: DocumentInput, options: RouterExtractOptions): Promise<ExtractResponse<T> | NativeFormatResponse>
}

// ---- registry, pricing, helpers ------------------------------------------------------------------

/** All `"<provider>/<model>"` strings, or only those serving `mode`. */
export declare function listModels(mode?: Mode): string[]
/** `['parse', 'ocr', 'extract']`. */
export declare function modes(): Mode[]

export interface ModelInfo {
  provider: string
  model: string
  description: string
  /** Default model for its provider within each mode it supports. */
  default: boolean
  modes: Mode[]
}

export interface ProviderInfo {
  name: string
  displayName: string
  envVar: string
  baseUrl: string
  docs: string
  models: ModelInfo[]
}

/** Provider metadata: name, env var, base URL, docs and models with their modes. */
export declare function providers(): ProviderInfo[]

/** Validate and canonicalise a model string (`'reducto'` -> `'reducto/standard'`). */
export declare function resolveModel(model: string, mode?: Mode): string

export interface PriceEntry {
  parse?: number
  ocr?: number
  extract?: number
  source?: string
  updated?: string
}

/** The active price table (USD per page, per mode). */
export declare function pricing(): Record<string, PriceEntry>
/** Override per-page USD prices for one mode (default `'parse'`). */
export declare function setPricing(prices: Record<string, number>, mode?: Mode): void
/** Restore the embedded price table. */
export declare function resetPricing(): void
/** Estimated USD cost for `pages` pages; `null` when the model has no price in `mode`. */
export declare function estimateCost(model: string, pages: number, mode?: Mode): number | null
/** Every value `outputFormat` accepts: `['liteocr', 'reducto', 'extend', 'llamaparse']`. */
export declare function outputFormats(): OutputFormat[]

/** Benchmark metrics (character similarity, CER, WER, word F1, ...) for a prediction. */
export declare function score(prediction: string, truth: string, options?: ScoreOptions): Metrics
/** The normalisation applied before scoring. */
export declare function normalizeText(text: string, options?: ScoreOptions): string
/** Strip markdown syntax to plain text. */
export declare function markdownToText(markdown: string): string
/** Enable the Rust core's tracing output on stderr (default `'info'`). */
export declare function initLogging(level?: string): void

// ---- errors (SPEC §6) ----------------------------------------------------------------------------

/** The core's `ErrorKind`, as serialised. */
export type ErrorKind =
  | 'authentication'
  | 'rate_limit'
  | 'bad_request'
  | 'provider'
  | 'timeout'
  | 'unsupported_model'
  | 'input'
  | 'network'

export type ErrorClassName =
  | 'AuthenticationError'
  | 'RateLimitError'
  | 'BadRequestError'
  | 'ProviderError'
  | 'TimeoutError'
  | 'UnsupportedModelError'
  | 'InputError'
  | 'NetworkError'

export interface LiteOCRErrorOptions {
  provider?: string | null
  statusCode?: number | null
  jobId?: string | null
  retryable?: boolean
}

/** Base class for every error LiteOCR throws (argument type errors are plain `TypeError`s). */
export declare class LiteOCRError extends Error {
  constructor(message: string, options?: LiteOCRErrorOptions)
  readonly kind: ErrorKind | 'error'
  /** The provider's own message is kept verbatim in `message`. */
  provider: string | null
  statusCode: number | null
  jobId: string | null
  /** Whether a router may try a fallback model for this error. */
  retryable: boolean
  toJSON(): {
    name: string
    kind: ErrorKind | 'error'
    message: string
    provider: string | null
    statusCode: number | null
    jobId: string | null
    retryable: boolean
  }
}

/** 401/403 from the provider, or no API key configured. */
export declare class AuthenticationError extends LiteOCRError {
  static readonly kind: 'authentication'
  readonly kind: 'authentication'
}
/** 429 after retries were exhausted. */
export declare class RateLimitError extends LiteOCRError {
  static readonly kind: 'rate_limit'
  readonly kind: 'rate_limit'
}
/** The provider rejected the request (4xx other than auth / rate limit), or an unknown `outputFormat`. */
export declare class BadRequestError extends LiteOCRError {
  static readonly kind: 'bad_request'
  readonly kind: 'bad_request'
}
/** 5xx, malformed provider payload, or a job that ended in a failed state. */
export declare class ProviderError extends LiteOCRError {
  static readonly kind: 'provider'
  readonly kind: 'provider'
}
/** The whole-call deadline (upload + polling + download) was exceeded. */
export declare class TimeoutError extends LiteOCRError {
  static readonly kind: 'timeout'
  readonly kind: 'timeout'
}
/** Unknown provider / model string, or a model that does not serve the requested mode. */
export declare class UnsupportedModelError extends LiteOCRError {
  static readonly kind: 'unsupported_model'
  readonly kind: 'unsupported_model'
}
/** Unreadable input, bytes without a filename, empty body, bad mode, router asked for another mode. */
export declare class InputError extends LiteOCRError {
  static readonly kind: 'input'
  readonly kind: 'input'
}
/** Network / TLS / DNS failure after retries. */
export declare class NetworkError extends LiteOCRError {
  static readonly kind: 'network'
  readonly kind: 'network'
}
