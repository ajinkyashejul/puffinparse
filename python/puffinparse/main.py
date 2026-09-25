"""Public API: the three modes (:func:`parse`, :func:`ocr`, :func:`extract`), :class:`Router`,
callbacks and pricing helpers.

A call always names a **mode**, and the mode decides the response type:

- :func:`parse` → :class:`~puffinparse.types.ParseResponse` — layout-aware markdown + typed blocks.
- :func:`ocr` → :class:`~puffinparse.types.TextResponse` — plain text + line/word boxes.
- :func:`extract` → :class:`~puffinparse.types.ExtractResponse` — a JSON object from your schema.

Providers can only be swapped **within** a mode: a model declares the modes it serves, and a
model that cannot serve the mode you asked for raises :class:`~puffinparse.UnsupportedModelError`
before any network call.
"""

from __future__ import annotations

import asyncio
import inspect
import logging
import os
from collections.abc import Awaitable
from pathlib import Path
from typing import Any, Callable, Literal, Optional, TypeVar, Union, overload

from . import _core
from .exceptions import BadRequestError, InputError, PuffinParseError, UnsupportedModelError, from_core
from .types import ExtractResponse, Metrics, Mode, ParseResponse, Response, TextResponse

logger = logging.getLogger("puffinparse")

DocumentLike = Union[str, "os.PathLike[str]", bytes, bytearray, memoryview]
SuccessCallback = Callable[[Response], Union[Awaitable[None], None]]
FailureCallback = Callable[[PuffinParseError], Union[Awaitable[None], None]]

#: Called after every successful call, in any mode, with the response.
success_callback: list[SuccessCallback] = []
#: Called after every failed call, in any mode, with the :class:`PuffinParseError`.
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
    output_format: Optional[str] = None,
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
    if output_format is not None:
        req["output_format"] = output_format
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
            logger.exception("puffinparse callback %r raised", cb)


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
            logger.exception("puffinparse callback %r raised", cb)


def _finish(resp: _R) -> _R:
    """Fire success callbacks for a response of any mode."""
    _run_callbacks_sync(success_callback, resp)
    return resp


def _fail(exc: BaseException, *, mode: Mode = "parse") -> PuffinParseError:
    err = _convert(exc, mode)
    _run_callbacks_sync(failure_callback, err)
    return err


async def _afail(exc: BaseException, *, mode: Mode = "parse") -> PuffinParseError:
    err = _convert(exc, mode)
    await _run_callbacks_async(failure_callback, err)
    return err


