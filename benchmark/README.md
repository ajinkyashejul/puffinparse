# LiteOCR Open Benchmark

A reproducible benchmark that ranks OCR / document-parsing providers on **accuracy**,
**latency** and **cost**, using the same unified client as the SDK. Results are committed
to `results/` and rendered into [`LEADERBOARD.md`](LEADERBOARD.md).

## Principles

1. **Exact ground truth.** Documents in `synthetic-v1` are rendered from the same source the
   truth markdown is written from, so there is no annotation noise. The generator is seeded
   and byte-reproducible (`python benchmark/generate_synthetic.py`).
2. **Deterministic metrics.** No LLM judge is needed. Everything is computed in Rust
   (`liteocr_core::bench`) from the prediction and the truth after normalisation.
3. **Tied to a dataset revision.** Every result file records the SHA-256 of the manifest plus
   every input and truth file, the LiteOCR version, the models and the normalisation options.
4. **Three axes.** Accuracy, latency (p50 / p95 / ms per page as observed from the client, which
   includes upload and polling), and cost per 1,000 pages from the public list price of each model.

## Metrics

Normalisation: NFKC, markdown syntax stripped (headings, emphasis, list bullets, table pipes and
separator rows), curly quotes and dashes straightened, whitespace collapsed, lowercased
(unless `--case-sensitive`).

| Metric | Definition |
|---|---|
| **Overall** | `100 × mean(char_similarity)` over documents; a failed call scores 0 |
| `char_similarity` | `1 − levenshtein(pred, truth) / max(|pred|, |truth|)` |
| `cer` | `levenshtein(pred, truth) / |truth|` |
| `wer` | word-level Levenshtein over whitespace tokens `/ |truth words|` |
| `word_recall`, `word_precision`, `word_f1` | bag-of-words overlap |
| `order_score` | Kendall-τ-style fraction of concordant pairs among lines present in both texts (reading order) |
| `table_score` | `char_similarity` restricted to markdown table rows (only when the truth has tables) |

## Running

```bash
cargo build --release -p liteocr-cli
./target/release/liteocr bench run \
    --dataset benchmark/datasets/synthetic-v1 \
    --models reducto/standard reducto/r-1 extend/parse_performance extend/parse_light \
             llamaparse/fast llamaparse/cost_effective llamaparse/agentic \
    --concurrency 4 --save-outputs benchmark/runs/outputs
./target/release/liteocr bench report benchmark/results/*.json > benchmark/LEADERBOARD.md
```

`--save-outputs` writes each model's markdown per document so mistakes can be inspected. Committed
runs keep them under `results/outputs/<run_id>/<model>/<doc_id>.md`; the static site under
`site/` renders them next to the input and the truth with a word-level diff.
`--filter <substring>` and `--limit N` select a subset of documents.

Scoring a single pair without any network access:

```bash
liteocr bench score prediction.md truth.md
```

or from Python: `liteocr.score(prediction, truth)`.

## Datasets

| Dataset | Docs | Categories | Source |
|---|---|---|---|
| [`synthetic-v1`](datasets/synthetic-v1/README.md) | 39 | plain, invoice, table, two_column, headings, noisy_scan, low_res, multipage, skewed, dense, faded, receipt, complex_table | generated, CC0 |
| [`parsebench`](datasets/parsebench/README.md) | 40 committed (1,009 indexed) | tables (transcript, `table-only`), text pages as rule assertions (`kind: rules`) | LlamaIndex ParseBench, Apache-2.0, pinned upstream commit |
| [`combined-v1`](datasets/combined-v1/README.md) | 79 | union of the above with source-prefixed ids | per source |

Adding a dataset: create `benchmark/datasets/<name>/manifest.json` with
`{name, version, description, license, documents:[{id, file, truth, pages, category, tags}]}`,
put inputs under `docs/` and truth markdown under `truth/`. Public benchmarks are converted by
adapters (`python -m benchmark.adapters <name>`; see [`docs/benchmarks/adapters.md`](../docs/benchmarks/adapters.md)),
which also introduce `kind: rules` documents scored by machine-checkable assertions instead of a
transcript. olmOCR-bench and OmniDocBench adapters are next.

## Caveats

- Synthetic documents are cleaner than most real-world scans. Treat `synthetic-v1` as a floor
  for basic fidelity, reading order and table structure, not as the last word on hard documents.
- Latency is measured from the client through the public API and includes upload, queueing and
  polling. Run from a different region or under load and numbers will move.
- Prices are list prices. Volume discounts, batch queues and cache hits change real cost.
