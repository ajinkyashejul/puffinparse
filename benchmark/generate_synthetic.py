#!/usr/bin/env python3
"""Generate the ``synthetic-v1`` benchmark dataset for PuffinParse.

The script renders a set of synthetic documents (PNG images and one multi-page
PDF per ``multipage`` document) together with *exact* markdown ground truth, so
OCR predictions can be scored without any manual labelling.

Everything is deterministic: the layout, the sentences picked from the built-in
corpus, the invoice numbers and the image noise are all driven by a single
``random.Random(1234)``. Re-running the script reproduces byte-identical output.

Dependencies: Pillow + the Python standard library only.

Usage::

    python benchmark/generate_synthetic.py

See docs/SPEC.md section 10 for the dataset format.
"""

from __future__ import annotations

import io
import json
import random
import re
import statistics
import time
from dataclasses import dataclass, field
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

# --------------------------------------------------------------------------
# Configuration
# --------------------------------------------------------------------------

SEED = 1234
# The v1.1 categories draw from their own stream so the v1.0 documents keep
# their exact bytes: ids, layout and noise for the first 24 docs never shift.
SEED_EXTRA = 5678
DATASET_NAME = "synthetic-v1"
DATASET_VERSION = "1.1.0"
DATASET_LICENSE = "CC0-1.0"
GENERATOR = "benchmark/generate_synthetic.py"
DESCRIPTION = (
    "Deterministically generated documents (plain text, invoices, tables, "
    "two-column layouts, headings, noisy scans, low resolution pages, "
    "multi-page PDFs, skewed pages, dense small-print pages, faded scans, "
    "thermal receipts and wide complex tables) with exact markdown ground truth."
)

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "datasets" / DATASET_NAME
DOCS_DIR = OUT_DIR / "docs"
TRUTH_DIR = OUT_DIR / "truth"

# A4 at 150 DPI.
PAGE_W, PAGE_H = 1240, 1754
MARGIN = 80
BODY_SIZE = 32
BG = 255
FG = 0

# Frozen PDF timestamp so multi-page output is byte-reproducible.
PDF_EPOCH = time.gmtime(0)

# Receipts are printed on a narrow roll rather than A4.
RECEIPT_W, RECEIPT_H = 600, 1600
RECEIPT_MARGIN = 36
# Rotated pages need slack so no glyph leaves the canvas when the page turns.
SKEW_MARGIN = 120

DEJAVU = "/usr/share/fonts/truetype/dejavu"
FREEFONT = "/usr/share/fonts/truetype/freefont"

# (regular, bold) font file pairs; one family is picked per document.
MONO = (f"{DEJAVU}/DejaVuSansMono.ttf", f"{DEJAVU}/DejaVuSansMono-Bold.ttf")

FAMILIES = [
    (f"{DEJAVU}/DejaVuSans.ttf", f"{DEJAVU}/DejaVuSans-Bold.ttf"),
    (f"{DEJAVU}/DejaVuSerif.ttf", f"{DEJAVU}/DejaVuSerif-Bold.ttf"),
    (f"{FREEFONT}/FreeSans.ttf", f"{FREEFONT}/FreeSansBold.ttf"),
    (f"{FREEFONT}/FreeSerif.ttf", f"{FREEFONT}/FreeSerifBold.ttf"),
]

_FONT_CACHE: dict[tuple[str, int], ImageFont.FreeTypeFont] = {}


def font(path: str, size: int) -> ImageFont.FreeTypeFont:
    key = (path, size)
    if key not in _FONT_CACHE:
        _FONT_CACHE[key] = ImageFont.truetype(path, size)
    return _FONT_CACHE[key]


# --------------------------------------------------------------------------
# Corpus (ASCII only, no lorem ipsum)
# --------------------------------------------------------------------------

CORPUS = [
    "The research team published its findings after three years of careful field work.",
    "Morning fog settled over the valley and did not lift until nearly noon.",
    "A small library opened on the corner of Fifth Street late last September.",
    "Engineers replaced the old bridge cables during a two week closure.",
    "She kept detailed notes in a leather bound journal that traveled everywhere with her.",
    "The committee met on Tuesday to review the proposed budget for the coming year.",
    "Volunteers planted more than four hundred saplings along the riverbank.",
    "Sales of electric bicycles grew steadily in every region except the far north.",
    "He learned to bake bread from his grandmother, who never once used a recipe.",
    "The museum extended its hours to accommodate the unexpected crowds.",
    "Snow covered the trail, so the guides chose a longer route through the pines.",
    "Most of the equipment arrived on time, although two crates were delayed in customs.",
    "Her lecture on coastal erosion drew students from several neighboring departments.",
    "The factory now recycles nearly ninety percent of the water it uses each day.",
    "A quiet enthusiasm spread through the office once the results were announced.",
    "They repaired the roof in the afternoon and finished the gutters before dark.",
    "Local farmers report that the soil has improved since they adopted cover crops.",
    "The train left the station four minutes late and still arrived on schedule.",
    "Nobody expected the small town to host a festival of that size.",
    "Careful measurement is the difference between a good estimate and a lucky guess.",
    "The new policy takes effect on the first day of the next quarter.",
    "Warm air from the south brought an early thaw to the northern counties.",
    "He spent the summer cataloging photographs that had been stored in a basement.",
    "The orchestra rehearsed the final movement until the balance felt right.",
    "Two of the three prototypes passed the durability test without any modification.",
    "Visitors are asked to remain on the marked paths in order to protect the dunes.",
    "Our supplier confirmed that the shipment will leave the port on Thursday morning.",
    "The doctor recommended shorter walks at first, followed by gradual increases.",
    "An old map showed a road that no longer appears in any modern atlas.",
    "Students designed a weather station using parts they found in the workshop.",
    "The kitchen staff prepared meals for three hundred guests without a single delay.",
    "Rising demand for storage pushed the warehouse to open a second location.",
    "She argued that the simplest explanation was also the most useful one.",
    "Wind turbines along the ridge supply power to nearly six thousand homes.",
    "The archive contains letters, receipts, and a handful of unlabeled photographs.",
    "After the storm the harbor was calm enough for the ferries to resume service.",
    "Costs fell once the team stopped shipping partial orders twice a week.",
    "He described the process patiently, pausing whenever someone raised a hand.",
    "The garden looks best in late May, when the hedges have finally filled out.",
    "Field notes from that expedition remain the only record of the northern camp.",
    "A revised schedule was posted on the notice board beside the main entrance.",
    "The company plans to open three service centers before the end of the year.",
    "Every measurement was repeated twice, and the averages were recorded by hand.",
    "Readers often skip the appendix, which is where the most interesting tables live.",
]

