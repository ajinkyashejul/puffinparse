---
name: New provider
about: Request or propose support for another OCR / document-parsing provider
title: "Add <provider> provider"
labels: enhancement, provider
assignees: ""
---

## Provider

- **Name**:
- **Homepage**:
- **API docs**:
- **Pricing page**:
- **Are you affiliated with this provider?** <!-- yes / no -->

## Why

<!-- What does it do better than Reducto / Extend / LlamaParse — accuracy on a
     document class, price, latency, languages, on-prem? -->

## API shape

- **Base URL**:
- **Auth**: <!-- header name and format, which env var it should read -->
- **Flow**: <!-- e.g. POST /upload -> file id -> POST /parse, or async + poll -->
- **Remote URL input supported?** <!-- yes / no -->
- **Async job + polling?** <!-- yes / no -->
- **Output**: <!-- markdown? per-page? per-block with types and bboxes? -->
- **Bounding boxes**: <!-- absolute or normalised, origin top-left or bottom-left -->
- **Usage reported**: <!-- pages, credits, dollars? field names -->

## Proposed model strings

<!-- One line per mode; mark the default. These become public API. -->

| Model string | Maps to | Default |
|---|---|---|
| `<provider>/<mode>` | | ✅ |

## Pricing

| Model | USD per page | Source |
|---|---|---|
| `<provider>/<mode>` | | <!-- public pricing page URL --> |

## Sample response

<!-- Paste a small, REDACTED response — this becomes the test fixture. Remove
     API keys, job ids, account ids and any customer document content. -->

<details>
<summary>response JSON</summary>

```json

```

</details>

## Implementation checklist

See [CONTRIBUTING.md](../../CONTRIBUTING.md#3-adding-a-provider).

- [ ] `crates/puffinparse-core/src/providers/<name>.rs` implements `OcrProvider`
      using the shared HTTP helpers (retries, backoff, deadlines)
- [ ] Response mapped to `OcrResponse` / `Page` / `Block` / `Usage`; block types
      mapped to `BlockType` (unknown → `other`); bboxes normalised to 0..1,
      top-left origin
- [ ] Provider errors mapped onto the `Error` variants (401/403 → auth,
      429 → rate limit, 5xx / failed job → provider)
- [ ] Registered in `providers/mod.rs` `build()`
- [ ] Models added to `model.rs` `PROVIDERS` (exactly one `default: true`)
- [ ] Pricing added to `crates/puffinparse-core/src/pricing.json` with `source`
      and `updated`
- [ ] Redacted fixture in `crates/puffinparse-core/tests/fixtures/` plus a
      normalisation unit test (no network)
- [ ] API-key env var added to `.env.example` and `README.md`
- [ ] `CHANGELOG.md` `## [Unreleased]` entry
- [ ] Benchmark run against `synthetic-v1` (results + leaderboard, if you have keys)

## Are you planning to send the PR?

<!-- yes / no — either is fine, it just tells us whether to pick it up. -->
