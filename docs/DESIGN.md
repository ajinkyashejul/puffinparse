# PuffinParse design language

PuffinParse reads documents for developers, so its interface borrows from the things that happen when a
document is read: a clean page, dark ink, a scanner's line of light, a detector's box around a
region, and a proofreader's marks. Everything else stays out of the way.

Tokens live in [`website/assets/tokens.css`](../website/assets/tokens.css). Every surface (the docs,
the landing page and the benchmark viewer) links that file first and styles only through its custom
properties. This page explains what the tokens are for.

## Principles

1. **One number per view.** Every screen has a single primary figure or answer: the leaderboard's
   top score, a document's score for the selected model. Everything else is secondary, and most
   secondary things sit behind a disclosure.
2. **Show the evidence one click away.** Claims are backed by the page, the output, the truth and
   the diff, but those open on demand. A reader who trusts the summary never has to scroll past
   the proof; a reader who doubts it reaches the proof in one click.
3. **Human names, machine ids on request.** Say "Headers & footers 3", not
   `olmocr/headers_footers_0678963a…`. The raw id is always available, in monospace, with a copy
   button, inside Details.
4. **Lite means light.** System fonts only (no font downloads), no framework, few borders, no
   decorative imagery in the tools. The puffin appears only in brand moments (see Brand). A page
   should render usefully before any script runs.
5. **Colour carries meaning.** The accent marks the one primary thing. Verdict colours mark good,
   fair and poor. Mode colours label parse, OCR and extract. Nothing is coloured for decoration.

## Brand

PuffinParse is named after the Atlantic puffin: black and white with an orange beak, which is
exactly the paper, ink and scan palette above. Puffins are known for carrying a neat row of fish
crosswise in their beak; PuffinParse carries a document back as a neat row of typed blocks. Both
brand assets tell that story with text in the beak: the mark's beak carries two parsed lines, the
mascot carries a row of three pages.

### The mark

[`website/assets/mark.svg`](../website/assets/mark.svg): a puffin head in a rounded-square tile. The
tile is `--mark-tile` (ink in light, a lifted slate in dark so it never disappears on the dark
ground), the face is white, the beak is `--scan` and carries two ink stripes, the parsed lines.

- Sits left of the wordmark "PuffinParse" (`--font-sans`, weight 640, tracking -0.02em) with an
  `--s-2` gap. Headers inline it (`MARK` in `website/build.py`) so the tile follows the theme toggle;
  the favicon is the file itself, which carries its own dark-mode tile.
- Minimum size 16px (favicon). Header size 22px (`.mark`). Clear space: a quarter of its width.
- Never recolour the beak, add a shadow or outline, rotate it, or place it on the accent colour.

### The puffin (mascot)

[`website/assets/puffin.svg`](../website/assets/puffin.svg): a chubby, front-facing puffin waving
with its raised wing, a row of three pages hanging from its beak (white, folded corner, a short
`--scan` heading bar and two grey text lines). Body `--mascot-body`, wing `--mascot-wing` outlined
in the body colour (both lift in dark mode), face and belly white, beak and feet `--scan`, a pale
`#ffb39f` base ridge and gape, a soft blush, two grey wave marks beside the wing.

- **Brand moments only:** the landing hero (perched on the demo card; it hops and waves once each
  time a response finishes typing), the 404 page, empty states, the social card and the README. Never in
  the working views of the viewer or docs, never next to data.
- Inline it (`puffin(width)` in `website/build.py`) where the manual theme toggle must reach it;
  as an `<img>` it follows the OS colour scheme through its own media query.
- Keep its classes `pp-*`: an inline SVG's `<style>` applies to the whole page.
- One puffin per view. It does not talk, wink or wear props; new poses keep the same shapes and
  colours, and keep something in the beak. Explored poses for later (reading, sleepy for empty
  states, terminal for the CLI, a round sticker) follow the same rules.
- The wing is its own group (`.pp-arm`), so a page can animate the wave by rotating it about the
  shoulder (68, 125 in SVG units).

### Brand motion

Tools keep the motion rules below (state changes only). Brand moments may move more, always
within these rules:

- **One gesture at a time, tied to an event.** The landing puffin hops and waves when a response
  finishes typing; nothing loops idly on a page people read. Everything stops under
  `prefers-reduced-motion`.
