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
| `rule_pass_rate` | `passed / total` over a rule-scored document's assertions (only for `kind: rules`) |

## Document kinds

A dataset document declares how it is scored (`kind` in the manifest, default `transcript`;
see [`docs/benchmarks/adapters.md`](../docs/benchmarks/adapters.md)):

| `kind` | Truth | Scored by |
|---|---|---|
| `transcript` | `truth`, a markdown file | the metrics above, headlined by `char_similarity` |
| `rules` | `rules`, a JSON list of machine-checkable assertions (`present`, `absent`, `order`, `table_cell`, `bag_of_sentences`) | `rule_pass_rate = passed / total`, which takes the place of `char_similarity` so the document aggregates with the rest |

Two per-document adjustments follow from that:

- A **`rules` document has no markdown truth.** The runner reads its rule file instead, and a
  document whose rules cannot be read or parsed fails with `rules unreadable: …` — without
  spending a provider call.
- A **`table-only` document** (ParseBench's table split: the truth is the page's table, the
  prediction is the whole page) is headlined by `table_score` instead of `char_similarity`, so
  `Overall` means the same thing for it as for every other document. `char_similarity`, `cer`
  and `wer` are still recorded, and are still systematically bad on those documents by
  construction.

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
`--filter <substring>` and `--limit N` select a subset of documents. Ids of a combined dataset
carry their source (`synthetic/plain_001`), so the saved output path keeps that directory level
and `--filter synthetic` runs one source.

Before spending money, check the plan and put a ceiling on it:

```bash
./target/release/liteocr bench run --dataset benchmark/datasets/combined-v1 \
    --models reducto/standard llamaparse/agentic --out benchmark/results/combined.json --dry-run
# | Model | Calls | Skipped (resumed) | Est. pages | $/page | Est. cost | … no provider is called
./target/release/liteocr bench run … --max-cost 5    # aborts before the first call if the estimate is higher
```

The estimate is manifest `pages` × list price (`pricing.json`), so it is only as good as the
manifest's page counts; `--max-cost` refuses to run a model that has no list price.

Runs survive interruptions. Every finished (model, document) call is appended to
`<out>.partial.jsonl` and flushed immediately; the final JSON is assembled from it at the end and
the log is removed. After a crash, Ctrl-C or a batch of provider failures, rerun the same command
with `--resume` (and the same `--out`: the default path contains today's date). Pairs that already
succeeded — in the partial log or in an existing result JSON — are not called again; missing and
failed ones are. The resumed run keeps the original `run_id`, so `--save-outputs` files of the
first attempt stay valid, and it refuses to mix in records from another dataset revision or
normalisation. `--retries N` re-issues a document after a retryable error (rate limit, 5xx,
timeout, network) with backoff; it is off by default because a retried provider job can be
billed twice.

Each document record is auditable: `provider_job_id` (look the job up in the provider's
dashboard), `cache_hit` (`false` when caches were disabled, the default), `attempts`,
`started_at`, and `error_kind` + `error` for failures. The run ends with a summary line: calls
made, resumed, failed, total cost and wall time.

`bench report` renders the leaderboard table (a **Rules** column shows the mean rule pass rate,
`–` for datasets with no rule documents), then the per-category breakdown, and — for datasets
whose ids carry a `<source>/` prefix — a per-source breakdown of documents and `Overall`.

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
| [`olmocr`](datasets/olmocr/README.md) | 40 committed (824 indexed) | headers_footers, long_tiny_text, multi_column, old_scans, table_tests — all `kind: rules` (205 assertions) | AI2 olmOCR-bench, ODC-BY-1.0, pinned upstream commit |
| [`omnidocbench`](datasets/omnidocbench/README.md) | 40 **indexed, fetched at run time** | 10 document types (book, newspaper, exam paper, slides, notes, …), English + Chinese, transcript | OpenDataLab OmniDocBench, research-only / non-commercial — not redistributed; `python -m benchmark.adapters omnidocbench` materialises it |
| [`combined-v1`](datasets/combined-v1/README.md) | 79 | synthetic-v1 + parsebench, source-prefixed ids (frozen: has committed results) | per source |
| [`combined-v2`](datasets/combined-v2/README.md) | 159 | synthetic-v1 + parsebench + olmocr + omnidocbench | per source |

Adding a dataset: create `benchmark/datasets/<name>/manifest.json` with
`{name, version, description, license, documents:[{id, file, truth, pages, category, tags}]}`,
put inputs under `docs/` and truth markdown under `truth/`. Public benchmarks are converted by
adapters (`python -m benchmark.adapters <name>`; see [`docs/benchmarks/adapters.md`](../docs/benchmarks/adapters.md)),
which also introduce `kind: rules` documents scored by machine-checkable assertions instead of a
transcript. The [academic benchmark survey](../docs/benchmarks/academic-benchmarks.md) covers olmOCR-bench,
OmniDocBench, DP-Bench, READoc and others; DP-Bench is the next adapter.

## Caveats

- Synthetic documents are cleaner than most real-world scans. Treat `synthetic-v1` as a floor
  for basic fidelity, reading order and table structure, not as the last word on hard documents.
- Latency is measured from the client through the public API and includes upload, queueing and
  polling. Run from a different region or under load and numbers will move.
- Prices are list prices. Volume discounts, batch queues and cache hits change real cost.
- **A rule pass rate is a floor, not an accuracy.** ParseBench's assertions are generated from its
  own reference extraction, so a rule's text can carry that extraction's artifacts: punctuation
  spaced as separate tokens (`this " agreement "`), or two lines of the page concatenated into one
  "sentence". Matching is exact substring after normalisation, so such a rule fails on output that
  is in fact correct, and an `order` rule whose `before` never matches fails with it. The effect is
  the same for every model, so it moves the absolute number far more than the ranking. The one
  `bag_of_sentences` rule per ParseBench document asks for *every* sentence of the page at
  `threshold: 1.0` and therefore fails almost always.
- **olmOCR rules are matched exactly**, while upstream tolerates `max_diffs` edits; its skipped
  tests (math, positional absences, vertical table neighbours, baseline) are counted in
  `datasets/olmocr/conversion-stats.json`. Its `absent-only` documents pass for an empty parse.
- **OmniDocBench must be fetched** before a run (`python -m benchmark.adapters omnidocbench`);
  otherwise its documents fail as file-not-found and the dataset `sha256` does not cover them.
- **A combined score mixes datasets, licences and document kinds.** Read `combined-v1` /
  `combined-v2` next to the per-source table under the leaderboard, not instead of it.
