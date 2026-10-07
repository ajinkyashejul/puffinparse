# Mathpix

> **Status: docs-only.** Everything below is taken from the official Mathpix documentation
> (read 2026-09-11) and from the implementation in `crates/puffinparse-core/src/providers/mathpix.rs`.
> No live call has been made — this repository has no Mathpix credentials. The fixtures under
> `crates/puffinparse-core/tests/fixtures/mathpix_*.json` are hand-built from the documented response
> shapes, not captured traffic. Mark this page **verified** only after the `#[ignore]`d live tests
> in `providers/mathpix.rs` pass with a real key pair.
> Help verify it: [issue #10](https://github.com/ajinkyashejul/puffinparse/issues/10).

## 1. Summary

| | |
|---|---|
| Provider name | `mathpix` |
| Base URL | `https://api.mathpix.com` (override: `base_url` on the request, or `MATHPIX_BASE_URL`) |
| API key | **two** values: `MATHPIX_APP_ID` + `MATHPIX_APP_KEY`, sent as the `app_id` and `app_key` **headers** (no `Authorization:` header). `api_key` on the request overrides `MATHPIX_APP_KEY`; `provider_options={"app_id": …}` overrides `MATHPIX_APP_ID` |
| Docs | <https://docs.mathpix.com> |
| API version | Path-versioned (`/v3/...`). No version header; the model is reported per response as `version` (e.g. `SuperNet-200`) |
| Checked against | **not live-verified** (see banner) — documentation read 2026-09-11 |
| Implementation | `crates/puffinparse-core/src/providers/mathpix.rs` |

Mathpix is an OCR engine rather than a layout parser: it is built for STEM content (printed *and*
handwritten math, tables, chemistry diagrams) and its native output is **Mathpix Markdown (MMD)**, a
markdown superset that can contain LaTeX (`$…$`, `\begin{tabular}`, `\section*{}`,
`<smiles>…</smiles>`). PuffinParse passes MMD through unchanged.

## 2. Models exposed by PuffinParse

| Model | Endpoint PuffinParse calls | Modes | List price (`pricing.json`) |
|---|---|---|---|
| `mathpix/pdf` *(default)* | `POST /v3/pdf` for documents; automatically `POST /v3/text` when the input is an image | `parse`, `ocr` | $0.005 / page |
| `mathpix/text` | always `POST /v3/text` (one image = one request) | `parse`, `ocr` | $0.002 / image |

Prices from <https://mathpix.com/pricing/api>: `v3/pdf` $5 per 1 000 pages (falling to $3.50 above
1M pages/month), `v3/text` $0.002 per image (falling to $0.0015 above 1M). A one-time **$19.99 setup
fee** activates the first API key, and the asynchronous Files API (`files/v1/*`, $1.50 per 1 000
pages) is **not** used by PuffinParse.

**Why two models.** `/v3/pdf` accepts documents and ebooks only (PDF, EPUB, DOCX, DOC, PPTX, AZW/
AZW3/KFX, MOBI, DJVU, WPD, ODT), while images (JPEG, PNG, BMP, JP2, WebP, PBM/PGM/PPM, PFM, Sun
raster, TIFF, OpenEXR, HDR) are only accepted by `/v3/text`. `mathpix/pdf` therefore routes by the
input's guessed MIME type — `image/*` goes to `/v3/text`, everything else to `/v3/pdf` — so one
model string works for a mixed workload. `mathpix/text` is the explicit escape hatch when you want
the cheaper per-image rate and snippet behaviour; sending it a PDF fails with `image_decode_error`.

## 3. Request flow PuffinParse uses

### 3.1 Documents (`mathpix/pdf` with a non-image input)

