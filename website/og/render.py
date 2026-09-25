"""Render the social card (website/og/card.html) to website/assets/og.png at 1200x630.

Dev-only: needs Playwright with a Chromium (`pip install playwright`). The site build does not run
this; it copies the committed PNG. Re-run after changing the card, the mark or the mascot.
"""

from __future__ import annotations

from pathlib import Path

from playwright.sync_api import sync_playwright

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "assets" / "og.png"


def main() -> None:
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 1200, "height": 630}, color_scheme="light")
        page.goto((HERE / "card.html").as_uri())
        page.wait_for_load_state("networkidle")
        page.screenshot(path=str(OUT))
        browser.close()
    print(f"wrote {OUT.relative_to(HERE.parent.parent)}")


if __name__ == "__main__":
    main()
