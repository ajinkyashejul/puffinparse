# LiteOCR design language

LiteOCR reads documents for developers, so its interface borrows from the things that happen when a
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
   decorative imagery. A page should render usefully before any script runs.
5. **Colour carries meaning.** The accent marks the one primary thing. Verdict colours mark good,
   fair and poor. Mode colours label parse, OCR and extract. Nothing is coloured for decoration.

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
| `--t-xs` | 12px | Uppercase labels (tracking `--tracking-label`), captions |
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

Two motifs make LiteOCR recognisable. Each is used sparingly.

- **The bounding box** (`.bbox`): four corner ticks in `--scan`, like a detector's box around a
  region. It marks the single focused object on a view: the selected model, the headline score.
  Never more than one per view, never on every card.
- **The scan line** (`.scanline`): a short 2px `--scan` rule. It is the brand mark next to the
  wordmark and the underline of the active tab. At most once per region.

## Components

- **Score:** the number in `--font-mono` tabular, a verdict shape and colour, and optionally a
  thin bar (`--scan` for the headline, `--ink-3` elsewhere). Scores are never cell fills by default.
- **Chip:** `--t-sm`, `--r-1`, `--paper-2` ground, no border. Selected: `--scan-wash` ground,
  `--scan-edge` border. Chips carry a model id and, at most, one number.
- **Tabs:** text labels, the active one underlined by the scan line. No boxed tabs.
- **Disclosure:** a native `<details>` with a quiet chevron and a `--t-sm` summary in `--ink-2`.
  Use it for anything secondary: details, all metrics, methodology, reproduce commands.
- **Table:** hairline row dividers only, no vertical rules, header in `--t-xs` uppercase
  `--ink-3`, numbers right-aligned and tabular. The best value in a column is bold, not coloured.
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
| Plain numbers, best in bold | Full-table heatmaps |
| Collapse methodology, commands, raw ids | Paragraphs of attribution above the content |
| System fonts, tabular numbers | Web fonts, emoji section markers |
| Hairlines and whitespace | A card with a shadow around every block |