- **The logo reveal is the canonical intro:** corner ticks lock on like a detector, the tile lands,
  the face pops, the beak slides in and its two stripes type out, then the wordmark rises. Use it to
  open or close videos and talks; do not invent other logo animations.
- **Physical, not floaty:** springs and squash-and-stretch for the puffin, ease-out for things that
  arrive, 160–600 ms per gesture. No spinning, bouncing loops or particle effects.
- **Loops are seamless:** every periodic motion divides the loop length.
- Sources live in [`marketing/videos/brand-motion/`](../marketing/videos/brand-motion/README.md)
  (Remotion): `LogoReveal`, `MascotIdle`, and the 3D `Puffin3D` and `Icon3D` explorations, which
  are not yet part of the brand.

### Social card

[`website/assets/og.png`](../website/assets/og.png) (1200×630), rendered from
[`website/og/card.html`](../website/og/card.html) with `python website/og/render.py` and committed.
Every page (landing, docs, viewer) sets it as `og:image` / `twitter:image` when the build knows the
public site URL.

## Colour

| Role | Token | Light | Use |
|---|---|---|---|
| Paper | `--paper`, `--paper-2`, `--paper-3` | `#ffffff` `#f7f8fa` `#eef0f4` | Page ground; recessed wells (code, table stripes); pressed/hover |
| Ink | `--ink`, `--ink-2`, `--ink-3` | `#14171c` `#4f5866` `#687180` | Primary text (18:1); secondary (7.2:1); labels and captions (4.9:1) |
| Rules | `--rule`, `--rule-2` | `#e3e6eb` `#eef0f3` | Borders and hairlines; dividers inside a component |
| Scan (accent) | `--scan`, `--scan-ink`, `--scan-wash`, `--scan-edge` | `#ff5230` `#c43818` `#fff1ec` `#ffc9b9` | Marks, bars, the scan line (graphic); accent text (5.3:1); selected ground; selected border |
| Verdict | `--good`, `--fair`, `--poor` (+ `-wash`) | `#16794a` `#9a6200` `#b42346` | Score states. Always paired with a CSS-drawn shape: filled dot good, half-filled dot fair, ring poor |
| Proof marks | `--proof-del`, `--proof-ins` (+ `-wash`) | `#b42346` `#16794a` | Diff: missing from the output is struck through; extra in the output is underlined |
| Modes | `--mode-parse`, `--mode-ocr`, `--mode-extract` | `#b8431c` `#2c5fd6` `#6b3fc0` | Labels for the three modes only |
| Layout boxes | `--box-*` (8 hues) | see tokens | Block-type overlays on a page: text, title, table, figure, list, furniture, formula, other |
| Brand | `--mark-tile`, `--mascot-body`, `--mascot-wing` | `#14171c` `#14171c` `#2a303a` | The mark's tile; the mascot's plumage. Dark: `#2a303a` `#2b313b` `#3b424e` |

Neutrals lean slightly toward blue ink. There is no pure grey. Links are ink with a quiet underline
(`--link-underline`); they turn `--scan-ink` on hover. The accent is **not** a link colour.

Dark theme redefines every token (never introduces new ones). The accent brightens to `#ff6a45`,
verdicts lift to stay above 6:1, and the paper stays near-black with the same blue bias.

**Score thresholds.** A score (0–100) is *good* at ≥ 90, *fair* at 70–89.9, *poor* below 70. The
thresholds apply to the colour and shape only; the number is always shown.

## Type

System faces only: `--font-sans` for everything readable, `--font-mono` for model ids, code, raw
ids, file paths and numbers that line up in columns (always with `font-variant-numeric:
tabular-nums`).

| Token | Size | Use |
|---|---|---|
| `--t-xs` | 12px | Uppercase labels (tracking `--tracking-label`) outside tables, captions |
| `--t-sm` | 13px | Table cells, meta lines, chips |
| `--t-md` | 14px | UI body (viewer, controls) |
| `--t-base` | 16px | Reading body (docs) |
| `--t-lg` | 20px | Section titles |
| `--t-xl` | 28px | Page titles |
| `--t-2xl` | 40px | The one hero number or headline on a view |

