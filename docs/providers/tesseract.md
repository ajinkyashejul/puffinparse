# Tesseract (local)

> **Status: verified locally** (a self-hosted engine, so there is no hosted API to verify against).
> Tesseract 5.3.4 and poppler-utils 24.02 (Ubuntu 24.04
> packages) were installed in the development sandbox on 2026-09-24; the `#[ignore]`d live test
> in `providers/tesseract.rs` passes and the fixture
> `crates/puffinparse-core/tests/fixtures/tesseract_headings.tsv` is real `tesseract ... tsv` output for
> `benchmark/datasets/synthetic-v1/docs/headings_001.png`.

## 1. Summary

| | |
|---|---|
| Provider name | `tesseract` |
| Runs | locally, by shelling out to the `tesseract` binary (no C bindings, no FFI) |
| Binaries | `tesseract` (≥ 4; 5.x recommended), plus `pdftoppm` from poppler for PDFs |
| Configuration | `TESSERACT_CMD` (default `tesseract`), `PDFTOPPM_CMD` (default `pdftoppm`) |
| API key | none — `puffinparse providers` shows `local` in the Key column |
| Price | $0 per page (`pricing.json` source `self-hosted`); the cost is your CPU time |
| Docs | <https://tesseract-ocr.github.io/tessdoc/> |
| Implementation | `crates/puffinparse-core/src/providers/tesseract.rs` (+ shared helpers in `providers/local.rs`) |

Install:

```bash
sudo apt-get install -y tesseract-ocr poppler-utils   # Debian / Ubuntu
brew install tesseract poppler                          # macOS
# extra languages: apt-get install tesseract-ocr-deu tesseract-ocr-fra ...
```

Tesseract is the zero-key, zero-cost baseline: useful offline, in CI, and as the open reference
point in the benchmark. It has **no layout model** — no headings, tables, figures or reading-order
analysis beyond its own page segmentation — so expect good character accuracy on clean scans and
a near-zero table score.

## 2. Models exposed by PuffinParse

| Model | Modes | List price |
|---|---|---|
| `tesseract/default` *(default)* | `ocr` (native), `parse` (derived) | $0 |

## 3. Request flow PuffinParse uses

1. The input is loaded (a URL input is downloaded first) and sniffed by magic bytes, then by
   extension. Anything that is not an image or a PDF is an `input` error.
2. **Images** (PNG, JPEG, TIFF incl. multi-page, BMP, GIF, WebP, PNM, JP2) are passed to Tesseract
   directly; a path input is used in place, bytes/URL inputs go to a private scratch directory that
   is deleted afterwards.
3. **PDFs** are rasterised with `pdftoppm -r <dpi> -png [-f first -l last] input.pdf page`
   (default 300 dpi), one PNG per page, then each page is OCR'd in order.
4. Per image: `tesseract <image> stdout [-l <lang>] [--psm N] [--oem N] [--dpi N] [-c k=v ...] tsv`.
   The whole call honours `timeout_secs`; the child process is killed if the deadline passes.

Page selection (`pages="1-3,7"`) limits rasterisation to the covering span and drops pages outside
the selection; for multi-page TIFFs it filters Tesseract's `page_num`.

## 4. Response mapping

Tesseract's TSV renderer emits one row per page (level 1), block (2), paragraph (3), line (4) and
word (5), each with a pixel box `left top width height`, and a 0–100 confidence on word rows.

| Unified field | Source |
|---|---|
| `TextPage.width/height`, `Page.width/height` | level-1 row (pixels of the image Tesseract read; for PDFs, the raster at `dpi`) |
| `Word.text/bbox/confidence` | level-5 rows with non-empty text; `conf / 100`; `-1` → `None` |
| `Line.text` | the line's words joined by a space |
| `Line.bbox` / `Line.confidence` | level-4 box; mean of its word confidences |
| `TextPage.text` | lines joined by `\n` |
| `Block` (parse mode) | one `text` block per paragraph (level 3); `content` = its lines joined by `\n`; box from the paragraph row; confidence = mean word confidence |
| `Page.markdown` | paragraphs joined by a blank line (no markdown syntax is invented) |
| `Usage.pages` | pages OCR'd |
| metadata | `tesseract_lang`, `tesseract_psm` (when set), `tesseract_pdf_dpi` (PDFs); parse responses also carry `puffinparse_derived_from: "ocr"` |
| `raw` (`include_raw=True`) | `{"engine": "tesseract", "format": "tsv", "pages": [{"page_number", "tsv"}]}` |

