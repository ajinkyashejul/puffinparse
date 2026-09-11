"""Unit tests that need no network."""

from __future__ import annotations

import liteocr
import pytest


def test_version_and_models() -> None:
    assert liteocr.__version__
    models = liteocr.list_models()
    assert "reducto/standard" in models
    assert "extend/parse_performance" in models
    assert "llamaparse/agentic" in models
    assert all("/" in m for m in models)


def test_resolve_model_defaults_and_aliases() -> None:
    assert liteocr.resolve_model("reducto") == "reducto/standard"
    assert liteocr.resolve_model("llama") == "llamaparse/cost_effective"
    assert liteocr.resolve_model("Extend/Parse_Light") == "extend/parse_light"
    with pytest.raises(liteocr.UnsupportedModelError):
        liteocr.resolve_model("nope/x")


def test_providers_metadata() -> None:
    provs = liteocr.providers()
    names = {p["name"] for p in provs}
    assert names == {"reducto", "extend", "llamaparse"}
    for p in provs:
        assert p["env_var"].endswith("_API_KEY")
        assert p["base_url"].startswith("https://")
        assert any(m["default"] for m in p["models"])


def test_pricing_override_and_reset() -> None:
    try:
        assert liteocr.estimate_cost("llamaparse/fast", 1000) == pytest.approx(1.25)
        liteocr.set_pricing({"llamaparse/fast": 0.5})
        assert liteocr.pricing()["llamaparse/fast"]["per_page_usd"] == 0.5
        assert liteocr.estimate_cost("llamaparse/fast", 2) == pytest.approx(1.0)
    finally:
        liteocr.reset_pricing()
    assert liteocr.estimate_cost("llamaparse/fast", 1000) == pytest.approx(1.25)


def test_unsupported_model_raises_before_network() -> None:
    with pytest.raises(liteocr.UnsupportedModelError) as ei:
        liteocr.ocr("does-not-matter.pdf", model="acme/ocr")
    assert "acme" in str(ei.value)


def test_bytes_require_filename() -> None:
    with pytest.raises(liteocr.InputError):
        liteocr.ocr(b"%PDF-1.4", model="reducto")


def test_missing_file_is_input_error(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("EXTEND_API_KEY", "test-key")
    with pytest.raises(liteocr.InputError) as ei:
        liteocr.ocr("/definitely/not/here.pdf", model="extend")
    assert ei.value.provider == "extend"


def test_missing_api_key_is_authentication_error(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:  # type: ignore[no-untyped-def]
    monkeypatch.delenv("REDUCTO_API_KEY", raising=False)
    f = tmp_path / "a.pdf"
    f.write_bytes(b"%PDF-1.4 minimal")
    with pytest.raises(liteocr.AuthenticationError) as ei:
        liteocr.ocr(f, model="reducto")
    assert "REDUCTO_API_KEY" in str(ei.value)
    assert isinstance(ei.value, liteocr.LiteOCRError)


def test_failure_callback_invoked(monkeypatch: pytest.MonkeyPatch) -> None:
    seen: list[liteocr.LiteOCRError] = []
    liteocr.failure_callback.append(seen.append)
    try:
        with pytest.raises(liteocr.UnsupportedModelError):
            liteocr.ocr("x.pdf", model="bogus")
    finally:
        liteocr.failure_callback.clear()
    assert len(seen) == 1 and seen[0].kind == "unsupported_model_error"


async def test_aocr_errors_are_typed() -> None:
    with pytest.raises(liteocr.UnsupportedModelError):
        await liteocr.aocr("x.pdf", model="bogus")


def test_router_plan_and_strategy() -> None:
    r = liteocr.Router(["reducto", "extend/parse_light"], strategy="round_robin")
    assert r.models == ["reducto/standard", "extend/parse_light"]
    assert r.plan() == ["reducto/standard", "extend/parse_light"]
    assert r.plan() == ["extend/parse_light", "reducto/standard"]
    with pytest.raises(liteocr.UnsupportedModelError):
        liteocr.Router(["nope"])
    with pytest.raises(liteocr.InputError):
        liteocr.Router([])
    with pytest.raises(TypeError):
        r.ocr("x.pdf", bogus_kwarg=1)


def test_router_does_not_fall_back_on_auth_error(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:  # type: ignore[no-untyped-def]
    monkeypatch.delenv("REDUCTO_API_KEY", raising=False)
    f = tmp_path / "a.pdf"
    f.write_bytes(b"%PDF-1.4 minimal")
    r = liteocr.Router(["reducto", "extend"])
    with pytest.raises(liteocr.AuthenticationError):
        r.ocr(f)
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
