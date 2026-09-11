"""Parse many documents concurrently with the async API."""

import asyncio
import glob

import liteocr


async def main() -> None:
    files = sorted(glob.glob("benchmark/datasets/synthetic-v1/docs/plain_*.png"))
    sem = asyncio.Semaphore(4)

    async def one(path: str) -> liteocr.OcrResponse:
        async with sem:
            return await liteocr.aocr(path, model="llamaparse/fast")

    results = await asyncio.gather(*(one(f) for f in files))
    total_cost = sum(r.cost_usd or 0.0 for r in results)
    for f, r in zip(files, results):
        print(f"{f}: {r.usage.pages} page(s), {r.latency_ms} ms")
    print(f"total ${total_cost:.4f}")


asyncio.run(main())
