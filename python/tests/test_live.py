"""Live provider tests. Skipped unless PUFFINPARSE_LIVE_TESTS=1 and the provider key is set."""

from __future__ import annotations

import puffinparse
import pytest
from conftest import SAMPLE_PDF, live

MODELS = [
    pytest.param("reducto/standard", marks=live("reducto")),
    pytest.param("extend/parse_light", marks=live("extend")),
    pytest.param("llamaparse/fast", marks=live("llamaparse")),
]


@pytest.mark.parametrize("model", MODELS)
def test_parse_sample_pdf(model: str) -> None:
    resp = puffinparse.parse(SAMPLE_PDF, model=model, timeout=240)
    assert resp.model == model
    assert resp.usage.pages == 2
    assert len(resp.pages) == 2
    assert resp.pages[0].page_number == 1
    assert resp.markdown.strip()
    assert resp.cost_usd is not None and resp.cost_usd > 0
    assert resp.latency_ms > 0
    assert resp.blocks, "expected typed blocks"


@pytest.mark.parametrize("model", MODELS)
def test_ocr_sample_pdf(model: str) -> None:
    resp = puffinparse.ocr(SAMPLE_PDF, model=model, timeout=240)
    assert resp.model == model
    assert resp.usage.pages == 2
    assert len(resp.pages) == 2
    assert resp.text.strip(), "expected plain text"
    assert resp.pages[0].lines, "expected recognised lines"
    assert resp.pages[0].lines[0].text.strip()
    assert resp.pages[0].words, "expected recognised words"
    assert resp.cost_usd is not None and resp.cost_usd > 0
    # the ocr result must not carry layout markdown: it is text only
    assert not hasattr(resp, "markdown")


@pytest.mark.parametrize("model", MODELS)
async def test_aparse_bytes_input(model: str) -> None:
    data = SAMPLE_PDF.read_bytes()
    resp = await puffinparse.aparse(data, model=model, filename="multipage_001.pdf", pages="1", timeout=240)
    assert resp.usage.pages >= 1
    assert resp.pages[0].markdown.strip()


@pytest.mark.parametrize("model", MODELS)
async def test_aocr_bytes_input(model: str) -> None:
    data = SAMPLE_PDF.read_bytes()
    resp = await puffinparse.aocr(data, model=model, filename="multipage_001.pdf", pages="1", timeout=240)
    assert resp.usage.pages >= 1
    assert resp.pages[0].text.strip()
    assert resp.pages[0].lines


# ---- native-format output (output_format) --------------------------------------------------------


def _prune(value: object) -> object:
    """Drop what the Rust serialiser omits (``None`` fields, empty maps) from a dataclass dump."""
    if isinstance(value, dict):
        return {k: _prune(v) for k, v in value.items() if v is not None and v != {}}
    if isinstance(value, list):
        return [_prune(v) for v in value]
    return value


@live("extend")
def test_parse_renders_extend_output_in_reductos_shape() -> None:
    """Reducto's parse JSON, produced by Extend's engine: the whole point of the compat layer."""
    doc = puffinparse.parse(SAMPLE_PDF, model="extend/parse_light", output_format="reducto", timeout=240)
    assert isinstance(doc, dict)
    assert doc["response_type"] == "parse"
    assert doc["usage"]["num_pages"] == 2
    assert doc["result"]["type"] == "full"
    chunks = doc["result"]["chunks"]
    assert len(chunks) == 2
    blocks = chunks[0]["blocks"]
    assert blocks, "expected Reducto-shaped blocks"
    first = blocks[0]
    assert first["content"].strip()
    assert set(first["bbox"]) == {"left", "top", "width", "height", "page", "original_page"}
    assert first["bbox"]["page"] == 1
    # Reducto's render uses a reduced block vocabulary (docs/COMPAT.md §4)
    reducto_types = {"Title", "Section Header", "Text", "List Item", "Table", "Figure", "Header", "Footer"}
    assert first["type"] in reducto_types


@live("extend")
def test_puffinparse_output_format_equals_the_dataclass() -> None:
    """``output_format="puffinparse"`` is the unified shape, and callbacks still get the dataclass."""
    seen: list[puffinparse.Response] = []
    puffinparse.success_callback.append(seen.append)
    try:
        doc = puffinparse.parse(
            SAMPLE_PDF, model="extend/parse_light", output_format="puffinparse", timeout=240
        )
    finally:
        puffinparse.success_callback.remove(seen.append)

    assert isinstance(doc, dict)
    assert len(seen) == 1 and isinstance(seen[0], puffinparse.ParseResponse)
    # same call, so nothing is volatile: the dict is exactly the dataclass, minus the fields the
    # Rust serialiser omits when they are empty.
    assert doc == _prune(seen[0].to_dict())


@live("llamaparse")
@pytest.mark.parametrize("output_format", ["reducto", "extend", "llamaparse"])
def test_every_vendor_shape_deserialises(output_format: str) -> None:
    doc = puffinparse.parse(SAMPLE_PDF, model="llamaparse/fast", output_format=output_format, timeout=240)
    assert isinstance(doc, dict) and doc
    pages = {"reducto": ("result", "chunks"), "extend": ("output", "chunks"), "llamaparse": ("pages",)}
    node: object = doc
    for key in pages[output_format]:
        assert isinstance(node, dict)
        node = node[key]
    assert isinstance(node, list) and len(node) == 2
