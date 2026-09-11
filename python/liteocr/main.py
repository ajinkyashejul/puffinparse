"""Public API: :func:`ocr`, :func:`aocr`, :class:`Router`, callbacks, pricing helpers."""

from __future__ import annotations

import asyncio
import inspect
import logging
import os
from collections.abc import Awaitable
from pathlib import Path
from typing import Any, Callable, Literal, Optional, Union

from . import _core
from .exceptions import InputError, LiteOCRError, from_core
from .types import Metrics, OcrResponse

logger = logging.getLogger("liteocr")

DocumentLike = Union[str, "os.PathLike[str]", bytes, bytearray, memoryview]
SuccessCallback = Callable[[OcrResponse], Union[None, Awaitable[None]]]
FailureCallback = Callable[[LiteOCRError], Union[None, Awaitable[None]]]

#: Called after every successful call (sync or async) with the :class:`OcrResponse`.
success_callback: list[SuccessCallback] = []
#: Called after every failed call with the :class:`LiteOCRError`.
failure_callback: list[FailureCallback] = []


def _build_request(
    input: DocumentLike,
    model: str,
    *,
    filename: Optional[str],
    pages: Optional[str],
    language: Optional[str],
    output: Literal["markdown", "text"],
    provider_options: Optional[dict[str, Any]],
    include_raw: bool,
    timeout: float,
    max_retries: int,
    api_key: Optional[str],
    base_url: Optional[str],
    metadata: Optional[dict[str, Any]],
) -> tuple[dict[str, Any], Optional[bytes]]:
    data: Optional[bytes] = None
    if isinstance(input, (bytes, bytearray, memoryview)):
        if not filename:
            raise InputError("filename is required when passing bytes (used to infer the document type)")
        data = bytes(input)
        doc: dict[str, Any] = {"kind": "bytes", "data": "", "filename": filename}
    else:
        s = os.fspath(input) if isinstance(input, os.PathLike) else str(input)
        if s.startswith(("http://", "https://")):
            doc = {"kind": "url", "url": s}
        else:
            doc = {"kind": "path", "path": str(Path(s))}
    req: dict[str, Any] = {
        "input": doc,
        "model": model,
        "output": output,
        "include_raw": include_raw,
        "timeout_secs": float(timeout),
        "max_retries": int(max_retries),
        "metadata": metadata or {},
    }
    if pages is not None:
        req["pages"] = pages
    if language is not None:
        req["language"] = language
    if provider_options is not None:
        req["provider_options"] = provider_options
    if api_key is not None:
        req["api_key"] = api_key
    if base_url is not None:
        req["base_url"] = base_url
    return req, data


def _run_callbacks_sync(callbacks: list[Any], arg: Any) -> None:
    for cb in list(callbacks):
        try:
            result = cb(arg)
            if inspect.isawaitable(result):
                _run_awaitable(result)
        except Exception:
            logger.exception("liteocr callback %r raised", cb)


def _run_awaitable(aw: Awaitable[Any]) -> None:
    async def runner() -> None:
        await aw

    asyncio.run(runner())


async def _run_callbacks_async(callbacks: list[Any], arg: Any) -> None:
    for cb in list(callbacks):
        try:
            result = cb(arg)
            if inspect.isawaitable(result):
                await result
        except Exception:
            logger.exception("liteocr callback %r raised", cb)


def _finish(resp_dict: dict[str, Any]) -> OcrResponse:
    resp = OcrResponse.from_dict(resp_dict)
    _run_callbacks_sync(success_callback, resp)
    return resp


def _fail(exc: BaseException) -> LiteOCRError:
    err = from_core(exc)
    _run_callbacks_sync(failure_callback, err)
    return err


