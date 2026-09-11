# LiteOCR Leaderboard

Generated from the result files in `benchmark/results/` with `liteocr bench report`. Higher **Overall** is better (100 = character-exact after normalisation). Latency is measured from the client through the public API, including upload and polling, with provider result caches disabled. Prices are public pay-as-you-go list prices. See [`benchmark/README.md`](README.md) for the methodology and caveats.

Last run: 2026-09-11, dataset `synthetic-v1` v1.1.0 (39 documents, 13 categories), LiteOCR 0.1.0.

| Rank | Model | Overall | Char sim | CER | WER | Word F1 | Order | Table | p50 latency | p95 latency | ms/page | $/1k pages | Failed | Dataset |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | `reducto/r-1` | **100.00** | 1.000 | 0.000 | 0.000 | 1.000 | 1.000 | 0.951 | 3262 ms | 8778 ms | 4231 | $10.00 | 0/39 | synthetic-v1 v1.1.0 |
| 2 | `llamaparse/cost_effective` | **100.00** | 1.000 | 0.000 | 0.000 | 1.000 | 1.000 | 1.000 | 9157 ms | 14872 ms | 8910 | $3.75 | 0/39 | synthetic-v1 v1.1.0 |
| 3 | `reducto/standard` | **99.96** | 1.000 | 0.000 | 0.001 | 0.999 | 1.000 | 0.998 | 2848 ms | 4347 ms | 2669 | $15.00 | 0/39 | synthetic-v1 v1.1.0 |
| 4 | `llamaparse/agentic` | **99.75** | 0.997 | 0.003 | 0.003 | 0.998 | 1.000 | 1.000 | 9666 ms | 15199 ms | 10658 | $12.50 | 0/39 | synthetic-v1 v1.1.0 |
| 5 | `extend/parse_performance` | **98.70** | 0.987 | 0.013 | 0.018 | 0.998 | 1.000 | 0.999 | 5680 ms | 9077 ms | 6184 | $25.00 | 0/39 | synthetic-v1 v1.1.0 |
| 6 | `extend/parse_light` | **98.48** | 0.985 | 0.015 | 0.022 | 0.998 | 1.000 | 1.000 | 5517 ms | 8932 ms | 5364 | $6.25 | 0/39 | synthetic-v1 v1.1.0 |
| 7 | `llamaparse/fast` | **87.58** | 0.876 | 0.124 | 0.144 | 0.926 | 1.000 | 0.161 | 5834 ms | 9286 ms | 5995 | $1.25 | 0/39 | synthetic-v1 v1.1.0 |

### Overall score by category

| Model | complex_table | dense | faded | headings | invoice | low_res | multipage | noisy_scan | plain | receipt | skewed | table | two_column |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `reducto/r-1` | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 |
| `llamaparse/cost_effective` | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 |
| `reducto/standard` | 100.0 | 99.9 | 99.9 | 100.0 | 99.9 | 100.0 | 99.9 | 99.8 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 |
| `llamaparse/agentic` | 100.0 | 100.0 | 100.0 | 96.8 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 |
| `extend/parse_performance` | 99.6 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.7 | 100.0 | 99.9 | 83.9 | 100.0 | 100.0 |
| `extend/parse_light` | 100.0 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.4 | 100.0 | 99.9 | 81.0 | 100.0 | 100.0 |
| `llamaparse/fast` | 99.2 | 99.9 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 99.8 | 100.0 | 99.4 | 11.1 | 100.0 | 29.3 |

