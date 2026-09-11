from __future__ import annotations

import os
from pathlib import Path

import pytest

FIXTURES = Path(__file__).resolve().parents[2] / "crates" / "liteocr-core" / "tests" / "fixtures"
SAMPLE_PDF = (
    Path(__file__).resolve().parents[2]
    / "benchmark"
    / "datasets"
    / "synthetic-v1"
    / "docs"
    / "multipage_001.pdf"
)

PROVIDER_ENV = {
    "reducto": "REDUCTO_API_KEY",
    "extend": "EXTEND_API_KEY",
    "llamaparse": "LLAMA_API_KEY",
}


def has_key(provider: str) -> bool:
    return bool(os.environ.get(PROVIDER_ENV[provider]))


def live(provider: str):  # type: ignore[no-untyped-def]
    """Skip a live test unless the provider's API key is configured and LITEOCR_LIVE_TESTS is set."""
    return pytest.mark.skipif(
        not (has_key(provider) and os.environ.get("LITEOCR_LIVE_TESTS")),
        reason=f"set {PROVIDER_ENV[provider]} and LITEOCR_LIVE_TESTS=1 to run live {provider} tests",
    )
