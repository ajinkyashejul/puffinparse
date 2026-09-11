from __future__ import annotations

from liteocr.types import BBox, Block, OcrResponse, Page, Usage


def test_response_round_trip() -> None:
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
    r = OcrResponse.from_dict(d)
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
