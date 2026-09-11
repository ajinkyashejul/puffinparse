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
