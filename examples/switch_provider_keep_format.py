"""Switch the provider, keep the vendor's response shape.

Code already written against Reducto, Extend or LlamaParse does not have to be rewritten to try
another engine: ask for that vendor's `output_format` and PuffinParse renders whatever provider ran
the call into the JSON your parser already knows.

What is guaranteed is structural fidelity — key set, chunk/page and block counts, content strings,
block-type vocabulary, coordinate units, billed pages — not byte equality with what the vendor
would have returned. `docs/COMPAT.md` lists the always-null fields and the lossy mappings.
"""

import sys

import puffinparse

path = sys.argv[1] if len(sys.argv) > 1 else "benchmark/datasets/synthetic-v1/docs/table_001.png"


# Your existing Reducto-shaped parsing code, unchanged.
def draw_blocks(reducto_response: dict) -> None:
    for chunk in reducto_response["result"]["chunks"]:
        for block in chunk["blocks"]:
            bbox = block["bbox"]
            print(f"  p{bbox['page']} {block['type']:<15} ({bbox['left']:.3f}, {bbox['top']:.3f})")


# ... now fed by three different providers.
for model in ["reducto/standard", "extend/parse_light", "llamaparse/cost_effective"]:
    try:
        doc = puffinparse.parse(path, model=model, output_format="reducto")
    except puffinparse.AuthenticationError as e:
        print(f"{model}: skipped ({e.message})")
        continue
    print(f"=== {model} -> {doc['response_type']}, {doc['usage']['num_pages']} page(s)")
    draw_blocks(doc)

# Any vendor's shape works the same way, and "puffinparse" (or omitting the argument) keeps the
# unified dataclass.
extend_shaped = puffinparse.parse(path, model="reducto/standard", output_format="extend")
print(extend_shaped["object"], extend_shaped["status"], extend_shaped["metrics"]["pageCount"])

unified = puffinparse.parse(path, model="reducto/standard")
print(unified.markdown[:200], unified.cost_usd)

# Callbacks always receive the unified dataclass, whatever shape the caller asked for.
puffinparse.success_callback.append(lambda r: print("callback:", r.model, r.usage.pages, r.cost_usd))
puffinparse.parse(path, model="extend/parse_light", output_format="llamaparse")
