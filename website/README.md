# PuffinParse website

The whole product site: a **landing page at `/`** and the **documentation under `/docs/`**. One
Python script, two stylesheets, one small JavaScript file — no toolchain, no framework, no CDN, no
web fonts.

```
website/
  build.py            the entire build (landing section + docs section + link check + redirects)
  nav.json            the docs site map: sections, pages, sources, descriptions
  landing/index.html  the landing page template; {{PLACEHOLDER}}s filled from repo data
  pages/*.md          the four pages written for the site (everything else is repo markdown)
  assets/style.css    the shared design system (docs chrome, prose, tokens)
  assets/landing.css  the landing layer: same tokens, bigger type, one vermilion accent
  assets/app.js       progressive enhancement for both halves
  dist/               generated, git-ignored
```

The docs do not duplicate documentation: most pages render markdown that already lives in the repo
(`README.md`, `docs/`, `benchmark/`). Only four pages are written for the site, under
`website/pages/`.

The landing page duplicates nothing either — every number, provider, model and benchmark row on it
is read out of the repository at build time (see *The landing page* below).

## Build

```bash
pip install markdown             # the only dependency; everything else is stdlib
python website/build.py          # -> website/dist/
```

| Flag | Default | What it does |
|---|---|---|
| `--base-url <PATH>` | `/` | URL prefix, for hosting under a subpath (`--base-url /docs`). |
| `--out <DIR>` | `website/dist` | Output directory. Wiped and rebuilt on every run. |
| `--site-url <URL>` | `https://ajinkyashejul.github.io/puffinparse` | Public base URL of the deployed site, used for `<link rel="canonical">` and `sitemap.xml`. Independent of `--base-url`. |
| `--docs-prefix <PATH>` | `docs` | Where the documentation is mounted below `--base-url`. The landing page always owns `--base-url` itself. |
| `--with-benchmark` / `--no-benchmark` | Also build the results viewer into `<out>/benchmark-results/` (default on) |
| `--check` | off | After building, verify that every internal link and `#fragment` resolves — on the landing page as well as the docs — and that `vercel.json` still redirects every old docs URL. |
| `--write-redirects` | off | Rewrite the `redirects` array in `vercel.json` from `nav.json`. Run it after adding, renaming or removing a page. |

```bash
python website/build.py --check                        # build + link check (what CI should run)
python website/build.py --write-redirects              # refresh the old-URL redirect map
python website/build.py --base-url /preview --out /tmp/s  # preview a subpath deployment
```

`website/dist/` is generated and git-ignored.

## Preview

```bash
python -m http.server -d website/dist 8001
# http://localhost:8001
```

`http.server` serves `index.html` for a directory URL, so pretty URLs work locally exactly as they
do in production: `/` is the landing page, `/docs/` the documentation home, `/docs/python/` a page.

## How `nav.json` works

`website/nav.json` is the whole **docs** site map (the landing page is not in it). `sections` is an
ordered list; each has a `title` (used as the sidebar group heading — empty means no heading) and an
ordered list of `pages`:

```json
{
  "slug": "project/spec",
  "title": "Specification",
  "nav_title": "Spec",
  "source": "docs/SPEC.md",
  "description": "One sentence, used for <meta name=description> and llms.txt.",
  "optional": true
}
```

| Key | Required | Meaning |
|---|---|---|
| `slug` | yes | Path below the docs prefix. `""` is the docs index at `/docs/`; `project/spec` becomes `/docs/project/spec/`. |
| `title` | yes | `<h1>`-level name, used in `<title>`, `llms.txt` and search. |
| `nav_title` | no | Shorter label for the sidebar. Defaults to `title`. |
| `source` | yes | Path of the markdown file, relative to the repository root. |
| `description` | yes | One line. Becomes the meta description and the `llms.txt` entry. |
| `optional` | no | Puts the page under `## Optional` in `llms.txt` instead of `## Docs`. |
| `home` | no | Home page: badges are stripped and the hero, leaderboard and agent cards are prepended. |
| `provider_index` | no | Prepends the generated provider cards. |

To add a page: write or pick the markdown, add an entry, rebuild. Nothing else references the list.

**Links are rewritten automatically.** A relative link in any source file is resolved against that
file's directory and then:

- if the target is a page on the site, it becomes the site URL (`docs/SPEC.md` → `/docs/project/spec/`,
  `../benchmark/README.md` → `/docs/benchmark/`);
- otherwise it becomes a GitHub URL on the branch in `nav.json`
  (`LICENSE` → `.../blob/main/LICENSE`, a directory → `.../tree/main/...`);
- a shorthand anchor such as `#adr-9` is expanded to the heading id the site actually generated.

Links inside fenced code blocks are never touched. Pages authored under `website/pages/` may use
absolute docs paths (`/python/`) directly — they are resolved through the same slug table, so they
follow the docs prefix and need no editing if it ever changes.

## What is generated

