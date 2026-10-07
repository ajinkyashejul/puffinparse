"""Unit tests that need no network."""

from __future__ import annotations

import puffinparse
import pytest

SCHEMA = {"type": "object", "properties": {"total": {"type": "number"}}}

#: Providers that serve layout parsing (and plain text derived from it), never extraction.
PARSE_ONLY_PROVIDERS = {"datalab", "unstructured", "upstage", "mathpix"}
PARSE_ONLY = {m for m in puffinparse.list_models() if m.split("/")[0] in PARSE_ONLY_PROVIDERS}


def test_version_and_models() -> None:
    assert puffinparse.__version__
    models = puffinparse.list_models()
    assert "reducto/standard" in models
    assert "extend/parse_performance" in models
    assert "llamaparse/agentic" in models
    assert all("/" in m for m in models)


def test_modes_and_models_per_mode() -> None:
    assert puffinparse.modes() == ["parse", "ocr", "extract"]
    assert list(puffinparse.MODES) == puffinparse.modes()
    every = set(puffinparse.list_models())
    for mode in puffinparse.modes():
        assert set(puffinparse.list_models(mode)) <= every
    # the original three providers do layout parsing and derive plain text from it ...
    assert set(puffinparse.list_models("parse")) >= PARSE_ONLY
    assert set(puffinparse.list_models("ocr")) >= PARSE_ONLY
    # ... and none of them does structured extraction
    assert PARSE_ONLY.isdisjoint(puffinparse.list_models("extract"))
    with pytest.raises(puffinparse.InputError):
        puffinparse.list_models("summarise")  # type: ignore[arg-type]


def test_resolve_model_defaults_and_aliases() -> None:
    assert puffinparse.resolve_model("reducto") == "reducto/standard"
    assert puffinparse.resolve_model("llama") == "llamaparse/cost_effective"
    assert puffinparse.resolve_model("Extend/Parse_Light") == "extend/parse_light"
    assert puffinparse.resolve_model("reducto", "ocr") == "reducto/standard"
    with pytest.raises(puffinparse.UnsupportedModelError):
        puffinparse.resolve_model("nope/x")
    with pytest.raises(puffinparse.UnsupportedModelError) as ei:
        puffinparse.resolve_model("reducto/standard", "extract")
    assert "extract" in str(ei.value)


def test_providers_metadata() -> None:
    provs = puffinparse.providers()
    names = {p["name"] for p in provs}
    assert names >= {"reducto", "extend", "llamaparse"}
    assert {p["name"] for p in provs if p["self_hosted"]} == {"tesseract", "docling", "paddleocr", "vllm"}
    for p in provs:
        if p["self_hosted"]:
            # Local engines need no key (docling and vllm name their optional one) and cost nothing.
            assert p["env_var"] in ("", "DOCLING_API_KEY", "VLLM_API_KEY")
            assert p["base_url"] == "" or p["base_url"].startswith("http://localhost")
            for m in p["models"]:
                for mode in m["modes"]:
                    assert puffinparse.estimate_cost(f"{p['name']}/{m['model']}", 10, mode) == 0.0
        else:
            assert p["env_var"].isupper() and any(t in p["env_var"] for t in ("KEY", "TOKEN"))
            assert p["base_url"].startswith("https://")
        assert any(m["default"] for m in p["models"])
        # every mode a provider serves is reachable from a bare provider name
        for mode in puffinparse.modes():
            if any(mode in m["modes"] for m in p["models"]):
                assert puffinparse.resolve_model(p["name"], mode).startswith(f"{p['name']}/")
        for m in p["models"]:
            assert m["modes"], f"{m['model']} declares no mode"
            assert set(m["modes"]) <= set(puffinparse.modes())
            if p["name"] in PARSE_ONLY_PROVIDERS:
                assert m["modes"] == ["parse", "ocr"]


