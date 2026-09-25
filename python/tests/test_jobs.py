"""Jobs API (submit / retrieve / handle_webhook) against a scripted local HTTP server."""

from __future__ import annotations

import asyncio
import json
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

import puffinparse
import pytest
from conftest import FIXTURES


class _Script:
    """Answers requests in order with (status, body) and records (method, path, headers, body)."""

    def __init__(self) -> None:
        self.responses: list[tuple[int, str]] = []
        self.seen: list[tuple[str, str, dict[str, str], bytes]] = []
        self.lock = threading.Lock()


@pytest.fixture()
def server() -> Iterator[tuple[str, _Script]]:
    script = _Script()

    class Handler(BaseHTTPRequestHandler):
        def _answer(self) -> None:
            length = int(self.headers.get("content-length") or 0)
            body = self.rfile.read(length) if length else b""
            if self.headers.get("transfer-encoding", "").lower() == "chunked":
                body = self._read_chunked()
            with script.lock:
                script.seen.append(
                    (self.command, self.path, {k.lower(): v for k, v in self.headers.items()}, body)
                )
                status, text = (
                    script.responses.pop(0) if script.responses else (500, '{"detail":"no script"}')
                )
            payload = text.encode()
            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def _read_chunked(self) -> bytes:
            out = b""
            while True:
                size = int(self.rfile.readline().strip() or b"0", 16)
                if size == 0:
                    self.rfile.readline()
                    return out
                out += self.rfile.read(size)
                self.rfile.readline()

        do_GET = _answer
        do_POST = _answer

        def log_message(self, format: str, *args: Any) -> None:
            pass

    httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{httpd.server_address[1]}", script
    finally:
        httpd.shutdown()


def _fixture(name: str) -> str:
    return (FIXTURES / name).read_text()


def _job(status: str, result: Any = None) -> tuple[int, str]:
    body: dict[str, Any] = {"status": status}
    if result is not None:
        body["result"] = result
    return 200, json.dumps(body)


def test_submit_then_retrieve(server: tuple[str, _Script]) -> None:
    base, script = server
    script.responses = [
        (200, json.dumps({"file_id": "reducto://abc.pdf"})),
        (200, json.dumps({"job_id": "job-42"})),
        _job("Pending"),
        _job("Completed", json.loads(_fixture("reducto_parse.json"))),
    ]
    job = puffinparse.submit(
        b"%PDF-1.4 test",
        model="reducto/standard",
        filename="doc.pdf",
        webhook_url="https://hooks.example.com/x",
        api_key="test-key",
        base_url=base,
        metadata={"run": "r1"},
    )
    assert isinstance(job, puffinparse.Job)
    assert (job.provider, job.model, job.job_id) == ("reducto", "reducto/standard", "job-42")
    assert job.base_url == base
    assert "test-key" not in json.dumps(job.to_dict()), "a job never holds the key"
    assert puffinparse.Job.from_dict(job.to_dict()) == job

    pending = puffinparse.retrieve(job, api_key="test-key")
    assert pending is job
    done = puffinparse.retrieve(job, api_key="test-key")
    assert isinstance(done, puffinparse.ParseResponse)
    assert done.model == "reducto/standard"
    assert done.markdown.startswith("# Hello LiteOCR")
    assert done.metadata["run"] == "r1"

    method, path, _, body = script.seen[1]
    assert (method, path) == ("POST", "/parse_async")
    assert json.loads(body)["async"]["webhook"] == {"mode": "direct", "url": "https://hooks.example.com/x"}
    assert script.seen[2][:2] == ("GET", "/job/job-42")
    assert script.seen[3][2]["authorization"] == "Bearer test-key"


def test_failed_job_raises_with_job_id(server: tuple[str, _Script]) -> None:
    base, script = server
    script.responses = [(200, json.dumps({"status": "Failed", "reason": "Password-protected document"}))]
    job = puffinparse.Job(
        provider="reducto", model="reducto/standard", job_id="job-9", submitted_at="", base_url=base
    )
    with pytest.raises(puffinparse.ProviderError) as info:
        puffinparse.retrieve(job, api_key="k")
    assert "Password-protected document" in info.value.message
    assert info.value.job_id == "job-9"


def test_async_variants(server: tuple[str, _Script]) -> None:
    base, script = server
    script.responses = [
        (200, json.dumps({"id": "0b6f-job", "status": "PENDING"})),
        (200, json.dumps({"id": "0b6f-job", "status": "SUCCESS"})),
        (200, _fixture("llamaparse_result_json.json")),
    ]

    async def run() -> Any:
        job = await puffinparse.asubmit(
            b"%PDF-1.4 test", model="llamaparse/fast", filename="doc.pdf", api_key="k", base_url=base
        )
        return await puffinparse.aretrieve(job, api_key="k")

    resp = asyncio.run(run())
    assert isinstance(resp, puffinparse.ParseResponse)
    assert resp.num_pages == 2
    assert resp.provider_job_id == "0b6f-job"


def test_extend_rejects_a_webhook_url() -> None:
    with pytest.raises(puffinparse.InputError, match="webhook"):
        puffinparse.submit(
            b"%PDF-1.4",
            model="extend/parse_light",
            filename="a.pdf",
            api_key="k",
            webhook_url="https://hooks.example.com/x",
        )


def test_providers_without_jobs() -> None:
    with pytest.raises(puffinparse.UnsupportedModelError):
        puffinparse.submit(b"%PDF-1.4", model="gemini/2.5-flash", filename="a.pdf", api_key="k")


def test_handle_webhook_result_push() -> None:
    push = {
        "txt": "Hello",
        "md": "# Hello",
        "json": [{"page": 1, "text": "Hello", "md": "# Hello"}],
        "images": [],
    }
    resp = puffinparse.handle_webhook(push, model="llamaparse/agentic")
    assert isinstance(resp, puffinparse.ParseResponse)
    assert resp.model == "llamaparse/agentic"
    assert resp.markdown == "# Hello"


def test_handle_webhook_events(server: tuple[str, _Script]) -> None:
    base, script = server
    pending = puffinparse.handle_webhook(
        {"event_type": "parse.pending", "data": {"job_id": "j1"}}, model="llamaparse"
    )
    assert isinstance(pending, puffinparse.Job)
    assert pending.job_id == "j1"

    failed = {
        "eventId": "evt_1",
        "eventType": "parse_run.failed",
        "payload": {
            "object": "parse_run_status",
            "id": "pr_9",
            "status": "FAILED",
            "failureReason": "OUT_OF_CREDITS",
            "failureMessage": "No credits left.",
        },
    }
    with pytest.raises(puffinparse.AuthenticationError, match="No credits left"):
        puffinparse.handle_webhook(failed, model="extend")

    script.responses = [_job("Completed", json.loads(_fixture("reducto_parse.json")))]
    done = puffinparse.handle_webhook(
        {"status": "Completed", "job_id": "job-7"}, model="reducto", api_key="k", base_url=base
    )
    assert isinstance(done, puffinparse.ParseResponse)
    assert script.seen[0][:2] == ("GET", "/job/job-7")

    with pytest.raises(puffinparse.InputError):
        puffinparse.handle_webhook({"unexpected": True}, model="reducto")
