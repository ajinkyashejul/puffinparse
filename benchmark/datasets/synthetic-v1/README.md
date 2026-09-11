# synthetic-v1

A small, fully synthetic OCR benchmark dataset for [LiteOCR](../../../README.md).
Every document is rendered by `benchmark/generate_synthetic.py` from a built-in corpus, so the
markdown in `truth/` is *exact* ground truth rather than a human transcription.

## Layout

```
benchmark/datasets/synthetic-v1/
  manifest.json      # {name, version, description, license, documents:[...]}
  docs/<id>.png      # input image (8-bit grayscale PNG)
  docs/<id>.pdf      # multi-page inputs
  truth/<id>.md      # expected markdown
```

`manifest.documents[]` entries are `{id, file, truth, pages, category, tags}`,
matching docs/SPEC.md section 10.2.

## Categories

| Category | Documents | Description |
| --- | --- | --- |
| `plain` | 3 | Body paragraphs of clean English text on a single page. |
| `invoice` | 3 | Company heading, address block, key/value metadata lines, a line-items table and totals. |
| `table` | 3 | A title, one sentence of context and a 5-8 row, 3-4 column numeric table. |
| `two_column` | 3 | Two columns of paragraphs; ground truth is the left column followed by the right column. |
| `headings` | 3 | An H1, two H2 sections, paragraphs and a bulleted list. |
| `noisy_scan` | 3 | Plain text degraded with grey speckles, a slight blur and gaussian noise. |
| `low_res` | 3 | Plain text rendered small and downscaled to roughly 100 DPI. |
| `multipage` | 3 | A two-page PDF: headings plus a paragraph on page 1, a table on page 2. |
| `skewed` | 3 | A text page rotated by 2-6 degrees, as if fed crookedly through a scanner. |
| `dense` | 3 | Small 17px print: an H1, two H2 sections and eight paragraphs, roughly 800 words on one page. |
| `faded` | 3 | Low-contrast grey text on off-white, blurred and pushed through a quality-35 JPEG round trip. |
| `receipt` | 3 | A narrow 600x1600 thermal receipt in monospace: store header, item table, totals, sensor noise and print-head streaks. |
| `complex_table` | 3 | A six-column, 12-15 row table of mixed text and numbers between a caption and a notes paragraph. |

## Ground-truth conventions

- Paragraphs are separated by a blank line.
- Headings use `# ` (H1) and `## ` (H2).
- Bullet lists use `- item`.
- Tables are GitHub-flavoured markdown with a `| --- |` separator row.
- Key/value runs (invoice metadata, address blocks) are consecutive lines inside
  a single block, e.g. `Invoice #: INV-2041`.
- Text is ASCII only, and no line carries trailing whitespace.
- For `two_column`, reading order is the whole left column followed by the whole
  right column. For `multipage`, page 1 is followed by a blank line and page 2.

## Regenerating

```
python benchmark/generate_synthetic.py
```

The generator is seeded with `random.Random(1234)` for the eight v1.0 categories
and `random.Random(5678)` for the five categories added in v1.1, so extending the
set never shifts the documents that already existed. It depends only on Pillow and
the standard library, so the output is reproducible. Fonts come from the
DejaVu and FreeFont families shipped with most Linux distributions
(`/usr/share/fonts/truetype/dejavu`, `/usr/share/fonts/truetype/freefont`).

## License

CC0-1.0. The documents, the ground truth and the generator are dedicated to the
public domain; the text corpus was written for this dataset. Use it for any
purpose without attribution.