TITLES = [
    "Annual Field Report",
    "Notes on Coastal Erosion",
    "Operations Review",
    "The Riverbank Planting Project",
    "Winter Maintenance Summary",
    "A Short History of the Harbor",
    "Regional Logistics Update",
    "Findings from the Northern Camp",
]

SECTIONS = [
    "Background",
    "Method",
    "Observations",
    "Results and Discussion",
    "Logistics",
    "Next Steps",
    "Open Questions",
    "Acknowledgements",
    "Scope of the Study",
    "Limitations",
]

BULLET_ITEMS = [
    "Record every measurement twice and keep the raw sheets.",
    "Check the calibration log before the first run of the day.",
    "Publish the revised schedule on the notice board each Monday.",
    "Keep spare cables and two fully charged batteries in the field kit.",
    "Report any damaged equipment to the workshop within one day.",
    "Store the photographs with their original labels attached.",
    "Confirm the ferry timetable before planning a site visit.",
    "Send the weekly summary to the committee by Friday afternoon.",
    "Rotate the sampling sites so no single plot is overused.",
    "Back up the field notes to the shared archive every evening.",
]

COMPANIES = [
    ("Northwind Trading Co.", "1420 Harbor Street", "Suite 300", "Portland, OR 97209"),
    ("Cedar Ridge Supply", "88 Foundry Lane", "Building C", "Akron, OH 44308"),
    ("Blue Harbor Logistics", "515 Dockside Avenue", "Floor 2", "Tacoma, WA 98402"),
    ("Granite Works Limited", "7 Quarry Road", "Unit 11", "Barre, VT 05641"),
    ("Meridian Paper Group", "2300 Millrace Drive", "Suite 140", "Appleton, WI 54911"),
    ("Fairfield Instruments", "64 Tamarack Way", "Annex B", "Boulder, CO 80301"),
]

CUSTOMERS = [
    ("Globex Industries", "88 Riverside Avenue", "Austin, TX 78701"),
    ("Halcyon Print Shop", "12 Bellweather Court", "Madison, WI 53703"),
    ("Pinecrest Outfitters", "901 Summit Road", "Bozeman, MT 59715"),
    ("Lakeshore Medical Group", "43 Elm Street", "Erie, PA 16501"),
    ("Sandpiper Cafe", "377 Gull Lane", "Newport, RI 02840"),
    ("Ironwood Construction", "2 Kiln Street", "Asheville, NC 28801"),
]

LINE_ITEMS = [
    "Kraft shipping boxes, large",
    "Thermal label rolls",
    "Stainless steel brackets",
    "Cotton canvas tarps",
    "Replacement drive belts",
    "Insulated cable, 50 ft",
    "Packing tape, case of 36",
    "Steel shelving unit",
    "Workbench vise, 6 inch",
    "Safety goggles, pack of 12",
    "Hydraulic fluid, 5 gal",
    "Copper fittings, assorted",
    "Pallet jack service kit",
    "Floor mats, heavy duty",
]

REGIONS = ["North", "South", "East", "West", "Central", "Northeast", "Southwest", "Coastal"]
QUARTERS = ["Q1 2023", "Q2 2023", "Q3 2023", "Q4 2023", "Q1 2024", "Q2 2024", "Q3 2024", "Q4 2024"]
SITES = [
    "Ash Grove",
    "Birch Hollow",
    "Cedar Flat",
    "Dunes East",
    "Elm Point",
    "Fern Ridge",
    "Gull Rock",
    "Hazel Bend",
]
PRODUCTS = [
    "Model A100",
    "Model A200",
    "Model B150",
    "Model C300",
    "Model C310",
    "Model D400",
    "Model E500",
    "Model F620",
]

# Titles are keyed by table kind so the heading always describes the columns.
TABLE_TITLES = {
    "sales": ["Regional Sales by Quarter", "Quarterly Revenue by Region"],
    "sites": ["Site Measurement Log", "Sampling Results by Site"],
    "inventory": ["Equipment Inventory Counts", "Parts on Hand by Model"],
}

STORES = [
    ("Harborview Market", "412 Pier Road", "Newport, RI 02840", "Tel: 555-0142"),
    ("Cedar Street Grocer", "77 Cedar Street", "Akron, OH 44308", "Tel: 555-0198"),
    ("Pinecrest Provisions", "9 Summit Road", "Bozeman, MT 59715", "Tel: 555-0164"),
    ("Millrace General Store", "2300 Millrace Drive", "Appleton, WI 54911", "Tel: 555-0121"),
]

