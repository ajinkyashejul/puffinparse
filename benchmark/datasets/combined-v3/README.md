# combined-v3

The combined open benchmark, version 3: [`combined-v2`](../combined-v2/README.md) plus the
[DP-Bench](../dpbench/README.md) subset, per [ADR-10](../../../docs/DECISIONS.md).
`combined-v1` and `combined-v2` are not modified: a result is only reproducible against the exact
manifest it scored.

Built by [`benchmark/adapters/combined.py`](../../adapters/combined.py):

```bash
python -m benchmark.adapters combined-v3
```

`manifest.json` is a **view**. Documents are referenced in place (`../dpbench/docs/…`), and ids
are prefixed with the source (`dpbench/01030000000047`).

| Source | Version | License | Docs | Kind | Committed? |
|---|---|---|---:|---|---|
| [`synthetic-v1`](../synthetic-v1/README.md) | 1.1.0 | CC0-1.0 | 39 | transcript | yes |
| [`parsebench`](../parsebench/README.md) | 1.0.0 | Apache-2.0 | 40 | 25 transcript (`table-only`) + 15 rules | yes |
| [`olmocr`](../olmocr/README.md) | 1.0.0 | ODC-BY-1.0 | 40 | rules (205 assertions) | yes |
| [`omnidocbench`](../omnidocbench/README.md) | 1.0.0 | research-only, non-commercial (no SPDX) | 40 | transcript | **no: fetch first** |
| [`dpbench`](../dpbench/README.md) | 1.0.0 | MIT | 40 | transcript | yes |
| **Total** | 3.0.0 | mixed | **199** | 144 transcript + 55 rules | |

The manifest's `sources` array records each source's version, license, count, attribution and
the SHA-256 of the source manifest it was built from.

## Running

```bash
python -m benchmark.adapters omnidocbench        # materialise the non-redistributable source first
cargo build --release -p liteocr-cli
./target/release/liteocr bench run --dataset benchmark/datasets/combined-v3 \
    --models reducto/standard --filter dpbench/ --limit 5
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
4. **DP-Bench truth keeps page headers and footers** (its NID scores them); OmniDocBench truth
   drops them (its evaluation does not). The two sources reward opposite habits on page
   furniture.
5. **A single combined score mixes licences, difficulty and document kinds.** Always publish it
   next to the per-source scores.

## License

Mixed: each document carries its own `license` and `attribution`. This directory contains only
the generated manifest.