1. **Submit.** `POST {base}/v3/pdf` with `app_id` + `app_key`.
   * path / bytes input → `multipart/form-data` with a `file` part and **all options as one
     stringified JSON field, `options_json`** (this is how Mathpix takes options on multipart);
   * URL input → a JSON body `{"url": "…", …options}` (Mathpix downloads the file itself).

   Options PuffinParse always sends: `math_inline_delimiters: ["$","$"]` and
   `math_display_delimiters: ["$$","$$"]` (markdown-friendly instead of the default `\(…\)` /
   `\[…\]`). `pages` becomes `page_ranges` (see §4). `provider_options` are deep-merged last and
   win. Response: `{"pdf_id": "2026_01_15_abc123def456"}`.
2. **Poll.** `GET {base}/v3/pdf/{pdf_id}` every 2 s, backing off ×1.5 to 10 s, until
   `status == "completed"` (or `"error"`). Intermediate statuses are `received`, `loaded`, `split`.
   PuffinParse deliberately ignores `percent_done` / `num_pages_completed`: both reach 100 % while the
   output files are still being assembled, and a download at that moment 404s.
3. **Download line data.** `GET {base}/v3/pdf/{pdf_id}.lines.json` — per-page lines with polygons.
   A `404` (body = the status object) or `202` means "not ready yet" and PuffinParse keeps polling for
   it until the deadline; any other non-2xx is an error.
4. **Download markdown** (`parse` mode only). `GET {base}/v3/pdf/{pdf_id}.mmd` — the assembled
   Mathpix Markdown for the whole document. `ocr` mode skips this call.

`.mmd` and `.lines.json` are generated automatically for every document ("Availability: Always");
they are *not* valid `conversion_formats` keys and cost nothing extra.

### 3.2 Images (`mathpix/text`, or `mathpix/pdf` with an image input)

One synchronous call: `POST {base}/v3/text`, multipart `file` + `options_json`, or a JSON body with
`{"src": "<url>"}` for URL inputs. Options PuffinParse sends:

| Option | Value | Why |
|---|---|---|
| `formats` | `["text"]` | Mathpix Markdown output. Add `"data"`/`"html"` through `provider_options` if you want TSV/LaTeX/MathML alongside |
| `include_line_data` | `true` | line polygons + per-line text (the source of `Block`s / `Line`s) |
| `enable_document_layout` | `true` — **`parse` mode only** | full-page layout recognition (nested lists, pseudocode). Off by default on `/v3/text`, which otherwise assumes a snippet |
| `include_word_data` | `true` — **`ocr` mode only** | word polygons for `TextPage.words` |
| `math_inline_delimiters` / `math_display_delimiters` | `["$","$"]` / `["$$","$$"]` | markdown-friendly math |

`enable_document_layout` and `include_word_data` **cannot be combined** (documented, rejected by the
API), which is exactly why the two modes send different option sets.

## 4. Response mapping

### 4.1 Documents — `.lines.json` + `.mmd`

| Mathpix field | PuffinParse unified field | Notes |
|---|---|---|
| `pdf_id` | `ParseResponse.provider_job_id` / `TextResponse.provider_job_id` | |
| `pages[].page` | `Page.page_number` / `TextPage.page_number` | Already 1-based. |
| `pages[].page_width` / `page_height` | `Page.width` / `Page.height` | Pixel coordinate space the polygons live in. |
| `pages[].lines[]` | `Page.blocks[]` (parse) · `TextPage.lines[]` (ocr) | Selection rules below. |
| `lines[].type` (+ `subtype`) | `Block.type` | See the table below. |
| `lines[].text_display` | `Block.content` | The line's MMD, exactly as it appears in the assembled `.mmd`. Falls back to `text` when empty. With `output="text"`, `text` is used instead. |
| `lines[].text` | `Block.text` / `Line.text` | Searchable plain text; falls back to `markdown_to_text(text_display)`. |
| `lines[].cnt` | `Block.bbox` / `Line.bbox` | Polygon in page pixels, `[TL, TR, BR, BL]`; PuffinParse takes the enclosing axis-aligned box and divides by `page_width`/`page_height`. |
| `lines[].confidence` | `Block.confidence` / `Line.confidence` | 0–1, the product of per-token OCR confidence. `confidence_rate` (geometric mean) is not mapped. |
| `.mmd` body | `ParseResponse.markdown` | Mathpix's own rendering of the whole document wins over the pages joined together. Page-level `markdown` stays line-derived so text and boxes agree. |
| status `num_pages` | `Usage.pages` | Falls back to the number of pages in `lines.json`. |
| `region`, `is_printed`, `is_handwritten`, `links`, `children_ids`, `parent_id` | — | Not mapped; visible with `include_raw=True`. |

