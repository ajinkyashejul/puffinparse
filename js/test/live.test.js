'use strict'

// Live provider test: one real LlamaParse call on a 1-page synthetic-v1 document.
// Skipped unless LITEOCR_LIVE_TESTS=1 and LLAMA_API_KEY are both set (it costs real money).

const { test } = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

const liteocr = require('..')

const DATASET = path.join(__dirname, '..', '..', 'benchmark', 'datasets', 'synthetic-v1')
const live = process.env.LITEOCR_LIVE_TESTS && process.env.LLAMA_API_KEY

test('llamaparse/cost_effective parses a 1-page synthetic document', { skip: !live && 'set LITEOCR_LIVE_TESTS=1 and LLAMA_API_KEY' }, async () => {
  const doc = path.join(DATASET, 'docs', 'headings_001.png')
  const truth = fs.readFileSync(path.join(DATASET, 'truth', 'headings_001.md'), 'utf8')

  const resp = await liteocr.parse(doc, { model: 'llamaparse/cost_effective', timeout: 180 })

  assert.equal(resp.provider, 'llamaparse')
  assert.equal(resp.model, 'llamaparse/cost_effective')
  assert.ok(resp.pages.length >= 1)
  assert.equal(resp.pages[0].pageNumber, 1)
  assert.ok(resp.markdown.trim().length > 0)
  assert.ok(resp.latencyMs > 0)
  const m = liteocr.score(resp.markdown, truth)
  assert.ok(m.charSimilarity > 0.5, `charSimilarity ${m.charSimilarity}`)
  console.log(
    `live: ${resp.model} pages=${resp.usage.pages} latencyMs=${resp.latencyMs} costUsd=${resp.costUsd} ` +
      `charSimilarity=${m.charSimilarity.toFixed(3)} blocks=${resp.pages[0].blocks.length}`,
  )
})
