"""`extract` mode: pull a JSON object out of a document with a schema.

NOTE: `extract` needs an extract-capable model. Ask `puffinparse.list_models("extract")` which ones
your build has; if it is empty, the call below raises `UnsupportedModelError` naming the mode.
Reducto, Extend and LlamaParse are parse/ocr only.
"""

import puffinparse

SCHEMA = {
    "type": "object",
    "properties": {
        "invoice_number": {"type": "string"},
        "issued_on": {"type": "string", "description": "ISO 8601 date"},
        "total": {"type": "number", "description": "Grand total including tax"},
        "currency": {"type": "string"},
        "line_items": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "description": {"type": "string"},
                    "quantity": {"type": "number"},
                    "amount": {"type": "number"},
                },
            },
        },
    },
    "required": ["invoice_number", "total"],
}

models = puffinparse.list_models("extract")
print("models that serve extract:", models or "none yet")
model = models[0] if models else "reducto/standard"

try:
    resp = puffinparse.extract(
        "benchmark/datasets/synthetic-v1/docs/invoice_001.png",
        SCHEMA,
        model=model,
        instructions="Amounts are in the currency printed next to the total.",
        citations=True,
    )
except puffinparse.UnsupportedModelError as e:
    print(f"\n{model} cannot do extract yet:\n  {e.message}")
    raise SystemExit(0) from None

print(resp.data)
print(f"{resp.usage.pages} page(s), {resp.latency_ms} ms, ${resp.cost_usd or 0:.4f}")

for pointer, info in resp.fields.items():
    where = ", ".join(f"p{c.page_number}" for c in info.citations) or "no citation"
    print(f"  {pointer}: confidence={info.confidence} ({where})")
