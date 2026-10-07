# omnidocbench

A 40-page subset of **[OmniDocBench](https://huggingface.co/datasets/opendatalab/OmniDocBench)**
(OpenDataLab / Shanghai AI Laboratory), indexed by
[`benchmark/adapters/omnidocbench.py`](../../adapters/omnidocbench.py) as
`kind: "transcript"` documents with reading-order markdown truth.

- Upstream data: `opendatalab/OmniDocBench`, pinned to commit
  **`aa1ee96d106dbe53d0ae59474d75c6e6d9b53fec`**.
- Truth construction follows `tools/json2md.py` in `github.com/opendatalab/OmniDocBench` at
  `f133a71e9e91c3621c7ce8994200a7b394a06eb3`.
- Cite: Ouyang et al., *OmniDocBench: Benchmarking Diverse PDF Document Parsing with
  Comprehensive Annotations*, arXiv:2412.07626.

```bibtex
@misc{ouyang2024omnidocbenchbenchmarkingdiversepdf,
  title={OmniDocBench: Benchmarking Diverse PDF Document Parsing with Comprehensive Annotations},
  author={Linke Ouyang and Yuan Qu and Hongbin Zhou and Jiawei Zhu and Rui Zhang and Qunshu Lin and Bin Wang and Zhiyuan Zhao and Man Jiang and Xiaomeng Zhao and Jin Shi and Fan Wu and Pei Chu and Minghao Liu and Zhenxiang Li and Chao Xu and Bo Zhang and Botian Shi and Zhongying Tu and Conghui He},
  year={2024},
  eprint={2412.07626},
  archivePrefix={arXiv},
  primaryClass={cs.CV},
  url={https://arxiv.org/abs/2412.07626},
}
```

The evaluation code at `github.com/opendatalab/OmniDocBench` is Apache-2.0 (the dataset is
not). The adapter's `_text_norm` reproduces the three repeated-character rules of upstream's
`json2md.text_norm` so the truth matches upstream's; no other upstream code is used.

## Index only: fetch before running

The dataset card has **no licence**. Its copyright statement says the data is *"for research
purposes only and not for commercial use"*. That grants no right to redistribute in an MIT
repository, so **only `manifest.json` is committed**. The page images and the truth derived from
the annotations are git-ignored and produced locally:

```bash
pip install huggingface_hub
python -m benchmark.adapters omnidocbench    # 42 MB annotations + ~33 MB of page images
```

The run downloads `OmniDocBench.json` and the 40 images at the pinned revision, writes
`docs/` and `truth/`, and rewrites `manifest.json` byte-identically. Each document carries the
image `sha256` and the `truth_sha256` it must produce. If you skip this step, every document
here (and every `omnidocbench/…` document in `combined-v2`) fails with file-not-found, and the
result's dataset `sha256` covers only the manifest. By using the fetched data you accept
OpenDataLab's research-only terms.

**Provider outputs are never committed for this source** (`.gitignore` excludes
`benchmark/results/outputs/*/*/omnidocbench/`, ADR-16): a parser's transcript of a page is a
copy of that page. Only per-document scores live in the result JSON.

```
benchmark/datasets/omnidocbench/
  manifest.json            # committed: ids, upstream_path, sha256, truth_sha256, tags
  conversion-stats.json    # committed: page counts, skips by reason
  .gitignore               # docs/ and truth/ never enter git
  docs/<id>.png|jpg        # local only
  truth/<id>.md            # local only
```

## What is selected

The builder goes round-robin over the 10 `data_source` values, 4 pages each. Inside a source it
picks the language furthest below a target mix of 40% English, 40% Simplified Chinese, 15%
mixed and 5% Traditional Chinese.

| Category (`data_source`) | Pages |
|---|---:|
| academic_literature, book, colorful_textbook, exam_paper, historical_document, magazine, newspaper, note, ppt2pdf, research_report | 4 each |

Languages: 15 Simplified Chinese, 14 English, 6 mixed, 4 Traditional Chinese, 1 other. 13
pages contain tables (6 of them with merged cells) and 2 contain display formulas. The subset
tags are v1.5 (25), layout_hard (9), table_hard (5) and equation_hard (1).

Tags: `omnidocbench`, `fetch-required`, the data source, `lang-*`, `layout-*`, `subset-*`,
`issue-*` (upstream `special_issue`), `has-table`, `has-formula`, `merged-cells`.

## How the truth is built

- Blocks with a reading `order` are emitted in that order. Paragraphs split across columns
  (`truncated` relations) are merged first: Latin text is joined with a space, or de-hyphenated;
  CJK text is joined directly.
- `title` becomes a `# heading`. `table` HTML becomes a GitHub pipe table (merged cells repeated,
  `merged-cells` tag). `equation_isolated` stays as its `$$…$$` LaTeX. Other text blocks keep
  their text, with upstream's `text_norm` (long runs of `_`, spaces or symbols shortened) and
  the annotations' literal `\t` indents removed.
- Excluded, as in OmniDocBench's own text matching: `header`, `footer`, `page_number`,
  `page_footnote`, `abandon`, and `figure` regions.
- Pages with `*_mask` regions (126 upstream) are never selected, because their content is
  deliberately unannotated and a correct parse would be penalised. Pages whose truth is under
  120 characters (82) are skipped too. 1,443 of 1,651 pages are convertible.

## Read the scores carefully

- `char_similarity` is whole-page edit distance. OmniDocBench matches block by block and
  forgives some reading-order differences, so PuffinParse numbers are stricter on multi-column and
  "other layout" pages.
- A parser that transcribes running headers or page numbers is penalised, because the truth
  omits them.
- Formulas are compared as LaTeX text, not rendered (upstream uses CDM). Tables are compared
  row by row with `table_score`, not with TEDS.

## Verified

`cargo test -p puffinparse-core --test benchmark_datasets` scores every locally built truth against
itself (`char_similarity` 1.0) and checks that `has-table` agrees with `table_score` being
present. `puffinparse bench score truth/<id>.md truth/<id>.md` returns 1.0 on every metric, including
`table_score`.
