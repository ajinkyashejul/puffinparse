"""Live provider tests. Skipped unless LITEOCR_LIVE_TESTS=1 and the provider key is set."""

from __future__ import annotations

import liteocr
import pytest
from conftest import SAMPLE_PDF, live

MODELS = [
    pytest.param("reducto/standard", marks=live("reducto")),
    pytest.param("extend/parse_light", marks=live("extend")),
    pytest.param("llamaparse/fast", marks=live("llamaparse")),
]


@pytest.mark.parametrize("model", MODELS)
def test_parse_sample_pdf(model: str) -> None:
    resp = liteocr.ocr(SAMPLE_PDF, model=model, timeout=240)
    assert resp.model == model
    assert resp.usage.pages == 2
    assert len(resp.pages) == 2
    assert resp.pages[0].page_number == 1
    assert resp.markdown.strip()
    assert resp.cost_usd is not None and resp.cost_usd > 0
    assert resp.latency_ms > 0
    assert resp.blocks, "expected typed blocks"


@pytest.mark.parametrize("model", MODELS)
async def test_aocr_bytes_input(model: str) -> None:
    data = SAMPLE_PDF.read_bytes()
    resp = await liteocr.aocr(data, model=model, filename="multipage_001.pdf", pages="1", timeout=240)
    assert resp.usage.pages >= 1
    assert resp.pages[0].markdown.strip()