def test_pricing_override_and_reset() -> None:
    try:
        assert puffinparse.estimate_cost("llamaparse/fast", 1000) == pytest.approx(1.25)
        assert puffinparse.estimate_cost("llamaparse/fast", 1000, "ocr") == pytest.approx(1.25)
        assert puffinparse.estimate_cost("llamaparse/fast", 1000, "extract") is None
        puffinparse.set_pricing({"llamaparse/fast": 0.5})
        assert puffinparse.pricing()["llamaparse/fast"]["parse"] == 0.5
        assert puffinparse.estimate_cost("llamaparse/fast", 2) == pytest.approx(1.0)
        # overriding one mode leaves the others alone
        assert puffinparse.estimate_cost("llamaparse/fast", 1000, "ocr") == pytest.approx(1.25)
        puffinparse.set_pricing({"llamaparse/fast": 0.25}, "ocr")
        assert puffinparse.estimate_cost("llamaparse/fast", 4, "ocr") == pytest.approx(1.0)
    finally:
        puffinparse.reset_pricing()
    assert puffinparse.estimate_cost("llamaparse/fast", 1000) == pytest.approx(1.25)
    assert puffinparse.pricing()["llamaparse/fast"]["source"].startswith("https://")


def test_unsupported_model_raises_before_network() -> None:
    with pytest.raises(puffinparse.UnsupportedModelError) as ei:
        puffinparse.parse("does-not-matter.pdf", model="acme/ocr")
    assert "acme" in str(ei.value)
    with pytest.raises(puffinparse.UnsupportedModelError):
        puffinparse.ocr("does-not-matter.pdf", model="acme/ocr")


def test_extract_rejects_a_parse_only_model() -> None:
    with pytest.raises(puffinparse.UnsupportedModelError) as ei:
        puffinparse.extract("does-not-matter.pdf", SCHEMA, model="reducto/standard")
    message = str(ei.value)
    assert "extract" in message
    assert "reducto/standard" in message
    # the message points at what *can* serve the mode
    assert "Models for mode 'extract'" in message


def test_extract_requires_a_schema_object() -> None:
    with pytest.raises(TypeError) as ei:
        puffinparse.extract("x.pdf", "total: number")  # type: ignore[arg-type]
    assert "JSON Schema" in str(ei.value)


def test_bytes_require_filename() -> None:
    for call in (puffinparse.parse, puffinparse.ocr):
        with pytest.raises(puffinparse.InputError):
            call(b"%PDF-1.4", model="reducto")
    with pytest.raises(puffinparse.InputError):
        puffinparse.extract(b"%PDF-1.4", SCHEMA, model="reducto")


