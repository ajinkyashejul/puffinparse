from __future__ import annotations

from puffinparse.types import (
    BBox,
    Block,
    Citation,
    ExtractResponse,
    FieldInfo,
    Line,
    Page,
    ParseResponse,
    TextPage,
    TextResponse,
    Usage,
    Word,
)


def test_parse_response_round_trip() -> None:
    d = {
        "id": "abc",
        "provider": "reducto",
        "model": "reducto/standard",
        "provider_job_id": "job-1",
        "pages": [
            {
                "page_number": 1,
                "width": 100.0,
                "height": 200.0,
                "markdown": "# Hi\n\n| a | b |",
                "text": "Hi\na b",
                "blocks": [
                    {
                        "type": "title",
                        "content": "# Hi",
                        "page_number": 1,
                        "bbox": {"x0": 0.1, "y0": 0.1, "x1": 0.5, "y1": 0.2},
                        "confidence": 0.9,
                    },
                    {"type": "table", "content": "| a | b |", "page_number": 1},
                ],
            }
        ],
        "markdown": "# Hi\n\n| a | b |",
        "text": "Hi\na b",
        "usage": {"pages": 1, "credits": 1.0},
        "cost_usd": 0.015,
        "latency_ms": 123,
        "created_at": "2026-09-11T00:00:00Z",
        "metadata": {"k": "v"},
    }
    r = ParseResponse.from_dict(d)
    assert r.num_pages == 1
    assert r.provider_job_id == "job-1"
    assert r.usage == Usage(pages=1, credits=1.0)
    assert r.cost_usd == 0.015
    assert r.tables[0].content == "| a | b |"
    assert r.pages[0].tables == r.tables
    blk = r.blocks[0]
    assert isinstance(blk, Block) and blk.type == "title"
    assert blk.bbox == BBox(0.1, 0.1, 0.5, 0.2)
    assert blk.bbox is not None and blk.bbox.to_pixels(100, 200) == (10.0, 20.0, 50.0, 40.0)
    assert blk.bbox.width == 0.4
    assert str(r) == r.markdown
    assert r.to_dict()["pages"][0]["blocks"][1]["bbox"] is None
    assert isinstance(r.pages[0], Page)


def test_text_response_round_trip() -> None:
    d = {
        "id": "t1",
        "provider": "llamaparse",
        "model": "llamaparse/fast",
        "provider_job_id": "job-2",
        "pages": [
            {
                "page_number": 1,
                "width": 612.0,
                "height": 792.0,
                "text": "Hello world\nSecond line",
                "lines": [
                    {
                        "text": "Hello world",
                        "bbox": {"x0": 0.1, "y0": 0.1, "x1": 0.5, "y1": 0.2},
                        "confidence": 0.98,
                    },
                    {"text": "Second line"},
                ],
                "words": [
                    {"text": "Hello", "bbox": {"x0": 0.1, "y0": 0.1, "x1": 0.2, "y1": 0.2}},
                    {"text": "world", "confidence": 0.9},
                ],
            },
            {"page_number": 2, "text": "Page two"},
        ],
        "text": "Hello world\nSecond line\n\nPage two",
        "usage": {"pages": 2},
        "cost_usd": 0.0025,
        "latency_ms": 42,
        "created_at": "2026-09-11T00:00:00Z",
        "metadata": {"puffinparse_derived_from": "parse"},
    }
    r = TextResponse.from_dict(d)
    assert r.num_pages == 2
    assert isinstance(r.pages[0], TextPage)
    assert r.text.startswith("Hello world")
    assert str(r) == r.text
    assert [line.text for line in r.lines] == ["Hello world", "Second line"]
    assert r.lines[0] == Line("Hello world", BBox(0.1, 0.1, 0.5, 0.2), 0.98)
    assert r.lines[1].bbox is None
    assert r.words[0] == Word("Hello", BBox(0.1, 0.1, 0.2, 0.2), None)
    assert r.words[1].confidence == 0.9
    assert r.pages[1].lines == [] and r.pages[1].words == []
    assert r.metadata["puffinparse_derived_from"] == "parse"
    assert r.to_dict()["pages"][0]["lines"][1]["bbox"] is None


def test_extract_response_round_trip() -> None:
    d = {
        "id": "x1",
        "provider": "acme",
        "model": "acme/extract",
        "data": {"invoice": {"total": 42.5, "currency": "USD"}},
        "fields": {
            "/invoice/total": {
                "confidence": 0.91,
                "citations": [
                    {
                        "page_number": 2,
                        "bbox": {"x0": 0.1, "y0": 0.2, "x1": 0.3, "y1": 0.25},
                        "text": "Total due 42.50",
                    }
                ],
            },
            "/invoice/currency": {"confidence": 0.5},
        },
        "usage": {"pages": 2},
        "cost_usd": 0.08,
        "latency_ms": 900,
        "created_at": "2026-09-11T00:00:00Z",
    }
    r = ExtractResponse.from_dict(d)
    assert r.data["invoice"]["total"] == 42.5
    assert isinstance(r.fields["/invoice/total"], FieldInfo)
    info = r.field_info("/invoice/total")
    assert info is not None and info.confidence == 0.91
    cite = r.citations("/invoice/total")[0]
    assert cite == Citation(2, BBox(0.1, 0.2, 0.3, 0.25), "Total due 42.50")
    assert r.citations("/invoice/currency") == []
    assert r.citations("/nope") == []
    assert r.field_info("/nope") is None
    assert r.to_dict()["fields"]["/invoice/currency"]["citations"] == []
    assert str(r) == str(r.data)