**Which lines become blocks.** A line is kept when `conversion_output` is true (missing ⇒ true) and
it has content; then any line whose `parent_id` points at another kept line is dropped, so a table's
`table_cell` children are not emitted next to the table they already belong to. `ocr` mode does not
apply that filter — every line with text is a `Line`, including ones excluded from the MMD, because
their geometry is still real.

**Words.** `/v3/pdf` has no word-level output, so `TextPage.words` is empty for documents. Only
`/v3/text` reports `word_data`.

### 4.2 Images — `/v3/text`

| Mathpix field | PuffinParse unified field | Notes |
|---|---|---|
| `request_id` | `provider_job_id` | |
| `text` | `Page.markdown` (page 1) | The whole image's MMD. |
| `image_width` / `image_height` | `Page.width` / `Page.height` | Pixel space for `cnt`. |
| `line_data[]` | `Page.blocks[]` / `TextPage.lines[]` | Blocks keep only `conversion_output`/`included` lines; OCR lines keep all of them. |
| `line_data[].text` | `Block.content` | Per-line MMD. |
| `line_data[].cnt` | `Block.bbox` / `Line.bbox` | Normalised by `image_width`/`image_height`. |
| `word_data[]` | `TextPage.words[]` | `text` + polygon + `confidence`. |
| `confidence` | `metadata.mathpix_confidence` | Whole-image confidence. |
| `latex_styled`, `data[]`, `html`, `detected_alphabets`, `auto_rotate_*` | — | Not mapped; visible with `include_raw=True`. |
| — | `Usage.pages` | Always `1`: one image is one billed request. |

### 4.3 Block types

Mathpix line types (the same vocabulary for `line_data` and PDF lines data):

| Mathpix `type` | `BlockType` |
|---|---|
| `title` | `title` |
| `section_header` | `section_header` |
| `text`, `abstract`, `authors`, `quote`, `code`, `pseudocode`, `form_field`, `multiple_choice_block`, `multiple_choice_option`, `table_of_contents_row`, `table_of_contents_item`, `column` | `text` |
| `math` | `formula` |
| `table`, `table_cell` | `table` |
| `diagram`, `chart` | `figure` |
| `diagram_info`, `chart_info`, `figure_label` | `caption` |
| `footnote` | `footnote` |
| `page_info` (headers, footers, page numbers, stamps, QR codes) | `header` — except `subtype: "margin_note"` → `footnote` |
| everything else (`equation_number`, `qed_symbol`, `rotated_container`, `table_of_contents_container`, …) | `other` |

### 4.4 `pages` selection

`pages="1-3,7,10-"` becomes `page_ranges: "1-3,7,10--1"`. Mathpix's `page_ranges` is 1-based like
PuffinParse's, and negative indices count from the end, so an open-ended range is closed with `-1` (the
last page). For `/v3/text` the option does not exist — an image is a single page — and the selection
is ignored.

## 5. Errors, status codes, rate limits, timeouts

**The v3 API answers most errors with HTTP 200** and an error body:

```json
{"error": "Image has no content", "error_info": {"id": "image_no_content", "message": "Image has no content"}}
```

