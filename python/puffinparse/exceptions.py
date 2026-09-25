"""Exception hierarchy. Every error raised by PuffinParse derives from :class:`PuffinParseError`."""

from __future__ import annotations

import json
from typing import Any, Optional


class PuffinParseError(Exception):
    """Base class for all PuffinParse errors."""

    kind: str = "error"

    def __init__(
        self,
        message: str,
        *,
        provider: Optional[str] = None,
        status_code: Optional[int] = None,
        job_id: Optional[str] = None,
        retryable: bool = False,
    ) -> None:
        super().__init__(message)
        self.message = message
        self.provider = provider
        self.status_code = status_code
        self.job_id = job_id
        self.retryable = retryable

    def __str__(self) -> str:
        parts = [self.kind]
        if self.provider:
            parts.append(f"[{self.provider}]")
        s = " ".join(parts) + f": {self.message}"
        if self.status_code is not None:
            s += f" (HTTP {self.status_code})"
        if self.job_id:
            s += f" [job {self.job_id}]"
        return s

    def to_dict(self) -> dict[str, Any]:
        return {
            "kind": self.kind,
            "message": self.message,
            "provider": self.provider,
            "status_code": self.status_code,
            "job_id": self.job_id,
            "retryable": self.retryable,
        }


class AuthenticationError(PuffinParseError):
    """401/403 from the provider, or no API key configured."""

    kind = "authentication_error"


class RateLimitError(PuffinParseError):
    """429 from the provider after retries were exhausted."""

    kind = "rate_limit_error"


class BadRequestError(PuffinParseError):
    """The provider rejected the request (4xx other than auth / rate limit)."""

    kind = "bad_request_error"


class ProviderError(PuffinParseError):
    """5xx, malformed provider payload, or a job that ended in a failed state."""

    kind = "provider_error"


class TimeoutError(PuffinParseError):
    """The overall deadline (upload + polling + download) was exceeded."""

    kind = "timeout_error"


class UnsupportedModelError(PuffinParseError):
    """Unknown provider or model string."""

    kind = "unsupported_model_error"


class InputError(PuffinParseError):
    """Unreadable input, bytes without a filename, empty body, ..."""

    kind = "input_error"


class NetworkError(PuffinParseError):
    """Network / TLS / DNS failure after retries."""

    kind = "network_error"


_KIND_TO_CLASS: dict[str, type[PuffinParseError]] = {
    "authentication": AuthenticationError,
    "rate_limit": RateLimitError,
    "bad_request": BadRequestError,
    "provider": ProviderError,
    "timeout": TimeoutError,
    "unsupported_model": UnsupportedModelError,
    "input": InputError,
    "network": NetworkError,
}


def from_core(exc: BaseException) -> PuffinParseError:
    """Convert a ``puffinparse._core.CoreError`` (JSON payload) into a typed exception."""
    payload: Any = exc.args[0] if exc.args else ""
    data: dict[str, Any]
    try:
        data = json.loads(payload) if isinstance(payload, str) else {}
    except ValueError:
        data = {}
    if not isinstance(data, dict) or "kind" not in data:
        return ProviderError(str(payload))
    cls = _KIND_TO_CLASS.get(str(data.get("kind")), ProviderError)
    return cls(
        str(data.get("message", "")),
        provider=data.get("provider"),
        status_code=data.get("status_code"),
        job_id=data.get("job_id"),
        retryable=bool(data.get("retryable", False)),
    )


__all__ = [
    "AuthenticationError",
    "BadRequestError",
    "InputError",
    "NetworkError",
    "ProviderError",
    "PuffinParseError",
    "RateLimitError",
    "TimeoutError",
    "UnsupportedModelError",
    "from_core",
]
