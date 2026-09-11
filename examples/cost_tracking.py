"""Track spend across calls with a success callback."""

import liteocr

spend: dict[str, float] = {}


def on_success(resp: liteocr.OcrResponse) -> None:
    spend[resp.model] = spend.get(resp.model, 0.0) + (resp.cost_usd or 0.0)


liteocr.success_callback.append(on_success)

for model in ["reducto/r-1", "extend/parse_light"]:
    liteocr.ocr("benchmark/datasets/synthetic-v1/docs/headings_001.png", model=model)

print(spend)