RECEIPT_ITEMS = [
    ("Sourdough Loaf", 4.25, 7.50),
    ("Whole Milk 1qt", 1.99, 3.60),
    ("Cheddar 8oz", 3.75, 8.20),
    ("Cold Brew 12oz", 2.50, 5.40),
    ("Bananas 2lb", 1.29, 2.80),
    ("Almond Butter", 6.40, 12.75),
    ("Trail Mix 10oz", 3.90, 7.10),
    ("Ginger Ale 6pk", 4.60, 9.30),
    ("Paper Towels", 5.10, 11.40),
    ("Dish Soap 16oz", 2.85, 6.20),
    ("Black Beans", 0.99, 2.40),
    ("Tomato Sauce", 1.45, 3.30),
    ("Rye Crackers", 2.75, 5.60),
    ("Olive Oil 500ml", 7.20, 16.80),
    ("Sea Salt 6oz", 1.85, 4.10),
    ("Green Tea 20ct", 3.40, 8.90),
]

CASHIERS = ["Dana", "Marcus", "Priya", "Elena", "Teo", "Roslyn"]

OPERATORS = [
    "R. Alvarez",
    "J. Whitfield",
    "M. Okonkwo",
    "L. Bergstrom",
    "T. Nakamura",
    "S. Delacroix",
    "K. Ferreira",
    "A. Lindqvist",
]

STATUSES = ["Complete", "Pending", "In Review", "Complete", "Verified"]

COMPLEX_TITLES = [
    "Sampling Coverage by Site",
    "Instrument Calibration Register",
    "Field Survey Control Sheet",
]

COMPLEX_CAPTIONS = [
    "Table 1. Sampling coverage for the 2024 season, ordered by the date each site was first visited.",
    "Table 2. Calibration records for every instrument returned to the workshop during the reporting period.",
    "Table 3. Control readings collected by each operator before the survey was signed off.",
]

COMPLEX_NOTES = [
    "Rows marked Pending are awaiting a second measurement. Depths are recorded "
    "in meters from the shoreline marker, and every value was checked against "
    "the original field sheet.",
    "Entries in review were flagged by the duty supervisor and will be reissued "
    "once the spare sensor arrives. No row has been removed from the register.",
    "Operators initial each row at the end of the shift. Where two operators "
    "share a site, the reading of the first visit is the one reported here.",
]

TABLE_INTROS = [
    "The table below summarizes the figures collected during the reporting period.",
    "All values were verified against the original field sheets before publication.",
    "Numbers are rounded to two decimal places and exclude pending adjustments.",
]


# --------------------------------------------------------------------------
# Document model: blocks render to both markdown and pixels
# --------------------------------------------------------------------------


@dataclass
class Heading:
    level: int
    text: str

    def markdown(self) -> str:
        return "#" * self.level + " " + self.text


@dataclass
class Para:
    text: str

    def markdown(self) -> str:
        return self.text


@dataclass
class Lines:
    """A run of short lines that belong to one markdown block (address, key/values)."""

    lines: list[str]

    def markdown(self) -> str:
        return "\n".join(self.lines)


@dataclass
class Bullets:
    items: list[str]

    def markdown(self) -> str:
        return "\n".join("- " + i for i in self.items)


@dataclass
class Table:
    headers: list[str]
    rows: list[list[str]] = field(default_factory=list)

    def markdown(self) -> str:
        out = ["| " + " | ".join(self.headers) + " |", "| " + " | ".join("---" for _ in self.headers) + " |"]
        out += ["| " + " | ".join(r) + " |" for r in self.rows]
        return "\n".join(out)


Block = Heading | Para | Lines | Bullets | Table


def to_markdown(*block_groups: list[Block]) -> str:
    blocks: list[Block] = []
    for group in block_groups:
        blocks.extend(group)
    text = "\n\n".join(b.markdown() for b in blocks)
    # Ground truth must never carry trailing whitespace.
    return "\n".join(line.rstrip() for line in text.split("\n")) + "\n"


# --------------------------------------------------------------------------
# Layout helpers
# --------------------------------------------------------------------------

NUMERIC = re.compile(r"^[-+$(]?[\d,]+(\.\d+)?\)?%?$")


def wrap(draw: ImageDraw.ImageDraw, text: str, fnt, max_w: float, first_indent: float = 0.0) -> list[str]:
    """Greedy word wrap using real glyph metrics."""
    words = text.split()
    lines: list[str] = []
    cur = ""
    avail = max_w - first_indent
    for w in words:
        cand = w if not cur else cur + " " + w
        if not cur or draw.textlength(cand, font=fnt) <= avail:
            cur = cand
        else:
            lines.append(cur)
            cur = w
            avail = max_w
    if cur:
        lines.append(cur)
    return lines or [""]


