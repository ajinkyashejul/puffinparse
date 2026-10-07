# Security Policy

## Supported versions

PuffinParse is pre-1.0. Security fixes land on `main` and ship in the next release;
only the latest released version is supported.

| Version | Supported |
|---|---|
| 0.1.x | ✅ |
| < 0.1 | ❌ |

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report it privately through GitHub security advisories:

1. Go to https://github.com/ajinkyashejul/puffinparse/security/advisories/new
   (repository → **Security** → **Advisories** → **Report a vulnerability**).
2. Describe the issue, the affected version or commit, and — if you can — a
   minimal reproduction and the impact you believe it has.

You will get an acknowledgement within 5 business days and an assessment with a
fix timeline within 10. We will keep you updated while we work on a fix, credit
you in the advisory and `CHANGELOG.md` unless you prefer otherwise, and publish
the advisory once a fixed release is out. Please give us a reasonable window to
ship a fix before disclosing publicly.

If GitHub advisories are unavailable to you, open a public issue that says only
that you have a security report and asks a maintainer to make contact — no
details — and we will take it from there.

## What is in scope

- The Rust crates (`puffinparse-core`, `puffinparse-cli`, `puffinparse-python`) and the
  Python package `puffinparse`.
- The release and CI workflows in `.github/workflows/`, and the published
  artifacts (PyPI wheels/sdist, GitHub Release binaries).

Out of scope: vulnerabilities in the third-party OCR providers themselves
(report those to Reducto, Extend or LlamaIndex directly), and issues that
require an already-compromised machine or a malicious local Rust/Python
dependency you introduced.

## How PuffinParse handles credentials

- **Provider API keys are only read from the environment** (`REDUCTO_API_KEY`,
  `EXTEND_API_KEY`, `LLAMA_API_KEY`) or passed explicitly as the `api_key`
  argument. PuffinParse never reads them from anywhere else, never writes them to
  disk, and never sends them anywhere but the provider's own base URL.
- **Keys are never logged.** `PUFFINPARSE_LOG=debug` traces requests, retries and
  polling, but `Authorization` headers and key values are redacted; errors carry
  provider, status code, message and request/job id only. If you ever see a key
  in log output, in an error message, or in a serialized `OcrResponse`, that is
  a vulnerability — please report it.
- Document bytes are sent only to the selected provider. PuffinParse has no
  telemetry and makes no network calls other than to the provider you choose.
- Recorded test fixtures under `crates/puffinparse-core/tests/fixtures/` must be
  redacted; never commit a fixture containing a real key, token or job id tied
  to a live account.
- Releases are published to PyPI with trusted publishing (OIDC), so no
  long-lived PyPI token exists in this repository's secrets.
