# Contributing to LiteOCR

Thanks for helping build LiteOCR — one API for every OCR / document-parsing
provider. This document covers local setup, the checks CI runs, and the two
contributions we get asked about most: **adding a provider** and **adding a
benchmark dataset**.

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).
LiteOCR is MIT licensed; contributions are accepted under the same license.

---

## 1. Setup

Prerequisites:

- **Rust stable** (≥ 1.80 — the workspace `rust-version`), via [rustup](https://rustup.rs).
- **Python 3.9+** (3.9 is the minimum we build abi3 wheels for).
- A C toolchain (whatever `cc` your platform ships) for the native deps.

```bash
git clone https://github.com/ajinkyashejul/liteocr
cd liteocr

# Rust side
cargo build --workspace

# Python side — use a virtualenv; maturin installs into the active one.
python -m venv .venv && source .venv/bin/activate
pip install maturin ruff mypy pytest
maturin develop            # builds crates/liteocr-python, installs `liteocr`
```

`maturin develop` compiles the PyO3 extension (`liteocr._core`) and links it
against the pure-Python package in `python/liteocr`, so edits to the Python
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

### Debugging

Set `LITEOCR_LOG` to turn on the core's `tracing` output — request URLs, retry
and backoff decisions, poll loops, router fallbacks:

```bash
LITEOCR_LOG=debug liteocr parse invoice.pdf --model reducto/standard
LITEOCR_LOG=liteocr_core::providers=trace pytest python/tests -q
```

On the Python side the same events also surface through
`logging.getLogger("liteocr")`.

---

## 2. Tests, lint, formatting

Run everything the CI runs with `make test lint`, or piecemeal:

```bash
# Tests
cargo test --workspace
pytest python/tests -q

# Lint / format
cargo fmt --all              # or `cargo fmt --all --check` to only verify
cargo clippy --workspace --all-targets -- -D warnings
ruff check python/ benchmark/
ruff format --check python/ benchmark/
mypy python/liteocr
```

Rules of the road:

- `cargo fmt` output is authoritative; do not hand-format around it.
- Clippy warnings are errors in CI. Prefer fixing over `#[allow]`; if an
  `#[allow]` is genuinely right, put a one-line comment saying why.
- **No network in unit tests.** Provider parsing is tested against recorded
  JSON in `crates/liteocr-core/tests/fixtures/`. Live tests are `#[ignore]`d
  and/or key-gated.
- `liteocr-core` is `#![forbid(unsafe_code)]`. Keep it that way.
- The Python package is fully typed and `mypy --strict`-clean; new public API
  needs annotations and a docstring.

---

## 3. Adding a provider

This is the highest-value contribution. A provider is a single file
implementing one trait. Check for an existing
[`new provider` issue](https://github.com/ajinkyashejul/liteocr/issues) first,
or open one from the template so we can agree on model naming before you write
code.

1. **Implement the trait.** Create
   `crates/liteocr-core/src/providers/<name>.rs` and implement
   `OcrProvider` (see `crates/liteocr-core/src/provider.rs`). Use the shared
   HTTP helpers in `src/http.rs` so you inherit retries, backoff, deadlines and
   error classification — do not build your own `reqwest::Client`, and do not
   vendor a provider SDK. Map the provider's response onto the unified
   `OcrResponse` / `Page` / `Block` / `Usage` types in `src/types.rs`:
   - normalise `bbox` to 0..1 with a top-left origin;
   - map the provider's block vocabulary onto `BlockType`, unknown → `other`;
   - fill `Usage.pages` with the *billed* page count;
   - map provider errors onto the `Error` variants in `src/error.rs`
     (401/403 → auth, 429 → rate limit, 5xx / failed job → provider error);
     only `ProviderError`, `RateLimitError` and `TimeoutError` are fallback-eligible
     in the router, so classify carefully.
2. **Register it.** Add the module and a `match` arm to `build()` in
   `crates/liteocr-core/src/providers/mod.rs`.
3. **Declare its models.** Add a `ProviderInfo` entry to `PROVIDERS` in
   `crates/liteocr-core/src/model.rs`: `name`, `display_name`, `env_var`,
   `base_url`, `docs`, and one `ModelInfo` per mode with exactly one
   `default: true`. Model strings are `"<provider>/<model>"`; keep them short,
   lowercase and stable — they are public API.
4. **Add pricing.** Add `"<provider>/<model>"` entries to
   `crates/liteocr-core/src/pricing.json` with `per_page_usd`, a `source` URL
   pointing at the public pricing page, and the `updated` date. Public list
   prices only.
5. **Add a fixture + normalisation test.** Save one real (redacted) response as
   `crates/liteocr-core/tests/fixtures/<name>_<endpoint>.json` and add a unit
   test that parses it and asserts the normalised output: page count, block
   types, a bbox inside 0..1, `usage.pages`, and that the document-level
   `markdown` is the pages joined in order. Scrub keys, job ids, customer
   names and anything else non-public from the fixture.
6. **Document it.** Add the API-key env var (and any `*_BASE_URL` override) to
   `.env.example` and the provider table in `README.md`, and add a line to the
   `## [Unreleased]` section of `CHANGELOG.md`.
7. **Benchmark it.** Run `make bench` with your model included and, if you have
   keys, commit the result JSON under `benchmark/results/` and regenerate the
   leaderboard with `make leaderboard`.

The Python SDK needs no changes: it forwards whatever model string the core
accepts.

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
  that produces this layout locally instead of the files themselves.

Ground truth must be *exact* — that is why the built-in set is generated:
`make dataset` (`python benchmark/generate_synthetic.py`) renders documents
deterministically from the same source text it writes to `truth/`. Prefer
extending the generator over hand-writing truth files.

Verify with a cheap model before proposing the dataset:

```bash
cargo run -p liteocr-cli --release -- bench run \
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
- [ ] `ruff check` / `ruff format --check` / `mypy python/liteocr` are clean
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
2. Bump `workspace.package.version` in the root `Cargo.toml`, run
   `cargo check --workspace` to refresh `Cargo.lock`, and commit.
3. Tag `vx.y.z` and push the tag. `.github/workflows/release.yml` builds
   wheels + an sdist + CLI archives, publishes to PyPI via trusted publishing,
   and creates the GitHub Release.
