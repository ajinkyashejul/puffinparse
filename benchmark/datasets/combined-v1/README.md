# combined-v1

The combined open benchmark: every committed LiteOCR dataset under one manifest, per
[ADR-10](../../../docs/DECISIONS.md).

Built by [`benchmark/adapters/combined.py`](../../adapters/combined.py):

```bash
python -m benchmark.adapters combined
```

## What it is

`manifest.json` is a **view, not a copy**. No document bytes live in this directory. Every entry
points back into its own dataset directory with a relative path, and its id is prefixed with the
source:

```json
{
  "id": "synthetic/plain_001",
  "file": "../synthetic-v1/docs/plain_001.png",
  "truth": "../synthetic-v1/truth/plain_001.md"
}
```

So a document scored here is byte-identical to the same document scored in its own dataset, and
adding a source is one line in `SOURCES` in the builder.

| Source | Version | License | Documents |
|---|---|---|---:|
| [`synthetic-v1`](../synthetic-v1/README.md) | 1.1.0 | CC0-1.0 | 39 |
| [`parsebench`](../parsebench/README.md) | 1.0.0 | Apache-2.0 | 40 |
| **Total** | | mixed | **79** |

The `sources` array in the manifest records each source's name, version, license, document
count, attribution and the SHA-256 of the source `manifest.json` it was built from, so a result
file can be traced back to an exact revision of every input.

## Running

```bash
cargo build --release -p liteocr-cli
./target/release/liteocr bench run --dataset benchmark/datasets/combined-v1 \
    --models reducto/standard --filter synthetic --limit 3
```

`--filter synthetic` / `--filter parsebench` selects one source, because the id prefix is part of
the id.

## Read the numbers carefully

1. **64 of 79 documents are `kind: "transcript"`, 15 are `kind: "rules"`.** The current Rust
   scorer only understands transcripts and reports each rules document as
   `truth unreadable: Is a directory`, counting it as a failure. Until a rules scorer lands,
   run with `--filter synthetic` or `--filter _page` for a clean number, or read the per-document
   results and ignore the `text_*` entries. See
   [`docs/benchmarks/adapters.md`](../../../docs/benchmarks/adapters.md).
2. **The 25 ParseBench table documents are tagged `table-only`.** Their truth is the page's table,
   not the whole page, so `table_score` is the metric that means something; `char_similarity`,
   `cer`, `wer` and therefore `overall` are systematically bad on them by construction.
3. **A single combined score mixes licenses, difficulty and document kinds.** Always publish it
   next to the per-dataset scores — that is the point of ADR-10, not a headline number.

## License

Mixed. Each document carries its own `license` and `attribution`; the summary is in `sources`.
This directory itself contains only the generated manifest.
