# LiteOCR Leaderboard

Generated from the result files in `benchmark/results/` with `liteocr bench report`. Higher **Overall** is better (100 = character-exact after normalisation). Latency is measured from the client through the public API, including upload and polling, with provider result caches disabled. Prices are public pay-as-you-go list prices. See [`benchmark/README.md`](README.md) for the methodology and caveats.

Last run: 2026-09-11, dataset `combined-v1` v1.0.0 (79 documents, 14 categories), LiteOCR 0.1.0. The combined dataset adds the redistributable ParseBench subset to `synthetic-v1`: its text pages are scored by rule assertions (`kind: rules`, the **Rules** column is the mean pass rate) and its `table-only` pages are headlined by `table_score` rather than `char_similarity`, because their ground truth is the page's table and not the whole page.

Both runs were re-scored offline on 2026-09-24 with scorer v2 (`liteocr bench rescore`, no provider calls; latency and cost are the originally measured values). Scorer v2 reads HTML `<table>`s as well as pipe tables, adds **TEDS** (tree-edit-distance similarity on the row/cell grid, i.e. table structure plus content), ignores spaces next to punctuation when matching rules, matches `bag_of_sentences` sentences fuzzily with a 0.8 threshold, and keeps **Char sim** the literal character similarity (the headline lives in **Overall**). See [`docs/benchmarks/findings.md`](../docs/benchmarks/findings.md) for what changed and why.

| Rank | Model | Overall | Char sim | CER | WER | Word F1 | Order | Table | TEDS | Rules | p50 latency | p95 latency | ms/page | $/1k pages | Failed | Dataset |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | `reducto/r-1` | **100.00** | 1.000 | 0.000 | 0.000 | 1.000 | 1.000 | 1.000 | 1.000 | – | 3262 ms | 8778 ms | 4231 | $10.00 | 0/39 | synthetic-v1 v1.1.0 |
| 2 | `llamaparse/cost_effective` | **100.00** | 1.000 | 0.000 | 0.000 | 1.000 | 1.000 | 1.000 | 1.000 | – | 9157 ms | 14872 ms | 8910 | $3.75 | 0/39 | synthetic-v1 v1.1.0 |
| 3 | `reducto/standard` | **99.96** | 1.000 | 0.000 | 0.001 | 0.999 | 1.000 | 0.998 | 0.986 | – | 2848 ms | 4347 ms | 2669 | $15.00 | 0/39 | synthetic-v1 v1.1.0 |
| 4 | `llamaparse/agentic` | **99.75** | 0.997 | 0.003 | 0.003 | 0.998 | 1.000 | 1.000 | 1.000 | – | 9666 ms | 15199 ms | 10658 | $12.50 | 0/39 | synthetic-v1 v1.1.0 |
| 5 | `extend/parse_performance` | **98.70** | 0.987 | 0.013 | 0.018 | 0.998 | 1.000 | 0.999 | 0.949 | – | 5680 ms | 9077 ms | 6184 | $25.00 | 0/39 | synthetic-v1 v1.1.0 |
| 6 | `extend/parse_light` | **98.48** | 0.985 | 0.015 | 0.022 | 0.998 | 1.000 | 1.000 | 1.000 | – | 5517 ms | 8932 ms | 5364 | $6.25 | 0/39 | synthetic-v1 v1.1.0 |
| 7 | `llamaparse/agentic` | **93.08** | 0.821 | 0.916 | 0.872 | 0.865 | 0.922 | 0.922 | 0.859 | 85.4% | 14912 ms | 29307 ms | 16179 | $12.50 | 0/79 | combined-v1 v1.0.0 |
| 8 | `reducto/r-1` | **91.37** | 0.802 | 0.921 | 0.872 | 0.842 | 0.894 | 0.924 | 0.872 | 75.9% | 3532 ms | 6565 ms | 3767 | $10.00 | 0/79 | combined-v1 v1.0.0 |
| 9 | `llamaparse/cost_effective` | **90.38** | 0.803 | 0.904 | 0.859 | 0.852 | 0.893 | 0.886 | 0.850 | 81.3% | 14005 ms | 29394 ms | 13057 | $3.75 | 0/79 | combined-v1 v1.0.0 |
| 10 | `reducto/standard` | **89.91** | 0.794 | 0.944 | 0.900 | 0.829 | 0.885 | 0.913 | 0.868 | 71.2% | 2825 ms | 4670 ms | 2931 | $15.00 | 0/79 | combined-v1 v1.0.0 |
| 11 | `extend/parse_light` | **88.02** | 0.775 | 1.020 | 0.975 | 0.822 | 0.905 | 0.892 | 0.863 | 71.2% | 5913 ms | 21784 ms | 8190 | $6.25 | 0/79 | combined-v1 v1.0.0 |
| 12 | `extend/parse_performance` | **87.80** | 0.777 | 1.006 | 0.962 | 0.821 | 0.905 | 0.889 | 0.853 | 70.7% | 8920 ms | 21986 ms | 10012 | $25.00 | 0/79 | combined-v1 v1.0.0 |
| 13 | `llamaparse/fast` | **87.58** | 0.876 | 0.124 | 0.144 | 0.926 | 1.000 | 0.161 | 0.170 | – | 5834 ms | 9286 ms | 5995 | $1.25 | 0/39 | synthetic-v1 v1.1.0 |

