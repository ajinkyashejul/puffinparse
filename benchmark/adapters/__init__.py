"""Benchmark adapters: public OCR benchmarks → LiteOCR dataset manifests.

Usage::

    python -m benchmark.adapters parsebench --limit 40
    python -m benchmark.adapters combined
    python -m benchmark.adapters --list

See ``docs/benchmarks/adapters.md`` for the manifest extension, the rule schema and how to
write a new adapter. Importing this package registers every adapter shipped in it.
"""

from __future__ import annotations

# Importing the modules is what populates `registry` through the @register decorator.
from . import combined as combined
from . import parsebench as parsebench
from .base import (
    KIND_RULES,
    KIND_TRANSCRIPT,
    RULE_TYPES,
    Adapter,
    Doc,
    Manifest,
    Rule,
    TableConversion,
    Upstream,
    default_cache_dir,
    html_table_to_markdown,
    pdf_page_count,
    register,
    registry,
    sha256_bytes,
    sha256_file,
    slugify,
    write_manifest,
    write_rules,
)

__all__ = [
    "KIND_RULES",
    "KIND_TRANSCRIPT",
    "RULE_TYPES",
    "Adapter",
    "Doc",
    "Manifest",
    "Rule",
    "TableConversion",
    "Upstream",
    "combined",
    "default_cache_dir",
    "html_table_to_markdown",
    "parsebench",
    "pdf_page_count",
    "register",
    "registry",
    "sha256_bytes",
    "sha256_file",
    "slugify",
    "write_manifest",
    "write_rules",
]
