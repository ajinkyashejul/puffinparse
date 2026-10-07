# olmocr

A curated subset of **[olmOCR-bench](https://huggingface.co/datasets/allenai/olmOCR-bench)**
(Allen Institute for AI), converted into PuffinParse `kind: "rules"` documents by
[`benchmark/adapters/olmocr.py`](../../adapters/olmocr.py).

- Upstream data: `allenai/olmOCR-bench`, pinned to commit
  **`54a96a6fb6a2bd3b297e59869491db4d3625b711`**.
- Rule semantics follow `olmocr/bench/tests.py` in `github.com/allenai/olmocr` at
  `f7cfe4c22098b154c76b6ec950d1c0a464eecf8d`.
- License: **ODC-BY-1.0**, for research and educational use under AI2's
  [Responsible Use Guidelines](https://allenai.org/responsible-use).
- Cite: Poznanski et al., *olmOCR: Unlocking Trillions of Tokens in PDFs with Vision Language
  Models*, arXiv:2502.18443. Each document keeps its page's original `source_url`.

## Redistribution check (2026-10-08)

The dataset card says the data *"is licensed under ODC-BY-1.0"* and is intended for research
and educational use in accordance with AI2's Responsible Use Guidelines. Both were read on
2026-10-08:

- **ODC-BY-1.0** allows copying, redistributing and adapting the database (a subset and a
  converted rule format count as a derivative database) provided the use is attributed and the
  licence notice is kept. We do both: this README, the manifest's `license` / `attribution`
  on every document, and the original `source_url` per page.
- **AI2's Responsible Use Guidelines** ask users to evaluate outputs critically and list
  prohibited uses (harm, harassment, deception, security and privacy attacks, undisclosed
  automated posting, consequential decisions without a human in the loop). They contain no
  redistribution or extra attribution clause and impose nothing on downstream redistributors.
  Benchmarking document parsers is research and evaluation, which the guidelines permit.

**Conclusion: redistributing this 40-document subset with attribution complies.** One caveat
the card does not address: ODC-BY covers the database, not the copyright in each page. The
pages come from arXiv, the Internet Archive, the Library of Congress and AI2's crawl, and AI2
itself redistributes them under ODC-BY. We rely on the same basis and keep each page's
`source_url` so a rights holder can identify a page and ask for it to be removed.

## Licence notice and citation

This directory contains a derivative of **olmOCR-bench** by the Allen Institute for AI, made
available under the [Open Data Commons Attribution License v1.0
(ODC-BY-1.0)](https://opendatacommons.org/licenses/by/1-0/) (notice in
[`LICENSE-ODC-BY-1.0`](LICENSE-ODC-BY-1.0)). The PuffinParse changes (a 40-page subset, tests
rewritten as `rules/<id>.json`) are made by `benchmark/adapters/olmocr.py`. The rule semantics
re-implement `olmocr/bench/tests.py` from `github.com/allenai/olmocr` (Apache-2.0) in Rust; no
upstream code is copied.

If you use these documents or scores, cite olmOCR as its authors ask:

```bibtex
@misc{olmocrbench,
  title={{olmOCR: Unlocking Trillions of Tokens in PDFs with Vision Language Models}},
  author={Jake Poznanski and Jon Borchardt and Jason Dunkelberger and Regan Huff and Daniel Lin and Aman Rangapur and Christopher Wilhelm and Kyle Lo and Luca Soldaini},
  year={2025},
  eprint={2502.18443},
  archivePrefix={arXiv},
  primaryClass={cs.CL},
  url={https://arxiv.org/abs/2502.18443},
}
```

## Layout

```
benchmark/datasets/olmocr/
  manifest.json           # the 40 committed documents, runnable as-is
  manifest.full.json      # index of all 824 convertible upstream PDFs (bytes NOT committed)
  subset-committed.txt    # the 40 committed ids
  conversion-stats.json   # every converted / relaxed / skipped count, by upstream type and reason
  docs/<id>.pdf           # 40 single-page PDFs (6.0 MB)
  rules/<id>.json         # 205 assertions in the common rule schema
```

## What is committed

8 documents from each of the five splits that have convertible tests. The two math-only splits
(`arxiv_math`, `old_scans_math`) have none.

| Category (upstream split) | Docs | Rules | Types | Rules with `max_diffs > 0` |
|---|---:|---:|---|---:|
| `headers_footers` | 8 | 34 | absent | 5 |
| `long_tiny_text` | 8 | 55 | present | 43 |
| `multi_column` | 8 | 34 | order | 34 |
| `old_scans` | 8 | 46 | present 25, order 16, absent 5 | 11 |
| `table_tests` | 8 | 36 | table_cell | 6 |
| **Total** | **40** | **205** | present 80 · order 50 · absent 39 · table_cell 36 | 99 |

Selection is deterministic for a given `--seed` (default `1234`). Each split is shuffled, and
a document needs at least 3 convertible rules, at most 400 KB, exactly one page, and room in the
6 MB budget.

## Whole-upstream conversion

7,019 upstream tests: 7,010 plus 9 explicit `baseline` tests.

| Upstream type | Count | → PuffinParse | Converted | Skipped | Why |
|---|---:|---|---:|---:|---|
| `present` | 721 | `present` | 721 | 0 | |
| `absent` | 823 | `absent` | 622 | 201 | `first_n` / `last_n` restrict upstream to the start or end of the output; a whole-page absence would wrongly fail, e.g., a page number that also appears in the body |
| `order` | 1,061 | `order` (case-sensitive, like upstream) | 1,061 | 0 | |
| `table` | 1,020 | `table_cell` | 636 | 384 | `top_heading` → `col_header`, `left_heading` → `row_header` (309). `left` / `right` → `row_header` (327, **relaxed**: adjacency becomes "same row"). `up` / `down` skipped (384): the schema has no column-adjacency relation |
| `math` | 3,385 | — | 0 | 3,385 | KaTeX-rendered equation equivalence cannot be expressed as a text rule |
| `baseline` | 9 | — | 0 | 9 | repetition / charset heuristic. The upstream runner also adds 1,394 implicit per-PDF baseline tests, likewise not represented |
| **Total** | **7,019** | | **3,040** | **3,979** | 824 of 1,403 PDFs keep at least one rule |

The builder prints these counts, and they are recorded in `conversion-stats.json` and the
manifest `notes`, so nothing is dropped silently.

## Read the scores carefully

1. **Fuzzy matching.** Upstream allows `max_diffs` Levenshtein edits. Since scorer v2
   (`puffinparse_core::bench::SCORER_VERSION = 2`) the PuffinParse scorer honours it the same way:
   fuzzy substring search for `present` / `absent`, "some fuzzy `before` starts before some fuzzy
   `after`" for `order`, and `max_diffs` edits tolerated in `table_cell` headers and values. A
   scorer-v1 result (no `scorer_version`) matched exactly, which was stricter for `present` /
   `order` / `table_cell` and looser for `absent`; re-score it with `puffinparse bench rescore`. (The
   manifest `notes` still carry the v1 sentence until the adapter is re-run.)
2. **`absent-only` documents pass for an empty parse.** All 8 `headers_footers` documents assert
   only that running headers and footers are *absent*. Upstream guards against empty output with
   its baseline test, which has no analogue here. They carry the `absent-only` tag.
3. **olmOCR's opinion about headers.** A parser that faithfully transcribes page headers fails
   `headers_footers`. That is the benchmark's definition of clean output, not a transcription
   error.
4. **Tables are read from markdown pipe tables** in the prediction. HTML table support in the
   scorer is in progress elsewhere. Until it lands, a provider that emits `<table>` HTML fails
   every `table_cell` rule.
5. **Aggregation differs from upstream.** A document scores `passed / total`, and the dataset
   score is the mean over documents. olmOCR's leaderboard averages pass rates per split, so the
   numbers are not directly comparable to published olmOCR-bench scores.

## Verified

- `cargo test -p puffinparse-core --test benchmark_datasets` builds a witness prediction from each
  document's own assertions and checks that all 205 rules pass (every rule is satisfiable,
  and no `absent` conflicts with a `present`). It also checks that an empty prediction fails
  every document that is not `absent-only`.
- The same witness check over the full 824-document conversion: 3,040 rules, 0 unsatisfiable.
- For contrast, plain `pdftotext` output passes present 23/80, order 24/50, absent 12/39 and
  table_cell 0/36. The old scans have no text layer, and pdftotext emits no tables.

## Regenerating

```bash
pip install huggingface_hub
python -m benchmark.adapters olmocr          # ~2.5 MB of test JSONL + the 40 PDFs
python -m benchmark.adapters combined-v2     # refresh the combined view afterwards
```

The output is byte-identical to what is committed (checked from an empty cache).
