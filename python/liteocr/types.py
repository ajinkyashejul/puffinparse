"""Unified response types. These mirror the Rust structs in ``liteocr-core`` 1:1.

There is one response type per **mode**:

==========  ==========================  ===================================================
Mode        Response                    Contents
==========  ==========================  ===================================================
``parse``   :class:`ParseResponse`      markdown + typed :class:`Block` s with boxes
``ocr``     :class:`TextResponse`       plain text + :class:`Line` / :class:`Word` boxes
``extract`` :class:`ExtractResponse`    a JSON object shaped by your schema, + citations
==========  ==========================  ===================================================
"""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from typing import Any, Literal, Optional, Union

#: What a call asks a provider to do. Providers can only be swapped within a mode.
Mode = Literal["parse", "ocr", "extract"]

MODES: tuple[Mode, ...] = ("parse", "ocr", "extract")

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


def _bbox(d: Optional[dict[str, Any]]) -> Optional[BBox]:
    return BBox(**d) if d else None


# ---- parse mode ----------------------------------------------------------------------------------


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
        return cls(
            type=d.get("type", "other"),
            content=d.get("content", ""),
            page_number=int(d.get("page_number", 1)),
            text=d.get("text"),
            bbox=_bbox(d.get("bbox")),
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
class ParseResponse:
    """Result of a ``parse``-mode call: layout-aware markdown and typed blocks."""

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
    def from_dict(cls, d: dict[str, Any]) -> ParseResponse:
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


# ---- ocr mode ------------------------------------------------------------------------------------


@dataclass
class Word:
    """A recognised word with its box and confidence."""

    text: str
    bbox: Optional[BBox] = None
    confidence: Optional[float] = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> Word:
        return cls(text=d.get("text", ""), bbox=_bbox(d.get("bbox")), confidence=d.get("confidence"))


@dataclass
class Line:
    """A recognised line of text (a run of words on one baseline)."""

    text: str
    bbox: Optional[BBox] = None
    confidence: Optional[float] = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> Line:
        return cls(text=d.get("text", ""), bbox=_bbox(d.get("bbox")), confidence=d.get("confidence"))


@dataclass
class TextPage:
    page_number: int
    text: str
    lines: list[Line] = field(default_factory=list)
    words: list[Word] = field(default_factory=list)
    width: Optional[float] = None
    height: Optional[float] = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> TextPage:
        return cls(
            page_number=int(d.get("page_number", 1)),
            text=d.get("text", ""),
            lines=[Line.from_dict(x) for x in d.get("lines", [])],
            words=[Word.from_dict(x) for x in d.get("words", [])],
            width=d.get("width"),
            height=d.get("height"),
        )


@dataclass
class TextResponse:
    """Result of an ``ocr``-mode call: plain text with word/line geometry, no layout semantics."""

    id: str
    provider: str
    model: str
    pages: list[TextPage]
    text: str
    usage: Usage
    latency_ms: int
    created_at: str
    provider_job_id: Optional[str] = None
    cost_usd: Optional[float] = None
    metadata: dict[str, Any] = field(default_factory=dict)
    raw: Any = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> TextResponse:
        return cls(
            id=d["id"],
            provider=d["provider"],
            model=d["model"],
            pages=[TextPage.from_dict(p) for p in d.get("pages", [])],
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
    def lines(self) -> list[Line]:
        return [line for p in self.pages for line in p.lines]

    @property
    def words(self) -> list[Word]:
        return [w for p in self.pages for w in p.words]

    def __str__(self) -> str:
        return self.text


# ---- extract mode --------------------------------------------------------------------------------


@dataclass
class Citation:
    """Where an extracted value came from."""

    page_number: int
    bbox: Optional[BBox] = None
    text: Optional[str] = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> Citation:
        return cls(page_number=int(d.get("page_number", 1)), bbox=_bbox(d.get("bbox")), text=d.get("text"))


@dataclass
class FieldInfo:
    """Per-field confidence and citations, keyed in :attr:`ExtractResponse.fields` by JSON pointer."""

    confidence: Optional[float] = None
    citations: list[Citation] = field(default_factory=list)

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> FieldInfo:
        return cls(
            confidence=d.get("confidence"),
            citations=[Citation.from_dict(c) for c in d.get("citations", [])],
        )


@dataclass
class ExtractResponse:
    """Result of an ``extract``-mode call: the object your schema asked for, plus provenance."""

    id: str
    provider: str
    model: str
    data: Any
    usage: Usage
    latency_ms: int
    created_at: str
    provider_job_id: Optional[str] = None
    fields: dict[str, FieldInfo] = field(default_factory=dict)
    cost_usd: Optional[float] = None
    metadata: dict[str, Any] = field(default_factory=dict)
    raw: Any = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> ExtractResponse:
        return cls(
            id=d["id"],
            provider=d["provider"],
            model=d["model"],
            data=d.get("data"),
            usage=Usage.from_dict(d.get("usage", {})),
            latency_ms=int(d.get("latency_ms", 0)),
            created_at=d.get("created_at", ""),
            provider_job_id=d.get("provider_job_id"),
            fields={k: FieldInfo.from_dict(v) for k, v in (d.get("fields") or {}).items()},
            cost_usd=d.get("cost_usd"),
            metadata=dict(d.get("metadata", {}) or {}),
            raw=d.get("raw"),
        )

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    def field_info(self, pointer: str) -> Optional[FieldInfo]:
        """Confidence / citations for a JSON pointer into :attr:`data`, e.g. ``"/invoice/total"``."""
        return self.fields.get(pointer)

    def citations(self, pointer: str) -> list[Citation]:
        """Citations for one field, or an empty list if the provider reported none."""
        info = self.fields.get(pointer)
        return list(info.citations) if info else []

    def __str__(self) -> str:
        return str(self.data)


#: Any mode's response.
Response = Union[ParseResponse, TextResponse, ExtractResponse]


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
    #: ``passed / total`` of a rule-scored document; ``None`` for transcript documents.
    rule_pass_rate: Optional[float] = None
    #: Rules that passed, for a rule-scored document.
    rules_passed: Optional[int] = None
    #: Rules checked, for a rule-scored document.
    rules_total: Optional[int] = None

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
            rule_pass_rate=d.get("rule_pass_rate"),
            rules_passed=d.get("rules_passed"),
            rules_total=d.get("rules_total"),
        )


__all__ = [
    "MODES",
    "BBox",
    "Block",
    "BlockType",
    "Citation",
    "ExtractResponse",
    "FieldInfo",
    "Line",
    "Metrics",
    "Mode",
    "Page",
    "ParseResponse",
    "Response",
    "TextPage",
    "TextResponse",
    "Usage",
    "Word",
]