def ocr(
    input: DocumentLike,
    model: str = "reducto",
    *,
    filename: Optional[str] = None,
    pages: Optional[str] = None,
    language: Optional[str] = None,
    output: Literal["markdown", "text"] = "markdown",
    provider_options: Optional[dict[str, Any]] = None,
    include_raw: bool = False,
    timeout: float = 300.0,
    max_retries: int = 2,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    metadata: Optional[dict[str, Any]] = None,
) -> OcrResponse:
    """Parse a document with any provider and return a unified :class:`OcrResponse`.

    Args:
        input: A file path, an ``http(s)://`` URL, or raw ``bytes`` (then pass ``filename``).
        model: ``"<provider>/<model>"``, e.g. ``"reducto/standard"``, ``"extend/parse_performance"``,
            ``"llamaparse/agentic"``. A bare provider name selects its default model.
        filename: Required when ``input`` is bytes; used to infer the document type.
        pages: 1-based page selection such as ``"1-3,7"`` (forwarded best-effort).
        language: Language hint (ISO 639-1) when the provider supports it.
        output: Preferred block content: ``"markdown"`` (default) or ``"text"``.
        provider_options: Provider-specific options merged verbatim into the provider request.
        include_raw: Attach the provider's raw payload as ``response.raw``.
        timeout: Whole-call deadline in seconds (upload + polling + download).
        max_retries: Retries on 429 / 5xx / network errors with exponential backoff.
        api_key: Override the API key (otherwise read from ``REDUCTO_API_KEY`` etc.).
        base_url: Override the provider base URL.
        metadata: Free-form dict echoed back in ``response.metadata``.
    """
    req, data = _build_request(
        input,
        model,
        filename=filename,
        pages=pages,
        language=language,
        output=output,
        provider_options=provider_options,
        include_raw=include_raw,
        timeout=timeout,
        max_retries=max_retries,
        api_key=api_key,
        base_url=base_url,
        metadata=metadata,
    )
    try:
        resp_dict = _core.ocr(req, data)
    except _core.CoreError as e:
        raise _fail(e) from None
    return _finish(resp_dict)


async def aocr(
    input: DocumentLike,
    model: str = "reducto",
    *,
    filename: Optional[str] = None,
    pages: Optional[str] = None,
    language: Optional[str] = None,
    output: Literal["markdown", "text"] = "markdown",
    provider_options: Optional[dict[str, Any]] = None,
    include_raw: bool = False,
    timeout: float = 300.0,
    max_retries: int = 2,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    metadata: Optional[dict[str, Any]] = None,
) -> OcrResponse:
    """Async version of :func:`ocr`. Runs on the Rust runtime; never blocks the event loop."""
    req, data = _build_request(
        input,
        model,
        filename=filename,
        pages=pages,
        language=language,
        output=output,
        provider_options=provider_options,
        include_raw=include_raw,
        timeout=timeout,
        max_retries=max_retries,
        api_key=api_key,
        base_url=base_url,
        metadata=metadata,
    )
    try:
        resp_dict = await _core.aocr(req, data)
    except _core.CoreError as e:
        err = from_core(e)
        await _run_callbacks_async(failure_callback, err)
        raise err from None
    resp = OcrResponse.from_dict(resp_dict)
    await _run_callbacks_async(success_callback, resp)
    return resp


