"""Unified response types. These mirror the Rust structs in ``liteocr-core`` 1:1."""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from typing import Any, Literal, Optional

BlockType = Literal[
    "text",
    "title",
    "section_header",
    "list",
    "table",
    "figure",
    "header",
    "footer",
    "footnote",
    "caption",
    "formula",
    "other",
]


@dataclass(frozen=True)
class BBox:
    """Normalised bounding box (0..1 relative to page size, origin top-left)."""

    x0: float
    y0: float
    x1: float
    y1: float

    @property
    def width(self) -> float:
        return self.x1 - self.x0

    @property
    def height(self) -> float:
        return self.y1 - self.y0

    def to_pixels(self, page_width: float, page_height: float) -> tuple[float, float, float, float]:
        """Scale to absolute coordinates ``(x0, y0, x1, y1)``."""
        return (
            self.x0 * page_width,
            self.y0 * page_height,
            self.x1 * page_width,
            self.y1 * page_height,
        )


@dataclass
class Block:
    type: BlockType
    content: str
    page_number: int
    text: Optional[str] = None
    bbox: Optional[BBox] = None
    confidence: Optional[float] = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> Block:
        bbox = d.get("bbox")
        return cls(
            type=d.get("type", "other"),
            content=d.get("content", ""),
            page_number=int(d.get("page_number", 1)),
            text=d.get("text"),
            bbox=BBox(**bbox) if bbox else None,
            confidence=d.get("confidence"),
        )


@dataclass
class Page:
    page_number: int
    markdown: str
    text: str
    blocks: list[Block] = field(default_factory=list)
    width: Optional[float] = None
    height: Optional[float] = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> Page:
        return cls(
            page_number=int(d.get("page_number", 1)),
            markdown=d.get("markdown", ""),
            text=d.get("text", ""),
            blocks=[Block.from_dict(b) for b in d.get("blocks", [])],
            width=d.get("width"),
            height=d.get("height"),
        )

    def blocks_of(self, *types: BlockType) -> list[Block]:
        return [b for b in self.blocks if b.type in types]

    @property
    def tables(self) -> list[Block]:
        return self.blocks_of("table")


@dataclass
class Usage:
    pages: int = 0
    credits: Optional[float] = None
    provider_cost_usd: Optional[float] = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> Usage:
        return cls(
            pages=int(d.get("pages", 0)),
            credits=d.get("credits"),
            provider_cost_usd=d.get("provider_cost_usd"),
        )


@dataclass
class OcrResponse:
    """The unified result of an OCR call, identical across providers."""

    id: str
    provider: str
    model: str
    pages: list[Page]
    markdown: str
    text: str
    usage: Usage
    latency_ms: int
    created_at: str
    provider_job_id: Optional[str] = None
    cost_usd: Optional[float] = None
    metadata: dict[str, Any] = field(default_factory=dict)
    raw: Any = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> OcrResponse:
        return cls(
            id=d["id"],
            provider=d["provider"],
            model=d["model"],
            pages=[Page.from_dict(p) for p in d.get("pages", [])],
            markdown=d.get("markdown", ""),
            text=d.get("text", ""),
            usage=Usage.from_dict(d.get("usage", {})),
            latency_ms=int(d.get("latency_ms", 0)),
            created_at=d.get("created_at", ""),
            provider_job_id=d.get("provider_job_id"),
            cost_usd=d.get("cost_usd"),
            metadata=dict(d.get("metadata", {}) or {}),
            raw=d.get("raw"),
        )

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    @property
    def num_pages(self) -> int:
        return len(self.pages)

    @property
    def blocks(self) -> list[Block]:
        return [b for p in self.pages for b in p.blocks]

    @property
    def tables(self) -> list[Block]:
        return [b for b in self.blocks if b.type == "table"]

    def __str__(self) -> str:
        return self.markdown


@dataclass
class Metrics:
    """Benchmark metrics between a prediction and a ground truth (see ``liteocr.score``)."""

    char_similarity: float
    cer: float
    wer: float
    word_recall: float
    word_precision: float
    word_f1: float
    pred_chars: int
    truth_chars: int
    order_score: Optional[float] = None
    table_score: Optional[float] = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> Metrics:
        return cls(
            char_similarity=d["char_similarity"],
            cer=d["cer"],
            wer=d["wer"],
            word_recall=d["word_recall"],
            word_precision=d["word_precision"],
            word_f1=d["word_f1"],
            pred_chars=d["pred_chars"],
            truth_chars=d["truth_chars"],
            order_score=d.get("order_score"),
            table_score=d.get("table_score"),
        )


__all__ = ["BBox", "Block", "BlockType", "Metrics", "OcrResponse", "Page", "Usage"]