def draw_table(
    draw: ImageDraw.ImageDraw,
    tbl: Table,
    x: float,
    y: float,
    width: float,
    fam: tuple[str, str],
    base_size: int,
) -> float:
    """Render a Table block as a ruled grid; returns the new y cursor."""
    size = base_size
    cols: list[float] = []
    pad = 0.0
    while True:
        reg = font(fam[0], size)
        bold = font(fam[1], size)
        pad = size * 0.55
        cols = []
        for i, head in enumerate(tbl.headers):
            w = draw.textlength(head, font=bold)
            for row in tbl.rows:
                w = max(w, draw.textlength(row[i], font=reg))
            cols.append(w)
        if sum(cols) + 2 * pad * len(cols) <= width or size <= 16:
            break
        size -= 2

    reg = font(fam[0], size)
    bold = font(fam[1], size)
    widths = [c + 2 * pad for c in cols]
    extra = width - sum(widths)
    if extra > 0:
        widths = [w + extra / len(widths) for w in widths]

    right = [all(NUMERIC.match(r[i]) for r in tbl.rows) and bool(tbl.rows) for i in range(len(tbl.headers))]

    row_h = int(size * 1.75)
    top = y

    def draw_row(cells: list[str], fnt, yy: float) -> None:
        cx = x
        for i, cell in enumerate(cells):
            tw = draw.textlength(cell, font=fnt)
            tx = cx + widths[i] - pad - tw if right[i] else cx + pad
            draw.text((tx, yy + (row_h - size * 1.25) / 2), cell, font=fnt, fill=FG)
            cx += widths[i]

    draw_row(tbl.headers, bold, y)
    y += row_h
    draw.line([(x, y), (x + sum(widths), y)], fill=FG, width=2)
    for row in tbl.rows:
        y += 4
        draw_row(row, reg, y)
        y += row_h
        draw.line([(x, y), (x + sum(widths), y)], fill=170, width=1)
    draw.rectangle([x, top, x + sum(widths), y], outline=110, width=1)
    return y


def draw_blocks(
    draw: ImageDraw.ImageDraw,
    blocks: list[Block],
    x: float,
    y: float,
    width: float,
    fam: tuple[str, str],
    base_size: int,
) -> float:
    reg = font(fam[0], base_size)
    line_h = int(base_size * 1.34)
    gap = int(base_size * 0.8)
    for i, b in enumerate(blocks):
        if i:
            y += gap
        if isinstance(b, Heading):
            size = int(base_size * (1.75 if b.level == 1 else 1.3))
            f = font(fam[1], size)
            for ln in wrap(draw, b.text, f, width):
                draw.text((x, y), ln, font=f, fill=FG)
                y += int(size * 1.3)
        elif isinstance(b, Para):
            for ln in wrap(draw, b.text, reg, width):
                draw.text((x, y), ln, font=reg, fill=FG)
                y += line_h
        elif isinstance(b, Lines):
            for raw in b.lines:
                for ln in wrap(draw, raw, reg, width):
                    draw.text((x, y), ln, font=reg, fill=FG)
                    y += line_h
        elif isinstance(b, Bullets):
            indent = draw.textlength("- ", font=reg)
            for item in b.items:
                parts = wrap(draw, "- " + item, reg, width)
                for j, ln in enumerate(parts):
                    draw.text((x + (0 if j == 0 else indent), y), ln, font=reg, fill=FG)
                    y += line_h
        elif isinstance(b, Table):
            y = draw_table(draw, b, x, y, width, fam, base_size)
        else:  # pragma: no cover - defensive
            raise TypeError(f"unknown block {b!r}")
    return y


def new_page(w: int = PAGE_W, h: int = PAGE_H, bg: int = BG) -> Image.Image:
    return Image.new("L", (w, h), bg)


def render_page(
    blocks: list[Block],
    fam: tuple[str, str],
    doc_id: str,
    base_size: int = BODY_SIZE,
    bg: int = BG,
    margin: int = MARGIN,
    size: tuple[int, int] = (PAGE_W, PAGE_H),
) -> Image.Image:
    w, h = size
    img = new_page(w, h, bg)
    draw = ImageDraw.Draw(img)
    y = draw_blocks(draw, blocks, margin, margin, w - 2 * margin, fam, base_size)
    if y > h - margin:
        raise ValueError(f"{doc_id}: content overflows the page ({y:.0f}px > {h - margin}px)")
    return img


def render_two_column(
    left: list[Block], right: list[Block], fam: tuple[str, str], doc_id: str, base_size: int = 28
) -> Image.Image:
    img = new_page()
    draw = ImageDraw.Draw(img)
    gutter = 64
    col_w = (PAGE_W - 2 * MARGIN - gutter) / 2
    y1 = draw_blocks(draw, left, MARGIN, MARGIN, col_w, fam, base_size)
    y2 = draw_blocks(draw, right, MARGIN + col_w + gutter, MARGIN, col_w, fam, base_size)
    draw.line(
        [(MARGIN + col_w + gutter / 2, MARGIN), (MARGIN + col_w + gutter / 2, max(y1, y2))], fill=180, width=1
    )
    if max(y1, y2) > PAGE_H - MARGIN:
        raise ValueError(f"{doc_id}: column overflows the page")
    return img


# --------------------------------------------------------------------------
# Image degradation (deterministic)
# --------------------------------------------------------------------------


def gaussian_noise(size: tuple[int, int], sigma: float, rnd: random.Random) -> Image.Image:
    """Deterministic gaussian noise image centred on 128.

    Uniform random bytes are mapped through an inverse-normal lookup table, which
    keeps the whole operation inside the seeded Python RNG (unlike
    ``Image.effect_noise``, which is not seedable).
    """
    dist = statistics.NormalDist(0.0, sigma)
    table = bytes(max(0, min(255, round(128 + dist.inv_cdf((i + 0.5) / 256)))) for i in range(256))
    raw = rnd.randbytes(size[0] * size[1])
    return Image.frombytes("L", size, raw.translate(table))


def degrade_scan(img: Image.Image, rnd: random.Random) -> Image.Image:
    w, h = img.size
    draw = ImageDraw.Draw(img)
    for _ in range(rnd.randint(700, 1000)):
        cx, cy = rnd.randrange(w), rnd.randrange(h)
        r = rnd.randint(1, 3)
        draw.ellipse([cx - r, cy - r, cx + r, cy + r], fill=rnd.randint(110, 205))
    img = img.filter(ImageFilter.GaussianBlur(0.9))
    img = ImageChops.add(img, gaussian_noise((w, h), 9.0, rnd), 1.0, -128)
    return img


