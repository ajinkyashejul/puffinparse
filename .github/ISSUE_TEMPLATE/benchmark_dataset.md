---
name: Benchmark or dataset suggestion
about: Propose a dataset, metric or scoring change for the PuffinParse benchmark
title: ""
labels: benchmark
assignees: ""
---

## What

<!-- A dataset to add, a metric or scoring rule to change, or a problem with an
     existing result. Link the relevant section of benchmark/README.md or
     benchmark/LEADERBOARD.md if there is one. -->

## Dataset (if proposing one)

- **Name and link**:
- **Paper / source**:
- **What it measures**: <!-- e.g. tables, reading order, handwriting, long documents, a language -->
- **Size**: <!-- documents / pages; how many you would include in a subset -->
- **Ground-truth format**: <!-- markdown, HTML tables, rule tests, JSON, ... -->
- **License**: <!-- exact license of the documents AND of the annotations -->
- **Redistributable?** <!-- can the files be committed (MIT-compatible), or must an
     adapter fetch them at run time? Research-only data is scored but its
     per-document outputs are not committed. -->

## Why it is worth adding

<!-- What does it show that synthetic-v1 and combined-v3 (ParseBench, olmOCR-bench,
     OmniDocBench, DP-Bench subsets) do not? -->

## Scoring

<!-- How should it be scored with the existing metrics (transcript similarity,
     table score, TEDS, rule checks)? Does it need a new rule or metric? -->

## Are you planning to send the PR?

<!-- yes / no. Adapters live in benchmark/adapters/; see CONTRIBUTING.md
     section 4 and docs/benchmarks/adapters.md. -->