Boxes are normalised by the page's pixel size, origin top-left, clamped to 0..1.

## 5. Errors and limits

| Situation | Error |
|---|---|
| `tesseract` / `pdftoppm` not found | `provider_error`: "tesseract binary 'tesseract' not found on PATH: install Tesseract (apt install tesseract-ocr / brew install tesseract) or set TESSERACT_CMD" (same shape for `pdftoppm`, naming poppler-utils and `PDFTOPPM_CMD`). `provider` kind so a router can fall back to another model |
| Non-zero exit (unknown language, unreadable image, bad `-c` variable) | `provider_error` with Tesseract's / pdftoppm's stderr verbatim |
| Deadline exceeded | `timeout_error`; the child process is killed |
| Not an image or PDF (e.g. `.docx`) | `input_error` |
| Bad `provider_options` (non-integer `psm`, non-object `config`) | `input_error` |

There is no concurrency limit beyond your CPU. Tesseract uses OpenMP threads by default, which
oversubscribe the CPU when several pages run in parallel (one small PNG took 70 s instead of 0.7 s
on 4 cores), so PuffinParse starts `tesseract` with `OMP_THREAD_LIMIT=1` unless `OMP_THREAD_LIMIT` is
already set in the environment. Set it yourself (e.g. `OMP_THREAD_LIMIT=4`) to give a single large
document more threads.

## 6. Gotchas

* **Language.** Default is Tesseract's `eng`. `language="de"` is mapped to `deu` (common ISO
  639-1 codes are mapped; unknown ones pass through); `provider_options.lang` takes Tesseract's own
  syntax, e.g. `"eng+deu"`. The matching `tesseract-ocr-<lang>` package must be installed.
* **Page segmentation.** The default `--psm 3` (automatic) handles multi-column pages well; `6`
  (single uniform block) can help receipts and forms; `11`/`12` for sparse text.
* **Resolution.** Low-resolution scans OCR poorly; for PDFs raise `dpi` (e.g. 400) rather than
  upscaling images yourself. Images without DPI metadata make Tesseract guess — pass `dpi` if you
  know it.
* **Skew** is not corrected; heavily rotated pages lose accuracy (the `skewed` category of
  `synthetic-v1` is its weakest).
* **No tables.** Table cells come out as lines of text in reading order; the table score is ~0.

## 7. `provider_options` examples

```python
import puffinparse

puffinparse.ocr("scan.png", model="tesseract/default")                                   # defaults
puffinparse.ocr("scan.png", model="tesseract", provider_options={"lang": "eng+fra", "psm": 6})
puffinparse.parse("book.pdf", model="tesseract", provider_options={"dpi": 400, "oem": 1})
puffinparse.ocr("form.png", model="tesseract",
            provider_options={"config": {"preserve_interword_spaces": 1}})         # -c k=v
puffinparse.ocr("scan.png", model="tesseract", provider_options={"cmd": "/opt/tesseract/bin/tesseract"})
```

| Option | Meaning |
|---|---|
| `lang` | Tesseract language string (`eng`, `eng+deu`, `chi_sim`) |
| `psm` | page segmentation mode (0–13) |
| `oem` | OCR engine mode (0–3; 1 = LSTM only) |
| `dpi` | PDF raster resolution (default 300); for images, passed to Tesseract as `--dpi` |
| `config` | object of Tesseract variables, each passed as `-c key=value` |
| `cmd` / `pdftoppm_cmd` | binary paths (override `TESSERACT_CMD` / `PDFTOPPM_CMD`) |

## 8. Links

* Command-line usage: <https://tesseract-ocr.github.io/tessdoc/Command-Line-Usage.html>
* Improving quality (psm, dpi, preprocessing): <https://tesseract-ocr.github.io/tessdoc/ImproveQuality.html>
* `pdftoppm(1)`: <https://manpages.debian.org/pdftoppm>
