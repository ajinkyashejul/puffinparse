# Benchmark adapters

How public OCR benchmarks become LiteOCR datasets.

[ADR-10](../DECISIONS.md) says LiteOCR does not author a competing benchmark: it runs every
public benchmark through one harness. An **adapter** is the piece that makes that true — it
fetches one upstream benchmark at a pinned revision and rewrites it into
`benchmark/datasets/<name>/manifest.json`. Datasets whose license permits redistribution are
vendored into the repository; the rest stay an index plus a `sha256`, fetched on demand.

Everything lives in [`benchmark/adapters/`](../../benchmark/adapters/):

| File | What it is |
|---|---|
| `base.py` | the framework: `Adapter`, the manifest model, the rule schema, shared helpers |
| `parsebench.py` | LlamaIndex ParseBench → `benchmark/datasets/parsebench/` |
| `combined.py` | union of the committed datasets → `benchmark/datasets/combined-v1/` |
| `__main__.py` | the `python -m benchmark.adapters` CLI |

## CLI

```bash
python -m benchmark.adapters --list
python -m benchmark.adapters <name> [--limit N] [--out DIR] [--cache DIR] [--seed N] [--no-download]
```

| Flag | Meaning |
|---|---|
| `--limit N` | cap the number of documents built (selection stays deterministic) |
| `--out DIR` | output dataset directory (default: the adapter's `default_out`) |
| `--cache DIR` | download cache (default: `$LITEOCR_BENCH_CACHE`, else `$XDG_CACHE_HOME/liteocr/benchmarks`, else `~/.cache/liteocr/benchmarks`) — never inside the repo |
| `--seed N` | selection seed, default `1234` |
| `--no-download` | build from an existing cache, never hit the network |

Downloads go over HTTPS through whatever proxy the environment configures. Behind the agent
proxy, point `huggingface_hub` at the CA bundle (the adapter sets these if they are unset) —
never disable TLS verification:

```bash
export REQUESTS_CA_BUNDLE=/root/.ccr/ca-bundle.crt SSL_CERT_FILE=/root/.ccr/ca-bundle.crt
```

The CLI prints a build summary: documents by kind, committed size, and the adapter's own
counters (rules converted, rules skipped by upstream type, documents skipped and why).

## Manifest extension

The manifest stays exactly what [`benchmark/README.md`](../../benchmark/README.md) and
[`docs/SPEC.md`](../SPEC.md) §10.2 describe:

```json
{"name", "version", "description", "license",
 "documents": [{"id", "file", "truth", "pages", "category", "tags"}]}
```

Adapters add optional keys. **Every added key is additive and ignored by the current Rust
`Manifest` / `ManifestDoc`** (serde ignores unknown fields by default), so old manifests keep
working and new manifests load in an unmodified CLI.

### Document-level additions

| Key | Type | Meaning |
|---|---|---|
| `kind` | `"transcript"` \| `"rules"` | how the document is scored. **Default `"transcript"`** — absent means transcript, which is what every pre-existing manifest is |
| `rules` | path | for `kind: "rules"`, the assertion file, relative to the dataset directory |
| `source_id` | string | the upstream id, kept verbatim when `id` had to be slugified |
| `upstream_path` | string | the document's path inside the upstream repo, for on-demand fetch |
| `sha256` | hex | the document file's hash |
| `license` | SPDX | per-document license, when a dataset mixes them |
| `attribution` | string | the citation this document must carry |

`truth` is **always emitted**, even for `kind: "rules"` documents, where it is the empty string.
That is not cosmetic: the Rust `ManifestDoc` declares `pub truth: String` without
`#[serde(default)]`, so a missing key fails to deserialise the *whole* manifest.

### Manifest-level additions

`generator`, `upstream` (`{repo_id, revision, url, repo_type}`), `attribution`, `notes`, and —
for combined datasets — `sources`:

```json
"sources": [{"name", "version", "license", "path", "documents", "manifest_sha256", "attribution"}]
```

### `kind: "transcript"`

The existing behaviour: `truth` is markdown, scored by `liteocr_core::bench` with
`char_similarity` / `cer` / `wer` / `word_f1` / `order_score` / `table_score`.

### `kind: "rules"`

`truth` is empty and `rules` points at a JSON **list** of machine-checkable assertions. One
schema covers every rule-based upstream benchmark we care about (ParseBench today,
olmOCR-bench next):

```json
{
  "id": "text_dense_baoutou_order_623",
  "type": "present" | "absent" | "order" | "table_cell" | "bag_of_sentences",
  "text": "…",                       // present / absent
  "before": "…", "after": "…",       // order
  "cell": {"row_header": "…", "col_header": "…", "value": "…"},   // table_cell
  "sentences": ["…"],                // bag_of_sentences
  "threshold": 1.0,                  // bag_of_sentences: required pass fraction
  "case_sensitive": false,
  "source": "text_dense__baoutou_order_623"
}
```

| Type | Passes when the parsed markdown… |
|---|---|
| `present` | contains `text` (after the run's normalisation) |
| `absent` | does **not** contain `text` |
| `order` | contains both `before` and `after`, and the first occurrence of `before` precedes the first occurrence of `after` |
| `table_cell` | has a markdown table with a row matching `cell.row_header` and a column matching `cell.col_header` whose cell equals `cell.value` |
| `bag_of_sentences` | contains at least `threshold` (fraction, default `1.0`) of `sentences` |

`case_sensitive` is always present and always explicit. `source` is the upstream rule id, so any
score can be pushed back to the publisher's own harness for cross-checking.

A document's rule score is `passed / total`; a dataset's is the mean over its rule documents.
That is deliberately the same shape as an accuracy in `[0, 1]`, so it slots next to
`char_similarity` in the existing `Summary`.

## ParseBench mapping

Upstream: [`llamaindex/ParseBench`](https://huggingface.co/datasets/llamaindex/ParseBench),
pinned to commit `2805a1d940f95a203e0ae4b88be9934f7765b3fc`, Apache-2.0, 2,078 single-page
documents and 169,011 assertions in five JSONL files.

### Rule conversion, whole upstream dataset

| Upstream file | Upstream type | Rules | → LiteOCR | Converted | Skipped |
|---|---|---:|---|---:|---:|
| `table.jsonl` | `expected_markdown` | 503 | `kind: transcript`, HTML table → markdown | 503 | 0 |
| `text_content.jsonl` | `missing_specific_word` | 105,369 | `present` (`rule.word`) | 105,369 | 0 |
| | `missing_specific_sentence` | 18,768 | `present` (`rule.sentence`) | 18,768 | 0 |
| | `order` | 13,087 | `order` (`rule.before` / `rule.after`) | 13,087 | 0 |
| | `missing_sentence_percent` | 503 | `bag_of_sentences` (`rule.bag_of_sentence` keys, `threshold: 1.0`) | 503 | 0 |
| | `unexpected_sentence_percent` | 503 | — | 0 | 503 |
| | `too_many_sentence_occurence_percent` | 503 | — | 0 | 503 |
| | `missing_word_percent` | 506 | — | 0 | 506 |
| | `unexpected_word_percent` | 506 | — | 0 | 506 |
| | `too_many_word_occurence_percent` | 506 | — | 0 | 506 |
| | `bag_of_digit_percent` | 486 | — | 0 | 486 |
| | `is_header` | 278 | — | 0 | 278 |
| | `is_footer` | 307 | — | 0 | 307 |
| `text_formatting.jsonl` | `is_title`, `is_bold`, `is_italic`, `is_sup`, `is_sub`, `is_underline`, `is_strikeout`, `is_mark`, `is_latex`, `is_code_block`, `title_hierarchy_percent` | 5,997 | — | 0 | 5,997 |
| `chart.jsonl` | `chart_data_point` | 4,864 | — | 0 | 4,864 |
| `layout.jsonl` | element bounding boxes | 16,325 | — | 0 | 16,325 |
| **Total** | | **169,011** | | **138,230** | **30,781** |

Why the skips:

- **`unexpected_*` / `too_many_*` / `*_percent` counters** are precision-direction assertions:
  "the output must not contain sentences the page does not have". Checking one needs the
  *complete* reference text, and ParseBench never publishes it for the text split — only bags.
  A `bag_of_*` compared against an incomplete reference would punish correct output.
- **`bag_of_digit_percent`** is a digit histogram, which no `present`/`absent` assertion expresses.
- **`is_header` / `is_footer`** assert a string's *role* on the page, not its presence.
- **`text_formatting.jsonl`** asserts style (bold, italic, superscript, LaTeX, heading level).
  Markdown carries some of this, but our normalisation strips it before scoring, so a rule about
  emphasis would be unscoreable. A future `formatting` kind could pick these up.
- **`chart.jsonl`** asserts a value read off a chart; **`layout.jsonl`** asserts bounding boxes
  with IoA matching. Neither has a text-similarity or substring analogue.

Net: **138,230 of 169,011 upstream assertions (81.8%)** are expressible, covering the `table`
and `text_content` dimensions — 1,009 of 2,078 pages (48.6%).

### Committed subset

The full conversion would vendor 517 MB of PDFs, so only a curated subset is committed
(`benchmark/datasets/parsebench/`, **5.7 MB** total, 4.0 MB of PDFs):

| | Docs | Rules | Notes |
|---|---:|---:|---|
| `kind: transcript` (table split) | 25 | — | 18 tagged `merged-cells`, 8 tagged `hard`, all tagged `table-only` |
| `kind: rules` (text split) | 15 | 3,703 | `present` 3,303 · `order` 385 · `bag_of_sentences` 15 |

Selection is deterministic for a given `--seed`: table pages are taken at most one per source
document with roughly a third `hard`; text pages round-robin across all eight ParseBench
document-type buckets (`simple`, `ocr`, `multicolumns`, `multilang`, `misc`, `dense`, `sparse`,
`handwritting`). Documents over 400 KB, over the 11 MB budget, or with more than one page are
skipped and counted.

`manifest.full.json` in the same directory indexes all 1,009 convertible documents with their
`upstream_path` and is *not* runnable as-is; `subset-committed.txt` lists the 40 committed ids.

### The `table-only` caveat

ParseBench's table truth is the page's **table**, not the page. A parser returns the **whole
page**. Measured on `apple_10_k_page1` with `reducto/standard`:

| Metric | Value | Reading |
|---|---:|---|
| `table_score` | **0.990** | the real signal — markdown table rows only |
| `char_similarity` | 0.311 | meaningless here: the prediction also has the rest of the page |
| `cer` / `wer` | 2.212 / 1.938 | same |
| `overall` | 31.13 | derived from `char_similarity`, so also meaningless |

So: **score `table-only` documents with `table_score`.** Every such document carries the
`table-only` tag precisely so a scorer can pick the right primary metric. `merged-cells` marks
the 18 documents where `colspan`/`rowspan` had to be flattened by repeating cells — cell content
survives, table structure (ParseBench's GriTS) does not.

## Framework reference (`base.py`)

```python
class Adapter(ABC):
    name: str            # CLI argument and dataset name
    license: str         # SPDX for the redistributable part
    upstream: Upstream   # repo_id + pinned revision + url
    default_out: str     # repo-relative output directory

    def download(self, cache_dir: Path) -> Path: ...
    def build(self, out_dir: Path, limit: int | None = None, seed: int = 1234) -> Manifest: ...
```

Shared helpers:

| Helper | What it does |
|---|---|
| `sha256_file`, `sha256_bytes` | streamed hashing for manifest provenance |
| `write_manifest`, `write_rules`, `write_json` | stable 2-space JSON with a trailing newline; returns the SHA-256 written |
| `slugify` | ASCII, lowercase, `_`-separated ids safe for filenames and `--filter` |
| `html_table_to_markdown` | HTML `<table>` → GitHub pipe table; flattens `colspan`/`rowspan` by repeating the cell into every position it covers, and reports `merged_cells` so the caller can add the `merged-cells` tag. Inline markup is dropped, `<br>` becomes a space, `\|` is escaped, short rows are padded |
| `pdf_page_count` | `pypdf` when importable, otherwise a byte scan: the `/Count` of the root `/Type /Pages` node (also inside inflated object streams), falling back to *distinct* `/Type /Page` object numbers so an incrementally-updated PDF is not double counted |
| `default_cache_dir` | `$LITEOCR_BENCH_CACHE` → `$XDG_CACHE_HOME/liteocr/benchmarks` → `~/.cache/liteocr/benchmarks` |

`pypdf` is optional on purpose. It pulls in `cryptography`, whose Rust extension can raise a
`pyo3` `PanicException` (a `BaseException`, not an `Exception`) on a mismatched wheel, so the
import is guarded broadly and cached; a broken optional dependency degrades to the byte scan
instead of aborting a build.

## Adding an adapter

1. Create `benchmark/adapters/<name>.py`.
2. Subclass `Adapter`, set `name`, `license`, `default_out`, `description` and `upstream` with a
   **pinned commit** — not a branch.
3. Implement `download(cache_dir)` (fetch only what conversion needs; keep the big media trees
   for a per-document fetch) and `build(out_dir, limit, seed)`.
4. Decorate the class with `@register` and import the module from
   `benchmark/adapters/__init__.py`.
5. Use `Doc` / `Manifest` / `Rule` and `write_manifest` / `write_rules` so output stays
   byte-stable, and record every skip with `self.bump(...)` so the summary is honest.
6. Add the dataset to `SOURCES` in `combined.py` if it should be in `combined-v1`, write a
   `README.md` in the dataset directory with the license, and list the dataset in
   `benchmark/README.md`.

Next candidate: **olmOCR-bench**, which is already rule-shaped — its `present` / `absent` /
`order` / `table` tests map onto `present` / `absent` / `order` / `table_cell` one-for-one, and
its baseline tests onto `bag_of_sentences`. That is the reason the schema has `table_cell` and
`absent` even though ParseBench never produces them.

Not adaptable into either kind, per the
[vendor benchmark survey](vendor-benchmarks.md): RealDoc-Bench (QA over a parse, needs an LLM
reader — conflicts with the deterministic-metrics principle; source PDFs are not
redistributable), RealDoc-Bench-Layout (bounding boxes, no text) and LongExtractBench
(schema-driven extraction, a different mode).

## Licensing

| Dataset | Redistributable? | What is committed |
|---|---|---|
| `synthetic-v1` | yes, CC0-1.0 | everything |
| `parsebench` | yes, Apache-2.0 (publisher's terms) | 40 documents + truth + rules |
| RealDoc-Bench / LongExtractBench | annotations only | would be manifest + `sha256` only |

Rules for any future adapter:

- Record the SPDX id in the manifest `license`, and per document when a dataset mixes them.
- Carry the publisher's citation in `attribution` on the manifest **and** on each document, so a
  combined manifest never loses it.
- Never vendor bytes whose license does not clearly allow it — index them with `upstream_path` +
  `sha256` and fetch at run time.
- Keep `upstream.revision` a commit hash. A result JSON is only reproducible if the input is.

## Known gaps in the Rust side

Both gaps are closed: `ManifestDoc` carries `kind` / `rules`, rule files are hashed into the dataset SHA, `kind: rules` documents are scored with `liteocr_core::bench::score_rules`, and `table-only` documents are headlined by `table_score` (`summarize_with`).
