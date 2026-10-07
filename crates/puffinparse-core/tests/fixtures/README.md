# Test fixtures: where each payload comes from

Every provider parse path is tested against a payload in this directory. They come from three
places, matching the **Status** column in [`docs/providers/README.md`](../../../../docs/providers/README.md):

| Fixtures | Provider status | Origin |
|---|---|---|
| `reducto_*.json`, `extend_*.json`, `llamaparse_*.json` | live-verified | Trimmed responses captured from our own calls to the live APIs, on documents we made (`benchmark/datasets/synthetic-v1`, the "Hello LiteOCR" / "Cedar Ridge Supply" sample invoices). Job ids, signed URLs, project ids and similar values are redacted or replaced. The provider pages say which file is which. |
| `docling_*.json`, `tesseract_headings.tsv` | verified locally | Real output of a locally installed docling-serve / Tesseract on `synthetic-v1` documents. |
| `anthropic_*`, `azure_*`, `datalab_*`, `gemini_*`, `google_documentai_*`, `landingai_*`, `mathpix_*`, `mistral_*`, `openai_*`, `opendocrouter_*`, `paddleocr_*`, `textract_*`, `unstructured_*`, `upstage_*`, `vllm_*` | docs-only | Hand-built to the response *schema* each provider documents, filled with our own sample content. They are not copies of the providers' documentation examples. |
| `rules_text_sparse_note.json` | n/a | A verbatim copy of `benchmark/datasets/parsebench/rules/text_sparse_note.json`, derived from [ParseBench](https://huggingface.co/datasets/llamaindex/ParseBench) (LlamaIndex, Apache-2.0); see that dataset's README for attribution. |

When you replace a hand-built fixture with a real response, redact every credential, signed URL,
account or project id and any personal data before committing, and update this table.