### Overall score by category

| Model | Dataset | complex_table | dense | faded | headings | invoice | low_res | multipage | noisy_scan | plain | receipt | skewed | table | text | two_column |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `reducto/r-1` | synthetic-v1 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | – | 100.0 |
| `llamaparse/cost_effective` | synthetic-v1 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | – | 100.0 |
| `reducto/standard` | synthetic-v1 | 100.0 | 99.9 | 99.9 | 100.0 | 99.9 | 100.0 | 99.9 | 99.8 | 100.0 | 100.0 | 100.0 | 100.0 | – | 100.0 |
| `llamaparse/agentic` | synthetic-v1 | 100.0 | 100.0 | 100.0 | 96.8 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | – | 100.0 |
| `extend/parse_performance` | synthetic-v1 | 99.6 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.7 | 100.0 | 99.9 | 83.9 | 100.0 | – | 100.0 |
| `extend/parse_light` | synthetic-v1 | 100.0 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.4 | 100.0 | 99.9 | 81.0 | 100.0 | – | 100.0 |
| `llamaparse/agentic` | combined-v1 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.9 | 100.0 | 100.0 | 100.0 | 88.3 | 85.4 | 100.0 |
| `reducto/r-1` | combined-v1 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 88.6 | 75.9 | 100.0 |
| `llamaparse/cost_effective` | combined-v1 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 82.9 | 81.3 | 100.0 |
| `reducto/standard` | combined-v1 | 100.0 | 99.9 | 99.9 | 100.0 | 99.9 | 100.0 | 99.9 | 99.8 | 100.0 | 100.0 | 100.0 | 87.0 | 71.2 | 100.0 |
| `extend/parse_light` | combined-v1 | 100.0 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.4 | 100.0 | 99.9 | 81.0 | 83.7 | 71.2 | 100.0 |
| `extend/parse_performance` | combined-v1 | 99.6 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.7 | 100.0 | 99.9 | 80.6 | 83.5 | 70.7 | 100.0 |
| `llamaparse/fast` | synthetic-v1 | 99.2 | 99.9 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 99.8 | 100.0 | 99.4 | 11.1 | 100.0 | – | 29.3 |

### `combined-v1` by source

| Source | Docs | Model | Overall |
|---|---:|---|---:|
| parsebench | 40 | `llamaparse/agentic` | 86.34 |
| parsebench | 40 | `reducto/r-1` | 82.95 |
| parsebench | 40 | `llamaparse/cost_effective` | 81.00 |
| parsebench | 40 | `reducto/standard` | 80.11 |
| parsebench | 40 | `extend/parse_light` | 77.82 |
| parsebench | 40 | `extend/parse_performance` | 77.42 |
| synthetic | 39 | `reducto/r-1` | 100.00 |
| synthetic | 39 | `llamaparse/cost_effective` | 100.00 |
| synthetic | 39 | `llamaparse/agentic` | 99.99 |
| synthetic | 39 | `reducto/standard` | 99.96 |
| synthetic | 39 | `extend/parse_light` | 98.48 |
| synthetic | 39 | `extend/parse_performance` | 98.44 |

