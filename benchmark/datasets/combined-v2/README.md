# combined-v2

The combined open benchmark, version 2: every PuffinParse dataset under one manifest, per
[ADR-10](../../../docs/DECISIONS.md). It extends [`combined-v1`](../combined-v1/README.md),
which stays frozen because committed results were scored against it, with the two academic
benchmarks surveyed in
[`docs/benchmarks/academic-benchmarks.md`](../../../docs/benchmarks/academic-benchmarks.md).

Built by [`benchmark/adapters/combined.py`](../../adapters/combined.py):

```bash
python -m benchmark.adapters combined-v2
```

Like v1, `manifest.json` is a **view**. Documents are referenced in place (`../olmocr/docs/…`),
and ids are prefixed with the source (`olmocr/old_scans_11`).

| Source | Version | License | Docs | Kind | Committed? |
|---|---|---|---:|---|---|
| [`synthetic-v1`](../synthetic-v1/README.md) | 1.1.0 | CC0-1.0 | 39 | transcript | yes |
| [`parsebench`](../parsebench/README.md) | 1.0.0 | Apache-2.0 | 40 | 25 transcript (`table-only`) + 15 rules | yes |
| [`olmocr`](../olmocr/README.md) | 1.0.0 | ODC-BY-1.0 | 40 | rules (205 assertions) | yes |
| [`omnidocbench`](../omnidocbench/README.md) | 1.0.0 | research-only, non-commercial (no SPDX) | 40 | transcript | **no: fetch first** |
| **Total** | 2.0.0 | mixed | **159** | 104 transcript + 55 rules | |

The manifest's `sources` array records each source's version, license, count, attribution and
the SHA-256 of the source manifest it was built from.

## Running

```bash
python -m benchmark.adapters omnidocbench        # materialise the non-redistributable source first
cargo build --release -p puffinparse-cli
./target/release/puffinparse bench run --dataset benchmark/datasets/combined-v2 \
    --models reducto/standard --filter olmocr/ --limit 5
```

`--filter <source>/` selects one source.

## Read the numbers carefully

1. **Skipping the OmniDocBench fetch** makes its 40 documents fail as file-not-found, and the
   result's dataset `sha256` no longer covers them. Those documents are tagged
   `fetch-required`.
2. **`table-only` ParseBench documents** are headlined by `table_score`. `char_similarity` on
   them is meaningless.
3. **`absent-only` olmOCR documents** (8 `headers_footers` pages) pass for an empty parse. Read
   olmOCR per category.
4. **olmOCR rules match exactly** where upstream allows `max_diffs` edits, so scores are lower
   than olmOCR's own harness would give. See the [olmocr README](../olmocr/README.md).
5. **A single combined score mixes licences, difficulty and document kinds.** Always publish it
   next to the per-source scores.

## License

Mixed: each document carries its own `license` and `attribution`. This directory contains only
the generated manifest.
