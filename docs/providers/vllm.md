# vLLM (open document-parsing VLMs, self-hosted)

> **Status: docs-only (implemented from docs).** Each preset follows its model card and the
> reference client code it points to (read 2026-10-08): `infinity_parser2/prompts.py`,
> `backends/vllm_server.py`, `utils/utils.py` and `utils/pdf.py` in
> [infly-ai/INF-MLLM](https://github.com/infly-ai/INF-MLLM/tree/main/Infinity-Parser2), and
> `dots_mocr/utils/prompts.py`, `model/inference.py`, `parser.py`, `utils/image_utils.py` and
> `utils/layout_utils.py` in [rednote-hilab/dots.mocr](https://github.com/rednote-hilab/dots.mocr).
> No GPU server was run: the fixtures `vllm_infinity_parser2_chat.json` and
> `vllm_dots_mocr_chat.json` are hand-built in the OpenAI chat-completions shape vLLM returns,
> with answers in each model's documented output format. Mark this page **verified** after the
> `#[ignore]`d `vllm_live_parse` test passes against a real server.

## 1. Summary

| | |
|---|---|
| Provider name | `vllm` (aliases `vllm-server`, `openai-compatible`) |
| Runs | on your own `vllm serve` (or any OpenAI-compatible chat-completions server) |
| Base URL | `http://localhost:8000` (override: `base_url` on the request, or `VLLM_BASE_URL`; with or without `/v1`) |
| API key | none; optional `VLLM_API_KEY` / `api_key`, sent as `Authorization: Bearer …` when the server runs with `--api-key` |
| Served model name | the preset's (below); override with `VLLM_SERVED_MODEL` or `provider_options.served_model` |
| PDFs | rasterised locally with poppler's `pdftoppm` (`PDFTOPPM_CMD`), one PNG per page |
| Modes | `parse`, `ocr` (derived from `parse`) |
| Price | $0 per page (`pricing.json` source `self-hosted`) |
| Implementation | `crates/puffinparse-core/src/providers/vllm.rs` |

Open document VLMs do not share an API: each one is trained on its own prompt and answers in its
own format. A **preset** pins the prompt, sampling and post-processing of one model so its output
lands in the unified `ParseResponse` with typed blocks and normalised bounding boxes.

## 2. Models exposed by PuffinParse

| Model | Hugging Face repo | Default served name | Output | List price |
|---|---|---|---|---|
| `vllm/infinity-parser2-flash` *(default)* | [`infly/Infinity-Parser2-Flash`](https://huggingface.co/infly/Infinity-Parser2-Flash) (Apache-2.0) | `infly/Infinity-Parser2-Flash` | JSON layout cells, boxes on a 0–1000 grid, tables HTML, formulas LaTeX | $0 |
| `vllm/dots.mocr` | [`rednote-hilab/dots.mocr`](https://huggingface.co/rednote-hilab/dots.mocr) (MIT) | `model` | JSON layout cells, boxes in resized-image pixels, tables HTML, formulas LaTeX | $0 |

### Serve them

`infly/Infinity-Parser2-Flash` — the model card's `vllm serve` command (it is written for the Pro
model; only the repo id changes, and the 2-way tensor parallelism is optional for Flash). The card
pins `vllm==0.17.1`:

```bash
vllm serve infly/Infinity-Parser2-Flash \
    --trust-remote-code \
    --reasoning-parser qwen3 \
    --host 0.0.0.0 \
    --port 8000 \
    --tensor-parallel-size 2 \
    --gpu-memory-utilization 0.85 \
    --max-model-len 65536 \
    --mm-encoder-tp-mode data \
    --mm-processor-cache-type shm \
    --enable-prefix-caching
export VLLM_BASE_URL=http://localhost:8000
```

`rednote-hilab/dots.mocr` — verbatim from the model card (vLLM 0.11.0 or later supports it
natively, e.g. the `vllm/vllm-openai:v0.11.0` image). `--served-model-name model` is why the
preset's served name is `model`:

```bash
CUDA_VISIBLE_DEVICES=0 vllm serve rednote-hilab/dots.mocr --tensor-parallel-size 1 --gpu-memory-utilization 0.9 --chat-template-content-format string --served-model-name model --trust-remote-code
export VLLM_BASE_URL=http://localhost:8000
```

One server serves one model. To use both, run them on two ports and pass `base_url` per request.

### Not available as presets

From the OpenDocRouter list, these open models are **not** presets here, because their cards do
not document a single-request full-page call that PuffinParse could reproduce faithfully:

* **`opendatalab/MinerU2.5-Pro`** (`MinerU2.5-Pro-2604-1.2B` / `-2605-1.2B`): the card drives it
  only through `mineru-vl-utils`' `MinerUClient.two_step_extract`, which runs layout detection,
  crops every region from the page image and recognises each crop, with a custom
  `MinerULogitsProcessor` in vLLM. That needs client-side image cropping and the model's
  layout-token format; a future `mineru` provider for the MinerU API server is the better fit.
* **`XingChen-AGI/TeleOCR`** (formerly NaviDC-OCR): a decoupled parser whose card documents
  per-element prompts on cropped regions (text, OTSL tables, LaTeX formulas) and a layout prompt
  whose output format is not specified.
* **`PaddlePaddle/PaddleOCR-VL-1.6`** is supported, but through the `paddleocr` provider as
  [`paddleocr/vl`](paddleocr.md): its card runs it inside the PaddleOCR-VL pipeline (layout
  detection + element prompts), whose serving API PuffinParse already speaks.

## 3. Request flow PuffinParse uses

1. Load the document (paths and bytes read locally, URLs downloaded). PDFs are rasterised with
   `pdftoppm -r <dpi> -png` (300 dpi for Infinity-Parser2, the `infinity_parser2` default; 200 dpi
   for dots.mocr, the `DotsMOCRParser` default), rendering only the span `pages` covers. Images
   are sent as they are; anything else is an `input_error`.
2. One `POST {base}/v1/chat/completions` per page, up to 4 pages in flight
   (`provider_options.concurrency`):

```jsonc
{
  "model": "infly/Infinity-Parser2-Flash",      // or "model" for dots.mocr
  "messages": [{"role": "user", "content": [
    {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBOR…"}},
    {"type": "text", "text": "\n- Extract layout information from the provided PDF image.\n…"}
  ]}],
  "temperature": 0.0,                            // dots.mocr: 0.1
  "top_p": 1.0,
  "max_tokens": 32768                            // dots.mocr: "max_completion_tokens": 32768
}
```

| | Infinity-Parser2-Flash | dots.mocr |
|---|---|---|
| Prompt | `PROMPT_DOC2JSON` from `infinity_parser2/prompts.py`, verbatim | `prompt_layout_all_en` from `dots_mocr/utils/prompts.py`, verbatim, prefixed with `<\|img\|><\|imgpad\|><\|endofimg\|>` as `inference_with_vllm` does (without it vLLM v1 inserts a newline) |
| Sampling | `temperature 0.0`, `top_p 1.0`, `max_tokens 32768` (`backends/vllm_server.py`) | `temperature 0.1`, `top_p 1.0`, `max_completion_tokens 32768` (`DotsMOCRParser` defaults) |
| Image | sent at rendered size; the server's processor resizes it | sent at rendered size (the parser's default, no client resize) |

`provider_options` keys `served_model`, `dpi`, `concurrency` and `prompt` are read by PuffinParse;
everything else is deep-merged into the request body (`repetition_penalty`, `chat_template_kwargs`,
a different `max_tokens`, …).

## 4. Response mapping

The answer is `choices[0].message.content`. A ```` ```json ```` fence is stripped, and the content
must be a JSON list of cells `{"bbox": [x1, y1, x2, y2], "category": …, "text": …}` (a list of
lists, or an object wrapping the list, is accepted too). If the list was cut off mid-cell
(`finish_reason: "length"`), the last incomplete cell is dropped and the rest parsed, as
Infinity-Parser2's `truncate_last_incomplete_element` does; the page is listed in
`metadata.vllm_recovered_pages`. An answer that is not a cell list at all becomes one `text` block
without a box (`metadata.vllm_unstructured_pages`).

| Infinity-Parser2 `category` | dots.mocr `category` | Block type |
|---|---|---|
| `title` | `Title` | `title` |
| — | `Section-header` | `section_header` |
| `text` | `Text` | `text` |
| — | `List-item` | `list` |
| `table` | `Table` | `table` — `content` keeps the HTML; `text` is one line per row |
| `formula` | `Formula` | `formula` — LaTeX as a `$$ … $$` block (dots.mocr's `get_formula_in_markdown` rules) |
| `figure` | `Picture` | `figure` — kept with its box even though it has no text |
| `figure_caption`, `table_caption`, `formula_caption` | `Caption` | `caption` |
| `figure_footnote`, `table_footnote`, `page_footnote` | `Footnote` | `footnote` |
| `header` / `footer` | `Page-header` / `Page-footer` | `header` / `footer` |

Text categories are already Markdown (both prompts ask for it), so `content` is the model's text
and `Block.text` is its plain-text rendering. Page `markdown` joins the blocks in the model's
reading order; headers and footers are kept as blocks (the reference converters drop them from
their Markdown).

**Bounding boxes.** Infinity-Parser2 answers on a 0–1000 grid (`restore_abs_bbox_coordinates`
divides by 1000), so a box is `x / 1000`. dots.mocr answers in pixels of the image the server
actually fed the model, i.e. after Qwen2-VL `smart_resize` (sides rounded to multiples of 28, total
pixels kept within 3,136–11,289,600); PuffinParse recomputes that size from the page image's PNG /
JPEG header and divides by it, which is `post_process_cells` followed by normalising to the
original page. When the header cannot be read (another image format), dots.mocr boxes are `None`.

`Page.width` / `height` are the page image's pixel size. `Usage.pages` is the number of pages
returned; metadata carries `vllm_served_model` and `vllm_input_tokens` / `vllm_output_tokens`
(sums of `usage.prompt_tokens` / `completion_tokens`), plus `vllm_truncated_pages` for pages that
stopped at the token limit. `raw` (with `include_raw`) is the list of per-page responses.

## 5. Errors and limits

vLLM errors (`{"object": "error", "message": …, "code": 404}`) keep their message and are mapped
by HTTP status like every provider (unknown served model → 404 `bad_request`, 5xx → retried
`provider` error). A refused connection is a `network_error` naming `VLLM_BASE_URL`. A missing
`pdftoppm` is a provider error with the install hint; a corrupt PDF or a page range past the end is
an `input_error`. `timeout_secs` covers the whole document, all pages included.

## 6. Gotchas

* **Served model name.** vLLM answers 404 when `model` does not match `--served-model-name` (the
  repo id by default). The dots.mocr preset assumes the card's `--served-model-name model`; set
  `VLLM_SERVED_MODEL` if you serve it under its repo id.
* **Thinking.** The Infinity-Parser2 card serves with `--reasoning-parser qwen3`, so any reasoning
  goes to `reasoning_content` and only the answer is parsed. Its transformers example passes
  `enable_thinking: false`; to do the same on vLLM, send
  `provider_options={"chat_template_kwargs": {"enable_thinking": False}}`.
* **One page per request.** Cost and latency scale with pages; raise `concurrency` on a big GPU.
* **No PDF text layer.** Everything is read from the page image, like the reference parsers.

## 7. `provider_options` examples

```python
import puffinparse

puffinparse.parse("report.pdf", model="vllm/infinity-parser2-flash")
puffinparse.parse("report.pdf", model="vllm/dots.mocr", base_url="http://gpu-box:8001",
                  provider_options={"dpi": 150, "concurrency": 8})
puffinparse.parse("scan.png", model="vllm/infinity-parser2-flash",
                  provider_options={"served_model": "infinity-flash",
                                    "chat_template_kwargs": {"enable_thinking": False}})
```

## 8. Links

* vLLM OpenAI-compatible server: <https://docs.vllm.ai/en/latest/serving/openai_compatible_server.html>
* Infinity-Parser2-Flash model card: <https://huggingface.co/infly/Infinity-Parser2-Flash>
* Infinity-Parser2 code: <https://github.com/infly-ai/INF-MLLM/tree/main/Infinity-Parser2>
* dots.mocr model card: <https://huggingface.co/rednote-hilab/dots.mocr>
* dots.mocr code: <https://github.com/rednote-hilab/dots.mocr>
