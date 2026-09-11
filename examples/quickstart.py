"""Parse one document with three providers and print what each returned."""

import sys

import liteocr

path = sys.argv[1] if len(sys.argv) > 1 else "benchmark/datasets/synthetic-v1/docs/invoice_001.png"

for model in ["reducto/standard", "extend/parse_light", "llamaparse/cost_effective"]:
    try:
        resp = liteocr.parse(path, model=model)
    except liteocr.AuthenticationError as e:
        print(f"{model}: skipped ({e.message})")
        continue
    print(f"=== {model}: {resp.usage.pages} page(s), {resp.latency_ms} ms, ${resp.cost_usd:.4f}")
    print(resp.markdown[:400])
    print()
