"""Unit tests that need no network."""

from __future__ import annotations

import liteocr
import pytest

SCHEMA = {"type": "object", "properties": {"total": {"type": "number"}}}

#: Providers that serve layout parsing (and plain text derived from it), never extraction.
PARSE_ONLY_PROVIDERS = {"datalab", "unstructured", "upstage", "mathpix"}
PARSE_ONLY = {m for m in liteocr.list_models() if m.split("/")[0] in PARSE_ONLY_PROVIDERS}


def test_version_and_models() -> None:
    assert liteocr.__version__
    models = liteocr.list_models()
    assert "reducto/standard" in models
    assert "extend/parse_performance" in models
    assert "llamaparse/agentic" in models
    assert all("/" in m for m in models)


def test_modes_and_models_per_mode() -> None:
    assert liteocr.modes() == ["parse", "ocr", "extract"]
    assert list(liteocr.MODES) == liteocr.modes()
    every = set(liteocr.list_models())
    for mode in liteocr.modes():
        assert set(liteocr.list_models(mode)) <= every
    # the original three providers do layout parsing and derive plain text from it ...
    assert set(liteocr.list_models("parse")) >= PARSE_ONLY
    assert set(liteocr.list_models("ocr")) >= PARSE_ONLY
    # ... and none of them does structured extraction
    assert PARSE_ONLY.isdisjoint(liteocr.list_models("extract"))
    with pytest.raises(liteocr.InputError):
        liteocr.list_models("summarise")  # type: ignore[arg-type]


def test_resolve_model_defaults_and_aliases() -> None:
    assert liteocr.resolve_model("reducto") == "reducto/standard"
    assert liteocr.resolve_model("llama") == "llamaparse/cost_effective"
    assert liteocr.resolve_model("Extend/Parse_Light") == "extend/parse_light"
    assert liteocr.resolve_model("reducto", "ocr") == "reducto/standard"
    with pytest.raises(liteocr.UnsupportedModelError):
        liteocr.resolve_model("nope/x")
    with pytest.raises(liteocr.UnsupportedModelError) as ei:
        liteocr.resolve_model("reducto/standard", "extract")
    assert "extract" in str(ei.value)


def test_providers_metadata() -> None:
    provs = liteocr.providers()
    names = {p["name"] for p in provs}
    assert names >= {"reducto", "extend", "llamaparse"}
    for p in provs:
        assert p["env_var"].isupper() and any(t in p["env_var"] for t in ("KEY", "TOKEN"))
        assert p["base_url"].startswith("https://")
        assert any(m["default"] for m in p["models"])
        # every mode a provider serves is reachable from a bare provider name
        for mode in liteocr.modes():
            if any(mode in m["modes"] for m in p["models"]):
                assert liteocr.resolve_model(p["name"], mode).startswith(f"{p['name']}/")
        for m in p["models"]:
            assert m["modes"], f"{m['model']} declares no mode"
            assert set(m["modes"]) <= set(liteocr.modes())
            if p["name"] in PARSE_ONLY_PROVIDERS:
                assert m["modes"] == ["parse", "ocr"]


def test_pricing_override_and_reset() -> None:
    try:
        assert liteocr.estimate_cost("llamaparse/fast", 1000) == pytest.approx(1.25)
        assert liteocr.estimate_cost("llamaparse/fast", 1000, "ocr") == pytest.approx(1.25)
        assert liteocr.estimate_cost("llamaparse/fast", 1000, "extract") is None
        liteocr.set_pricing({"llamaparse/fast": 0.5})
        assert liteocr.pricing()["llamaparse/fast"]["parse"] == 0.5
        assert liteocr.estimate_cost("llamaparse/fast", 2) == pytest.approx(1.0)
        # overriding one mode leaves the others alone
        assert liteocr.estimate_cost("llamaparse/fast", 1000, "ocr") == pytest.approx(1.25)
        liteocr.set_pricing({"llamaparse/fast": 0.25}, "ocr")
        assert liteocr.estimate_cost("llamaparse/fast", 4, "ocr") == pytest.approx(1.0)
    finally:
        liteocr.reset_pricing()
    assert liteocr.estimate_cost("llamaparse/fast", 1000) == pytest.approx(1.25)
    assert liteocr.pricing()["llamaparse/fast"]["source"].startswith("https://")


def test_unsupported_model_raises_before_network() -> None:
    with pytest.raises(liteocr.UnsupportedModelError) as ei:
        liteocr.parse("does-not-matter.pdf", model="acme/ocr")
    assert "acme" in str(ei.value)
    with pytest.raises(liteocr.UnsupportedModelError):
        liteocr.ocr("does-not-matter.pdf", model="acme/ocr")


