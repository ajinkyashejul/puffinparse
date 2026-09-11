"""Route across providers: try the first, fall back on provider / rate-limit / timeout errors.

A router is bound to one mode, so every model in the list must serve it.
"""

import liteocr

router = liteocr.Router(
    ["reducto/standard", "llamaparse/agentic", "extend/parse_light"],
    mode="parse",
    strategy="ordered",
)

resp = router.parse("benchmark/datasets/synthetic-v1/docs/table_001.png")
print(resp.model, resp.metadata.get("liteocr_fallback_index", 0))
print(resp.markdown)
print(router.stats())

# The same models, routed for plain text instead.
text_router = liteocr.Router(["llamaparse/fast", "extend/parse_light"], mode="ocr")
print(text_router.ocr("benchmark/datasets/synthetic-v1/docs/plain_001.png").text[:200])
