"""Route across providers: try the first, fall back on provider / rate-limit / timeout errors."""

import liteocr

router = liteocr.Router(
    ["reducto/standard", "llamaparse/agentic", "extend/parse_light"],
    strategy="ordered",
)

resp = router.ocr("benchmark/datasets/synthetic-v1/docs/table_001.png")
print(resp.model, resp.metadata.get("liteocr_fallback_index", 0))
print(resp.markdown)
print(router.stats())
