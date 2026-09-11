"""LiteOCR: one API for every OCR / document-parsing provider.

>>> import liteocr
>>> resp = liteocr.ocr("invoice.pdf", model="reducto/standard")
>>> print(resp.markdown, resp.usage.pages, resp.cost_usd)
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
    aocr,
    estimate_cost,
    failure_callback,
    init_logging,
    list_models,
    markdown_to_text,
    normalize_text,
    ocr,
    pricing,
    providers,
    reset_pricing,
    resolve_model,
    score,
    set_pricing,
    success_callback,
)
from .types import BBox, Block, BlockType, Metrics, OcrResponse, Page, Usage

__version__: str = str(_core.__version__)

__all__ = [
    "AuthenticationError",
    "BBox",
    "BadRequestError",
    "Block",
    "BlockType",
    "InputError",
    "LiteOCRError",
    "Metrics",
    "NetworkError",
    "OcrResponse",
    "Page",
    "ProviderError",
    "RateLimitError",
    "Router",
    "TimeoutError",
    "UnsupportedModelError",
    "Usage",
    "__version__",
    "aocr",
    "estimate_cost",
    "failure_callback",
    "init_logging",
    "list_models",
    "markdown_to_text",
    "normalize_text",
    "ocr",
    "pricing",
    "providers",
    "reset_pricing",
    "resolve_model",
    "score",
    "set_pricing",
    "success_callback",
]