| Path | What |
|---|---|
| `index.html` | The landing page, from `website/landing/index.html`. |
| `docs/<slug>/index.html` | A docs page. Pretty URL, no extension needed; `docs/index.html` is the docs home. |
| `docs/<slug>/index.md` | The same page as raw markdown, links rewritten, for agents and `curl`. |
| `llms.txt` | [llms.txt](https://llmstxt.org) site map, at the **site root**; entries point at `/docs/<slug>/index.md`. |
| `llms-full.txt` | Every docs page concatenated in nav order, with `# Title` and a source-path comment. |
| `search.json` | Page titles + headings, fetched lazily by the search box. |
| `sitemap.xml`, `robots.txt` | Standard crawler files, built from `--site-url`. The sitemap lists the landing page and every docs page. |
| `404.html` | Not-found page, styled like the rest, pointing at both halves. |
| `style.css`, `landing.css`, `app.js` | Copied verbatim from `website/assets/`. |

Every HTML page carries `<meta name="description">`, `<link rel="canonical">` and
`<link rel="alternate" type="text/markdown">` pointing at its `index.md`, plus a "View as Markdown"
link in the footer.

Two small blocks in the docs are generated from live repository data rather than prose, so they
cannot drift:

- the docs home **leaderboard strip** reads `benchmark/results/*.json` (best score per model, top 5);
- the **provider cards** on `/docs/providers/` read `crates/puffinparse-core/src/model.rs` and
  `pricing.json` for display names, env vars, model lists and vendor doc links.

Both degrade to nothing if those files are missing.

## The landing page

`website/landing/index.html` is a plain HTML template. `build.py` replaces every `{{NAME}}`
placeholder and writes the result to `dist/index.html`; an unsubstituted placeholder fails the
build. Nothing on the page is hand-maintained prose about counts or results:

| Source | What it feeds |
|---|---|
| `crates/puffinparse-core/src/model.rs` | `read_registry()` parses the `PROVIDERS` table — provider id, display name, every `ModelInfo` and its `modes` expression (`PARSE_OCR`, `Mode::ALL`, `&[Mode::Extract]`). It drives the provider grid (one card per provider: id, name, mode chips, model count), the `15 / 53 / 3` stat band, and the list of model strings the hero cycles through. |
| `docs/providers/README.md` | The status column of the provider table → the *live-verified* / *verified locally* / *docs-only* dot on each card, and the per-status counts in the section intro. |
| `benchmark/results/*.json` | `read_leaderboard(7)` — best score per model → the leaderboard table (the same reader the docs home uses with a limit of 5). |
| `website/nav.json` | Every link on the page is `site.url(slug)`, so the landing follows the docs prefix automatically. |

The three literal blocks left in the template are the mode sketches, the compatibility example and
the `bench run` command — illustrations, not data.

`website/assets/landing.css` layers on the same tokens as `style.css` (it never redefines them) and
adds one vermilion accent, a larger type scale and the landing components. Two moments of motion,
both skipped under `prefers-reduced-motion` and both degrading to a complete static page without
JavaScript: the hero scanner (a sweep across a skeleton document, then the response typed out, then
the next model string) and the leaderboard rows fading in on scroll. The provider tab strip is
CSS-only — stacked radio panels in one grid cell, so switching tabs shifts nothing.

## Redirects

The docs used to live at the site root, so `vercel.json` carries one permanent redirect per docs
slug (`/python/` → `/docs/python/`, 31 of them; `/` belongs to the landing page and `/llms.txt`
stays at the root). They are generated, not hand-written:

```bash
python website/build.py --write-redirects     # rewrites only the "redirects" array in vercel.json
```

`--check` fails if that array has drifted from `nav.json`, or if a redirect points at a page that
does not exist in `dist/`. Run `--write-redirects` and commit `vercel.json` whenever a page is
added, renamed or removed.

## Design notes

`website/assets/style.css` is the entire design: system font stack, ~76ch measure, hairline
borders, one accent colour used only for links and the active nav item, tabular numerals in
tables, light/dark from `prefers-color-scheme` with a toggle that remembers the choice. Under
1100px the "On this page" column drops; under 860px the sidebar collapses into a
`<details>` menu.

`website/assets/landing.css` is the only other stylesheet. It defines no new base tokens: it reads
`--bg`, `--fg`, `--line`, `--mono` and friends from `style.css` and adds its own `--lp-*` layer on
top, so a change to the design system moves both halves at once.

`website/assets/app.js` serves both halves and is progressive enhancement only — every page is
fully readable with JavaScript disabled. It adds: copy buttons on code blocks and on the landing's
`pip install` pill, the animated model string in the docs hero, the landing's hero scanner and
leaderboard reveal, scroll-spy on "On this page", the theme toggle, and the client-side search
(`/` or `⌘K` to focus). Everything motion-related is skipped under `prefers-reduced-motion`.

Keep `style.css` under ~250 lines, `landing.css` under ~300, and resist adding a build step.

## Deploying

Vercel builds and serves the site from `vercel.json`: `buildCommand` runs this script with
`--site-url https://puffinparse.com`, `outputDirectory` is `website/dist`, and `cleanUrls` +
`trailingSlash` give the pretty URLs. The benchmark results viewer is a separate artifact published
under **`/benchmark-results/`**, so the two never collide.

The URL flags are independent. `--base-url` decides how links are *written* in the HTML,
`--docs-prefix` decides where the docs sit below it, and `--site-url` decides the absolute address
used for canonical links and the sitemap. For a project page served at
`https://user.github.io/puffinparse/`, build with `--base-url /puffinparse --site-url
https://user.github.io/puffinparse`. Every internal URL, including `llms.txt` and the sitemap, follows
from those flags.