def _convert(exc: BaseException, mode: Mode) -> PuffinParseError:
    """Turn a core error into a typed exception, adding a mode hint when the model is wrong."""
    err = from_core(exc)
    if isinstance(err, UnsupportedModelError) and mode != "parse" and f"'{mode}'" in err.message:
        try:
            available = list_models(mode)
        except PuffinParseError:  # pragma: no cover - the registry is static
            available = []
        hint = (
            ", ".join(available)
            if available
            else (
                f"none yet - no provider in this build implements '{mode}' "
                f"(puffinparse.list_models({mode!r}) is empty)"
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


# ---- native-format compatibility -----------------------------------------------------------------


def output_formats() -> list[str]:
    """Every value ``output_format`` accepts: ``["puffinparse", "reducto", "extend", "llamaparse"]``."""
    return list(_core.output_formats())


def _validate_output_format(output_format: str) -> str:
    """Canonicalise ``output_format`` in the core, *before* any network call."""
    try:
        return str(_core.validate_output_format(output_format))
    except _core.CoreError as e:
        err = from_core(e)
        if isinstance(err, UnsupportedModelError):
            raise err from None
        raise BadRequestError(
            f"unknown output_format {output_format!r}: expected one of "
            f"{' | '.join(output_formats())}, or output_format=None for PuffinParse's unified response"
        ) from None


def _render(resp_dict: dict[str, Any], output_format: str, mode: Mode) -> dict[str, Any]:
    """Render a unified response dict in a vendor's own shape. All of it happens in Rust."""
    render = _core.render_extract if mode == "extract" else _core.render_parse
    try:
        rendered: dict[str, Any] = render(resp_dict, output_format)
    except _core.CoreError as e:
        raise _fail(e, mode=mode) from None
    return rendered


# ---- parse mode ----------------------------------------------------------------------------------


@overload
def parse(
    input: DocumentLike,
    model: str = ...,
    *,
    output_format: None = ...,
    filename: Optional[str] = ...,
    pages: Optional[str] = ...,
    language: Optional[str] = ...,
    output: Literal["markdown", "text"] = ...,
    provider_options: Optional[dict[str, Any]] = ...,
    include_raw: bool = ...,
    timeout: float = ...,
    max_retries: int = ...,
    api_key: Optional[str] = ...,
    base_url: Optional[str] = ...,
    metadata: Optional[dict[str, Any]] = ...,
) -> ParseResponse: ...


@overload
def parse(
    input: DocumentLike,
    model: str = ...,
    *,
    output_format: str,
    filename: Optional[str] = ...,
    pages: Optional[str] = ...,
    language: Optional[str] = ...,
    output: Literal["markdown", "text"] = ...,
    provider_options: Optional[dict[str, Any]] = ...,
    include_raw: bool = ...,
    timeout: float = ...,
    max_retries: int = ...,
    api_key: Optional[str] = ...,
    base_url: Optional[str] = ...,
    metadata: Optional[dict[str, Any]] = ...,
) -> dict[str, Any]: ...


def parse(
    input: DocumentLike,
    model: str = "reducto",
    *,
    output_format: Optional[str] = None,
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
) -> Union[ParseResponse, dict[str, Any]]:
    """Parse a document into markdown + typed blocks (``parse`` mode).

    Args:
        input: A file path, an ``http(s)://`` URL, or raw ``bytes`` (then pass ``filename``).
        model: ``"<provider>/<model>"``, e.g. ``"reducto/standard"``, ``"extend/parse_performance"``,
            ``"llamaparse/agentic"``. A bare provider name selects its default parse model.
        filename: Required when ``input`` is bytes; used to infer the document type.
        pages: 1-based page selection such as ``"1-3,7"`` (forwarded best-effort).
        language: Language hint (ISO 639-1) when the provider supports it.
        output: Preferred block content: ``"markdown"`` (default) or ``"text"``.
        output_format: Return the vendor's own JSON ``dict`` instead of a dataclass:
            ``"reducto"``, ``"extend"``, ``"llamaparse"`` (or ``"puffinparse"`` for the unified
            shape, which is also what ``None`` gives you). Whatever provider actually ran the
            call, the response is rendered into that vendor's response shape, so code already
            written against their SDK keeps parsing it. Structural fidelity is guaranteed, byte
            equality is not — ``docs/COMPAT.md`` lists the always-null fields and the lossy
            block-type mappings. Callbacks still receive the dataclass.
        provider_options: Provider-specific options merged verbatim into the provider request.
        include_raw: Attach the provider's raw payload as ``response.raw``.
        timeout: Whole-call deadline in seconds (upload + polling + download).
        max_retries: Retries on 429 / 5xx / network errors with exponential backoff.
        api_key: Override the API key (otherwise read from ``REDUCTO_API_KEY`` etc.).
        base_url: Override the provider base URL.
        metadata: Free-form dict echoed back in ``response.metadata``.

    Returns:
        A :class:`~puffinparse.types.ParseResponse`, identical in shape across providers — or, when
        ``output_format`` names a vendor, that vendor's own JSON as a ``dict``.

    Example:
        >>> doc = puffinparse.parse("invoice.pdf", model="extend/parse_light", output_format="reducto")
        >>> doc["result"]["chunks"][0]["blocks"][0]["bbox"]["left"]   # Reducto's shape, Extend's engine
    """
    if output_format is not None:
        output_format = _validate_output_format(output_format)
    req, data = _build_request(
        input,
        model,
        output=output,
        output_format=output_format,
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
    resp = _finish(ParseResponse.from_dict(resp_dict))
    return resp if output_format is None else _render(resp_dict, output_format, "parse")


@overload
async def aparse(
    input: DocumentLike,
    model: str = ...,
    *,
    output_format: None = ...,
    filename: Optional[str] = ...,
    pages: Optional[str] = ...,
    language: Optional[str] = ...,
    output: Literal["markdown", "text"] = ...,
    provider_options: Optional[dict[str, Any]] = ...,
    include_raw: bool = ...,
    timeout: float = ...,
    max_retries: int = ...,
    api_key: Optional[str] = ...,
    base_url: Optional[str] = ...,
    metadata: Optional[dict[str, Any]] = ...,
) -> ParseResponse: ...


@overload
async def aparse(
    input: DocumentLike,
    model: str = ...,
    *,
    output_format: str,
    filename: Optional[str] = ...,
    pages: Optional[str] = ...,
    language: Optional[str] = ...,
    output: Literal["markdown", "text"] = ...,
    provider_options: Optional[dict[str, Any]] = ...,
    include_raw: bool = ...,
    timeout: float = ...,
    max_retries: int = ...,
    api_key: Optional[str] = ...,
    base_url: Optional[str] = ...,
    metadata: Optional[dict[str, Any]] = ...,
) -> dict[str, Any]: ...


async def aparse(
    input: DocumentLike,
    model: str = "reducto",
    *,
    output_format: Optional[str] = None,
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
) -> Union[ParseResponse, dict[str, Any]]:
    """Async version of :func:`parse`, including ``output_format``."""
    if output_format is not None:
        output_format = _validate_output_format(output_format)
    req, data = _build_request(
        input,
        model,
        output=output,
        output_format=output_format,
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
    return resp if output_format is None else _render(resp_dict, output_format, "parse")


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
    then carries ``metadata["puffinparse_derived_from"] == "parse"``.

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
        A :class:`~puffinparse.types.TextResponse` with ``text``, ``pages[].lines`` and ``pages[].words``.
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


@overload
def extract(
    input: DocumentLike,
    schema: dict[str, Any],
    *,
    output_format: None = ...,
    model: str = ...,
    instructions: Optional[str] = ...,
    citations: bool = ...,
    filename: Optional[str] = ...,
    pages: Optional[str] = ...,
    language: Optional[str] = ...,
    provider_options: Optional[dict[str, Any]] = ...,
    include_raw: bool = ...,
    timeout: float = ...,
    max_retries: int = ...,
    api_key: Optional[str] = ...,
    base_url: Optional[str] = ...,
    metadata: Optional[dict[str, Any]] = ...,
) -> ExtractResponse: ...


@overload
def extract(
    input: DocumentLike,
    schema: dict[str, Any],
    *,
    output_format: str,
    model: str = ...,
    instructions: Optional[str] = ...,
    citations: bool = ...,
    filename: Optional[str] = ...,
    pages: Optional[str] = ...,
    language: Optional[str] = ...,
    provider_options: Optional[dict[str, Any]] = ...,
    include_raw: bool = ...,
    timeout: float = ...,
    max_retries: int = ...,
    api_key: Optional[str] = ...,
    base_url: Optional[str] = ...,
    metadata: Optional[dict[str, Any]] = ...,
) -> dict[str, Any]: ...


def extract(
    input: DocumentLike,
    schema: dict[str, Any],
    *,
    output_format: Optional[str] = None,
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
) -> Union[ExtractResponse, dict[str, Any]]:
    """Pull a structured JSON object out of a document with a schema (``extract`` mode).

    Args:
        input: A file path, an ``http(s)://`` URL, or raw ``bytes`` (then pass ``filename``).
        schema: A JSON Schema **object** describing the fields you want.
        model: ``"<provider>/<model>"`` that supports ``extract``; a parse-only model raises
            :class:`~puffinparse.UnsupportedModelError` before any network call. See
            ``puffinparse.list_models("extract")``.
        instructions: Extra natural-language guidance, forwarded when the provider accepts it.
        citations: Ask for per-field citations (page, box, source text) where supported.
        output_format: Return the vendor's own JSON ``dict`` instead of a dataclass:
            ``"reducto"``, ``"extend"``, ``"llamaparse"`` (or ``"puffinparse"`` for the unified
            shape, which is also what ``None`` gives you). Whatever provider actually ran the
            call, the response is rendered into that vendor's response shape, so code already
            written against their SDK keeps parsing it. Structural fidelity is guaranteed, byte
            equality is not — ``docs/COMPAT.md`` lists the always-null fields and the lossy
            block-type mappings. Callbacks still receive the dataclass.
            Extract-mode rendering is best effort (see ``docs/COMPAT.md`` §7).
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
        An :class:`~puffinparse.types.ExtractResponse` whose ``data`` follows ``schema`` and whose
        ``fields`` maps JSON pointers to confidence and citations — or, when ``output_format``
        names a vendor, that vendor's own extract JSON as a ``dict``.
    """
    if output_format is not None:
        output_format = _validate_output_format(output_format)
    req, data = _build_extract_request(
        input,
        model,
        schema,
        instructions,
        citations,
        {
            "output_format": output_format,
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
    resp = _finish(ExtractResponse.from_dict(resp_dict))
    return resp if output_format is None else _render(resp_dict, output_format, "extract")


@overload
async def aextract(
    input: DocumentLike,
    schema: dict[str, Any],
    *,
    output_format: None = ...,
    model: str = ...,
    instructions: Optional[str] = ...,
    citations: bool = ...,
    filename: Optional[str] = ...,
    pages: Optional[str] = ...,
    language: Optional[str] = ...,
    provider_options: Optional[dict[str, Any]] = ...,
    include_raw: bool = ...,
    timeout: float = ...,
    max_retries: int = ...,
    api_key: Optional[str] = ...,
    base_url: Optional[str] = ...,
    metadata: Optional[dict[str, Any]] = ...,
) -> ExtractResponse: ...


@overload
async def aextract(
    input: DocumentLike,
    schema: dict[str, Any],
    *,
    output_format: str,
    model: str = ...,
    instructions: Optional[str] = ...,
    citations: bool = ...,
    filename: Optional[str] = ...,
    pages: Optional[str] = ...,
    language: Optional[str] = ...,
    provider_options: Optional[dict[str, Any]] = ...,
    include_raw: bool = ...,
    timeout: float = ...,
    max_retries: int = ...,
    api_key: Optional[str] = ...,
    base_url: Optional[str] = ...,
    metadata: Optional[dict[str, Any]] = ...,
) -> dict[str, Any]: ...


async def aextract(
    input: DocumentLike,
    schema: dict[str, Any],
    *,
    output_format: Optional[str] = None,
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
) -> Union[ExtractResponse, dict[str, Any]]:
    """Async version of :func:`extract`, including ``output_format``."""
    if output_format is not None:
        output_format = _validate_output_format(output_format)
    req, data = _build_extract_request(
        input,
        model,
        schema,
        instructions,
        citations,
        {
            "output_format": output_format,
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
    return resp if output_format is None else _render(resp_dict, output_format, "extract")


# ---- router --------------------------------------------------------------------------------------


class Router:
    """Route calls across several models with ordered fallbacks or round-robin.

    A router is bound to one **mode** at construction: every model must support it, and calling a
    method for another mode raises :class:`~puffinparse.InputError`. That keeps fallbacks honest —
    a parse model can never quietly answer an extraction.

    Example::

        router = puffinparse.Router(["reducto/standard", "llamaparse/agentic", "extend/parse_light"])
        resp = router.parse("contract.pdf")

        text_router = puffinparse.Router(["reducto/r-1", "extend/parse_light"], mode="ocr")
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

    @overload
    def parse(self, input: DocumentLike, *, output_format: None = ..., **kwargs: Any) -> ParseResponse: ...

    @overload
    def parse(self, input: DocumentLike, *, output_format: str, **kwargs: Any) -> dict[str, Any]: ...

    def parse(
        self, input: DocumentLike, *, output_format: Optional[str] = None, **kwargs: Any
    ) -> Union[ParseResponse, dict[str, Any]]:
        """Run a ``parse`` call across the configured models.

        ``output_format`` renders the winning model's response in a vendor's own JSON shape,
        exactly as :func:`parse` does — the fallback chain is invisible to your parsing code.
        """
        if output_format is not None:
            output_format = _validate_output_format(output_format)
        kw = _router_kwargs(kwargs, "parse")
        kw["output_format"] = output_format
        req, data = _build_request(input, "reducto", **kw)
        try:
            resp_dict = self._inner.parse(req, data)
        except _core.CoreError as e:
            raise _fail(e, mode="parse") from None
        resp = _finish(ParseResponse.from_dict(resp_dict))
        return resp if output_format is None else _render(resp_dict, output_format, "parse")

    @overload
    async def aparse(
        self, input: DocumentLike, *, output_format: None = ..., **kwargs: Any
    ) -> ParseResponse: ...

    @overload
    async def aparse(self, input: DocumentLike, *, output_format: str, **kwargs: Any) -> dict[str, Any]: ...

    async def aparse(
        self, input: DocumentLike, *, output_format: Optional[str] = None, **kwargs: Any
    ) -> Union[ParseResponse, dict[str, Any]]:
        """Async version of :meth:`parse`."""
        if output_format is not None:
            output_format = _validate_output_format(output_format)
        kw = _router_kwargs(kwargs, "parse")
        kw["output_format"] = output_format
        req, data = _build_request(input, "reducto", **kw)
        try:
            resp_dict = await self._inner.aparse(req, data)
        except _core.CoreError as e:
            raise await _afail(e, mode="parse") from None
        resp = ParseResponse.from_dict(resp_dict)
        await _run_callbacks_async(success_callback, resp)
        return resp if output_format is None else _render(resp_dict, output_format, "parse")

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

    @overload
    def extract(
        self,
        input: DocumentLike,
        schema: dict[str, Any],
        *,
        output_format: None = ...,
        instructions: Optional[str] = ...,
        citations: bool = ...,
        **kwargs: Any,
    ) -> ExtractResponse: ...

    @overload
    def extract(
        self,
        input: DocumentLike,
        schema: dict[str, Any],
        *,
        output_format: str,
        instructions: Optional[str] = ...,
        citations: bool = ...,
        **kwargs: Any,
    ) -> dict[str, Any]: ...

    def extract(
        self,
        input: DocumentLike,
        schema: dict[str, Any],
        *,
        output_format: Optional[str] = None,
        instructions: Optional[str] = None,
        citations: bool = False,
        **kwargs: Any,
    ) -> Union[ExtractResponse, dict[str, Any]]:
        """Run an ``extract`` call across the configured models."""
        if output_format is not None:
            output_format = _validate_output_format(output_format)
        kw = _router_kwargs(kwargs, "extract")
        kw["output_format"] = output_format
        req, data = _build_extract_request(input, "reducto", schema, instructions, citations, kw)
        try:
            resp_dict = self._inner.extract(req, data)
        except _core.CoreError as e:
            raise _fail(e, mode="extract") from None
        resp = _finish(ExtractResponse.from_dict(resp_dict))
        return resp if output_format is None else _render(resp_dict, output_format, "extract")

    @overload
    async def aextract(
        self,
        input: DocumentLike,
        schema: dict[str, Any],
        *,
        output_format: None = ...,
        instructions: Optional[str] = ...,
        citations: bool = ...,
        **kwargs: Any,
    ) -> ExtractResponse: ...

    @overload
    async def aextract(
        self,
        input: DocumentLike,
        schema: dict[str, Any],
        *,
        output_format: str,
        instructions: Optional[str] = ...,
        citations: bool = ...,
        **kwargs: Any,
    ) -> dict[str, Any]: ...

    async def aextract(
        self,
        input: DocumentLike,
        schema: dict[str, Any],
        *,
        output_format: Optional[str] = None,
        instructions: Optional[str] = None,
        citations: bool = False,
        **kwargs: Any,
    ) -> Union[ExtractResponse, dict[str, Any]]:
        """Async version of :meth:`extract`."""
        if output_format is not None:
            output_format = _validate_output_format(output_format)
        kw = _router_kwargs(kwargs, "extract")
        kw["output_format"] = output_format
        req, data = _build_extract_request(input, "reducto", schema, instructions, citations, kw)
        try:
            resp_dict = await self._inner.aextract(req, data)
        except _core.CoreError as e:
            raise await _afail(e, mode="extract") from None
        resp = ExtractResponse.from_dict(resp_dict)
        await _run_callbacks_async(success_callback, resp)
        return resp if output_format is None else _render(resp_dict, output_format, "extract")

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
    """The modes PuffinParse knows: ``["parse", "ocr", "extract"]``."""
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