class Router:
    """Route calls across several models with ordered fallbacks or round-robin.

    Example::

        router = liteocr.Router(["reducto/standard", "llamaparse/agentic", "extend/parse_light"])
        resp = router.ocr("contract.pdf")
    """

    def __init__(
        self,
        models: list[str],
        *,
        strategy: Literal["ordered", "round_robin"] = "ordered",
        fallback_on: Optional[list[str]] = None,
    ) -> None:
        try:
            self._inner = _core.Router(list(models), strategy, fallback_on)
        except _core.CoreError as e:
            raise from_core(e) from None

    @property
    def models(self) -> list[str]:
        return list(self._inner.models())

    def plan(self) -> list[str]:
        """The order models would be tried for the next call (advances round-robin)."""
        return list(self._inner.plan())

    def stats(self) -> dict[str, dict[str, Any]]:
        return dict(self._inner.stats())

    def ocr(self, input: DocumentLike, **kwargs: Any) -> OcrResponse:
        req, data = _build_request(input, "reducto", **_router_kwargs(kwargs))
        try:
            resp_dict = self._inner.ocr(req, data)
        except _core.CoreError as e:
            raise _fail(e) from None
        return _finish(resp_dict)

    async def aocr(self, input: DocumentLike, **kwargs: Any) -> OcrResponse:
        req, data = _build_request(input, "reducto", **_router_kwargs(kwargs))
        try:
            resp_dict = await self._inner.aocr(req, data)
        except _core.CoreError as e:
            err = from_core(e)
            await _run_callbacks_async(failure_callback, err)
            raise err from None
        resp = OcrResponse.from_dict(resp_dict)
        await _run_callbacks_async(success_callback, resp)
        return resp

    def __repr__(self) -> str:
        return f"Router(models={self.models!r})"


def _router_kwargs(kwargs: dict[str, Any]) -> dict[str, Any]:
    allowed = {
        "filename": None,
        "pages": None,
        "language": None,
        "output": "markdown",
        "provider_options": None,
        "include_raw": False,
        "timeout": 300.0,
        "max_retries": 2,
        "api_key": None,
        "base_url": None,
        "metadata": None,
    }
    unknown = set(kwargs) - set(allowed)
    if unknown:
        raise TypeError(f"unexpected keyword argument(s): {', '.join(sorted(unknown))}")
    allowed.update(kwargs)
    return allowed


# ---- helpers ------------------------------------------------------------------------------------


def list_models() -> list[str]:
    """All supported ``"<provider>/<model>"`` strings."""
    return list(_core.list_models())


def providers() -> list[dict[str, Any]]:
    """Provider metadata: name, env var, base URL, docs, models."""
    return list(_core.providers())


def resolve_model(model: str) -> str:
    """Validate and canonicalise a model string (``"reducto"`` -> ``"reducto/standard"``)."""
    try:
        return str(_core.resolve_model(model))
    except _core.CoreError as e:
        raise from_core(e) from None


def pricing() -> dict[str, dict[str, Any]]:
    """The active per-page price table."""
    return dict(_core.pricing())


def set_pricing(prices: dict[str, float]) -> None:
    """Override per-page USD prices, e.g. ``set_pricing({"reducto/standard": 0.012})``."""
    _core.set_pricing({k: float(v) for k, v in prices.items()})


def reset_pricing() -> None:
    _core.reset_pricing()


def estimate_cost(model: str, pages: int) -> Optional[float]:
    """Estimated USD cost for ``pages`` pages at ``model``'s list price, or ``None`` if unknown."""
    return _core.estimate_cost(resolve_model(model), int(pages))


def score(
    prediction: str,
    truth: str,
    *,
    case_insensitive: bool = True,
    strip_markdown: bool = True,
    strip_punctuation: bool = False,
) -> Metrics:
    """Benchmark metrics (character similarity, CER, WER, word F1, ...) for a prediction."""
    return Metrics.from_dict(
        _core.score(
            prediction,
            truth,
            case_insensitive=case_insensitive,
            strip_markdown=strip_markdown,
            strip_punctuation=strip_punctuation,
        )
    )


def normalize_text(
    text: str,
    *,
    case_insensitive: bool = True,
    strip_markdown: bool = True,
    strip_punctuation: bool = False,
) -> str:
    """The normalisation applied before scoring (NFKC, markdown stripped, whitespace collapsed)."""
    return str(_core.normalize_text(text, case_insensitive, strip_markdown, strip_punctuation))


def markdown_to_text(markdown: str) -> str:
    return str(_core.markdown_to_text(markdown))


def init_logging(level: str = "info") -> None:
    """Enable the Rust core's tracing output on stderr (``"debug"`` shows every HTTP step)."""
    _core.init_logging(level)


__all__ = [
    "Router",
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