Weights: 400 regular, 540 medium (UI emphasis), 640 strong (titles, the primary number). Headings
use `text-wrap: balance`. Reading text stays within `--measure` (68ch).

## Space, shape, motion

- **Space:** a 4px grid, `--s-1` (4px) to `--s-8` (72px). Lay out siblings with flex/grid `gap`,
  never per-element margins. Side gutter is `--gutter` at every width.
- **Shape:** `--r-1` 4px for chips, code and inputs; `--r-2` 7px for buttons and panels;
  `--r-pill` only for segmented controls. Documents are rectangles; nothing is a bubble.
- **Borders over shadows.** A hairline `--rule` separates; `--lift` is only for things that float
  above the page (menus, dialogs). Most sections need neither: whitespace separates them.
- **Focus:** a solid 2px `--scan-ink` outline offset by 2px on every interactive element (3:1 or
  better on every ground in both themes). Never remove focus styles.
- **Motion:** `--dur` 160ms with `--ease`, for state changes only (tab, disclosure, hover). No
  entrance animations in tools. Everything is disabled under `prefers-reduced-motion`.

## Signatures

Besides the puffin (see Brand), two interface motifs make PuffinParse recognisable. Each is used
sparingly.

- **The bounding box** (`.bbox`): four corner ticks in `--scan`, like a detector's box around a
  region. It marks the single focused object on a view: the selected model, the headline score.
  Never more than one per view, never on every card.
- **The scan line** (`.scanline`): a short 2px `--scan` rule. It underlines the active tab and
  leads an eyebrow label. At most once per region. (It used to stand in for a logo; the puffin
  mark does that now.)

## Components

- **Score:** the number in `--font-mono` tabular, a verdict shape and colour, and optionally a
  thin bar (`--scan` for the headline, `--ink-3` elsewhere). Scores are never cell fills by default.
- **Chip:** `--t-sm`, `--r-1`, `--paper-2` ground, no border. Selected: `--scan-wash` ground,
  `--scan-edge` border. Chips carry a model id and, at most, one number.
- **Tabs:** text labels, the active one underlined by the scan line. No boxed tabs.
- **Disclosure:** a native `<details>` with a quiet chevron and a `--t-sm` summary in `--ink-2`.
  Use it for anything secondary: details, all metrics, methodology, reproduce commands.
- **Table:** hairline row dividers only, no vertical rules, header in `--t-sm` sentence case
  `--ink-3` (never uppercase: column names include brand names such as DP-Bench and olmOCR-bench),
  numbers right-aligned and tabular. The best value in a column is bold, not coloured, and only
  when it is unique as displayed: two cells that both read "100.0" are a tie, and nothing is bold.
- **Check row** (rule results): one line, a ✓ or ✗ shape in the verdict colour, then a plain
  sentence ("Should not contain “ARTICLE IN PRESS”"). A second muted line only when it adds
  information.
- **Diff:** proof marks. `--proof-del` strikethrough on `--proof-del-wash` for text missing from the
  output; `--proof-ins` underline on `--proof-ins-wash` for extra text.
- **Code block:** `--paper-2` ground, `--r-1`, `--t-sm` mono, a Copy button that says "Copied" for
  1.5s. Long lines scroll inside the block.
- **Layout-box overlay:** 1.5px strokes in the `--box-*` hue with a 12% fill, drawn only on top of
  a rendered page image, never on an empty placeholder.

## Writing

- Name things by what developers recognise: "Score", "Checks", "Output", "Truth", "Compare
  models", "Reproduce this score".
- Verdicts are plain sentences: "0 of 6 checks pass", "99.1% character match".
- No marketing adjectives in tools. No exclamation marks. Errors say what failed and what to do.

## Do and don't

| Do | Don't |
|---|---|
| One accent mark per view | Accent on every heading, badge and border |
| Plain numbers, a unique best in bold | Full-table heatmaps, bold ties |
| Collapse methodology, commands, raw ids | Paragraphs of attribution above the content |
| System fonts, tabular numbers | Web fonts, emoji section markers |
| The puffin in brand moments, once per view | The puffin beside data, or several on a page |
| Hairlines and whitespace | A card with a shadow around every block |
