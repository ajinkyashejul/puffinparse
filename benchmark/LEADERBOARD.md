# PuffinParse Leaderboard

Generated from the result files in `benchmark/results/` with `puffinparse bench report` (one section
per dataset; each section is that command's output for one result file). Higher **Overall** is
better (100 = character-exact after normalisation, or every rule passing). Latency is measured
from the client through the public API, including upload and polling, with provider result caches
disabled. Prices are public pay-as-you-go list prices. Methodology and caveats:
[`benchmark/README.md`](README.md). Every document, output, diff and rule check is browsable at
[puffinparse.vercel.app/benchmark-results](https://puffinparse.vercel.app/benchmark-results/).

**Headline: `combined-v2`** (run 2026-09-24, scorer v2) — 159 documents from four sources, each
scored by its own ground truth: `synthetic-v1` (exact transcripts), a ParseBench subset (rules and
table truth), an olmOCR-bench subset (its unit-test style rules, `max_diffs` honoured) and an
OmniDocBench subset (reading-order transcripts; English and Chinese). Compare models within a source
column rather than across sources. `tesseract/default` is a free local OCR baseline with no layout
model, so it scores 0 wherever table structure is scored. 1,113 calls, 0 failures, $11.74 at list
price. OmniDocBench is research-only, so its per-page outputs are not committed (scores are).

Older runs are kept for comparison: `combined-v1` (79 documents, 2026-09-11) and `synthetic-v1`
(39 documents, 2026-09-11), both re-scored offline with scorer v2 on 2026-09-24 (`puffinparse bench
rescore`; latency and cost as originally measured). See
[`docs/benchmarks/findings.md`](../docs/benchmarks/findings.md) for what scorer v2 changed.

## combined-v2 (headline)

| Rank | Model | Overall | Char sim | CER | WER | Word F1 | Order | Table | TEDS | Rules | p50 latency | p95 latency | ms/page | $/1k pages | Failed | Dataset |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | `llamaparse/cost_effective` | **83.79** | 0.786 | 0.578 | 0.683 | 0.757 | 0.903 | 0.876 | 0.880 | 73.0% | 9535 ms | 19846 ms | 11864 | $3.75 | 0/159 | combined-v2 v2.0.0 |
| 2 | `llamaparse/agentic` | **83.05** | 0.774 | 0.609 | 0.768 | 0.759 | 0.905 | 0.892 | 0.879 | 71.7% | 14269 ms | 26848 ms | 15701 | $12.50 | 0/159 | combined-v2 v2.0.0 |
| 3 | `reducto/r-1` | **81.32** | 0.758 | 0.633 | 0.784 | 0.748 | 0.895 | 0.899 | 0.867 | 70.6% | 3654 ms | 8437 ms | 4841 | $10.00 | 0/159 | combined-v2 v2.0.0 |
| 4 | `reducto/standard` | **77.87** | 0.727 | 0.660 | 0.817 | 0.715 | 0.887 | 0.877 | 0.860 | 65.7% | 3570 ms | 8253 ms | 4364 | $15.00 | 0/159 (+1 empty) | combined-v2 v2.0.0 |
| 5 | `extend/parse_performance` | **77.35** | 0.720 | 0.729 | 0.966 | 0.720 | 0.895 | 0.893 | 0.853 | 67.4% | 9089 ms | 22019 ms | 10592 | $25.00 | 0/159 | combined-v2 v2.0.0 |
| 6 | `extend/parse_light` | **75.92** | 0.710 | 0.744 | 0.979 | 0.707 | 0.888 | 0.866 | 0.846 | 65.4% | 5709 ms | 14298 ms | 7478 | $6.25 | 0/159 | combined-v2 v2.0.0 |
| 7 | `tesseract/default` | **48.68** | 0.556 | 0.815 | 1.344 | 0.572 | 0.786 | 0.007 | 0.007 | 45.4% | 2136 ms | 19769 ms | 3841 | $0.00 | 0/159 (+4 empty) | combined-v2 v2.0.0 |

### Overall score by category

| Model | academic_literature | book | colorful_textbook | complex_table | dense | exam_paper | faded | headers_footers | headings | historical_document | invoice | long_tiny_text | low_res | magazine | multi_column | multipage | newspaper | noisy_scan | note | old_scans | plain | ppt2pdf | receipt | research_report | skewed | table | table_tests | text | two_column |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `llamaparse/cost_effective` | 79.2 | 82.9 | 77.9 | 100.0 | 100.0 | 73.0 | 100.0 | 28.1 | 100.0 | 73.8 | 100.0 | 94.3 | 100.0 | 84.9 | 82.5 | 100.0 | 77.4 | 100.0 | 93.2 | 69.1 | 100.0 | 91.0 | 100.0 | 85.2 | 100.0 | 86.9 | 72.9 | 82.7 | 100.0 |
| `llamaparse/agentic` | 83.5 | 82.6 | 73.4 | 100.0 | 100.0 | 55.0 | 100.0 | 19.8 | 100.0 | 73.2 | 100.0 | 91.2 | 100.0 | 80.3 | 82.5 | 100.0 | 86.8 | 99.9 | 91.9 | 66.4 | 100.0 | 79.7 | 100.0 | 83.8 | 100.0 | 89.3 | 72.9 | 85.3 | 100.0 |
| `reducto/r-1` | 75.0 | 82.6 | 70.5 | 100.0 | 100.0 | 46.7 | 100.0 | 14.6 | 100.0 | 76.3 | 100.0 | 93.6 | 100.0 | 59.3 | 83.1 | 100.0 | 65.9 | 100.0 | 93.4 | 66.3 | 100.0 | 88.7 | 100.0 | 83.4 | 100.0 | 88.6 | 85.4 | 75.8 | 100.0 |
| `reducto/standard` | 78.9 | 76.4 | 69.1 | 100.0 | 99.9 | 54.4 | 99.9 | 11.5 | 100.0 | 17.3 | 99.9 | 84.1 | 100.0 | 67.2 | 83.1 | 99.9 | 64.9 | 99.8 | 90.6 | 60.1 | 100.0 | 81.0 | 100.0 | 86.8 | 100.0 | 86.5 | 79.5 | 71.2 | 100.0 |
| `extend/parse_performance` | 78.4 | 77.0 | 57.4 | 99.6 | 99.9 | 47.6 | 100.0 | 5.2 | 100.0 | 44.3 | 100.0 | 90.9 | 100.0 | 53.2 | 83.1 | 100.0 | 61.5 | 99.7 | 91.7 | 58.8 | 100.0 | 77.6 | 99.9 | 73.8 | 83.9 | 85.5 | 92.7 | 70.7 | 100.0 |
| `extend/parse_light` | 74.7 | 77.3 | 57.9 | 100.0 | 99.9 | 47.6 | 100.0 | 8.3 | 100.0 | 48.5 | 100.0 | 77.7 | 100.0 | 53.2 | 73.8 | 100.0 | 60.3 | 99.4 | 91.4 | 61.3 | 100.0 | 77.4 | 99.9 | 73.4 | 81.0 | 81.7 | 84.4 | 76.7 | 100.0 |
| `tesseract/default` | 15.0 | 50.5 | 44.1 | 99.8 | 99.6 | 33.2 | 99.2 | 26.0 | 100.0 | 6.2 | 100.0 | 75.2 | 99.9 | 59.9 | 67.5 | 99.4 | 50.5 | 97.8 | 2.5 | 39.9 | 100.0 | 50.5 | 97.7 | 43.2 | 80.0 | 10.7 | 0.0 | 55.3 | 100.0 |

### `combined-v2` by source

| Source | Docs | Model | Overall |
|---|---:|---|---:|
| olmocr | 40 | `llamaparse/cost_effective` | 69.39 |
| olmocr | 40 | `reducto/r-1` | 68.61 |
| olmocr | 40 | `llamaparse/agentic` | 66.57 |
| olmocr | 40 | `extend/parse_performance` | 66.15 |
| olmocr | 40 | `reducto/standard` | 63.65 |
| olmocr | 40 | `extend/parse_light` | 61.09 |
| olmocr | 40 | `tesseract/default` | 41.72 |
| omnidocbench | 40 | `llamaparse/cost_effective` | 81.85 |
| omnidocbench | 40 | `llamaparse/agentic` | 79.02 |
| omnidocbench | 40 | `reducto/r-1` | 74.18 |
| omnidocbench | 40 | `reducto/standard` | 68.66 |
| omnidocbench | 40 | `extend/parse_performance` | 66.25 |
| omnidocbench | 40 | `extend/parse_light` | 66.18 |
| omnidocbench | 40 | `tesseract/default` | 35.56 |
| parsebench | 40 | `llamaparse/agentic` | 87.04 |
| parsebench | 40 | `llamaparse/cost_effective` | 84.33 |
| parsebench | 40 | `reducto/r-1` | 82.95 |
| parsebench | 40 | `reducto/standard` | 79.75 |
| parsebench | 40 | `extend/parse_performance` | 78.82 |
| parsebench | 40 | `extend/parse_light` | 78.49 |
| parsebench | 40 | `tesseract/default` | 20.73 |
| synthetic | 39 | `reducto/r-1` | 100.00 |
| synthetic | 39 | `llamaparse/cost_effective` | 100.00 |
| synthetic | 39 | `llamaparse/agentic` | 99.99 |
| synthetic | 39 | `reducto/standard` | 99.96 |
| synthetic | 39 | `extend/parse_performance` | 98.70 |
| synthetic | 39 | `extend/parse_light` | 98.48 |
| synthetic | 39 | `tesseract/default` | 97.94 |


## combined-v1

| Rank | Model | Overall | Char sim | CER | WER | Word F1 | Order | Table | TEDS | Rules | p50 latency | p95 latency | ms/page | $/1k pages | Failed | Dataset |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | `llamaparse/agentic` | **93.08** | 0.821 | 0.916 | 0.872 | 0.865 | 0.922 | 0.922 | 0.859 | 85.4% | 14912 ms | 29307 ms | 16179 | $12.50 | 0/79 | combined-v1 v1.0.0 |
| 2 | `reducto/r-1` | **91.37** | 0.802 | 0.921 | 0.872 | 0.842 | 0.894 | 0.924 | 0.872 | 75.9% | 3532 ms | 6565 ms | 3767 | $10.00 | 0/79 | combined-v1 v1.0.0 |
| 3 | `llamaparse/cost_effective` | **90.38** | 0.803 | 0.904 | 0.859 | 0.852 | 0.893 | 0.886 | 0.850 | 81.3% | 14005 ms | 29394 ms | 13057 | $3.75 | 0/79 | combined-v1 v1.0.0 |
| 4 | `reducto/standard` | **89.91** | 0.794 | 0.944 | 0.900 | 0.829 | 0.885 | 0.913 | 0.868 | 71.2% | 2825 ms | 4670 ms | 2931 | $15.00 | 0/79 | combined-v1 v1.0.0 |
| 5 | `extend/parse_light` | **88.02** | 0.775 | 1.020 | 0.975 | 0.822 | 0.905 | 0.892 | 0.863 | 71.2% | 5913 ms | 21784 ms | 8190 | $6.25 | 0/79 | combined-v1 v1.0.0 |
| 6 | `extend/parse_performance` | **87.80** | 0.777 | 1.006 | 0.962 | 0.821 | 0.905 | 0.889 | 0.853 | 70.7% | 8920 ms | 21986 ms | 10012 | $25.00 | 0/79 | combined-v1 v1.0.0 |

### Overall score by category

| Model | complex_table | dense | faded | headings | invoice | low_res | multipage | noisy_scan | plain | receipt | skewed | table | text | two_column |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `llamaparse/agentic` | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.9 | 100.0 | 100.0 | 100.0 | 88.3 | 85.4 | 100.0 |
| `reducto/r-1` | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 88.6 | 75.9 | 100.0 |
| `llamaparse/cost_effective` | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 82.9 | 81.3 | 100.0 |
| `reducto/standard` | 100.0 | 99.9 | 99.9 | 100.0 | 99.9 | 100.0 | 99.9 | 99.8 | 100.0 | 100.0 | 100.0 | 87.0 | 71.2 | 100.0 |
| `extend/parse_light` | 100.0 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.4 | 100.0 | 99.9 | 81.0 | 83.7 | 71.2 | 100.0 |
| `extend/parse_performance` | 99.6 | 99.9 | 100.0 | 100.0 | 100.0 | 100.0 | 100.0 | 99.7 | 100.0 | 99.9 | 80.6 | 83.5 | 70.7 | 100.0 |

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


## synthetic-v1

| Rank | Model | Overall | Char sim | CER | WER | Word F1 | Order | Table | TEDS | Rules | p50 latency | p95 latency | ms/page | $/1k pages | Failed | Dataset |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | `reducto/r-1` | **100.00** | 1.000 | 0.000 | 0.000 | 1.000 | 1.000 | 1.000 | 1.000 | – | 3262 ms | 8778 ms | 4231 | $10.00 | 0/39 | synthetic-v1 v1.1.0 |
| 2 | `llamaparse/cost_effective` | **100.00** | 1.000 | 0.000 | 0.000 | 1.000 | 1.000 | 1.000 | 1.000 | – | 9157 ms | 14872 ms | 8910 | $3.75 | 0/39 | synthetic-v1 v1.1.0 |
| 3 | `reducto/standard` | **99.96** | 1.000 | 0.000 | 0.001 | 0.999 | 1.000 | 0.998 | 0.986 | – | 2848 ms | 4347 ms | 2669 | $15.00 | 0/39 | synthetic-v1 v1.1.0 |
| 4 | `llamaparse/agentic` | **99.75** | 0.997 | 0.003 | 0.003 | 0.998 | 1.000 | 1.000 | 1.000 | – | 9666 ms | 15199 ms | 10658 | $12.50 | 0/39 | synthetic-v1 v1.1.0 |
| 5 | `extend/parse_performance` | **98.70** | 0.987 | 0.013 | 0.018 | 0.998 | 1.000 | 0.999 | 0.949 | – | 5680 ms | 9077 ms | 6184 | $25.00 | 0/39 | synthetic-v1 v1.1.0 |
| 6 | `extend/parse_light` | **98.48** | 0.985 | 0.015 | 0.022 | 0.998 | 1.000 | 1.000 | 1.000 | – | 5517 ms | 8932 ms | 5364 | $6.25 | 0/39 | synthetic-v1 v1.1.0 |
| 7 | `llamaparse/fast` | **87.58** | 0.876 | 0.124 | 0.144 | 0.926 | 1.000 | 0.161 | 0.170 | – | 5834 ms | 9286 ms | 5995 | $1.25 | 0/39 | synthetic-v1 v1.1.0 |

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

