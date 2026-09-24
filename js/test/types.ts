// Compile-only checks for index.d.ts (`npm run typecheck`); never executed.
import * as liteocr from '../index'
import { ExtractResponse, NativeFormatResponse, ParseResponse, Router, TextResponse, UnsupportedModelError } from '../index'

async function usage(): Promise<void> {
  const doc: ParseResponse = await liteocr.parse('invoice.pdf', { model: 'reducto/standard', pages: [1, 2] })
  const md: string = doc.pages[0].blocks[0].content
  const box: number | undefined = doc.pages[0].blocks[0].bbox?.x0
  const vendor: NativeFormatResponse = await liteocr.parse('a.pdf', { outputFormat: 'reducto' })

  const text: TextResponse = await liteocr.ocr(Buffer.from('x'), { filename: 'a.png', timeout: 60 })
  const words: number = text.pages[0].words.length

  const ex: ExtractResponse<{ total: number }> = await liteocr.extract<{ total: number }>(
    { url: 'https://example.com/a.pdf' },
    { schema: { type: 'object' }, citations: true, model: 'reducto/extract' },
  )
  const total: number = ex.data.total
  const cite = ex.fields['/total']?.citations[0]?.pageNumber

  const router = new Router({ models: ['reducto/standard', 'llamaparse/agentic'], fallbackOn: ['provider', 'TimeoutError'] })
  const routed: ParseResponse = await router.parse(new URL('file:///tmp/a.pdf'))
  const avg: number | null = router.stats()['reducto/standard'].avgLatencyMs

  const job: liteocr.Job = await liteocr.submit('big.pdf', { model: 'reducto/standard', webhookUrl: 'https://h/x' })
  const jobId: string = job.jobId
  const later = await liteocr.retrieve(JSON.parse(JSON.stringify(job)) as liteocr.Job, { timeout: 30 })
  if (!('jobId' in later)) {
    const pages: number = later.pages.length
    void pages
  }
  const shaped: liteocr.Job | NativeFormatResponse = await liteocr.retrieve(job, { outputFormat: 'reducto' })
  const hooked = await liteocr.handleWebhook({ status: 'Completed', job_id: 'j' }, { model: 'reducto', apiKey: 'k' })
  const hookedMd: string | undefined = 'markdown' in hooked ? hooked.markdown : undefined

  const cost: number | null = liteocr.estimateCost('reducto', 10, 'parse')
  const m = liteocr.score('a b', 'a b c')
  const f1: number = m.wordF1
  const formats: liteocr.OutputFormat[] = liteocr.outputFormats()

  try {
    liteocr.resolveModel('nope/x')
  } catch (e) {
    if (e instanceof UnsupportedModelError) {
      const k: 'unsupported_model' = e.kind
      const status: number | null = e.statusCode
      void k
      void status
    }
  }
  void [md, box, vendor, words, total, cite, routed, avg, cost, f1, formats, jobId, shaped, hookedMd]
}
void usage