def test_missing_file_is_input_error(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("EXTEND_API_KEY", "test-key")
    with pytest.raises(puffinparse.InputError) as ei:
        puffinparse.parse("/definitely/not/here.pdf", model="extend")
    assert ei.value.provider == "extend"


def test_missing_api_key_is_authentication_error(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:  # type: ignore[no-untyped-def]
    monkeypatch.delenv("REDUCTO_API_KEY", raising=False)
    f = tmp_path / "a.pdf"
    f.write_bytes(b"%PDF-1.4 minimal")
    for call in (puffinparse.parse, puffinparse.ocr):
        with pytest.raises(puffinparse.AuthenticationError) as ei:
            call(f, model="reducto")
        assert "REDUCTO_API_KEY" in str(ei.value)
        assert isinstance(ei.value, puffinparse.PuffinParseError)


def test_failure_callback_invoked_for_every_mode() -> None:
    seen: list[puffinparse.PuffinParseError] = []
    puffinparse.failure_callback.append(seen.append)
    try:
        with pytest.raises(puffinparse.UnsupportedModelError):
            puffinparse.parse("x.pdf", model="bogus")
        with pytest.raises(puffinparse.UnsupportedModelError):
            puffinparse.ocr("x.pdf", model="bogus")
        with pytest.raises(puffinparse.UnsupportedModelError):
            puffinparse.extract("x.pdf", SCHEMA, model="bogus")
    finally:
        puffinparse.failure_callback.clear()
    assert [e.kind for e in seen] == ["unsupported_model_error"] * 3


async def test_async_errors_are_typed() -> None:
    with pytest.raises(puffinparse.UnsupportedModelError):
        await puffinparse.aparse("x.pdf", model="bogus")
    with pytest.raises(puffinparse.UnsupportedModelError):
        await puffinparse.aocr("x.pdf", model="bogus")
    with pytest.raises(puffinparse.UnsupportedModelError):
        await puffinparse.aextract("x.pdf", SCHEMA, model="reducto/standard")


def test_router_plan_and_strategy() -> None:
    r = puffinparse.Router(["reducto", "extend/parse_light"], strategy="round_robin")
    assert r.models == ["reducto/standard", "extend/parse_light"]
    assert r.mode == "parse"
    assert r.plan() == ["reducto/standard", "extend/parse_light"]
    assert r.plan() == ["extend/parse_light", "reducto/standard"]
    with pytest.raises(puffinparse.UnsupportedModelError):
        puffinparse.Router(["nope"])
    with pytest.raises(puffinparse.InputError):
        puffinparse.Router([])
    with pytest.raises(TypeError):
        r.parse("x.pdf", bogus_kwarg=1)


def test_router_is_bound_to_one_mode() -> None:
    r = puffinparse.Router(["reducto", "extend/parse_light"], mode="parse")
    with pytest.raises(puffinparse.InputError) as ei:
        r.ocr("x.pdf")
    assert "mode 'parse'" in str(ei.value)
    with pytest.raises(puffinparse.InputError):
        r.extract("x.pdf", SCHEMA)

    text_router = puffinparse.Router(["reducto"], mode="ocr")
    assert text_router.mode == "ocr"
    with pytest.raises(puffinparse.InputError):
        text_router.parse("x.pdf")

    # a router only accepts models that serve its mode
    with pytest.raises(puffinparse.UnsupportedModelError):
        puffinparse.Router(["datalab"], mode="extract")
    assert puffinparse.Router(["reducto", "extend"], mode="extract").models == [
        "reducto/extract",
        "extend/extraction_performance",
    ]
    with pytest.raises(puffinparse.InputError):
        puffinparse.Router(["reducto"], mode="translate")  # type: ignore[arg-type]


def test_router_does_not_fall_back_on_auth_error(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:  # type: ignore[no-untyped-def]
    monkeypatch.delenv("REDUCTO_API_KEY", raising=False)
    f = tmp_path / "a.pdf"
    f.write_bytes(b"%PDF-1.4 minimal")
    r = puffinparse.Router(["reducto", "extend"])
    with pytest.raises(puffinparse.AuthenticationError):
        r.parse(f)
    stats = r.stats()
    assert stats["reducto/standard"]["failures"] == 1
    assert stats["extend/parse_performance"]["failures"] == 0


def test_score_and_normalize() -> None:
    m = puffinparse.score("# Hello **World**", "hello world")
    assert m.char_similarity == 1.0
    assert m.cer == 0.0 and m.wer == 0.0
    assert puffinparse.normalize_text("# Hello   **World**") == "hello world"
    assert puffinparse.markdown_to_text("| a | b |\n|---|---|\n| 1 | 2 |") == "a b\n1 2"
    partial = puffinparse.score("hello there world", "hello world")
    assert 0.5 < partial.char_similarity < 1.0
    assert partial.word_recall == 1.0
    assert partial.teds_grid is None


def test_score_reads_html_tables_and_reports_teds() -> None:
    truth = "| a | b |\n|---|---|\n| 1 | 2 |"
    html = "<table><tr><th>a</th><th>b</th></tr><tr><td>1</td><td>2</td></tr></table>"
    m = puffinparse.score(html, truth)
    assert m.table_score == 1.0
    assert m.teds_grid == 1.0


# ---- native-format output (output_format) --------------------------------------------------------


def test_output_formats_lists_the_unified_shape_and_every_vendor() -> None:
    assert puffinparse.output_formats() == ["puffinparse", "reducto", "extend", "llamaparse"]


def test_bad_output_format_raises_before_any_network_call(monkeypatch: pytest.MonkeyPatch) -> None:
    """A typo must fail at request-build time, not after paying for a provider call."""

    def unreachable(*args: object, **kwargs: object) -> None:
        raise AssertionError("the core must not be called with an invalid output_format")

    monkeypatch.setattr(puffinparse.main._core, "parse", unreachable)
    monkeypatch.setattr(puffinparse.main._core, "extract", unreachable)

    with pytest.raises(puffinparse.BadRequestError) as ei:
        puffinparse.parse("invoice.pdf", model="reducto/standard", output_format="nope")
    message = str(ei.value)
    for name in ("puffinparse", "reducto", "extend", "llamaparse"):
        assert name in message, message

    with pytest.raises(puffinparse.BadRequestError):
        puffinparse.extract("invoice.pdf", SCHEMA, model="reducto/extract", output_format="nope")


async def test_bad_output_format_raises_before_any_network_call_async() -> None:
    # No key, no readable file: reaching the core at all would raise something else entirely.
    with pytest.raises(puffinparse.BadRequestError):
        await puffinparse.aparse("invoice.pdf", model="reducto/standard", output_format="reduct")
    with pytest.raises(puffinparse.BadRequestError):
        await puffinparse.aextract("invoice.pdf", SCHEMA, model="reducto/extract", output_format="reduct")


def test_router_validates_output_format_before_any_network_call() -> None:
    router = puffinparse.Router(["reducto/standard", "extend/parse_light"])
    with pytest.raises(puffinparse.BadRequestError):
        router.parse("invoice.pdf", output_format="reductoo")


def test_output_format_accepts_vendor_aliases() -> None:
    # aliases and case are resolved by the core, so the SDK and CLI agree on what is valid
    assert puffinparse.main._validate_output_format("Reducto") == "reducto"
    assert puffinparse.main._validate_output_format("llama_parse") == "llamaparse"
    assert puffinparse.main._validate_output_format("unified") == "puffinparse"


def test_render_of_a_unified_response_is_vendor_shaped() -> None:
    """The renderers are pure functions of a response, so they need no network."""
    resp = puffinparse.ParseResponse(
        id="resp_1",
        provider="extend",
        model="extend/parse_light",
        pages=[
            puffinparse.Page(
                page_number=1,
                markdown="# Title",
                text="Title",
                width=100.0,
                height=200.0,
                blocks=[
                    puffinparse.Block(
                        type="title",
                        content="# Title",
                        page_number=1,
                        bbox=puffinparse.BBox(0.1, 0.1, 0.5, 0.2),
                        confidence=0.9,
                    )
                ],
            )
        ],
        markdown="# Title",
        text="Title",
        usage=puffinparse.Usage(pages=1),
        latency_ms=10,
        created_at="2026-09-11T00:00:00Z",
    )
    reducto = puffinparse.main._render(resp.to_dict(), "reducto", "parse")
    assert reducto["response_type"] == "parse"
    block = reducto["result"]["chunks"][0]["blocks"][0]
    assert block["type"] == "Title"
    assert block["bbox"] == {
        "left": 0.1,
        "top": 0.1,
        "width": 0.4,
        "height": 0.1,
        "page": 1,
        "original_page": 1,
    }
    extend = puffinparse.main._render(resp.to_dict(), "extend", "parse")
    assert extend["object"] == "parse_run" and extend["status"] == "PROCESSED"
    # Extend reports boxes in page pixels, so the page size is used verbatim
    assert extend["output"]["chunks"][0]["blocks"][0]["boundingBox"]["left"] == pytest.approx(10.0)
    llamaparse = puffinparse.main._render(resp.to_dict(), "llamaparse", "parse")
    assert llamaparse["pages"][0]["items"][0]["type"] == "heading"
