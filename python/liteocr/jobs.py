"""Asynchronous jobs: submit a ``parse`` now, collect the result later.

:func:`liteocr.parse` waits for the provider (polling job-queue providers for you). For long
documents, large batches or webhook-driven pipelines, split the call in two instead:

>>> job = liteocr.submit("200-pages.pdf", model="reducto/standard",
...                      webhook_url="https://example.com/hooks/liteocr")
>>> store(job.to_dict())                       # a Job never contains an API key
>>> result = liteocr.retrieve(job)             # Job (still running) or ParseResponse
>>> # ... or, in the webhook handler:
>>> result = liteocr.handle_webhook(request.json(), model="reducto")

Supported by the providers with a job queue: ``reducto``, ``extend``, ``llamaparse``. A failed
job raises the same typed exceptions as :func:`liteocr.parse`.
"""

from __future__ import annotations

import json
from typing import Any, Literal, Optional, Union

from . import _core
from .main import (
    DocumentLike,
    _afail,
    _build_request,
    _fail,
    _finish,
    _run_callbacks_async,
    success_callback,
)
from .types import Job, ParseResponse

#: What :func:`retrieve` and :func:`handle_webhook` return: the job while it is still running,
#: the unified response once it is done.
JobResult = Union[Job, ParseResponse]


def _submit_request(
    input: DocumentLike,
    model: str,
    webhook_url: Optional[str],
    kwargs: dict[str, Any],
) -> tuple[dict[str, Any], Optional[bytes]]:
    req, data = _build_request(input, model, **kwargs)
    if webhook_url is not None:
        req["webhook_url"] = webhook_url
    return req, data


def _options(
    api_key: Optional[str], base_url: Optional[str], timeout: float, max_retries: int
) -> dict[str, Any]:
    opts: dict[str, Any] = {"timeout_secs": float(timeout), "max_retries": int(max_retries)}
    if api_key is not None:
        opts["api_key"] = api_key
    if base_url is not None:
        opts["base_url"] = base_url
    return opts


def _job_result(job: Job, status: dict[str, Any]) -> JobResult:
    if status.get("status") == "succeeded":
        return ParseResponse.from_dict(status["result"])
    return job


