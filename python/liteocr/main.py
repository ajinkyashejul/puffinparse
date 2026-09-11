"""Public API: the three modes (:func:`parse`, :func:`ocr`, :func:`extract`), :class:`Router`,
callbacks and pricing helpers.

A call always names a **mode**, and the mode decides the response type:

- :func:`parse` → :class:`~liteocr.types.ParseResponse` — layout-aware markdown + typed blocks.
- :func:`ocr` → :class:`~liteocr.types.TextResponse` — plain text + line/word boxes.
- :func:`extract` → :class:`~liteocr.types.ExtractResponse` — a JSON object from your schema.

Providers can only be swapped **within** a mode: a model declares the modes it serves, and a
model that cannot serve the mode you asked for raises :class:`~liteocr.UnsupportedModelError`
before any network call.
"""

from __future__ import annotations

import asyncio
import inspect
import logging
import os
from collections.abc import Awaitable
from pathlib import Path
from typing import Any, Callable, Literal, Optional, TypeVar, Union

from . import _core
from .exceptions import InputError, LiteOCRError, UnsupportedModelError, from_core
from .types import ExtractResponse, Metrics, Mode, ParseResponse, Response, TextResponse

logger = logging.getLogger("liteocr")

DocumentLike = Union[str, "os.PathLike[str]", bytes, bytearray, memoryview]
SuccessCallback = Callable[[Response], Union[None, Awaitable[None]]]
FailureCallback = Callable[[LiteOCRError], Union[None, Awaitable[None]]]

#: Called after every successful call, in any mode, with the response.
success_callback: list[SuccessCallback] = []
#: Called after every failed call, in any mode, with the :class:`LiteOCRError`.
failure_callback: list[FailureCallback] = []

_R = TypeVar("_R", ParseResponse, TextResponse, ExtractResponse)

#: Keyword arguments every mode accepts, with their defaults.
_COMMON_KWARGS: dict[str, Any] = {
    "filename": None,
    "pages": None,
    "language": None,
    "provider_options": None,
    "include_raw": False,
    "timeout": 300.0,
    "max_retries": 2,
    "api_key": None,
    "base_url": None,
    "metadata": None,
}


