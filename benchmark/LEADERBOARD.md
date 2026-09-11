# LiteOCR Leaderboard

Generated from the result files in `benchmark/results/` with `liteocr bench report`. Higher **Overall** is better (100 = character-exact after normalisation). Latency is measured from the client through the public API, including upload and polling, with provider result caches disabled. Prices are public pay-as-you-go list prices. See [`benchmark/README.md`](README.md) for the methodology and caveats.

Last run: 2026-09-11, dataset `combined-v1` v1.0.0 (79 documents, 14 categories), LiteOCR 0.1.0. The combined dataset adds the redistributable ParseBench subset to `synthetic-v1`: its text pages are scored by rule assertions (`kind: rules`, the **Rules** column is the mean pass rate) and its `table-only` pages are headlined by `table_score` rather than `char_similarity`, because their ground truth is the page's table and not the whole page.

| Rank | Model | Overall | Char sim | CER | WER | Word F1 | Order | Table | Rules | p50 latency | p95 latency | ms/page | $/1k pages | Failed | Dataset |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | `reducto/r-1` | **100.00** | 1.000 | 0.000 | 0.000 | 1.000 | 1.000 | 0.951 | – | 3262 ms | 8778 ms | 4231 | $10.00 | 0/39 | synthetic-v1 v1.1.0 |
| 2 | `llamaparse/cost_effective` | **100.00** | 1.000 | 0.000 | 0.000 | 1.000 | 1.000 | 1.000 | – | 9157 ms | 14872 ms | 8910 | $3.75 | 0/39 | synthetic-v1 v1.1.0 |
| 3 | `reducto/standard` | **99.96** | 1.000 | 0.000 | 0.001 | 0.999 | 1.000 | 0.998 | – | 2848 ms | 4347 ms | 2669 | $15.00 | 0/39 | synthetic-v1 v1.1.0 |
| 4 | `llamaparse/agentic` | **99.75** | 0.997 | 0.003 | 0.003 | 0.998 | 1.000 | 1.000 | – | 9666 ms | 15199 ms | 10658 | $12.50 | 0/39 | synthetic-v1 v1.1.0 |
| 5 | `extend/parse_performance` | **98.70** | 0.987 | 0.013 | 0.018 | 0.998 | 1.000 | 0.999 | – | 5680 ms | 9077 ms | 6184 | $25.00 | 0/39 | synthetic-v1 v1.1.0 |
| 6 | `extend/parse_light` | **98.48** | 0.985 | 0.015 | 0.022 | 0.998 | 1.000 | 1.000 | – | 5517 ms | 8932 ms | 5364 | $6.25 | 0/39 | synthetic-v1 v1.1.0 |
| 7 | `llamaparse/agentic` | **92.08** | 0.921 | 0.918 | 0.874 | 0.862 | 0.909 | 0.908 | 84.2% | 14912 ms | 29307 ms | 16179 | $12.50 | 0/79 | combined-v1 v1.0.0 |
| 8 | `llamaparse/cost_effective` | **89.09** | 0.891 | 0.906 | 0.861 | 0.850 | 0.880 | 0.866 | 80.0% | 14005 ms | 29394 ms | 13057 | $3.75 | 0/79 | combined-v1 v1.0.0 |
| 9 | `reducto/standard` | **88.66** | 0.887 | 0.947 | 0.903 | 0.826 | 0.863 | 0.895 | 69.6% | 2825 ms | 4670 ms | 2931 | $15.00 | 0/79 | combined-v1 v1.0.0 |
| 10 | `extend/parse_light` | **87.68** | 0.877 | 1.023 | 0.978 | 0.819 | 0.882 | 0.891 | 69.6% | 5913 ms | 21784 ms | 8190 | $6.25 | 0/79 | combined-v1 v1.0.0 |
| 11 | `llamaparse/fast` | **87.58** | 0.876 | 0.124 | 0.144 | 0.926 | 1.000 | 0.161 | – | 5834 ms | 9286 ms | 5995 | $1.25 | 0/39 | synthetic-v1 v1.1.0 |
| 12 | `extend/parse_performance` | **87.44** | 0.874 | 1.009 | 0.965 | 0.818 | 0.885 | 0.889 | 68.8% | 8920 ms | 21986 ms | 10012 | $25.00 | 0/79 | combined-v1 v1.0.0 |
| 13 | `reducto/r-1` | **87.19** | 0.872 | 0.923 | 0.874 | 0.840 | 0.883 | 0.830 | 74.5% | 3532 ms | 6565 ms | 3767 | $10.00 | 0/79 | combined-v1 v1.0.0 |

### Overall score by category

| Model | Dataset | complex_table | dense | faded | headings | invoice | low_res | multipage | noisy_scan | plain | receipt | skewed | table | text | two_column |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `reducto/r-1` | synthetic-v1 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | – | 100.0 |
| `llamaparse/cost_effective` | synthetic-v1 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | – | 100.0 |
| `reducto/standard` | synthetic-v1 | 100.0 | 99.9 | 99.9 | 100.0 | 99.9 | 100.0 | 99.9 | 99.8 | 100.0 | 100.0 | 100.0 | 100.0 | – | 100.0 |
| `llamaparse/agentic` | synthetic-v1 | 100.0 | 100.0 | 100.0 | 96.8 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | – | 100.0 |
| `extend/parse_performance` | synthetic-v1 | 99.6 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.7 | 100.0 | 99.9 | 83.9 | 100.0 | – | 100.0 |
| `extend/parse_light` | synthetic-v1 | 100.0 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.4 | 100.0 | 99.9 | 81.0 | 100.0 | – | 100.0 |
| `llamaparse/agentic` | combined-v1 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.9 | 100.0 | 100.0 | 100.0 | 86.2 | 84.2 | 100.0 |
| `llamaparse/cost_effective` | combined-v1 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 79.9 | 80.0 | 100.0 |
| `reducto/standard` | combined-v1 | 100.0 | 99.9 | 99.9 | 100.0 | 99.9 | 100.0 | 99.9 | 99.8 | 100.0 | 100.0 | 100.0 | 84.3 | 69.6 | 100.0 |
| `extend/parse_light` | combined-v1 | 100.0 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.4 | 100.0 | 99.9 | 81.0 | 83.7 | 69.6 | 100.0 |
| `llamaparse/fast` | synthetic-v1 | 99.2 | 99.9 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 99.8 | 100.0 | 99.4 | 11.1 | 100.0 | – | 29.3 |
| `extend/parse_performance` | combined-v1 | 99.6 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.7 | 100.0 | 99.9 | 80.6 | 83.4 | 68.8 | 100.0 |
| `reducto/r-1` | combined-v1 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 77.5 | 74.5 | 100.0 |

### `combined-v1` by source

| Source | Docs | Model | Overall |
|---|---:|---|---:|
| parsebench | 40 | `llamaparse/agentic` | 84.37 |
| parsebench | 40 | `llamaparse/cost_effective` | 78.47 |
| parsebench | 40 | `reducto/standard` | 77.64 |
| parsebench | 40 | `extend/parse_light` | 77.15 |
| parsebench | 40 | `extend/parse_performance` | 76.71 |
| parsebench | 40 | `reducto/r-1` | 74.71 |
| synthetic | 39 | `reducto/r-1` | 100.00 |
| synthetic | 39 | `llamaparse/cost_effective` | 100.00 |
| synthetic | 39 | `llamaparse/agentic` | 99.99 |
| synthetic | 39 | `reducto/standard` | 99.96 |
| synthetic | 39 | `extend/parse_light` | 98.48 |
| synthetic | 39 | `extend/parse_performance` | 98.44 |

