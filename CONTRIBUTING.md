# Contributing to PuffinParse

Thanks for helping build PuffinParse — one API for every OCR / document-parsing
provider. This document covers local setup, the checks CI runs, and the
contributions we get asked about most: **verifying or adding a provider** and
**adding a benchmark dataset**. If you use a coding agent, point it at
[AGENTS.md](AGENTS.md), which summarises the same rules.

## Good first issues

Issues labelled
[`good first issue`](https://github.com/ajinkyashejul/puffinparse/labels/good%20first%20issue)
are scoped to one area and list a "done when" condition. Comment on the issue to
claim it so two people don't do the same work. Another useful first contribution
if you have a key for one of the **docs-only** providers (Mistral, Azure,
Textract, Gemini, OpenAI, Anthropic, Mathpix, Datalab, Unstructured, Upstage,
Landing AI, Google Document AI, PaddleOCR) is running its live tests and
reporting what differs; see
[issue #10](https://github.com/ajinkyashejul/puffinparse/issues/10).
Questions and ideas that are not yet issues go to
[Discussions](https://github.com/ajinkyashejul/puffinparse/discussions).

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).
PuffinParse is MIT licensed; contributions are accepted under the same license.

---

## 1. Setup

Prerequisites:

- **Rust stable** (≥ 1.80 — the workspace `rust-version`), via [rustup](https://rustup.rs).
- **Python 3.9+** (3.9 is the minimum we build abi3 wheels for).
- A C toolchain (whatever `cc` your platform ships) for the native deps.
- **Node.js 18+** only if you work on the TypeScript SDK in `js/`.

```bash
git clone https://github.com/ajinkyashejul/puffinparse
cd puffinparse

# Rust side
cargo build --workspace

# Python side — use a virtualenv; maturin installs into the active one.
python -m venv .venv && source .venv/bin/activate
pip install maturin ruff mypy pytest pytest-asyncio
maturin develop            # builds crates/puffinparse-python, installs `puffinparse`
```

`maturin develop` compiles the PyO3 extension (`puffinparse._core`) and links it
against the pure-Python package in `python/puffinparse`, so edits to the Python
sources take effect immediately; re-run it after changing any Rust code.
Use `maturin develop --release` when you care about speed (benchmarks, large
documents) — debug builds of the core are slow.

Provider keys go in a `.env` (copy `.env.example`) or in your shell:

```bash
export REDUCTO_API_KEY=...
export EXTEND_API_KEY=...
export LLAMA_API_KEY=...       # LlamaCloud / LlamaParse, starts with llx-
```

**You do not need any keys to contribute.** Every test that talks to a live
provider skips itself when the relevant key is absent, and CI sets no secrets.
Live tests are opt-in: `PUFFINPARSE_LIVE_TESTS=1 pytest python/tests -q` for
Python and `cargo test -p puffinparse-core -- --ignored` for Rust. Never commit
a `.env` file or paste a key into an issue, log or fixture.

### Debugging

Set `PUFFINPARSE_LOG` to turn on the core's `tracing` output — request URLs, retry
and backoff decisions, poll loops, router fallbacks:

```bash
PUFFINPARSE_LOG=debug puffinparse parse invoice.pdf --model reducto/standard
PUFFINPARSE_LOG=puffinparse_core::providers=trace pytest python/tests -q
```

On the Python side the same events also surface through
`logging.getLogger("puffinparse")`.

---

## 2. Tests, lint, formatting

Run everything the CI runs with `make lint test` (and `make test-node` for the
TypeScript SDK), or piecemeal:

```bash
# Tests
cargo test --workspace
pytest python/tests -q
cd js && npm ci && npm run build:debug && npm test     # Node SDK

# Lint / format
cargo fmt --all              # or `cargo fmt --all --check` to only verify
cargo clippy --workspace --all-targets -- -D warnings
ruff check python/ benchmark/ examples/
ruff format --check python/ benchmark/ examples/
mypy python/puffinparse
```

Rules of the road:

- `cargo fmt` output is authoritative; do not hand-format around it.
- Clippy warnings are errors in CI. Prefer fixing over `#[allow]`; if an
  `#[allow]` is genuinely right, put a one-line comment saying why.
- **No network in unit tests.** Provider parsing is tested against recorded
  JSON in `crates/puffinparse-core/tests/fixtures/`. Live tests are `#[ignore]`d
  and/or key-gated.
- `puffinparse-core` is `#![forbid(unsafe_code)]`. Keep it that way.
- The Python package is fully typed and `mypy --strict`-clean; new public API
  needs annotations and a docstring.

---

## 3. Adding a provider

This is the highest-value contribution. A provider is a single file
implementing one trait. Check for an existing
[`new provider` issue](https://github.com/ajinkyashejul/puffinparse/issues) first,
or open one from the template so we can agree on model naming before you write
code.

1. **Implement the trait.** Create
   `crates/puffinparse-core/src/providers/<name>.rs` and implement
   `Provider` (see `crates/puffinparse-core/src/provider.rs`): `parse`, plus
   `ocr` (derived from `parse` by default) and `extract` for the modes the
   provider serves. Use the shared
   HTTP helpers in `src/http.rs` so you inherit retries, backoff, deadlines and
   error classification — do not build your own `reqwest::Client`, and do not
   vendor a provider SDK. Map the provider's response onto the unified
   `ParseResponse` / `TextResponse` / `ExtractResponse`, `Page`, `Block` and
   `Usage` types in `src/types.rs`:
   - normalise `bbox` to 0..1 with a top-left origin;
   - map the provider's block vocabulary onto `BlockType`, unknown → `other`;
   - fill `Usage.pages` with the *billed* page count;
   - map provider errors onto the `Error` variants in `src/error.rs`
     (401/403 → auth, 429 → rate limit, 5xx / failed job → provider error);
     only `ProviderError`, `RateLimitError`, `TimeoutError` and `NetworkError` are
     fallback-eligible in the router by default, so classify carefully.
2. **Register it.** Add the module and a `match` arm to `build()` in
   `crates/puffinparse-core/src/providers/mod.rs`.
3. **Declare its models.** Add a `ProviderInfo` entry to `PROVIDERS` in
   `crates/puffinparse-core/src/model.rs`: `name`, `display_name`, `env_var`,
   `base_url`, `docs`, and one `ModelInfo` per model listing the `modes` it
   serves; mark the provider's default with `default: true` (a bare provider
   name resolves to the default model that serves the requested mode, else
   the first model that does). Model strings are `"<provider>/<model>"`; keep them short,
   lowercase and stable — they are public API.
4. **Add pricing.** Add `"<provider>/<model>"` entries to
   `crates/puffinparse-core/src/pricing.json` with a USD-per-page price for each
   mode the model serves (`"parse"`, `"ocr"`, `"extract"`), a `source` URL
   pointing at the public pricing page, and the `updated` date. Public list
   prices only.
5. **Add a fixture + normalisation test.** Save one real (redacted) response as
   `crates/puffinparse-core/tests/fixtures/<name>_<endpoint>.json` and add a unit
   test that parses it and asserts the normalised output: page count, block
   types, a bbox inside 0..1, `usage.pages`, and that the document-level
   `markdown` is the pages joined in order. Scrub keys, job ids, customer
   names and anything else non-public from the fixture.
6. **Document it.** Add `docs/providers/<name>.md` (same sections as the
   existing pages) with a status banner, a row in `docs/providers/README.md`,
   the API-key env var (and any `*_BASE_URL` override) in `.env.example`, the
   model table in `README.md`, and a line in the `## [Unreleased]` section of
   `CHANGELOG.md`. Label the provider **live-verified** only if its live tests
   passed against the real API; otherwise it is **docs-only**.
7. **Benchmark it.** If you have keys, run the benchmark with
   `--save-outputs`, commit the result JSON under `benchmark/results/` and the
   per-document outputs under `benchmark/results/outputs/<run_id>/`, and add a
   section to `benchmark/LEADERBOARD.md` (it is stitched per dataset by hand
   until [#13](https://github.com/ajinkyashejul/puffinparse/issues/13) lands).

### Verifying a docs-only provider

Run its `#[ignore]`d live tests with your key
(`cargo test -p puffinparse-core <provider> -- --ignored --nocapture`), fix any
wire-format differences, replace the hand-built fixture with a redacted real
response, and change the label to live-verified in `docs/providers/README.md`,
the provider page's banner and `README.md`. One PR per provider.

The Python SDK needs no changes: it forwards whatever model string the core
accepts.

### Local and self-hosted engines

Engines that run on the user's machine or their own server (`tesseract`,
`docling`, `paddleocr`) follow the same steps, with these differences:

- **No API key.** Add the provider name to `SELF_HOSTED` in `model.rs`; set
  `env_var` to `""` (or to an *optional* key, as Docling does) and `base_url`
  to the local default. `puffinparse providers` then shows `local` in the Key
  column instead of a missing-key cross. Read the base URL from
  `<NAME>_BASE_URL` via `provider::resolve_base_url`.
- **Price 0.** `pricing.json` gets `0.0` for each mode with
  `"source": "self-hosted (...)"`.
- **Local binaries** are run with `tokio::process` (never C bindings or FFI),
  with the call's deadline and `kill_on_drop`. A missing binary must produce an
  error that names the binary and how to install it or point at it
  (`TESSERACT_CMD`). Shared helpers (download a URL input, base64, file-type
  sniffing, a self-cleaning scratch directory) are in
  `crates/puffinparse-core/src/providers/local.rs`.
- **Fixtures.** Capture a real output from a local install where you can (the
  Tesseract TSV and docling-serve fixtures are real); otherwise shape it from
  the server's documented schema and mark the doc page *docs-only*. The
  `#[ignore]`d live test needs the binary or server rather than a key.
- **Document** how to install or start the engine in `docs/providers/<name>.md`.

---

## 4. Adding a benchmark dataset

Datasets live in `benchmark/datasets/<name>/` and follow SPEC §10.2:

```
benchmark/datasets/<name>/
  manifest.json          # {name, version, description, license, documents:[…]}
  docs/<id>.<ext>        # the input file
  truth/<id>.md          # expected markdown (.txt for text-only documents)
```

Each entry in `manifest.documents[]` is:

```json
{
  "id": "invoice_001",
  "file": "docs/invoice_001.png",
  "truth": "truth/invoice_001.md",
  "pages": 1,
  "category": "invoice",
  "tags": ["clean", "table"]
}
```

- `id` is unique within the dataset and is what shows up in results JSON.
- `category` groups documents for per-category scores. The built-in
  `synthetic-v1` categories are `plain`, `invoice`, `table`, `two_column`,
  `noisy_scan`, `handwriting_like`, `low_res`, `rotated`, `headings`,
  `multipage`.
- `tags` are free-form; documents tagged `table` additionally get a
  `table_score`.
- `license` in the manifest is required and must permit redistribution.
  **Only open datasets are committed.** For a public set that cannot be
  redistributed (olmOCR-bench, OmniDocBench), contribute a downloader/adapter
  that produces this layout locally instead of the files themselves. The same
  goes for outputs: per-document outputs of research-only datasets
  (OmniDocBench) are not committed, only their scores.

Ground truth must be *exact* — that is why the built-in set is generated:
`make dataset` (`python benchmark/generate_synthetic.py`) renders documents
deterministically from the same source text it writes to `truth/`. Prefer
extending the generator over hand-writing truth files.

Verify with a cheap model before proposing the dataset:

```bash
cargo run -p puffinparse-cli --release -- bench run \
  --dataset benchmark/datasets/<name> \
  --models llamaparse/cost_effective \
  --out benchmark/results/$(date +%F)-<name>.json
```

---

## 5. Commit messages and pull requests

Write commit subjects in the **imperative mood**, ≤ 72 characters, no trailing
period — `Add Mistral OCR provider`, not `Added…` / `Adds…`.
[Conventional Commits](https://www.conventionalcommits.org) prefixes
(`feat:`, `fix:`, `docs:`, `perf:`, `refactor:`, `test:`, `chore:`) are
welcome but optional. Explain the *why* in the body; wrap it at 72 columns.
Keep one logical change per commit and rebase rather than merge `main`.

Before opening a PR:

- [ ] `cargo fmt --all --check` is clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean
- [ ] `cargo test --workspace` passes
- [ ] `ruff check` / `ruff format --check` / `mypy python/puffinparse` are clean
- [ ] `pytest python/tests -q` passes
- [ ] New behaviour has a test (fixture-based, no network)
- [ ] Docs updated — `README.md`, `docs/SPEC.md` if the contract changed,
      `.env.example` for a new key
- [ ] `CHANGELOG.md` `## [Unreleased]` has an entry
- [ ] Public API changes are semver-appropriate and noted in the PR description

Fill in the PR template (summary, test plan, checklist). Small, focused PRs get
reviewed fastest. If you are planning something large — a new crate, a change
to the unified response shape, a new metric — open an issue first so we can
agree on the design.

---

## 6. Releasing (maintainers)

1. Update `CHANGELOG.md`: move `## [Unreleased]` items under a new
   `## [x.y.z] - YYYY-MM-DD` heading.
2. Bump the version in every manifest (root `Cargo.toml`,
   `crates/puffinparse-cli/Cargo.toml`, `pyproject.toml`, `js/package.json`
   and `js/package-lock.json`), run `cargo check --workspace` to refresh
   `Cargo.lock`, and commit. The release workflow fails if any of them
   disagree with the tag.
3. Tag `vx.y.z` and push the tag. `.github/workflows/release.yml` builds
   wheels, an sdist, CLI archives and the gateway image, publishes them, and
   creates the GitHub Release. [docs/RELEASING.md](docs/RELEASING.md) has the
   full procedure.
