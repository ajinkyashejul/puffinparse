# AGENTS.md

Guidance for coding agents (and humans) working in this repository. The full version is
[CONTRIBUTING.md](CONTRIBUTING.md); [docs/README.md](docs/README.md) indexes every other document.

## Repository

- Rust workspace: `crates/puffinparse-core` (types, providers, router, pricing, benchmark
  metrics), `crates/puffinparse-cli` (the `puffinparse` binary), `crates/puffinparse-server`
  (the `puffinparse serve` gateway), `crates/puffinparse-python` (PyO3 extension),
  `crates/puffinparse-node` (napi-rs addon).
- Python package in `python/puffinparse`, built with maturin. Node package in `js/`.
- Specification: [docs/SPEC.md](docs/SPEC.md). Decisions: [docs/DECISIONS.md](docs/DECISIONS.md).
- `main` is the only long-lived branch. Never rewrite published history or force-push.

## Build, test, lint

```bash
# Rust
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Python (inside a virtualenv; rebuild after any Rust change that Python calls)
pip install maturin ruff mypy pytest pytest-asyncio
maturin develop --release
ruff check python/ benchmark/ examples/ && ruff format --check python/ benchmark/ examples/
mypy python/puffinparse
pytest python/tests -q

# Node
cd js && npm ci && npm run build:debug && npm test

# Docs site (must report no link problems)
uv run --no-project --with markdown --with pillow --with pypdfium2 \
  python website/build.py --out /tmp/puffinparse-site --check
```

`make lint test` runs the Rust and Python checks; `make test-node` runs the Node suite.

Tests make no network calls. Live provider tests are opt-in: `PUFFINPARSE_LIVE_TESTS=1` for
pytest, `cargo test -p puffinparse-core -- --ignored` for Rust, and both need the provider's key
in the environment.

## Where things go

| Change | Also update |
|---|---|
| New provider or model | `model.rs` registry, `pricing.json` (with source URL and date), a redacted fixture plus a unit test under `crates/puffinparse-core/tests/fixtures/`, `docs/providers/<name>.md` and its row in `docs/providers/README.md`, the README model table, `.env.example`, `CHANGELOG.md` |
| Provider verification status | the status column in `docs/providers/README.md`, the banner on the provider page, the README model table. Only mark a provider live-verified after its live tests pass against the real API. |
| Unified types or errors | `docs/SPEC.md` §4–6, `python/puffinparse/types.py` or `exceptions.py`, `_core.pyi`, the Node typings, tests |
| Benchmark metrics or result JSON | `docs/SPEC.md` §10, `benchmark/README.md`, the results viewer in `benchmark/site/` |
| Benchmark results | the result JSON under `benchmark/results/`, per-document outputs under `benchmark/results/outputs/<run_id>/`, then `benchmark/LEADERBOARD.md` from committed results only |
| Dataset | `benchmark/datasets/<name>/manifest.json` and a README stating the license |
| A change of direction | a new ADR in `docs/DECISIONS.md` |
| Anything user-visible | a line under `## [Unreleased]` in `CHANGELOG.md` |

## Conventions

- Rust: `#![forbid(unsafe_code)]`; no provider SDK crates, only the shared HTTP helpers in
  `http.rs`; local engines run as subprocesses, never through FFI.
- Every provider parse path has a fixture test built from a real (redacted) payload, or from the
  documented shape for a docs-only provider.
- Map provider HTTP status and job failures onto `ErrorKind`; never swallow a provider's message.
- Python is fully typed (`mypy --strict`), its dataclasses mirror the Rust structs one to one,
  and it contains no provider logic.
- Benchmark runs keep provider result caches disabled (the default).
- Commit messages: imperative subject of 72 characters or fewer; the body explains why.

## Never commit

- API keys, tokens, `.env` files, or provider payloads that still contain account ids, job ids or
  customer content. Keys are read from the environment (`.env.example` lists the variables);
  never print or log their values.
- Per-document outputs or inputs from research-only datasets (OmniDocBench, for example). Their
  scores are committed; their outputs are not.
- Documents whose license does not allow redistribution. Write an adapter that fetches them at
  run time instead.