Only `http_unauthorized` (401) and `http_max_requests` (429) use a real status code. PuffinParse
therefore inspects **every** 200 payload (submit, status poll, `.lines.json`, `/v3/text`) and maps
`error_info.id`:

| `error_info.id` | PuffinParse `ErrorKind` |
|---|---|
| `http_unauthorized`, `account_disabled`, `expired_license`, `unauthorized_token_request` | `authentication` |
| `http_max_requests` (monthly page/image quota **or** per-minute rate) | `rate_limit` (retried) |
| `sys_exception`, `connection_closed` | `provider` |
| `image_no_content`, `math_confidence`, `math_syntax`, `strokes_no_content` | `provider` (content the engine could not read — a router may fall back to another provider) |
| `opts_*`, `json_syntax`, `pdf_missing`, `pdf_encrypted`, `pdf_unknown_id`, `pdf_page_limit_exceeded`, `image_*`, `file_missing`, `sys_request_too_large` | `bad_request` |
| an `error` string with no `error_info.id` | `provider` |

HTTP-level failures still go through `Error::from_http`, which picks up the `error` / `message`
fields and classifies 401/403 → `authentication`, 429 → `rate_limit`, other 4xx → `bad_request`,
5xx → `provider`.

**Size and time limits** (from the endpoint reference): `/v3/pdf` accepts files up to **1 GB**;
`/v3/text` accepts a 5 MB JSON body, a 2 MB base64 image, a 10 MB image download from `src`, and
gives that download 15 s. Per-account page limits per document exist (`pdf_page_limit_exceeded`) and
`http_max_requests` carries `limit_name` / `limit_value` / `count` in `error_info`. The published
per-minute request ceiling is on the *Limits & Quotas* page, which renders client-side and could not
be read here — treat the exact number as unverified.

**Retention.** Text outputs (MMD, JSON lines) are kept for up to **90 days**, uploaded source files
and CDN image crops for **30 days**. `DELETE /v3/pdf/{pdf_id}` removes everything at once; PuffinParse
never deletes on your behalf.

**Timeouts.** `timeout_secs` (default 300) is the whole-call deadline: submit + status polling +
`.lines.json` + `.mmd`, and it caps each individual HTTP request.

## 6. Gotchas

* **Errors hide behind HTTP 200** (see §5). A client that only checks the status code will treat
  `{"error_info": {"id": "pdf_encrypted"}}` as a successful parse with no pages.
* **Two credentials, not one.** `app_id` *and* `app_key`, both as plain headers. `api_key` on the
  request only replaces the key; the id still comes from `MATHPIX_APP_ID` or
  `provider_options.app_id`. A missing id is an `authentication` error before any network call.
* **`/v3/pdf` rejects images and `/v3/text` rejects PDFs.** `mathpix/pdf` handles that by routing on
  the input's MIME type; `mathpix/text` does not, by design.
* **Poll `status`, not `percent_done`.** `percent_done` reaches 100 % when OCR finishes, which is
  before the outputs are assembled; downloading then returns `404` with the status object as the
  body. PuffinParse treats such a `404` (and a `202`, used while a conversion format is still running)
  as "not ready" and keeps polling.
* **MMD is not plain markdown.** Expect `$…$` / `$$…$$` math (PuffinParse asks for those delimiters
  instead of the default `\(…\)`), `\begin{tabular}` or `\begin{array}` for complex tables,
  `\section*{}` headings on some documents, `<smiles>…</smiles>` for chemistry, and `\pagebreak`
  markers if you set `include_page_breaks`. Benchmark scoring against plain-markdown ground truth
  will punish this; it is the provider's format, not a bug.
* **Table cells are children of the table line.** Both the `table` line and its `table_cell`
  children carry `conversion_output: true`, so PuffinParse drops any line whose `parent_id` is itself
  kept. Without that rule every table would appear twice.
