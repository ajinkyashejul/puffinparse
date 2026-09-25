---
name: Bug report
about: Something in PuffinParse behaves incorrectly
title: ""
labels: bug
assignees: ""
---

## What happened

<!-- A clear description of the wrong behaviour. -->

## What you expected

## Reproduction

<!-- Smallest snippet or command that shows the problem. Please REDACT API keys. -->

```python
import puffinparse
resp = puffinparse.ocr("doc.pdf", model="reducto/standard")
```

or

```bash
puffinparse parse doc.pdf --model reducto/standard
```

**Input document**: <!-- kind of file, page count, whether you can share it -->

## Logs

<!-- Re-run with PUFFINPARSE_LOG=debug and paste the relevant output. Keys are
     redacted by PuffinParse, but please double-check before pasting. -->

<details>
<summary><code>PUFFINPARSE_LOG=debug</code> output</summary>

```
paste here
```

</details>

## Environment

- PuffinParse version: <!-- pip show puffinparse, or the commit SHA -->
- Installed via: <!-- PyPI wheel / maturin develop / cargo -->
- Provider and model: <!-- e.g. llamaparse/cost_effective -->
- Python version:
- Rust version (if building from source): <!-- rustc -V -->
- OS / architecture:

## Checklist

- [ ] I am on the latest released version (or `main`)
- [ ] I searched existing issues
- [ ] No API keys, tokens or confidential document content appear above
