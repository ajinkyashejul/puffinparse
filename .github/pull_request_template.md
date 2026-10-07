## What and why

<!-- What does this change, and why? Link the issue it closes: "Closes #123". -->

## Checks run

<!-- Paste or tick what you ran. `make lint test` covers the Rust and Python
     checks; add `make test-node` if you touched js/ or crates/puffinparse-node. -->

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- [ ] `ruff check` / `ruff format --check` on `python/ benchmark/ examples/`, `mypy python/puffinparse`, `pytest python/tests -q`
- [ ] New behaviour has a test that makes no network calls
- [ ] Docs and `CHANGELOG.md` `## [Unreleased]` updated where relevant
- [ ] No API keys, tokens, unredacted provider payloads or research-only dataset outputs are committed

<!-- For a provider or benchmark change, include the numbers before and after.
     For a breaking change, say what breaks and what users must do. -->
