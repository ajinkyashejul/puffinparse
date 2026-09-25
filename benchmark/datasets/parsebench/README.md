# parsebench

A curated, redistributable subset of **[ParseBench](https://parsebench.ai)** (LlamaIndex),
converted into the PuffinParse manifest format by
[`benchmark/adapters/parsebench.py`](../../adapters/parsebench.py).

Upstream: `llamaindex/ParseBench` on Hugging Face, pinned to commit
**`2805a1d940f95a203e0ae4b88be9934f7765b3fc`**. License **Apache-2.0**.

## Layout

```
benchmark/datasets/parsebench/
  manifest.json          # the 40 COMMITTED documents — runnable as-is
  manifest.full.json     # index of all 1,009 convertible upstream documents (bytes NOT committed)
  subset-committed.txt   # the 40 committed ids, one per line
  docs/<id>.pdf          # committed single-page PDFs (4.0 MB)
  truth/<id>.md          # markdown table truth for kind=transcript documents
  rules/<id>.json        # assertion lists for kind=rules documents
```

`manifest.json` is what the CLI and `combined-v1` use. `manifest.full.json` is an *index only*:
its `file` / `truth` / `rules` paths do not exist until you rebuild with a larger budget, but
every entry carries `upstream_path` so the bytes can be fetched from the pinned revision.

## What is in the committed subset

| Kind | Docs | Category | Truth | Scored with |
|---|---:|---|---|---|
| `transcript` | 25 | `table` | `truth/<id>.md` — the page's ground-truth table as a GitHub-markdown pipe table | **`table_score`** (see caveat) |
| `rules` | 15 | `text` | `rules/<id>.json` — 3,703 assertions in the common rule schema | a rules scorer (not implemented yet) |

Total committed size: **5.7 MB** (4.0 MB of PDFs), all single-page.

### Caveat: `table-only` truth

Every `kind: transcript` document here is tagged **`table-only`**. ParseBench's table ground
truth is *only the table*, not the whole page, while a parser returns the **whole page**. So:

- `table_score` is the metric to read — it compares markdown table rows only, and in a smoke run
  `reducto/standard` scored **0.990** on `apple_10_k_page1`.
- `char_similarity` / `cer` / `wer` / `overall` on the same document were **0.311 / 2.212 /
  1.938 / 31.13** purely because the prediction contains the rest of the page. Those numbers are
  *not* a quality signal for `table-only` documents.

Documents tagged **`merged-cells`** (18 of 25) had `colspan` / `rowspan` in the upstream HTML.
Markdown cannot express spans, so the adapter repeats the spanning cell into every grid position
it covers. Structure fidelity (ParseBench's GriTS) is lost; cell content is preserved.

### `kind: rules` documents and today's CLI

`rules` documents have no markdown truth (`"truth": ""`, kept as an empty string only so the
manifest still deserialises into the current Rust `ManifestDoc`). The current scorer does not
understand them and reports

```
✗ parsebench/text_dense_legalnotices: truth unreadable: Is a directory (os error 21)
```

which counts as a failure and drags `overall` down. Until a rules scorer lands, run

```bash
puffinparse bench run --dataset benchmark/datasets/parsebench --models <model> --filter _page
```

to select only the table documents (every table id ends in `_pageN`), or filter `text_` for the
rules documents once a scorer exists. See [`docs/benchmarks/adapters.md`](../../../docs/benchmarks/adapters.md).

## Regenerating

```bash
pip install huggingface_hub
export REQUESTS_CA_BUNDLE=/root/.ccr/ca-bundle.crt      # only behind the agent proxy
python -m benchmark.adapters parsebench                 # ~71 MB of rule JSONL + 40 PDFs
python -m benchmark.adapters parsebench --limit 12      # smaller subset
python -m benchmark.adapters combined                   # refresh combined-v1 afterwards
```

Selection is deterministic for a given `--seed` (default `1234`): table pages are picked one per
source document with roughly a third tagged `hard`, text pages round-robin across all eight
ParseBench document-type buckets (`simple`, `ocr`, `multicolumns`, `multilang`, `misc`, `dense`,
`sparse`, `handwritting`). Documents over 400 KB, over the 11 MB total budget, or with more than
one page are skipped.

## License and attribution

The upstream dataset card sets `license: apache-2.0` and states: *"All documents are sourced
from public online channels. The dataset is released under the Apache 2.0 License. If there are
any copyright concerns, please contact us via the GitHub repository."* Redistribution inside
this repository is therefore permitted by the publisher's terms.

Worth knowing: the underlying pages are third-party corporate and government documents that
LlamaIndex re-licensed unilaterally. Every document carries its `sha256` and `upstream_path`, so
if a document has to be withdrawn it can be removed from `docs/` and still be fetched on demand.

> ParseBench — Zhang, Acosta, Carlson, Bron, Doulcet, Ospina, Suo (2026), arXiv:2604.08538.
> Dataset: <https://huggingface.co/datasets/llamaindex/ParseBench> ·
> Code: <https://github.com/run-llama/ParseBench> · Apache-2.0.