def skew(img: Image.Image, angle: float) -> Image.Image:
    """Rotate the page in place; SKEW_MARGIN keeps every glyph on the canvas."""
    return img.rotate(angle, resample=Image.BICUBIC, expand=False, fillcolor=BG)


def fade(img: Image.Image, quality: int = 35) -> Image.Image:
    """Wash the page out, soften it, then push it through a lossy JPEG round trip."""
    # Remap black-on-white to grey-on-off-white without touching the draw code.
    img = img.point(lambda v: 150 + (v * 100) // 255)
    img = img.filter(ImageFilter.GaussianBlur(1.0))
    buf = io.BytesIO()
    img.save(buf, "JPEG", quality=quality)
    buf.seek(0)
    return Image.open(buf).convert("L")


def thermal(img: Image.Image, rnd: random.Random) -> Image.Image:
    """Mild sensor noise plus the vertical streaks a worn print head leaves."""
    w, h = img.size
    streaks = Image.new("L", (w, h), 255)
    sd = ImageDraw.Draw(streaks)
    for _ in range(rnd.randint(5, 9)):
        x = rnd.randrange(RECEIPT_MARGIN, w - RECEIPT_MARGIN)
        sd.rectangle([x, 0, x + rnd.randint(0, 2), h], fill=rnd.randint(205, 240))
    img = ImageChops.multiply(img, streaks)
    return ImageChops.add(img, gaussian_noise((w, h), 5.0, rnd), 1.0, -128)


# --------------------------------------------------------------------------
# Content builders
# --------------------------------------------------------------------------


def paragraph(rnd: random.Random, lo: int = 4, hi: int = 5) -> Para:
    return Para(" ".join(rnd.sample(CORPUS, rnd.randint(lo, hi))))


def money(v: float) -> str:
    return f"{v:,.2f}"


def build_plain(rnd: random.Random, n_paras: int = 5) -> list[Block]:
    return [paragraph(rnd, 3, 5) for _ in range(n_paras)]


def build_invoice(rnd: random.Random) -> list[Block]:
    company = rnd.choice(COMPANIES)
    customer = rnd.choice(CUSTOMERS)
    number = f"INV-{rnd.randint(1000, 9999)}"
    month = rnd.randint(1, 12)
    day = rnd.randint(1, 28)
    date = f"2024-{month:02d}-{day:02d}"
    due_month = month % 12 + 1
    due_year = 2024 + (1 if due_month < month else 0)
    due = f"{due_year}-{due_month:02d}-{day:02d}"

    rows = []
    subtotal = 0.0
    for desc in rnd.sample(LINE_ITEMS, rnd.randint(4, 6)):
        qty = rnd.randint(1, 24)
        price = round(rnd.uniform(4.5, 320.0), 2)
        amount = round(qty * price, 2)
        subtotal += amount
        rows.append([desc, str(qty), money(price), "$" + money(amount)])
    subtotal = round(subtotal, 2)
    tax_rate = rnd.choice([6.25, 7.0, 8.25, 8.75])
    tax = round(subtotal * tax_rate / 100.0, 2)
    shipping = round(rnd.uniform(12.0, 95.0), 2)
    total = round(subtotal + tax + shipping, 2)

    return [
        Heading(1, company[0]),
        Lines([company[1], company[2], company[3]]),
        Lines(
            [
                "Bill To:",
                customer[0],
                customer[1],
                customer[2],
            ]
        ),
        Lines(
            [
                f"Invoice #: {number}",
                f"Date: {date}",
                f"Due Date: {due}",
                f"Purchase Order: PO-{rnd.randint(10000, 99999)}",
                "Terms: Net 30",
            ]
        ),
        Table(["Description", "Qty", "Unit Price", "Amount"], rows),
        Lines(
            [
                f"Subtotal: ${money(subtotal)}",
                f"Sales Tax ({tax_rate:.2f}%): ${money(tax)}",
                f"Shipping and Handling: ${money(shipping)}",
                f"Total Due: ${money(total)}",
            ]
        ),
        Para(
            "Payment is due within 30 days of the invoice date. "
            "Please include the invoice number with your remittance."
        ),
    ]


TABLE_KINDS = ["sales", "sites", "inventory"]


def build_table_block(rnd: random.Random, kind: str | None = None) -> tuple[str, Table]:
    """Return a (title, table) pair; the title always matches the columns."""
    kind = kind or rnd.choice(TABLE_KINDS)
    n = rnd.randint(5, 8)
    if kind == "sales":
        headers = ["Region", "Quarter", "Units", "Revenue"]
        rows = []
        for region in rnd.sample(REGIONS, n):
            units = rnd.randint(120, 9800)
            rows.append([region, rnd.choice(QUARTERS), f"{units:,}", money(units * rnd.uniform(3.2, 21.0))])
    elif kind == "sites":
        headers = ["Site", "Samples", "Mean Depth", "Variance"]
        rows = []
        for site in rnd.sample(SITES, n):
            rows.append(
                [
                    site,
                    str(rnd.randint(12, 240)),
                    f"{rnd.uniform(0.4, 18.5):.2f}",
                    f"{rnd.uniform(0.01, 3.9):.3f}",
                ]
            )
    else:
        headers = ["Product", "On Hand", "Unit Cost"]
        rows = []
        for product in rnd.sample(PRODUCTS, n):
            rows.append([product, f"{rnd.randint(3, 1450):,}", money(rnd.uniform(2.5, 480.0))])
    return rnd.choice(TABLE_TITLES[kind]), Table(headers, rows)


def build_table_doc(rnd: random.Random, kind: str | None = None) -> list[Block]:
    title, table = build_table_block(rnd, kind)
    return [
        Heading(1, title),
        Para(rnd.choice(TABLE_INTROS)),
        table,
    ]


def build_headings(rnd: random.Random) -> list[Block]:
    sections = rnd.sample(SECTIONS, 2)
    return [
        Heading(1, rnd.choice(TITLES)),
        paragraph(rnd, 3, 4),
        Heading(2, sections[0]),
        paragraph(rnd, 3, 4),
        Bullets(rnd.sample(BULLET_ITEMS, rnd.randint(4, 5))),
        Heading(2, sections[1]),
        paragraph(rnd, 3, 4),
    ]


def build_two_column(rnd: random.Random) -> tuple[list[Block], list[Block]]:
    left = [Heading(2, rnd.choice(SECTIONS))] + [paragraph(rnd, 2, 3) for _ in range(4)]
    right = [Heading(2, rnd.choice(SECTIONS))] + [paragraph(rnd, 2, 3) for _ in range(4)]
    return left, right


def build_multipage(rnd: random.Random, kind: str | None = None) -> tuple[list[Block], list[Block]]:
    page1 = [
        Heading(1, rnd.choice(TITLES)),
        paragraph(rnd, 3, 4),
        Heading(2, rnd.choice(SECTIONS)),
        paragraph(rnd, 4, 5),
    ]
    title, table = build_table_block(rnd, kind)
    page2 = [
        Heading(2, title),
        Para(rnd.choice(TABLE_INTROS)),
        table,
    ]
    return page1, page2


def build_dense(rnd: random.Random) -> list[Block]:
    """A wall of small print: 8 paragraphs, two H2 sections, ~700-900 words."""
    blocks: list[Block] = [Heading(1, rnd.choice(TITLES))]
    blocks.extend(paragraph(rnd, 7, 9) for _ in range(2))
    for section in rnd.sample(SECTIONS, 2):
        blocks.append(Heading(2, section))
        blocks.extend(paragraph(rnd, 7, 9) for _ in range(3))
    return blocks


def build_receipt(rnd: random.Random) -> list[Block]:
    store = rnd.choice(STORES)
    rows = []
    subtotal = 0.0
    for name, lo, hi in rnd.sample(RECEIPT_ITEMS, rnd.randint(8, 12)):
        qty = rnd.randint(1, 3)
        price = round(rnd.uniform(lo, hi), 2)
        subtotal += qty * price
        rows.append([name, str(qty), money(price)])
    subtotal = round(subtotal, 2)
    tax_rate = rnd.choice([5.5, 7.0, 8.25])
    tax = round(subtotal * tax_rate / 100.0, 2)
    total = round(subtotal + tax, 2)
    return [
        Heading(1, store[0]),
        Lines([store[1], store[2], store[3]]),
        Lines(
            [
                f"Order: {rnd.randint(1000, 9999)}",
                f"Date: 2024-{rnd.randint(1, 12):02d}-{rnd.randint(1, 28):02d} "
                f"{rnd.randint(8, 20):02d}:{rnd.randint(0, 59):02d}",
                f"Cashier: {rnd.choice(CASHIERS)}",
            ]
        ),
        Table(["Item", "Qty", "Price"], rows),
        Lines(
            [
                f"Subtotal: {money(subtotal)}",
                f"Tax ({tax_rate:.2f}%): {money(tax)}",
                f"TOTAL: {money(total)}",
                f"Card: Visa ending {rnd.randint(1000, 9999)}",
            ]
        ),
        Para("Thank you for shopping with us. Returns accepted within 14 days with this receipt."),
    ]


def build_complex_table(rnd: random.Random) -> list[Block]:
    """Six columns of mixed text and numbers, wrapped in a caption and notes."""
    idx = rnd.randrange(len(COMPLEX_TITLES))
    rows = []
    for _ in range(rnd.randint(12, 15)):
        rows.append(
            [
                f"S-{rnd.randint(100, 989):03d}",
                rnd.choice(SITES),
                rnd.choice(OPERATORS),
                f"2024-{rnd.randint(1, 12):02d}-{rnd.randint(1, 28):02d}",
                f"{rnd.uniform(0.4, 19.9):.2f}",
                rnd.choice(STATUSES),
            ]
        )
    rows.sort(key=lambda r: r[3])  # the caption promises date order
    return [
        Heading(1, COMPLEX_TITLES[idx]),
        Para(COMPLEX_CAPTIONS[idx]),
        Table(["ID", "Site", "Operator", "Visited", "Mean Depth", "Status"], rows),
        Para(COMPLEX_NOTES[idx]),
    ]


# --------------------------------------------------------------------------
# Document generation
# --------------------------------------------------------------------------


def save_png(img: Image.Image, path: Path) -> None:
    img.save(path, "PNG", optimize=True)


def generate() -> list[dict]:
    rnd = random.Random(SEED)
    DOCS_DIR.mkdir(parents=True, exist_ok=True)
    TRUTH_DIR.mkdir(parents=True, exist_ok=True)

    documents: list[dict] = []

    def emit(doc_id: str, filename: str, truth: str, pages: int, category: str, tags: list[str]) -> None:
        (TRUTH_DIR / f"{doc_id}.md").write_text(truth, encoding="ascii")
        documents.append(
            {
                "id": doc_id,
                "file": f"docs/{filename}",
                "truth": f"truth/{doc_id}.md",
                "pages": pages,
                "category": category,
                "tags": tags,
            }
        )

    for i in range(1, 4):
        # plain
        doc_id = f"plain_{i:03d}"
        fam = rnd.choice(FAMILIES)
        blocks = build_plain(rnd)
        save_png(render_page(blocks, fam, doc_id), DOCS_DIR / f"{doc_id}.png")
        emit(doc_id, f"{doc_id}.png", to_markdown(blocks), 1, "plain", ["clean", "text"])

        # invoice
        doc_id = f"invoice_{i:03d}"
        fam = rnd.choice(FAMILIES)
        blocks = build_invoice(rnd)
        save_png(render_page(blocks, fam, doc_id, base_size=28), DOCS_DIR / f"{doc_id}.png")
        emit(doc_id, f"{doc_id}.png", to_markdown(blocks), 1, "invoice", ["clean", "table", "key_value"])

        # table
        doc_id = f"table_{i:03d}"
        fam = rnd.choice(FAMILIES)
        blocks = build_table_doc(rnd, TABLE_KINDS[(i - 1) % 3])
        save_png(render_page(blocks, fam, doc_id), DOCS_DIR / f"{doc_id}.png")
        emit(doc_id, f"{doc_id}.png", to_markdown(blocks), 1, "table", ["clean", "table"])

        # two_column
        doc_id = f"two_column_{i:03d}"
        fam = rnd.choice(FAMILIES)
        left, right = build_two_column(rnd)
        save_png(render_two_column(left, right, fam, doc_id), DOCS_DIR / f"{doc_id}.png")
        emit(
            doc_id,
            f"{doc_id}.png",
            to_markdown(left, right),
            1,
            "two_column",
            ["clean", "layout", "reading_order"],
        )

        # headings
        doc_id = f"headings_{i:03d}"
        fam = rnd.choice(FAMILIES)
        blocks = build_headings(rnd)
        save_png(render_page(blocks, fam, doc_id), DOCS_DIR / f"{doc_id}.png")
        emit(doc_id, f"{doc_id}.png", to_markdown(blocks), 1, "headings", ["clean", "structure", "list"])

        # noisy_scan
        doc_id = f"noisy_scan_{i:03d}"
        fam = rnd.choice(FAMILIES)
        blocks = build_plain(rnd)
        img = render_page(blocks, fam, doc_id, bg=246)
        save_png(degrade_scan(img, rnd), DOCS_DIR / f"{doc_id}.png")
        emit(doc_id, f"{doc_id}.png", to_markdown(blocks), 1, "noisy_scan", ["degraded", "noise", "blur"])

        # low_res
        doc_id = f"low_res_{i:03d}"
        fam = rnd.choice(FAMILIES)
        blocks = build_plain(rnd, 4)
        img = render_page(blocks, fam, doc_id, base_size=26)
        img = img.resize((827, 1170), Image.LANCZOS)  # ~100 DPI for A4
        save_png(img, DOCS_DIR / f"{doc_id}.png")
        emit(doc_id, f"{doc_id}.png", to_markdown(blocks), 1, "low_res", ["degraded", "low_res"])

        # multipage (2-page PDF)
        doc_id = f"multipage_{i:03d}"
        fam = rnd.choice(FAMILIES)
        page1, page2 = build_multipage(rnd, TABLE_KINDS[i % 3])
        img1 = render_page(page1, fam, doc_id + " p1")
        img2 = render_page(page2, fam, doc_id + " p2")
        img1.save(
            DOCS_DIR / f"{doc_id}.pdf",
            "PDF",
            save_all=True,
            append_images=[img2],
            resolution=150.0,
            # Fixed timestamps keep the PDF bytes reproducible.
            creationDate=PDF_EPOCH,
            modDate=PDF_EPOCH,
        )
        emit(
            doc_id,
            f"{doc_id}.pdf",
            to_markdown(page1, page2),
            2,
            "multipage",
            ["clean", "multipage", "table", "structure"],
        )

    # ---- v1.1 categories: separate stream, so nothing above shifts ----
    rx = random.Random(SEED_EXTRA)
    for i in range(1, 4):
        # skewed
        doc_id = f"skewed_{i:03d}"
        fam = rx.choice(FAMILIES)
        blocks = build_headings(rx) if i % 2 else build_plain(rx, 4)
        img = render_page(blocks, fam, doc_id, base_size=30, margin=SKEW_MARGIN)
        angle = rx.choice([-1.0, 1.0]) * rx.uniform(2.0, 6.0)
        save_png(skew(img, angle), DOCS_DIR / f"{doc_id}.png")
        emit(doc_id, f"{doc_id}.png", to_markdown(blocks), 1, "skewed", ["degraded", "rotation", "layout"])

        # dense
        doc_id = f"dense_{i:03d}"
        fam = rx.choice(FAMILIES)
        blocks = build_dense(rx)
        save_png(render_page(blocks, fam, doc_id, base_size=17), DOCS_DIR / f"{doc_id}.png")
        emit(doc_id, f"{doc_id}.png", to_markdown(blocks), 1, "dense", ["clean", "small_print", "long"])

        # faded
        doc_id = f"faded_{i:03d}"
        fam = rx.choice(FAMILIES)
        blocks = build_invoice(rx) if i % 2 else build_headings(rx)
        img = render_page(blocks, fam, doc_id, base_size=28)
        save_png(fade(img), DOCS_DIR / f"{doc_id}.png")
        emit(
            doc_id,
            f"{doc_id}.png",
            to_markdown(blocks),
            1,
            "faded",
            ["degraded", "low_contrast", "jpeg", "blur"],
        )

        # receipt
        doc_id = f"receipt_{i:03d}"
        blocks = build_receipt(rx)
        img = render_page(
            blocks, MONO, doc_id, base_size=22, margin=RECEIPT_MARGIN, size=(RECEIPT_W, RECEIPT_H)
        )
        save_png(thermal(img, rx), DOCS_DIR / f"{doc_id}.png")
        emit(
            doc_id,
            f"{doc_id}.png",
            to_markdown(blocks),
            1,
            "receipt",
            ["degraded", "receipt", "table", "monospace"],
        )

        # complex_table
        doc_id = f"complex_table_{i:03d}"
        fam = rx.choice(FAMILIES)
        blocks = build_complex_table(rx)
        save_png(render_page(blocks, fam, doc_id, base_size=22), DOCS_DIR / f"{doc_id}.png")
        emit(
            doc_id, f"{doc_id}.png", to_markdown(blocks), 1, "complex_table", ["clean", "table", "wide_table"]
        )

    documents.sort(key=lambda d: d["id"])
    return documents


README = """\
# synthetic-v1

A small, fully synthetic OCR benchmark dataset for [PuffinParse](../../../README.md).
Every document is rendered by `{generator}` from a built-in corpus, so the
markdown in `truth/` is *exact* ground truth rather than a human transcription.

## Layout

```
benchmark/datasets/synthetic-v1/
  manifest.json      # {{name, version, description, license, documents:[...]}}
  docs/<id>.png      # input image (8-bit grayscale PNG)
  docs/<id>.pdf      # multi-page inputs
  truth/<id>.md      # expected markdown
```

`manifest.documents[]` entries are `{{id, file, truth, pages, category, tags}}`,
matching docs/SPEC.md section 10.2.

## Categories

{categories}

## Ground-truth conventions

- Paragraphs are separated by a blank line.
- Headings use `# ` (H1) and `## ` (H2).
- Bullet lists use `- item`.
- Tables are GitHub-flavoured markdown with a `| --- |` separator row.
- Key/value runs (invoice metadata, address blocks) are consecutive lines inside
  a single block, e.g. `Invoice #: INV-2041`.
- Text is ASCII only, and no line carries trailing whitespace.
- For `two_column`, reading order is the whole left column followed by the whole
  right column. For `multipage`, page 1 is followed by a blank line and page 2.

## Regenerating

```
python benchmark/generate_synthetic.py
```

The generator is seeded with `random.Random({seed})` for the eight v1.0 categories
and `random.Random({seed_extra})` for the five categories added in v1.1, so extending the
set never shifts the documents that already existed. It depends only on Pillow and
the standard library, so the output is reproducible. Fonts come from the
DejaVu and FreeFont families shipped with most Linux distributions
(`/usr/share/fonts/truetype/dejavu`, `/usr/share/fonts/truetype/freefont`).

## License

CC0-1.0. The documents, the ground truth and the generator are dedicated to the
public domain; the text corpus was written for this dataset. Use it for any
purpose without attribution.
"""

CATEGORY_DOCS = [
    ("plain", "Body paragraphs of clean English text on a single page."),
    ("invoice", "Company heading, address block, key/value metadata lines, a line-items table and totals."),
    ("table", "A title, one sentence of context and a 5-8 row, 3-4 column numeric table."),
    (
        "two_column",
        "Two columns of paragraphs; ground truth is the left column followed by the right column.",
    ),
    ("headings", "An H1, two H2 sections, paragraphs and a bulleted list."),
    ("noisy_scan", "Plain text degraded with grey speckles, a slight blur and gaussian noise."),
    ("low_res", "Plain text rendered small and downscaled to roughly 100 DPI."),
    ("multipage", "A two-page PDF: headings plus a paragraph on page 1, a table on page 2."),
    ("skewed", "A text page rotated by 2-6 degrees, as if fed crookedly through a scanner."),
    (
        "dense",
        "Small 17px print: an H1, two H2 sections and eight paragraphs, roughly 800 words on one page.",
    ),
    (
        "faded",
        "Low-contrast grey text on off-white, blurred and pushed through a quality-35 JPEG round trip.",
    ),
    (
        "receipt",
        "A narrow 600x1600 thermal receipt in monospace: store header, item table, totals, "
        "sensor noise and print-head streaks.",
    ),
    (
        "complex_table",
        "A six-column, 12-15 row table of mixed text and numbers between a caption and a notes paragraph.",
    ),
]


def write_readme(documents: list[dict]) -> None:
    counts: dict[str, int] = {}
    for d in documents:
        counts[d["category"]] = counts.get(d["category"], 0) + 1
    missing = set(counts) - {name for name, _ in CATEGORY_DOCS}
    if missing:
        raise ValueError(f"CATEGORY_DOCS is missing a description for: {sorted(missing)}")
    lines = ["| Category | Documents | Description |", "| --- | --- | --- |"]
    lines += [f"| `{name}` | {counts.get(name, 0)} | {desc} |" for name, desc in CATEGORY_DOCS]
    (OUT_DIR / "README.md").write_text(
        README.format(generator=GENERATOR, categories="\n".join(lines), seed=SEED, seed_extra=SEED_EXTRA),
        encoding="ascii",
    )


def main() -> None:
    documents = generate()
    manifest = {
        "name": DATASET_NAME,
        "version": DATASET_VERSION,
        "description": DESCRIPTION,
        "license": DATASET_LICENSE,
        "generator": GENERATOR,
        "documents": documents,
    }
    (OUT_DIR / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="ascii")
    write_readme(documents)

    total = sum(f.stat().st_size for f in DOCS_DIR.iterdir() if f.is_file())
    print(f"documents: {len(documents)}")
    print(f"docs dir:  {DOCS_DIR}")
    print(f"docs size: {total / 1024 / 1024:.2f} MB ({total} bytes)")


if __name__ == "__main__":
    main()
