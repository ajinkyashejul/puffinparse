"""LiteOCR: one API for every OCR / document-parsing provider.

Three modes, one shape each — switch providers by changing one string:

>>> import liteocr
>>> doc = liteocr.parse("invoice.pdf", model="reducto/standard")    # markdown + blocks
>>> print(doc.markdown, doc.usage.pages, doc.cost_usd)
>>> text = liteocr.ocr("scan.png", model="llamaparse/fast")         # plain text + boxes
>>> print(text.text, len(text.pages[0].lines))
>>> data = liteocr.extract("invoice.pdf", schema, model="...")      # JSON from a schema
"""

from __future__ import annotations

from . import _core
from .exceptions import (
    AuthenticationError,
    BadRequestError,
    InputError,
    LiteOCRError,
    NetworkError,
    ProviderError,
    RateLimitError,
    TimeoutError,
    UnsupportedModelError,
)
from .main import (
    Router,
    aextract,
    aocr,
    aparse,
    estimate_cost,
    extract,
    failure_callback,
    init_logging,
    list_models,
    markdown_to_text,
    modes,
    normalize_text,
    ocr,
    output_formats,
    parse,
    pricing,
    providers,
    reset_pricing,
    resolve_model,
    score,
    set_pricing,
    success_callback,
)
from .types import (
    MODES,
    BBox,
    Block,
    BlockType,
    Citation,
    ExtractResponse,
    FieldInfo,
    Line,
    Metrics,
    Mode,
    Page,
    ParseResponse,
    Response,
    TextPage,
    TextResponse,
    Usage,
    Word,
)

__version__: str = str(_core.__version__)

__all__ = [
    "MODES",
    "AuthenticationError",
    "BBox",
    "BadRequestError",
    "Block",
    "BlockType",
    "Citation",
    "ExtractResponse",
    "FieldInfo",
    "InputError",
    "Line",
    "LiteOCRError",
    "Metrics",
    "Mode",
    "NetworkError",
    "Page",
    "ParseResponse",
    "ProviderError",
    "RateLimitError",
    "Response",
    "Router",
    "TextPage",
    "TextResponse",
    "TimeoutError",
    "UnsupportedModelError",
    "Usage",
    "Word",
    "__version__",
    "aextract",
    "aocr",
    "aparse",
    "estimate_cost",
    "extract",
    "failure_callback",
    "init_logging",
    "list_models",
    "markdown_to_text",
    "modes",
    "normalize_text",
    "ocr",
    "output_formats",
    "parse",
    "pricing",
    "providers",
    "reset_pricing",
    "resolve_model",
    "score",
    "set_pricing",
    "success_callback",
]
