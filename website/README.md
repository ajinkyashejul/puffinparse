# LiteOCR documentation site

The static docs site at the root of GitHub Pages. One Python script, one stylesheet, one small
JavaScript file — no toolchain, no framework, no CDN, no web fonts.

It does not duplicate documentation: most pages render markdown that already lives in the repo
(`README.md`, `docs/`, `benchmark/`). Only four pages are written for the site, under
`website/pages/`.

## Build

```bash
pip install markdown             # the only dependency; everything else is stdlib
python website/build.py          # -> website/dist/
```

| Flag | Default | What it does |
|---|---|---|
| `--base-url <PATH>` | `/` | URL prefix, for hosting under a subpath (`--base-url /docs`). |
| `--out <DIR>` | `website/dist` | Output directory. Wiped and rebuilt on every run. |
| `--site-url <URL>` | `https://ajinkyashejul.github.io/liteocr` | Public base URL of the deployed site, used for `<link rel="canonical">` and `sitemap.xml`. Independent of `--base-url`. |
| `--check` | off | After building, verify that every internal link and `#fragment` resolves. |

```bash
python website/build.py --check                       # build + link check (what CI should run)
python website/build.py --base-url /docs --out /tmp/s # preview a subpath deployment
```

`website/dist/` is generated and git-ignored.

## Preview

```bash
python -m http.server -d website/dist 8001
# http://localhost:8001
```

`http.server` serves `index.html` for a directory URL, so pretty URLs work locally exactly as they
do on GitHub Pages.

## How `nav.json` works

`website/nav.json` is the whole site map. `sections` is an ordered list; each has a `title` (used as
the sidebar group heading — empty means no heading) and an ordered list of `pages`:

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
| `slug` | yes | Site path. `""` is the home page; `project/spec` becomes `/project/spec/`. |
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

- if the target is a page on the site, it becomes the site URL (`docs/SPEC.md` → `/project/spec/`,
  `../benchmark/README.md` → `/benchmark/`);
- otherwise it becomes a GitHub URL on the branch in `nav.json`
  (`LICENSE` → `.../blob/main/LICENSE`, a directory → `.../tree/main/...`);
- a shorthand anchor such as `#adr-9` is expanded to the heading id the site actually generated.

Links inside fenced code blocks are never touched. Pages authored under `website/pages/` may use
absolute site paths (`/python/`) directly.

## What is generated

| Path | What |
|---|---|
| `<slug>/index.html` | The page. Pretty URL, no extension needed. |
| `<slug>/index.md` | The same page as raw markdown, links rewritten, for agents and `curl`. |
| `llms.txt` | [llms.txt](https://llmstxt.org) site map: title, summary, `## Docs`, `## Optional`. |
| `llms-full.txt` | Every page concatenated in nav order, with `# Title` and a source-path comment. |
| `search.json` | Page titles + headings, fetched lazily by the search box. |
| `sitemap.xml`, `robots.txt` | Standard crawler files, built from `--site-url`. |
| `404.html` | Not-found page, styled like the rest. |
| `style.css`, `app.js` | Copied verbatim from `website/assets/`. |

Every HTML page carries `<meta name="description">`, `<link rel="canonical">` and
`<link rel="alternate" type="text/markdown">` pointing at its `index.md`, plus a "View as Markdown"
link in the footer.

Two small blocks are generated from live repository data rather than prose, so they cannot drift:

- the home page **leaderboard strip** reads `benchmark/results/*.json` (best score per model, top 5);
- the **provider cards** on `/providers/` read `crates/liteocr-core/src/model.rs` and
  `pricing.json` for display names, env vars, model lists and vendor doc links.

Both degrade to nothing if those files are missing.

## Design notes

`website/assets/style.css` is the entire design: system font stack, ~76ch measure, hairline
borders, one accent colour used only for links and the active nav item, tabular numerals in
tables, light/dark from `prefers-color-scheme` with a toggle that remembers the choice. Under
1100px the "On this page" column drops; under 860px the sidebar collapses into a
`<details>` menu.

`website/assets/app.js` is progressive enhancement only — the site is fully readable with
JavaScript disabled. It adds: copy buttons on code blocks, the animated model string in the hero,
scroll-spy on "On this page", the theme toggle, and the client-side search (`/` or `⌘K` to focus).

Keep the CSS under ~250 lines and resist adding a build step.

## Deploying

GitHub Pages serves this at the **root** of the site, built by the Pages workflow (owned by the
maintainer, not by this directory). The benchmark results viewer is a separate artifact and is
published under **`/benchmark-results/`**, so the two never collide. Links from the docs to the
viewer should therefore point at `/benchmark-results/`.

The two URL flags are independent: `--base-url` decides how links are *written* in the HTML
(`/python/` vs `/docs/python/`), `--site-url` decides the absolute address used for canonical links
and the sitemap. For a project page served at `https://user.github.io/liteocr/`, build with
`--base-url /liteocr --site-url https://user.github.io/liteocr`. Every internal URL, including
`llms.txt` and the sitemap, follows from those two flags.
