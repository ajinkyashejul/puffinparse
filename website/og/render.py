"""Render the social cards (website/og/card*.html) to website/assets/og*.png at 1200x630.

Dev-only: needs Playwright with a Chromium (`pip install playwright`). The site build does not run
this; it copies the committed PNG. Re-run after changing the card, the mark or the mascot.
"""

from __future__ import annotations

from pathlib import Path

from playwright.sync_api import sync_playwright

HERE = Path(__file__).resolve().parent
CARDS = {"card.html": "og.png", "card-benchmark.html": "og-benchmark.png"}
ASSETS = HERE.parent / "assets"


def main() -> None:
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 1200, "height": 630}, color_scheme="light")
        for source, png in CARDS.items():
            out = ASSETS / png
            page.goto((HERE / source).as_uri())
            page.wait_for_load_state("networkidle")
            page.screenshot(path=str(out))
            print(f"wrote {out.relative_to(HERE.parent.parent)}")
        browser.close()


if __name__ == "__main__":
    main()
