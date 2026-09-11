"""Track spend across calls with a success callback (it fires in every mode)."""

import liteocr

spend: dict[str, float] = {}


def on_success(resp: liteocr.Response) -> None:
    spend[resp.model] = spend.get(resp.model, 0.0) + (resp.cost_usd or 0.0)


liteocr.success_callback.append(on_success)

page = "benchmark/datasets/synthetic-v1/docs/headings_001.png"
for model in ["reducto/r-1", "extend/parse_light"]:
    liteocr.parse(page, model=model)  # parse mode: markdown + blocks
    liteocr.ocr(page, model=model)  # ocr mode: plain text + boxes

print(spend)
print("parse rate:", liteocr.estimate_cost("reducto/r-1", 1000))
print("ocr rate:  ", liteocr.estimate_cost("reducto/r-1", 1000, "ocr"))
