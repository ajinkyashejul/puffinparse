# AWS Textract

## 1. Summary

| | |
|---|---|
| Provider name | `textract` |
| Base URL | `https://textract.{region}.amazonaws.com` (override: `base_url` on the request, or `TEXTRACT_BASE_URL`) |
| API key | **A key pair, not a single token.** `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY`, optional `AWS_SESSION_TOKEN`, region from `AWS_REGION` (then `AWS_DEFAULT_REGION`, then `us-east-1`). `api_key` on the request overrides the **access key id only**. |
| Auth header | `Authorization: AWS4-HMAC-SHA256 …` — SigV4, computed in `providers/textract.rs`; no AWS SDK crate is vendored |
| Protocol | AWS JSON 1.1 RPC: always `POST /`, operation chosen by `X-Amz-Target`, `Content-Type: application/x-amz-json-1.1` |
| API version | `textract-2018-06-27` (implicit in the target names; there is no version header) |
| Verified | 2026-09-11 — request/response shapes and quotas from the AWS API reference; pricing from the AWS Price List API (`AmazonTextract`, `us-east-1`, publication `2026-08-31`). **Response fixtures are built from the AWS documentation's own examples, not from a live capture** (see §6). |
| Implementation | `crates/liteocr-core/src/providers/textract.rs` |

Textract is not a "parse a document to markdown" product: it returns a flat array of `Block` objects
linked by ids. LiteOCR reassembles those into pages, blocks, markdown tables and extraction results.
There is no upload endpoint and no remote-URL input — synchronous calls carry the document inline as
base64, asynchronous ones read it from S3.

## 2. Models exposed by LiteOCR

| Model | Modes | Textract operation + `FeatureTypes` | List price (`pricing.json`) |
|---|---|---|---|
| `textract/detect-text` *(default for `ocr`)* | `ocr` | `DetectDocumentText` (no features) | $0.0015 / page |
| `textract/layout` | `parse`, `ocr` | `AnalyzeDocument`, `["LAYOUT", "TABLES"]` | $0.015 / page |
| `textract/queries` *(default for `extract`)* | `extract` | `AnalyzeDocument`, `["QUERIES"]` + `QueriesConfig` | $0.015 / page |
| `textract/forms` | `extract` | `AnalyzeDocument`, `["FORMS"]` | $0.050 / page |

Prices are the us-east-1 pay-as-you-go list prices for the **first 1M pages/month**, taken from the
AWS Price List API rather than the marketing page, and used only to fill
`cost_usd = per_page_usd × usage.pages`:

| Usage type (us-east-1) | $ / page (0–1M) | $ / page (1M+) |
|---|---|---|
| `USE1-SyncTextPagesProcessed` (DetectDocumentText) | 0.0015 | 0.0006 |
| `USE1-SyncTablesPagesProcessed` (AnalyzeDocument TABLES) | 0.015 | 0.010 |
| `USE1-SyncQueriesPagesProcessed` (AnalyzeDocument QUERIES) | 0.015 | 0.010 |
| `USE1-SyncFormsPagesProcessed` (AnalyzeDocument FORMS) | 0.050 | 0.040 |
| `USE1-SyncLayoutPagesProcessed` (AnalyzeDocument LAYOUT alone) | 0.004 | 0.003 |

**Why `textract/layout` is priced at the TABLES rate and not TABLES + LAYOUT.** AWS bills a combined
`AnalyzeDocument` call at a single combination rate, and there is no `Layout…Tables` usage type in the
price list: *"Layout is available for free when used with the Tables feature."* So `["LAYOUT","TABLES"]`
bills exactly like `["TABLES"]` — $0.015 / page. A LAYOUT-only call would be $0.004 / page, which you
can get with `provider_options={"FeatureTypes": ["LAYOUT"]}` (the passthrough replaces the array), at
the cost of losing markdown tables. Prices are region-dependent and LiteOCR's table is us-east-1 only,
so `cost_usd` is an estimate for any other region.

Async (`Start*`/`Get*`) pages cost the same as sync pages; the `USE1-Async…` usage types carry
identical rates.

## 3. Request flow LiteOCR uses

Every call is `POST {base}/` with these headers, all of them signed:

