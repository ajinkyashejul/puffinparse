## Summary

<!-- What does this change and why? Link the issue it closes: "Closes #123". -->

## Test plan

<!-- How did you verify this? Commands you ran, fixtures you added, and for a
     provider or benchmark change the actual numbers before/after. -->

```bash
cargo test --workspace
pytest python/tests -q
```

## Checklist

- [ ] `cargo fmt --all --check` is clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean
- [ ] `cargo test --workspace` passes
- [ ] `ruff check python/ benchmark/` and `ruff format --check python/ benchmark/` are clean
- [ ] `mypy python/liteocr` is clean
- [ ] `pytest python/tests -q` passes
- [ ] New behaviour is covered by a test that makes no network calls
- [ ] Docs updated (`README.md`, `docs/SPEC.md`, `.env.example`) where relevant
- [ ] `CHANGELOG.md` `## [Unreleased]` updated
- [ ] No API keys, tokens or unredacted provider payloads are committed

## Breaking changes

<!-- None, or: what breaks, and what users must do. Note the semver impact. -->