def submit(
    input: DocumentLike,
    model: str = "reducto",
    *,
    webhook_url: Optional[str] = None,
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
) -> Job:
    """Upload a document and start a ``parse`` job without waiting for it.

    Takes the same arguments as :func:`liteocr.parse` (``timeout`` covers the upload and the
    submission only), plus:

    Args:
        webhook_url: Ask the provider to POST to this URL when the job finishes — Reducto
            ``async.webhook`` (direct mode), LlamaParse ``webhook_url``. Extend has no per-job
            webhook (register an endpoint in Extend instead); passing one raises
            :class:`~liteocr.InputError`.

    Returns:
        A :class:`~liteocr.types.Job` to pass to :func:`retrieve`.
    """
    req, data = _submit_request(
        input,
        model,
        webhook_url,
        {
            "output": output,
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
        return Job.from_dict(_core.submit(req, data))
    except _core.CoreError as e:
        raise _fail(e, mode="parse") from None


async def asubmit(
    input: DocumentLike,
    model: str = "reducto",
    *,
    webhook_url: Optional[str] = None,
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
) -> Job:
    """Async version of :func:`submit`."""
    req, data = _submit_request(
        input,
        model,
        webhook_url,
        {
            "output": output,
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
        return Job.from_dict(await _core.asubmit(req, data))
    except _core.CoreError as e:
        raise await _afail(e, mode="parse") from None


def retrieve(
    job: Job,
    *,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    timeout: float = 120.0,
    max_retries: int = 2,
) -> JobResult:
    """Check a submitted job once.

    Returns the same :class:`~liteocr.types.Job` while the provider is still working, or the
    :class:`~liteocr.types.ParseResponse` (normalised exactly like :func:`liteocr.parse`) once it
    is done. A job the provider reports as failed raises the matching typed exception, with
    ``job_id`` set. Credentials come from the environment unless ``api_key`` is given; the base
    URL defaults to the one used at submit time.
    """
    try:
        status = _core.retrieve(job.to_dict(), _options(api_key, base_url, timeout, max_retries))
    except _core.CoreError as e:
        raise _fail(e, mode="parse") from None
    result = _job_result(job, status)
    return _finish(result) if isinstance(result, ParseResponse) else result


async def aretrieve(
    job: Job,
    *,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    timeout: float = 120.0,
    max_retries: int = 2,
) -> JobResult:
    """Async version of :func:`retrieve`."""
    try:
        status = await _core.aretrieve(job.to_dict(), _options(api_key, base_url, timeout, max_retries))
    except _core.CoreError as e:
        raise await _afail(e, mode="parse") from None
    result = _job_result(job, status)
    if isinstance(result, ParseResponse):
        await _run_callbacks_async(success_callback, result)
    return result


def _webhook_event(payload: dict[str, Any], model: str) -> tuple[Optional[Job], dict[str, Any]]:
    """Parse a webhook body with the core; raise a typed error on an unreadable body."""
    event = _core.parse_webhook(model, payload)
    job = Job.from_dict(event["job"]) if event.get("job") else None
    return job, event["status"]


def handle_webhook(
    payload: dict[str, Any],
    model: str = "reducto",
    *,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    timeout: float = 120.0,
    max_retries: int = 2,
) -> JobResult:
    """Turn the JSON body a provider POSTed to your webhook into a :class:`Job` or a result.

    ``model`` names the provider (``"reducto"``, ``"extend"``, ``"llamaparse"``) or one of its
    models. Bodies that carry the whole result (LlamaParse's ``webhook_url`` push) are normalised
    directly; bodies that only name a finished job (Reducto, Extend, LlamaCloud events) trigger
    one :func:`retrieve`. A body reporting a failure raises the matching typed exception. Verify
    the provider's signature (Extend, LlamaCloud) or your own secret *before* calling this.
    """
    try:
        job, status = _webhook_event(payload, model)
    except _core.CoreError as e:
        raise _fail(e, mode="parse") from None
    state = status.get("status")
    if state == "failed":
        raise _fail(_core.CoreError(json.dumps(status["result"])), mode="parse")
    if state == "succeeded":
        return _finish(ParseResponse.from_dict(status["result"]))
    if job is None:
        raise _fail(_core.CoreError(json.dumps(_no_job_error())), mode="parse")
    if state == "finished":
        return retrieve(job, api_key=api_key, base_url=base_url, timeout=timeout, max_retries=max_retries)
    return job


async def ahandle_webhook(
    payload: dict[str, Any],
    model: str = "reducto",
    *,
    api_key: Optional[str] = None,
    base_url: Optional[str] = None,
    timeout: float = 120.0,
    max_retries: int = 2,
) -> JobResult:
    """Async version of :func:`handle_webhook`."""
    try:
        job, status = _webhook_event(payload, model)
    except _core.CoreError as e:
        raise await _afail(e, mode="parse") from None
    state = status.get("status")
    if state == "failed":
        raise await _afail(_core.CoreError(json.dumps(status["result"])), mode="parse")
    if state == "succeeded":
        resp = ParseResponse.from_dict(status["result"])
        await _run_callbacks_async(success_callback, resp)
        return resp
    if job is None:
        raise await _afail(_core.CoreError(json.dumps(_no_job_error())), mode="parse")
    if state == "finished":
        return await aretrieve(
            job, api_key=api_key, base_url=base_url, timeout=timeout, max_retries=max_retries
        )
    return job


def _no_job_error() -> dict[str, Any]:
    return {
        "kind": "input",
        "message": "webhook payload names no job id",
        "provider": None,
        "status_code": None,
        "job_id": None,
        "retryable": False,
    }


__all__ = [
    "JobResult",
    "ahandle_webhook",
    "aretrieve",
    "asubmit",
    "handle_webhook",
    "retrieve",
    "submit",
]