```
Content-Type: application/x-amz-json-1.1
X-Amz-Target: Textract.<Operation>
X-Amz-Date: 20260911T123456Z
X-Amz-Content-Sha256: <sha256 hex of the body>
X-Amz-Security-Token: <AWS_SESSION_TOKEN>        # only when set
Authorization: AWS4-HMAC-SHA256 Credential=<id>/<date>/<region>/textract/aws4_request,
               SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date;
                             [x-amz-security-token;]x-amz-target,
               Signature=<hex>
```

SigV4 is implemented in-file with `hmac` + `sha2` (~60 lines: canonical request → string to sign →
four chained HMACs for the signing key). It is re-signed on every retry, because the signature
expires with `X-Amz-Date`. Unit tests assert AWS's own published `iam/ListUsers` example vectors
(signing key `c4afb1cc…a4b9`, signature `5d672d79…b5d7`), so canonicalisation drift is caught
without a network call.

### 3.1 Synchronous (default)

1. **Load bytes.** Path and bytes inputs are read locally. A **URL input is downloaded by LiteOCR**
   and sent inline — Textract cannot fetch URLs itself.
2. **Multi-page guard.** If the bytes are a PDF, `pdf_page_count()` counts `/Type /Page` objects
   (falling back to the page tree's `/Count`). More than one page ⇒ a `bad_request` error before any
   billable call (§5).
3. **Call.** `Textract.DetectDocumentText` or `Textract.AnalyzeDocument` with

   ```json
   {
     "Document": { "Bytes": "<base64 of the file>" },
     "FeatureTypes": ["LAYOUT", "TABLES"],
     "QueriesConfig": { "Queries": [{ "Text": "What is the total?", "Alias": "total" }] }
   }
   ```

   `FeatureTypes` is omitted for `detect-text`; `QueriesConfig` only for `textract/queries`.

### 3.2 Asynchronous (multi-page, `provider_options.s3_object`)

Textract's async API reads **only from S3** — there is no way to hand it bytes. LiteOCR has no S3
client and deliberately does not grow one, so you upload the object yourself and name it:

```python
provider_options={"s3_object": {"bucket": "my-bucket", "name": "invoices/2026-q3.pdf"}}
# optional: "version": "<S3 object version id>"
```

Then LiteOCR:

1. `Textract.StartDocumentTextDetection` / `Textract.StartDocumentAnalysis` with
   `{"DocumentLocation": {"S3Object": {"Bucket": …, "Name": …}}, "FeatureTypes": […]}` → `{"JobId": …}`.
2. Polls `Textract.GetDocumentTextDetection` / `Textract.GetDocumentAnalysis` with
   `{"JobId": …, "MaxResults": 1000}` starting at 2 s, backing off ×1.5 to 10 s, until `JobStatus`
   leaves `IN_PROGRESS`. `SUCCEEDED` and `PARTIAL_SUCCESS` continue; anything else becomes a
   `provider` error carrying `StatusMessage` and the job id.
3. Follows `NextToken` until it is absent, concatenating every `Blocks` page (a 3 000-page document
   is many round trips — budget `timeout_secs` accordingly).

`NotificationChannel` / SNS is not used: LiteOCR polls. You can still set it (and `OutputConfig`,
`KMSKeyId`, `JobTag`, `ClientRequestToken`, `AdaptersConfig`) through `provider_options`, which is
deep-merged into whichever body is actually sent.

**Where `provider_options` are merged:** the keys `region` and `s3_object` are consumed by LiteOCR;
everything else is deep-merged verbatim into the Textract request body, so the remaining keys are
top-level Textract request members in `PascalCase` (`FeatureTypes`, `QueriesConfig`, `AdaptersConfig`,
`HumanLoopConfig`, `OutputConfig`, `KMSKeyId`, `JobTag`, `ClientRequestToken`, `NotificationChannel`).

### 3.3 Page selection

Textract has no page-range parameter. `pages="1-3,7"` is therefore applied **client-side**: blocks
whose `Page` falls outside the selection are dropped after the response arrives. It reduces the
output, never the bill. The one exception is `textract/queries` in async mode, where the ranges are
also forwarded as `Query.Pages` (`["1-3", "7"]`), which Textract does honour.

`language` is ignored — Textract auto-detects (English, French, German, Italian, Portuguese, Spanish)
and never reports which language it found.

## 4. Response mapping

Every operation returns the same envelope: `{"Blocks": [...], "DocumentMetadata": {"Pages": n},
"AnalyzeDocumentModelVersion": "1.0"}` (plus `JobStatus` / `NextToken` / `Warnings` for `Get*`).

| Textract field | LiteOCR unified field | Notes |
|---|---|---|
| `DocumentMetadata.Pages` | `Usage.pages` | Falls back to the highest `Block.Page`, minimum 1. |
| `JobId` (async only) | `provider_job_id` | `None` for synchronous calls — Textract returns no id there. |
| `Geometry.BoundingBox.{Left,Top,Width,Height}` | `Block.bbox` / `Line.bbox` / `Word.bbox` | **Already normalised 0–1**, origin top-left; `from_normalized_ltwh` only clamps and converts to `{x0,y0,x1,y1}`. `Geometry.Polygon` and `RotationAngle` are ignored. |
| `Confidence` (0–100) | `confidence` (0–1) | Divided by 100 and clamped. |
| `Page` | `page_number` | 1-based. Always `1` for JPEG/PNG, even multi-page scans. |
| — | `Page.width` / `height` | Always `None`: Textract never reports page dimensions. |
| `AnalyzeDocumentModelVersion` | `metadata.textract_model_version` | |
| resolved region | `metadata.textract_region` | |
| `FeatureTypes` sent | `metadata.textract_feature_types` | |
| `Warnings[]` | `metadata.textract_warnings` | Present only when non-empty (e.g. `INVALID_REQUEST_PARAMETERS` for an over-quota query page). |
| — | `Usage.credits`, `Usage.provider_cost_usd` | Always `None`; `cost_usd` comes from `pricing.json`. |

### 4.1 `ocr` mode — `LINE` / `WORD`

`LINE` blocks become `TextPage.lines`, `WORD` blocks become `TextPage.words`, both with box and
confidence; `TextPage.text` is the lines joined by `\n` in response order. This is the native path
for `textract/detect-text`, and `textract/layout` uses it too — `AnalyzeDocument` returns *all* lines
and words regardless of `FeatureTypes`, so no second call is needed (and no extra charge).

### 4.2 `parse` mode — `LAYOUT_*` + `TABLE`

Textract returns `LAYOUT_*` blocks **in implied reading order** (left to right, top to bottom;
column by column on multi-column pages). LiteOCR walks them in array order, so `Page.markdown` is in
reading order. A layout block's text is its descendant `LINE` blocks via `Relationships[CHILD]`.

| `BlockType` | `Block.type` | `Block.content` |
|---|---|---|
| `LAYOUT_TITLE` | `title` | `# <text>` |
| `LAYOUT_SECTION_HEADER` | `section_header` | `## <text>` |
| `LAYOUT_HEADER` | `header` | plain text |
| `LAYOUT_FOOTER` | `footer` | plain text |
| `LAYOUT_TEXT`, `LAYOUT_KEY_VALUE` | `text` | plain text, one line per child `LINE` |
| `LAYOUT_LIST` | `list` | `- ` per child `LAYOUT_TEXT` |
| `LAYOUT_TABLE` | `table` | markdown table (below) |
| `LAYOUT_FIGURE` | `figure` | the caption lines, if any |
| `LAYOUT_PAGE_NUMBER` | `other` | plain text |

`LAYOUT_LIST` points at `LAYOUT_TEXT` children, and those children *also* appear at the top level of
`Blocks`; LiteOCR suppresses any layout block that is another layout block's child so list items are
not emitted twice.

**Tables.** A `LAYOUT_TABLE` is matched to its `TABLE` block by a direct `CHILD` reference, or — when
it only points at lines — by the highest-overlap unclaimed `TABLE` on the same page (>10 % of the
table's area). The `TABLE`'s `CHILD` `CELL` blocks are placed on a `RowIndex` × `ColumnIndex` grid
(`MERGED_CELL` blocks are skipped: they repeat content already present in the individual cells) and
rendered as markdown. Cell text is the `CHILD` `WORD` texts joined by spaces; a `SELECTION_ELEMENT`
child becomes `[x]` / `[ ]`; `|` is escaped. The header row is the lowest `RowIndex` among cells with
`EntityTypes: ["COLUMN_HEADER"]`, else row 1; rows above the header row and `TABLE_TITLE` /
`TABLE_FOOTER` blocks are emitted as plain lines around the table.

**Fallback.** If a response carries no `LAYOUT_*` blocks at all (LAYOUT disabled through
`provider_options`, or nothing detected), LiteOCR emits every `TABLE` as a markdown table plus one
`text` block per `LINE`, skipping lines whose words are all inside table cells so table content is
not duplicated. `output="text"` renders every block through `markdown_to_text`.

Trimmed fixture (`crates/liteocr-core/tests/fixtures/textract_layout.json`, most blocks elided):

```json
{
  "DocumentMetadata": { "Pages": 1 },
  "AnalyzeDocumentModelVersion": "1.0",
  "Blocks": [
    { "BlockType": "LAYOUT_TITLE", "Confidence": 98.12, "Id": "lay-title", "Page": 1,
      "Geometry": { "BoundingBox": { "Left": 0.12, "Top": 0.06, "Width": 0.26, "Height": 0.02 } },
      "Relationships": [ { "Type": "CHILD", "Ids": ["ll1"] } ] },
    { "BlockType": "LAYOUT_TABLE", "Confidence": 97.4, "Id": "lay-table", "Page": 1,
      "Relationships": [ { "Type": "CHILD", "Ids": ["tbl-1"] } ] },
    { "BlockType": "TABLE", "Confidence": 99.21, "Id": "tbl-1", "Page": 1,
      "EntityTypes": ["STRUCTURED_TABLE"],
      "Relationships": [ { "Type": "CHILD", "Ids": ["c11","c12","c21","c22","c31","c32"] } ] },
    { "BlockType": "CELL", "RowIndex": 1, "ColumnIndex": 1, "RowSpan": 1, "ColumnSpan": 1,
      "EntityTypes": ["COLUMN_HEADER"], "Id": "c11", "Page": 1,
      "Relationships": [ { "Type": "CHILD", "Ids": ["tw1"] } ] },
    { "BlockType": "LINE", "Confidence": 98.9, "Text": "Quarterly Report", "Id": "ll1", "Page": 1,
      "Relationships": [ { "Type": "CHILD", "Ids": ["lw1","lw2"] } ] },
    { "BlockType": "WORD", "Confidence": 99.0, "Text": "Region", "TextType": "PRINTED", "Id": "tw1", "Page": 1 }
  ]
}
```

### 4.3 `extract` mode — `textract/queries`

Each **flat** property of the request schema becomes one Textract query:

```
{"Text": <description> ?? <title> ?? <key with _ and - as spaces>, "Alias": <sanitised key>}
```

The alias keeps `[A-Za-z0-9_.:-]` (everything else becomes `_`, duplicates get a numeric suffix) and
maps back to the original property name, so `"invoice number"` in the schema is queried as
`invoice_number` and returned under `"invoice number"`.

Responses are `QUERY` blocks carrying `Query.Alias` and a `Relationships[{"Type":"ANSWER"}]` list of
`QUERY_RESULT` ids. LiteOCR takes the highest-confidence non-empty `QUERY_RESULT`, coerces its `Text`
to the schema's type (`number`/`integer` strip currency and separators; `boolean` understands
yes/no/true/false/selected/`[x]`), and writes it to `data[<key>]`. Confidence lands in
`fields["/<key>"].confidence`; with `citations=True` the answer's page, box and text land in
`fields["/<key>"].citations`. A query with no answer yields `data[<key>] = null` and **no** entry in
`fields` — the key is always present, so the shape of `data` matches the schema.

> **Only flat string-like fields are supported.** Properties typed `object` or `array` (or carrying
> `properties` / `items`) cannot be expressed as a Textract query; they are skipped, reported in
> `metadata.textract_unsupported_fields`, and set to `null`. Textract Queries answers one question
> with one span of text — there is no nesting and no repeated-row extraction. Flatten the schema
> (`line_item_1_total`, …) or use a provider with native structured extraction.

`instructions` on the request is ignored (Textract has no free-text guidance parameter); the fact is
recorded in `metadata.textract_instructions_ignored`.

Textract allows **15 queries per page synchronously and 30 asynchronously**. A schema with more flat
properties than the applicable limit is rejected with an `input` error before any call is made.

### 4.4 `extract` mode — `textract/forms` (best effort)

`FORMS` returns `KEY_VALUE_SET` blocks: a block with `EntityTypes: ["KEY"]` holds the label (its
`CHILD` `WORD`s) and points at its value block through `Relationships[{"Type":"VALUE"}]`; the value
block's `CHILD`ren are `WORD`s or a `SELECTION_ELEMENT`.

LiteOCR matches each schema property to a detected key by **case- and punctuation-insensitive**
comparison (both sides reduced to lowercase alphanumerics), trying the property name and its `title`,
first for an exact match and then for "one name contains the other" (≥3 characters). Each detected
key is consumed at most once. Values are coerced by the schema's type exactly as in §4.3, and a
selected checkbox becomes `true`.

> **This is a heuristic, not schema-driven extraction.** Textract decides what the keys are; LiteOCR
> only tries to line them up with your field names. Properties with no match are set to `null` and
> listed in `metadata.textract_unmatched_fields`; `metadata.textract_form_keys_found` reports how many
> key-value pairs Textract actually detected, which is the first thing to look at when fields come
> back empty. Nested (`object`/`array`) properties are never matched. At $0.050/page `textract/forms`
> is also the most expensive model here — prefer `textract/queries` when you know what you want.

## 5. Errors, status codes, rate limits, timeouts

Textract reports failures as `{"__type": "<Exception>", "message": "…"}`, almost always with
**HTTP 400 — including throttling and server-side failures**, so the shared `Error::from_http`
mapping (`4xx → bad_request`) is wrong for Textract. `textract::map_error` keys off `__type` instead
(a fully-qualified `com.amazonaws.textract#ThrottlingException` is accepted too) and prefixes the
exception name onto the message:

| `__type` | HTTP | LiteOCR `ErrorKind` | Retried? |
|---|---|---|---|
| `AccessDeniedException` | 400 | `authentication` | no |
| `UnrecognizedClientException`, `InvalidClientTokenId`, `ExpiredTokenException` | 400/403 | `authentication` | no |
| `IncompleteSignature`, `InvalidSignatureException`, `MissingAuthenticationTokenException` | 400/403 | `authentication` | no |
| `ThrottlingException` | 400/500 | `rate_limit` | **yes** |
| `ProvisionedThroughputExceededException` | 400 | `rate_limit` | **yes** |
| `LimitExceededException` (too many concurrent async jobs) | 400 | `rate_limit` | **yes** |
| `InternalServerError`, `ServiceUnavailable`, `InternalFailure` | 500 | `provider` | **yes** |
| `BadDocumentException`, `UnsupportedDocumentException`, `DocumentTooLargeException` | 400 | `bad_request` | no |
| `InvalidParameterException`, `InvalidS3ObjectException`, `InvalidKMSKeyException`, `InvalidJobIdException` | 400 | `bad_request` | no |
| `IdempotentParameterMismatchException`, `HumanLoopQuotaExceededException` | 400 | `bad_request` | no |
| anything unrecognised | — | falls back to `Error::from_http` | per status |

An async job that ends `FAILED` becomes a `provider` error carrying `StatusMessage` and the job id.
`PARTIAL_SUCCESS` is accepted, logged, and its `Warnings` surfaced in metadata.

**Multi-page PDFs.** Synchronous Textract accepts PDF and TIFF at **one page only**. LiteOCR raises a
`bad_request` before the call:

> textract: synchronous operations accept single-page PDF/TIFF only (this document has 2 pages).
> Multi-page documents require Textract's asynchronous API, which reads the file from S3. Upload the
> file yourself and pass `provider_options={"s3_object": {"bucket": "my-bucket", "name": "path/doc.pdf"}}`,
> or split the PDF into single pages first.

The page count is best effort — it reads uncompressed page objects and the `/Count` in the page tree,
which covers most producers but not PDFs that hide their structure in object streams. When the count
cannot be determined, the call goes out and Textract's own 4xx is rewritten into the same message, so
the advice is identical either way.

**Retries.** `max_retries` (default 2), exponential backoff with full jitter, only on the
`rate_limit`, `network` and 5xx `provider` kinds above. **Textract returns no `Retry-After` and no
rate-limit headers**, so backoff is blind. Default us-east-1 quotas worth knowing (all adjustable in
Service Quotas, and lower in most other regions): synchronous `AnalyzeDocument` 10 TPS,
`DetectDocumentText` 25 TPS; `StartDocumentAnalysis` 10 TPS, `StartDocumentTextDetection` 15 TPS;
`GetDocumentAnalysis` 10 TPS, `GetDocumentTextDetection` 25 TPS; at most 600 asynchronous jobs
existing simultaneously per account (exceeding that is `LimitExceededException`, which LiteOCR
retries).

**Timeouts.** `timeout_secs` (default 300) is the whole-call deadline — download, signing, the call,
job polling and `NextToken` pagination — and also caps each individual HTTP request, shrinking as the
budget is spent.

## 6. Gotchas (verified)

* **The "API key" is a key pair.** `api_key` on a LiteOCR request can only stand in for
  `AWS_ACCESS_KEY_ID`; `AWS_SECRET_ACCESS_KEY` must be in the environment, otherwise the request is
  refused with an `authentication` error that says so. Set `AWS_SESSION_TOKEN` as well for STS /
  assumed-role credentials — it is signed as `x-amz-security-token` and included in `SignedHeaders`.
  LiteOCR reads **only** those environment variables: it does not parse `~/.aws/credentials`, does not
  honour `AWS_PROFILE`, and does not call IMDS or the ECS credential endpoint.
* **The region is part of the signature, not just the URL.** Signing with the wrong region gives a
  400 that talks about the *credential scope*, not about the host. If you override `base_url` to a
  VPC endpoint or a mock, set `provider_options={"region": …}` to match.
* **No fixtures were captured live.** The credentials available in this repository's build
  environment are proxy placeholders; a real `DetectDocumentText` call against
  `textract.us-east-1.amazonaws.com` returned
  `{"__type":"UnrecognizedClientException","message":"The security token included in the request is
  invalid."}`. That round trip does confirm the transport and the signature format (a malformed
  canonical request yields `IncompleteSignature`/`InvalidSignatureException`, not
  `UnrecognizedClientException`) and the error mapping (`ErrorKind::Authentication`), but every
  `textract_*.json` fixture is assembled from the shapes in the AWS API reference, with confidences,
  ids and boxes filled in to be realistic. **Re-capture them from a real account before trusting the
  numbers**, and run the `#[ignore]`d live tests at the bottom of `textract.rs`.
* **Confidence is 0–100, not 0–1.** Every `Confidence` is a percentage; LiteOCR divides by 100.
  The `QUERY_RESULT` sample in the AWS docs shows `"Confidence": 1.0`, which is 1 %, not certainty.
* **Boxes are already normalised.** Unlike most providers, `BoundingBox` is 0–1 relative to the page,
  so no page dimensions are needed — which is just as well, because Textract never reports them and
  `Page.width`/`height` are always `None`.
* **`Page` is always 1 for JPEG/PNG**, even for a scanned image that visually contains several pages.
  Only PDF and TIFF produce `Page > 1`, and only through the async API.
* **Synchronous PDFs are single-page, full stop** (10 MB in memory). Async accepts 500 MB / 3 000
  pages but **only from S3** — there is no bytes variant of `StartDocument*`. This is the single
  biggest limitation of this provider and the reason `provider_options.s3_object` exists.
* **Password-protected PDFs and XFA PDFs are rejected**; images must be ≤10 000 px per side.
* **`AnalyzeDocument` always returns every `LINE` and `WORD`**, whatever `FeatureTypes` says. That is
  why `textract/layout` can serve `ocr` mode from the same response, and why a `FORMS`-only call still
  gives you the full text in `raw`.
* **`LAYOUT_LIST` children are `LAYOUT_TEXT`, not `LINE`** — one level of indirection that also means
  those `LAYOUT_TEXT` blocks appear twice in `Blocks` (once nested, once at the top level). Naive
  iteration duplicates every list item.
* **`MERGED_CELL` duplicates content.** A merged cell's `CHILD` ids are the individual `CELL`s, which
  are *also* children of the `TABLE`. Rendering both repeats the text; LiteOCR renders only the plain
  cells, so a row/column span shows its text in the first cell and blanks beside it.
* **Queries are English-only** and capped at 15 per page synchronously / 30 asynchronously. Answers
  are capped at 128 characters. A query aimed at a page that does not exist comes back as an
  `INVALID_REQUEST_PARAMETERS` entry in `Warnings`, not as an error.
* **Throttling arrives as HTTP 400.** Treating Textract's 400s as non-retryable (the usual rule)
  means giving up on `ThrottlingException` and `ProvisionedThroughputExceededException`; the mapping
  in §5 exists entirely for this.
* **`Warnings` is silent data loss.** A `PARTIAL_SUCCESS` job returns blocks for the pages that
  worked and lists the failures in `Warnings` — check `metadata.textract_warnings` before trusting
  the page count.
* **No `provider_job_id` for sync calls.** Textract returns the request id only in the
  `x-amzn-RequestId` response header, which LiteOCR does not surface today.
* **Pagination is per 1 000 blocks, not per page.** A dense 50-page document can need dozens of
  `Get*` round trips; they all come out of `timeout_secs`.

## 7. Useful `provider_options` passthrough

```python
# 1. Multi-page PDF: upload to S3 yourself, then let LiteOCR drive Start*/Get* + NextToken.
liteocr.parse("ignored-when-s3.pdf", model="textract/layout",
              provider_options={"s3_object": {"bucket": "my-bucket", "name": "reports/q3.pdf"}},
              timeout=1200)

# 2. Cheapest layout: drop TABLES to bill at the LAYOUT rate ($4 vs $15 per 1k pages).
#    Tables then come back as plain lines instead of markdown grids.
liteocr.parse("memo.png", model="textract/layout",
              provider_options={"FeatureTypes": ["LAYOUT"]})

# 3. A different region (also changes the signing scope, not just the host).
liteocr.ocr("scan.png", model="textract/detect-text", provider_options={"region": "eu-west-1"})

# 4. Signatures alongside layout, and the raw Block array for anything LiteOCR does not map.
liteocr.parse("contract.png", model="textract/layout", include_raw=True,
              provider_options={"FeatureTypes": ["LAYOUT", "TABLES", "SIGNATURES"]})

# 5. A trained Custom Queries adapter (adapters are Queries-only).
liteocr.extract("claim.png", model="textract/queries", schema=schema,
                provider_options={"AdaptersConfig": {"Adapters": [
                    {"AdapterId": "abc123", "Version": "1"}]}})

# 6. Async with your own output bucket, KMS key and an idempotency token.
liteocr.parse("big.pdf", model="textract/layout", timeout=1800,
              provider_options={"s3_object": {"bucket": "in", "name": "big.pdf"},
                                "OutputConfig": {"S3Bucket": "out", "S3Prefix": "textract/"},
                                "KMSKeyId": "alias/textract",
                                "ClientRequestToken": "big-pdf-2026-09-11"})
```

## 8. Links

* What is Amazon Textract: <https://docs.aws.amazon.com/textract/latest/dg/what-is.html>
* `AnalyzeDocument`: <https://docs.aws.amazon.com/textract/latest/dg/API_AnalyzeDocument.html> ·
  `DetectDocumentText`: <https://docs.aws.amazon.com/textract/latest/dg/API_DetectDocumentText.html>
* `StartDocumentAnalysis`: <https://docs.aws.amazon.com/textract/latest/dg/API_StartDocumentAnalysis.html> ·
  `GetDocumentAnalysis`: <https://docs.aws.amazon.com/textract/latest/dg/API_GetDocumentAnalysis.html>
* `Block` reference: <https://docs.aws.amazon.com/textract/latest/dg/API_Block.html>
* Layout response objects: <https://docs.aws.amazon.com/textract/latest/dg/layoutresponse.html>
* Tables: <https://docs.aws.amazon.com/textract/latest/dg/how-it-works-tables.html> ·
  Form data: <https://docs.aws.amazon.com/textract/latest/dg/how-it-works-kvp.html> ·
  Queries: <https://docs.aws.amazon.com/textract/latest/dg/queryresponse.html>
* Quotas: <https://docs.aws.amazon.com/textract/latest/dg/limits-document.html> ·
  <https://docs.aws.amazon.com/general/latest/gr/textract.html>
* Pricing: <https://aws.amazon.com/textract/pricing/> · machine-readable price list:
  <https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AmazonTextract/current/index.json>
* Signature Version 4: <https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv4-signing-examples.html>