def _build_request(
    input: DocumentLike,
    model: str,
    *,
    output: Literal["markdown", "text"] = "markdown",
    filename: Optional[str] = None,
    pages: Optional[str] = None,
    language: Optional[str] = None,
    provider_options: Optional[dict[str, Any]] = None,
    include_raw: bool = False,
    timeout: float = 300.0,
    max_retries: int = 2,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    metadata: Optional[dict[str, Any]] = None,
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


def _build_extract_request(
    input: DocumentLike,
    model: str,
    schema: dict[str, Any],
    instructions: Optional[str],
    citations: bool,
    kwargs: dict[str, Any],
) -> tuple[dict[str, Any], Optional[bytes]]:
    if not isinstance(schema, dict):
        raise TypeError(
            "extract(schema=...) must be a dict holding a JSON Schema object, "
            f"got {type(schema).__name__}. Example: {{'type': 'object', 'properties': "
            "{'total': {'type': 'number'}}}"
        )
    req, data = _build_request(input, model, **kwargs)
    req["schema"] = schema
    req["citations"] = bool(citations)
    if instructions is not None:
        req["instructions"] = instructions
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


def _finish(resp: _R) -> _R:
    """Fire success callbacks for a response of any mode."""
    _run_callbacks_sync(success_callback, resp)
    return resp


def _fail(exc: BaseException, *, mode: Mode = "parse") -> LiteOCRError:
    err = _convert(exc, mode)
    _run_callbacks_sync(failure_callback, err)
    return err


async def _afail(exc: BaseException, *, mode: Mode = "parse") -> LiteOCRError:
    err = _convert(exc, mode)
    await _run_callbacks_async(failure_callback, err)
    return err


def _convert(exc: BaseException, mode: Mode) -> LiteOCRError:
    """Turn a core error into a typed exception, adding a mode hint when the model is wrong."""
    err = from_core(exc)
    if isinstance(err, UnsupportedModelError) and mode != "parse" and f"'{mode}'" in err.message:
        try:
            available = list_models(mode)
        except LiteOCRError:  # pragma: no cover - the registry is static
            available = []
        hint = (
            ", ".join(available)
            if available
            else (
                f"none yet - no provider in this build implements '{mode}' "
                f"(liteocr.list_models({mode!r}) is empty)"
            )
        )
        return UnsupportedModelError(
            f"{_trim_empty_list(err.message, mode)}. Models for mode '{mode}': {hint}",
            provider=err.provider,
            status_code=err.status_code,
            job_id=err.job_id,
            retryable=err.retryable,
        )
    return err


def _trim_empty_list(message: str, mode: Mode) -> str:
    """Drop the core's dangling "Models for '<mode>' from <provider>:" when that list is empty."""
    marker = f"Models for '{mode}' from "
    at = message.find(marker)
    if at != -1 and message[at:].rstrip().endswith(":"):
        return message[:at].rstrip().rstrip(".")
    return message


# ---- parse mode ----------------------------------------------------------------------------------


def parse(
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
) -> ParseResponse:
    """Parse a document into markdown + typed blocks (``parse`` mode).

    Args:
        input: A file path, an ``http(s)://`` URL, or raw ``bytes`` (then pass ``filename``).
        model: ``"<provider>/<model>"``, e.g. ``"reducto/standard"``, ``"extend/parse_performance"``,
            ``"llamaparse/agentic"``. A bare provider name selects its default parse model.
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

    Returns:
        A :class:`~liteocr.types.ParseResponse`, identical in shape across providers.
    """
    req, data = _build_request(
        input,
        model,
        output=output,
        filename=filename,
        pages=pages,
        language=language,
        provider_options=provider_options,
        include_raw=include_raw,
        timeout=timeout,
        max_retries=max_retries,
        api_key=api_key,
        base_url=base_url,
        metadata=metadata,
    )
    try:
        resp_dict = _core.parse(req, data)
    except _core.CoreError as e:
        raise _fail(e, mode="parse") from None
    return _finish(ParseResponse.from_dict(resp_dict))


async def aparse(
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
) -> ParseResponse:
    """Async version of :func:`parse`. Runs on the Rust runtime; never blocks the event loop."""
    req, data = _build_request(
        input,
        model,
        output=output,
        filename=filename,
        pages=pages,
        language=language,
        provider_options=provider_options,
        include_raw=include_raw,
        timeout=timeout,
        max_retries=max_retries,
        api_key=api_key,
        base_url=base_url,
        metadata=metadata,
    )
    try:
        resp_dict = await _core.aparse(req, data)
    except _core.CoreError as e:
        raise await _afail(e, mode="parse") from None
    resp = ParseResponse.from_dict(resp_dict)
    await _run_callbacks_async(success_callback, resp)
    return resp


# ---- ocr mode ------------------------------------------------------------------------------------


def ocr(
    input: DocumentLike,
    model: str = "reducto",
    *,
    filename: Optional[str] = None,
    pages: Optional[str] = None,
    language: Optional[str] = None,
    provider_options: Optional[dict[str, Any]] = None,
    include_raw: bool = False,
    timeout: float = 300.0,
    max_retries: int = 2,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    metadata: Optional[dict[str, Any]] = None,
) -> TextResponse:
    """Read a document as plain text with line/word geometry (``ocr`` mode).

    Use this when you want text and boxes rather than document structure: search indexes,
    redaction, overlays, or feeding a downstream model. For markdown, tables and block types,
    use :func:`parse` instead.

    Providers without a native OCR endpoint serve this mode from their parse output; the response
    then carries ``metadata["liteocr_derived_from"] == "parse"``.

    Args:
        input: A file path, an ``http(s)://`` URL, or raw ``bytes`` (then pass ``filename``).
        model: ``"<provider>/<model>"`` supporting ``ocr``; a bare provider name picks its default.
        filename: Required when ``input`` is bytes.
        pages: 1-based page selection such as ``"1-3,7"``.
        language: Language hint (ISO 639-1) when the provider supports it.
        provider_options: Provider-specific options merged verbatim into the provider request.
        include_raw: Attach the provider's raw payload as ``response.raw``.
        timeout: Whole-call deadline in seconds.
        max_retries: Retries on 429 / 5xx / network errors.
        api_key: Override the API key.
        base_url: Override the provider base URL.
        metadata: Free-form dict echoed back in ``response.metadata``.

    Returns:
        A :class:`~liteocr.types.TextResponse` with ``text``, ``pages[].lines`` and ``pages[].words``.
    """
    req, data = _build_request(
        input,
        model,
        output="text",
        filename=filename,
        pages=pages,
        language=language,
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
        raise _fail(e, mode="ocr") from None
    return _finish(TextResponse.from_dict(resp_dict))


async def aocr(
    input: DocumentLike,
    model: str = "reducto",
    *,
    filename: Optional[str] = None,
    pages: Optional[str] = None,
    language: Optional[str] = None,
    provider_options: Optional[dict[str, Any]] = None,
    include_raw: bool = False,
    timeout: float = 300.0,
    max_retries: int = 2,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    metadata: Optional[dict[str, Any]] = None,
) -> TextResponse:
    """Async version of :func:`ocr`."""
    req, data = _build_request(
        input,
        model,
        output="text",
        filename=filename,
        pages=pages,
        language=language,
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
        raise await _afail(e, mode="ocr") from None
    resp = TextResponse.from_dict(resp_dict)
    await _run_callbacks_async(success_callback, resp)
    return resp


# ---- extract mode --------------------------------------------------------------------------------


def extract(
    input: DocumentLike,
    schema: dict[str, Any],
    *,
    model: str = "reducto",
    instructions: Optional[str] = None,
    citations: bool = False,
    filename: Optional[str] = None,
    pages: Optional[str] = None,
    language: Optional[str] = None,
    provider_options: Optional[dict[str, Any]] = None,
    include_raw: bool = False,
    timeout: float = 300.0,
    max_retries: int = 2,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    metadata: Optional[dict[str, Any]] = None,
) -> ExtractResponse:
    """Pull a structured JSON object out of a document with a schema (``extract`` mode).

    Args:
        input: A file path, an ``http(s)://`` URL, or raw ``bytes`` (then pass ``filename``).
        schema: A JSON Schema **object** describing the fields you want.
        model: ``"<provider>/<model>"`` that supports ``extract``; a parse-only model raises
            :class:`~liteocr.UnsupportedModelError` before any network call. See
            ``liteocr.list_models("extract")``.
        instructions: Extra natural-language guidance, forwarded when the provider accepts it.
        citations: Ask for per-field citations (page, box, source text) where supported.
        filename: Required when ``input`` is bytes.
        pages: 1-based page selection such as ``"1-3,7"``.
        language: Language hint (ISO 639-1) when the provider supports it.
        provider_options: Provider-specific options merged verbatim into the provider request.
        include_raw: Attach the provider's raw payload as ``response.raw``.
        timeout: Whole-call deadline in seconds.
        max_retries: Retries on 429 / 5xx / network errors.
        api_key: Override the API key.
        base_url: Override the provider base URL.
        metadata: Free-form dict echoed back in ``response.metadata``.

    Returns:
        An :class:`~liteocr.types.ExtractResponse` whose ``data`` follows ``schema`` and whose
        ``fields`` maps JSON pointers to confidence and citations.
    """
    req, data = _build_extract_request(
        input,
        model,
        schema,
        instructions,
        citations,
        {
            "filename": filename,
            "pages": pages,
            "language": language,
            "provider_options": provider_options,
            "include_raw": include_raw,
            "timeout": timeout,
            "max_retries": max_retries,
            "api_key": api_key,
            "base_url": base_url,
            "metadata": metadata,
        },
    )
    try:
        resp_dict = _core.extract(req, data)
    except _core.CoreError as e:
        raise _fail(e, mode="extract") from None
    return _finish(ExtractResponse.from_dict(resp_dict))


async def aextract(
    input: DocumentLike,
    schema: dict[str, Any],
    *,
    model: str = "reducto",
    instructions: Optional[str] = None,
    citations: bool = False,
    filename: Optional[str] = None,
    pages: Optional[str] = None,
    language: Optional[str] = None,
    provider_options: Optional[dict[str, Any]] = None,
    include_raw: bool = False,
    timeout: float = 300.0,
    max_retries: int = 2,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    metadata: Optional[dict[str, Any]] = None,
) -> ExtractResponse:
    """Async version of :func:`extract`."""
    req, data = _build_extract_request(
        input,
        model,
        schema,
        instructions,
        citations,
        {
            "filename": filename,
            "pages": pages,
            "language": language,
            "provider_options": provider_options,
            "include_raw": include_raw,
            "timeout": timeout,
            "max_retries": max_retries,
            "api_key": api_key,
            "base_url": base_url,
            "metadata": metadata,
        },
    )
    try:
        resp_dict = await _core.aextract(req, data)
    except _core.CoreError as e:
        raise await _afail(e, mode="extract") from None
    resp = ExtractResponse.from_dict(resp_dict)
    await _run_callbacks_async(success_callback, resp)
    return resp


# ---- router --------------------------------------------------------------------------------------


class Router:
    """Route calls across several models with ordered fallbacks or round-robin.

    A router is bound to one **mode** at construction: every model must support it, and calling a
    method for another mode raises :class:`~liteocr.InputError`. That keeps fallbacks honest —
    a parse model can never quietly answer an extraction.

    Example::

        router = liteocr.Router(["reducto/standard", "llamaparse/agentic", "extend/parse_light"])
        resp = router.parse("contract.pdf")

        text_router = liteocr.Router(["reducto/r-1", "extend/parse_light"], mode="ocr")
        text = text_router.ocr("scan.png").text
    """

    def __init__(
        self,
        models: list[str],
        *,
        mode: Mode = "parse",
        strategy: Literal["ordered", "round_robin"] = "ordered",
        fallback_on: Optional[list[str]] = None,
    ) -> None:
        try:
            self._inner = _core.Router(list(models), mode, strategy, fallback_on)
        except _core.CoreError as e:
            raise _convert(e, mode) from None

    @property
    def models(self) -> list[str]:
        return list(self._inner.models())

    @property
    def mode(self) -> Mode:
        """The mode this router serves (``"parse"``, ``"ocr"`` or ``"extract"``)."""
        m: Mode = self._inner.mode()  # type: ignore[assignment]
        return m

    def plan(self) -> list[str]:
        """The order models would be tried for the next call (advances round-robin)."""
        return list(self._inner.plan())

    def stats(self) -> dict[str, dict[str, Any]]:
        return dict(self._inner.stats())

    def parse(self, input: DocumentLike, **kwargs: Any) -> ParseResponse:
        """Run a ``parse`` call across the configured models."""
        req, data = _build_request(input, "reducto", **_router_kwargs(kwargs, "parse"))
        try:
            resp_dict = self._inner.parse(req, data)
        except _core.CoreError as e:
            raise _fail(e, mode="parse") from None
        return _finish(ParseResponse.from_dict(resp_dict))

    async def aparse(self, input: DocumentLike, **kwargs: Any) -> ParseResponse:
        """Async version of :meth:`parse`."""
        req, data = _build_request(input, "reducto", **_router_kwargs(kwargs, "parse"))
        try:
            resp_dict = await self._inner.aparse(req, data)
        except _core.CoreError as e:
            raise await _afail(e, mode="parse") from None
        resp = ParseResponse.from_dict(resp_dict)
        await _run_callbacks_async(success_callback, resp)
        return resp

    def ocr(self, input: DocumentLike, **kwargs: Any) -> TextResponse:
        """Run an ``ocr`` call across the configured models."""
        req, data = _build_request(input, "reducto", **_router_kwargs(kwargs, "ocr"))
        try:
            resp_dict = self._inner.ocr(req, data)
        except _core.CoreError as e:
            raise _fail(e, mode="ocr") from None
        return _finish(TextResponse.from_dict(resp_dict))

    async def aocr(self, input: DocumentLike, **kwargs: Any) -> TextResponse:
        """Async version of :meth:`ocr`."""
        req, data = _build_request(input, "reducto", **_router_kwargs(kwargs, "ocr"))
        try:
            resp_dict = await self._inner.aocr(req, data)
        except _core.CoreError as e:
            raise await _afail(e, mode="ocr") from None
        resp = TextResponse.from_dict(resp_dict)
        await _run_callbacks_async(success_callback, resp)
        return resp

    def extract(
        self,
        input: DocumentLike,
        schema: dict[str, Any],
        *,
        instructions: Optional[str] = None,
        citations: bool = False,
        **kwargs: Any,
    ) -> ExtractResponse:
        """Run an ``extract`` call across the configured models."""
        req, data = _build_extract_request(
            input, "reducto", schema, instructions, citations, _router_kwargs(kwargs, "extract")
        )
        try:
            resp_dict = self._inner.extract(req, data)
        except _core.CoreError as e:
            raise _fail(e, mode="extract") from None
        return _finish(ExtractResponse.from_dict(resp_dict))

    async def aextract(
        self,
        input: DocumentLike,
        schema: dict[str, Any],
        *,
        instructions: Optional[str] = None,
        citations: bool = False,
        **kwargs: Any,
    ) -> ExtractResponse:
        """Async version of :meth:`extract`."""
        req, data = _build_extract_request(
            input, "reducto", schema, instructions, citations, _router_kwargs(kwargs, "extract")
        )
        try:
            resp_dict = await self._inner.aextract(req, data)
        except _core.CoreError as e:
            raise await _afail(e, mode="extract") from None
        resp = ExtractResponse.from_dict(resp_dict)
        await _run_callbacks_async(success_callback, resp)
        return resp

    def __repr__(self) -> str:
        return f"Router(models={self.models!r}, mode={self.mode!r})"


def _router_kwargs(kwargs: dict[str, Any], mode: Mode) -> dict[str, Any]:
    allowed = dict(_COMMON_KWARGS)
    if mode == "parse":
        allowed["output"] = "markdown"
    unknown = set(kwargs) - set(allowed)
    if unknown:
        raise TypeError(
            f"unexpected keyword argument(s) for {mode}: {', '.join(sorted(unknown))}"
            f" (accepted: {', '.join(sorted(allowed))})"
        )
    allowed.update(kwargs)
    if mode == "ocr":
        allowed["output"] = "text"
    return allowed


# ---- helpers ------------------------------------------------------------------------------------


def list_models(mode: Optional[Mode] = None) -> list[str]:
    """All supported ``"<provider>/<model>"`` strings, or only those serving ``mode``."""
    try:
        return list(_core.list_models(mode))
    except _core.CoreError as e:
        raise from_core(e) from None


def modes() -> list[str]:
    """The modes LiteOCR knows: ``["parse", "ocr", "extract"]``."""
    return list(_core.modes())


def providers() -> list[dict[str, Any]]:
    """Provider metadata: name, env var, base URL, docs, and models with the modes they serve."""
    return list(_core.providers())


def resolve_model(model: str, mode: Optional[Mode] = None) -> str:
    """Validate and canonicalise a model string (``"reducto"`` -> ``"reducto/standard"``).

    With ``mode``, the model must support it and a bare provider name resolves to that provider's
    default model *for that mode*.
    """
    try:
        return str(_core.resolve_model(model, mode))
    except _core.CoreError as e:
        raise _convert(e, mode or "parse") from None


def pricing() -> dict[str, dict[str, Any]]:
    """The active price table: model → ``{"parse": $/page, "ocr": ..., "extract": ..., "source", "updated"}``.

    Prices are per page, per mode; a mode a model is not priced in is simply absent.
    """
    return dict(_core.pricing())


def set_pricing(prices: dict[str, float], mode: Mode = "parse") -> None:
    """Override per-page USD prices for one mode, e.g. ``set_pricing({"reducto/standard": 0.012})``."""
    try:
        _core.set_pricing({k: float(v) for k, v in prices.items()}, mode)
    except _core.CoreError as e:
        raise from_core(e) from None


def reset_pricing() -> None:
    """Restore the embedded price table, discarding every :func:`set_pricing` override."""
    _core.reset_pricing()


def estimate_cost(model: str, pages: int, mode: Mode = "parse") -> Optional[float]:
    """Estimated USD cost for ``pages`` pages at ``model``'s list price in ``mode``.

    Returns ``None`` when the model has no published price for that mode.
    """
    return _core.estimate_cost(resolve_model(model), mode, int(pages))


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
    "parse",
    "pricing",
    "providers",
    "reset_pricing",
    "resolve_model",
    "score",
    "set_pricing",
    "success_callback",
]