def test_extract_rejects_a_parse_only_model() -> None:
    with pytest.raises(liteocr.UnsupportedModelError) as ei:
        liteocr.extract("does-not-matter.pdf", SCHEMA, model="reducto/standard")
    message = str(ei.value)
    assert "extract" in message
    assert "reducto/standard" in message
    # the message points at what *can* serve the mode
    assert "Models for mode 'extract'" in message


def test_extract_requires_a_schema_object() -> None:
    with pytest.raises(TypeError) as ei:
        liteocr.extract("x.pdf", "total: number")  # type: ignore[arg-type]
    assert "JSON Schema" in str(ei.value)


def test_bytes_require_filename() -> None:
    for call in (liteocr.parse, liteocr.ocr):
        with pytest.raises(liteocr.InputError):
            call(b"%PDF-1.4", model="reducto")
    with pytest.raises(liteocr.InputError):
        liteocr.extract(b"%PDF-1.4", SCHEMA, model="reducto")


def test_missing_file_is_input_error(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("EXTEND_API_KEY", "test-key")
    with pytest.raises(liteocr.InputError) as ei:
        liteocr.parse("/definitely/not/here.pdf", model="extend")
    assert ei.value.provider == "extend"


def test_missing_api_key_is_authentication_error(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:  # type: ignore[no-untyped-def]
    monkeypatch.delenv("REDUCTO_API_KEY", raising=False)
    f = tmp_path / "a.pdf"
    f.write_bytes(b"%PDF-1.4 minimal")
    for call in (liteocr.parse, liteocr.ocr):
        with pytest.raises(liteocr.AuthenticationError) as ei:
            call(f, model="reducto")
        assert "REDUCTO_API_KEY" in str(ei.value)
        assert isinstance(ei.value, liteocr.LiteOCRError)


def test_failure_callback_invoked_for_every_mode() -> None:
    seen: list[liteocr.LiteOCRError] = []
    liteocr.failure_callback.append(seen.append)
    try:
        with pytest.raises(liteocr.UnsupportedModelError):
            liteocr.parse("x.pdf", model="bogus")
        with pytest.raises(liteocr.UnsupportedModelError):
            liteocr.ocr("x.pdf", model="bogus")
        with pytest.raises(liteocr.UnsupportedModelError):
            liteocr.extract("x.pdf", SCHEMA, model="bogus")
    finally:
        liteocr.failure_callback.clear()
    assert [e.kind for e in seen] == ["unsupported_model_error"] * 3


async def test_async_errors_are_typed() -> None:
    with pytest.raises(liteocr.UnsupportedModelError):
        await liteocr.aparse("x.pdf", model="bogus")
    with pytest.raises(liteocr.UnsupportedModelError):
        await liteocr.aocr("x.pdf", model="bogus")
    with pytest.raises(liteocr.UnsupportedModelError):
        await liteocr.aextract("x.pdf", SCHEMA, model="reducto/standard")


def test_router_plan_and_strategy() -> None:
    r = liteocr.Router(["reducto", "extend/parse_light"], strategy="round_robin")
    assert r.models == ["reducto/standard", "extend/parse_light"]
    assert r.mode == "parse"
    assert r.plan() == ["reducto/standard", "extend/parse_light"]
    assert r.plan() == ["extend/parse_light", "reducto/standard"]
    with pytest.raises(liteocr.UnsupportedModelError):
        liteocr.Router(["nope"])
    with pytest.raises(liteocr.InputError):
        liteocr.Router([])
    with pytest.raises(TypeError):
        r.parse("x.pdf", bogus_kwarg=1)


def test_router_is_bound_to_one_mode() -> None:
    r = liteocr.Router(["reducto", "extend/parse_light"], mode="parse")
    with pytest.raises(liteocr.InputError) as ei:
        r.ocr("x.pdf")
    assert "mode 'parse'" in str(ei.value)
    with pytest.raises(liteocr.InputError):
        r.extract("x.pdf", SCHEMA)

    text_router = liteocr.Router(["reducto"], mode="ocr")
    assert text_router.mode == "ocr"
    with pytest.raises(liteocr.InputError):
        text_router.parse("x.pdf")

    # a router only accepts models that serve its mode
    with pytest.raises(liteocr.UnsupportedModelError):
        liteocr.Router(["datalab"], mode="extract")
    assert liteocr.Router(["reducto", "extend"], mode="extract").models == [
        "reducto/extract",
        "extend/extraction_performance",
    ]
    with pytest.raises(liteocr.InputError):
        liteocr.Router(["reducto"], mode="translate")  # type: ignore[arg-type]


def test_router_does_not_fall_back_on_auth_error(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:  # type: ignore[no-untyped-def]
    monkeypatch.delenv("REDUCTO_API_KEY", raising=False)
    f = tmp_path / "a.pdf"
    f.write_bytes(b"%PDF-1.4 minimal")
    r = liteocr.Router(["reducto", "extend"])
    with pytest.raises(liteocr.AuthenticationError):
        r.parse(f)
    stats = r.stats()
    assert stats["reducto/standard"]["failures"] == 1
    assert stats["extend/parse_performance"]["failures"] == 0


def test_score_and_normalize() -> None:
    m = liteocr.score("# Hello **World**", "hello world")
    assert m.char_similarity == 1.0
    assert m.cer == 0.0 and m.wer == 0.0
    assert liteocr.normalize_text("# Hello   **World**") == "hello world"
    assert liteocr.markdown_to_text("| a | b |\n|---|---|\n| 1 | 2 |") == "a b\n1 2"
    partial = liteocr.score("hello there world", "hello world")
    assert 0.5 < partial.char_similarity < 1.0
    assert partial.word_recall == 1.0
    assert partial.teds_grid is None


def test_score_reads_html_tables_and_reports_teds() -> None:
    truth = "| a | b |\n|---|---|\n| 1 | 2 |"
    html = "<table><tr><th>a</th><th>b</th></tr><tr><td>1</td><td>2</td></tr></table>"
    m = liteocr.score(html, truth)
    assert m.table_score == 1.0
    assert m.teds_grid == 1.0


# ---- native-format output (output_format) --------------------------------------------------------


def test_output_formats_lists_the_unified_shape_and_every_vendor() -> None:
    assert liteocr.output_formats() == ["liteocr", "reducto", "extend", "llamaparse"]


def test_bad_output_format_raises_before_any_network_call(monkeypatch: pytest.MonkeyPatch) -> None:
    """A typo must fail at request-build time, not after paying for a provider call."""

    def unreachable(*args: object, **kwargs: object) -> None:
        raise AssertionError("the core must not be called with an invalid output_format")

    monkeypatch.setattr(liteocr.main._core, "parse", unreachable)
    monkeypatch.setattr(liteocr.main._core, "extract", unreachable)

    with pytest.raises(liteocr.BadRequestError) as ei:
        liteocr.parse("invoice.pdf", model="reducto/standard", output_format="nope")
    message = str(ei.value)
    for name in ("liteocr", "reducto", "extend", "llamaparse"):
        assert name in message, message

    with pytest.raises(liteocr.BadRequestError):
        liteocr.extract("invoice.pdf", SCHEMA, model="reducto/extract", output_format="nope")


async def test_bad_output_format_raises_before_any_network_call_async() -> None:
    # No key, no readable file: reaching the core at all would raise something else entirely.
    with pytest.raises(liteocr.BadRequestError):
        await liteocr.aparse("invoice.pdf", model="reducto/standard", output_format="reduct")
    with pytest.raises(liteocr.BadRequestError):
        await liteocr.aextract("invoice.pdf", SCHEMA, model="reducto/extract", output_format="reduct")


def test_router_validates_output_format_before_any_network_call() -> None:
    router = liteocr.Router(["reducto/standard", "extend/parse_light"])
    with pytest.raises(liteocr.BadRequestError):
        router.parse("invoice.pdf", output_format="reductoo")


def test_output_format_accepts_vendor_aliases() -> None:
    # aliases and case are resolved by the core, so the SDK and CLI agree on what is valid
    assert liteocr.main._validate_output_format("Reducto") == "reducto"
    assert liteocr.main._validate_output_format("llama_parse") == "llamaparse"
    assert liteocr.main._validate_output_format("unified") == "liteocr"


def test_render_of_a_unified_response_is_vendor_shaped() -> None:
    """The renderers are pure functions of a response, so they need no network."""
    resp = liteocr.ParseResponse(
        id="resp_1",
        provider="extend",
        model="extend/parse_light",
        pages=[
            liteocr.Page(
                page_number=1,
                markdown="# Title",
                text="Title",
                width=100.0,
                height=200.0,
                blocks=[
                    liteocr.Block(
                        type="title",
                        content="# Title",
                        page_number=1,
                        bbox=liteocr.BBox(0.1, 0.1, 0.5, 0.2),
                        confidence=0.9,
                    )
                ],
            )
        ],
        markdown="# Title",
        text="Title",
        usage=liteocr.Usage(pages=1),
        latency_ms=10,
        created_at="2026-09-11T00:00:00Z",
    )
    reducto = liteocr.main._render(resp.to_dict(), "reducto", "parse")
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
    extend = liteocr.main._render(resp.to_dict(), "extend", "parse")
    assert extend["object"] == "parse_run" and extend["status"] == "PROCESSED"
    # Extend reports boxes in page pixels, so the page size is used verbatim
    assert extend["output"]["chunks"][0]["blocks"][0]["boundingBox"]["left"] == pytest.approx(10.0)
    llamaparse = liteocr.main._render(resp.to_dict(), "llamaparse", "parse")
    assert llamaparse["pages"][0]["items"][0]["type"] == "heading"
