"""`ocr` mode: plain text with line and word boxes, ready to draw as overlays.

Use `parse` when you want markdown, tables and block types; use `ocr` when you want the text
and where it sits on the page.
"""

import sys

import liteocr

path = sys.argv[1] if len(sys.argv) > 1 else "benchmark/datasets/synthetic-v1/docs/receipt_001.png"

resp = liteocr.ocr(path, model="reducto/standard")

print(f"{resp.model}: {resp.usage.pages} page(s), {resp.latency_ms} ms, ${resp.cost_usd or 0:.4f}")
print(resp.text[:300])
print()

page = resp.pages[0]
print(f"page 1: {len(page.lines)} lines, {len(page.words)} words, size {page.width}x{page.height}")
for line in page.lines[:5]:
    if line.bbox is not None and page.width and page.height:
        x0, y0, x1, y1 = line.bbox.to_pixels(page.width, page.height)
        print(f"  [{x0:7.1f} {y0:7.1f} {x1:7.1f} {y1:7.1f}] {line.text}")
    else:
        print(f"  [no box] {line.text}")

if resp.metadata.get("liteocr_derived_from") == "parse":
    print("\n(this provider has no native OCR endpoint; the text was derived from its parse output)")