* **`include_page_info` defaults differ per endpoint**: `true` on `/v3/text`, `false` on `/v3/pdf`.
  Running heads and page numbers are therefore in image output but not in document output unless you
  ask for them (`provider_options={"include_page_info": true}`).
* **`include_word_data` + `enable_document_layout` is rejected**, so parse and ocr modes send
  different options for images — an `ocr`-mode image call gets snippet-style layout.
* **Images with more than 12 rows of text may be billed at the `v3/pdf` per-page rate**, so
  `mathpix/text` on a full page is not reliably $0.002.
* **`conversion_output` supersedes `included`.** `/v3/text` still emits both; `/v3/pdf` lines only
  carry `conversion_output`. PuffinParse reads `conversion_output` first and defaults to keeping a line
  when neither is present.
* **No `language` support.** Mathpix takes `alphabets_allowed` (which alphabets to *exclude*), not a
  language hint, so `language` on the request is ignored. Use
  `provider_options={"alphabets_allowed": {"ru": false}}` if you need it.
* **Streaming exists but is unused.** `streaming: true` + `GET /v3/pdf/{id}/stream` (SSE) delivers
  pages as they finish; PuffinParse polls instead, because the unified response is whole-document.
* **The Files API is a different product** (`files/v1/*`, $1.50 per 1 000 pages, results written to
  your own S3/GCS/Azure bucket) with a *different* error model — real HTTP status codes and a closed
  error-code set. PuffinParse does not use it.

## 7. Useful `provider_options` passthrough

```python
# 1. Credentials in code instead of the environment (app_id is stripped from the request body).
puffinparse.parse("paper.pdf", model="mathpix/pdf",
              api_key=MATHPIX_APP_KEY, provider_options={"app_id": MATHPIX_APP_ID})

# 2. Keep running heads, page numbers and QR codes, and mark page boundaries in the MMD.
puffinparse.parse("book.pdf", model="mathpix/pdf",
              provider_options={"include_page_info": True, "include_page_breaks": True})

# 3. Idiomatic LaTeX for equation-heavy papers, with equation numbers preserved.
puffinparse.parse("paper.pdf", model="mathpix/pdf",
              provider_options={"idiomatic_eqn_arrays": True, "include_equation_tags": True,
                                "math_inline_delimiters": ["\\(", "\\)"]})

# 4. Plain markdown fences and flat lists instead of lstlisting / itemize environments.
puffinparse.parse("manual.pdf", model="mathpix/pdf",
              provider_options={"disable_lstlisting": True, "disable_itemize": True})

# 5. Chemistry + table data on a single image, with the raw payload attached.
puffinparse.parse("reaction.png", model="mathpix/text", include_raw=True,
              provider_options={"include_smiles": True, "formats": ["text", "data"],
                                "data_options": {"include_table_html": True, "include_tsv": True}})

# 6. Ask for a DOCX conversion alongside the parse (downloaded separately from
#    GET /v3/pdf/{id}.docx once its conversion_status is completed — PuffinParse does not fetch it).
puffinparse.parse("report.pdf", model="mathpix/pdf",
              provider_options={"conversion_formats": {"docx": True}})
```

## 8. Links

* Docs home: <https://docs.mathpix.com>
* Process Documents (`v3/pdf`, status, `.lines.json`, `.mmd`):
  <https://docs.mathpix.com/reference/post-v3-pdf>
* Process Images (`v3/text`, `line_data`, `word_data`):
  <https://docs.mathpix.com/reference/post-v3-text>
* Error handling (the HTTP-200 error model): <https://docs.mathpix.com/reference/error-handling>
* Supported formats: <https://docs.mathpix.com/reference/supported-formats>
* Limits & quotas: <https://docs.mathpix.com/reference/limits-quotas>
* Pricing: <https://mathpix.com/pricing/api>
* Mathpix Markdown spec: <https://mathpix.com/docs/mathpix-markdown/overview>
* Console (keys, usage): <https://console.mathpix.com>
