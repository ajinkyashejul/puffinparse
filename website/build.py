#!/usr/bin/env python3
"""Build the LiteOCR documentation site.

Reads the site map in ``website/nav.json``, renders every listed markdown file (existing repo
docs plus the pages under ``website/pages/``) into ``website/dist/`` as pretty URLs, and emits the
agent-friendly companions: a ``index.md`` next to every page, ``llms.txt``, ``llms-full.txt``,
``search.json``, ``sitemap.xml``, ``robots.txt`` and ``404.html``.

    python website/build.py                     # -> website/dist, base URL "/"
    python website/build.py --base-url /docs/   # hosted under a subpath
    python website/build.py --check             # build, then verify every internal link

The only third-party dependency is ``markdown`` (``pip install markdown``); everything else is
standard library. No syntax highlighter is used, so code blocks stay plain ``<pre><code>``.
"""

from __future__ import annotations

import argparse
import glob
import html
import json
import os
import posixpath
import re
import shutil
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional

import markdown

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "website"

FAVICON = (
    "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'%3E"
    "%3Crect width='16' height='16' rx='3.5' fill='%232c5fd6'/%3E%3Cg fill='white'%3E"
    "%3Crect x='4' y='4' width='8' height='1.6' rx='.8'/%3E"
    "%3Crect x='4' y='7.2' width='8' height='1.6' rx='.8'/%3E"
    "%3Crect x='4' y='10.4' width='5' height='1.6' rx='.8'/%3E%3C/g%3E%3C/svg%3E"
)

THEME_BUTTON = (
    '<button class="iconbtn" id="theme" type="button" aria-label="Switch theme" title="Switch theme">'
    '<svg width="15" height="15" viewBox="0 0 16 16" aria-hidden="true">'
    '<circle cx="8" cy="8" r="6.4" fill="none" stroke="currentColor" stroke-width="1.4"/>'
    '<path d="M8 1.6a6.4 6.4 0 0 0 0 12.8z" fill="currentColor"/></svg></button>'
)

FENCE_RE = re.compile(r"(^```.*?^```|^~~~.*?^~~~)", re.M | re.S)
LINK_RE = re.compile(r"(\]\()(?!https?://|mailto:|#)([^)\s]+)((?:\s+\"[^\"]*\")?\))")
REFDEF_RE = re.compile(r"^(\s{0,3}\[[^\]]+\]:[ \t]*)(?!https?://|mailto:|#)(\S+)", re.M)
TABLE_OPEN_RE = re.compile(r"<table>")
H1_RE = re.compile(r"^#\s+.*$", re.M)


# ---------------------------------------------------------------------------------------- model


@dataclass
class Page:
    slug: str
    title: str
    nav_title: str
    source: str
    description: str
    section: str
    optional: bool = False
    home: bool = False
    provider_index: bool = False
    has_md: bool = True
    text: str = ""
    body_html: str = ""
    toc: list[dict[str, Any]] = field(default_factory=list)


@dataclass
class Site:
    title: str
    tagline: str
    summary: str
    repo: str
    branch: str
    license: str
    base: str
    site_url: str

    def blob(self, path: str, tree: bool = False) -> str:
        kind = "tree" if tree else "blob"
        return f"{self.repo}/{kind}/{self.branch}/{path}"

    def url(self, slug: str, md: bool = False) -> str:
        u = self.base if not slug else f"{self.base}{slug}/"
        return u + "index.md" if md else u

    def absolute(self, rel: str) -> str:
        """Public URL for a base-relative path. ``site_url`` and ``base`` are independent:
        ``base`` is how links are written in the HTML, ``site_url`` is where the site is served."""
        if not self.site_url:
            return rel
        path = rel[len(self.base) :] if rel.startswith(self.base) else rel.lstrip("/")
        return f"{self.site_url.rstrip('/')}/{path}"


def load_nav(path: Path, base: str, site_url: str) -> tuple[Site, list[Page]]:
    data = json.loads(path.read_text(encoding="utf-8"))
    s = data["site"]
    site = Site(
        title=s["title"],
        tagline=s["tagline"],
        summary=s["summary"],
        repo=s["repo"].rstrip("/"),
        branch=s.get("branch", "main"),
        license=s.get("license", "MIT"),
        base=base,
        site_url=site_url,
    )
    pages: list[Page] = []
    for section in data["sections"]:
        for p in section["pages"]:
            pages.append(
                Page(
                    slug=p["slug"].strip("/"),
                    title=p["title"],
                    nav_title=p.get("nav_title", p["title"]),
                    source=p["source"],
                    description=p["description"],
                    section=section["title"],
                    optional=bool(p.get("optional")),
                    home=bool(p.get("home")),
                    provider_index=bool(p.get("provider_index")),
                )
            )
    return site, pages


# ------------------------------------------------------------------------------- link rewriting


def rewrite_links(text: str, source: str, site: Site, by_source: dict[str, Page], md_mode: bool) -> str:
    """Point every relative link at its site URL, or at the GitHub blob when it is not on the site."""
    src_dir = posixpath.dirname(source)

    def resolve(target: str) -> str:
        target, _, frag = target.partition("#")
        frag = f"#{frag}" if frag else ""
        if not target:
            return frag
        if target.startswith("/"):  # authored as an absolute site path, e.g. /python/
            return site.url(target.strip("/"), md_mode) + frag
        path = posixpath.normpath(posixpath.join(src_dir, target))
        page = by_source.get(path)
        if page is not None:
            return site.url(page.slug, md_mode) + frag
        is_dir = (ROOT / path).is_dir()
        return site.blob(path.rstrip("/"), tree=is_dir) + frag

    def sub_links(chunk: str) -> str:
        chunk = LINK_RE.sub(lambda m: m.group(1) + resolve(m.group(2)) + m.group(3), chunk)
        return REFDEF_RE.sub(lambda m: m.group(1) + resolve(m.group(2)), chunk)

    # Never touch anything inside a fenced code block.
    return "".join(part if i % 2 else sub_links(part) for i, part in enumerate(FENCE_RE.split(text)))


# ------------------------------------------------------------------------------------ rendering


def make_md() -> markdown.Markdown:
    return markdown.Markdown(
        extensions=["fenced_code", "tables", "toc"],
        extension_configs={"toc": {"permalink": "#", "permalink_class": "headerlink", "toc_depth": "2-3"}},
        output_format="html",
    )


def wrap_tables(body: str) -> str:
    body = TABLE_OPEN_RE.sub('<div class="table-wrap"><table>', body)
    return body.replace("</table>", "</table></div>")


def render_toc(tokens: list[dict[str, Any]]) -> str:
    if not tokens:
        return ""
    out = ["<ul>"]
    for t in tokens:
        out.append(f'<li><a href="#{t["id"]}">{html.escape(t["name"])}</a>')
        out.append(render_toc(t.get("children") or []))
        out.append("</li>")
    out.append("</ul>")
    return "".join(out)


def flat_headings(tokens: list[dict[str, Any]]) -> list[list[str]]:
    out: list[list[str]] = []
    for t in tokens:
        out.append([t["name"], t["id"]])
        out.extend(flat_headings(t.get("children") or []))
    return out


# ------------------------------------------------------------------------------- generated bits


def read_providers() -> list[dict[str, Any]]:
    """Provider metadata straight from the Rust registry + price table, so it cannot drift."""
    model_rs = ROOT / "crates/liteocr-core/src/model.rs"
    pricing_json = ROOT / "crates/liteocr-core/src/pricing.json"
    if not model_rs.exists() or not pricing_json.exists():
        return []
    prices = {k: v for k, v in json.loads(pricing_json.read_text(encoding="utf-8")).items() if "/" in k}
    fields = ("name", "display_name", "env_var", "base_url", "docs")
    found: list[dict[str, Any]] = []
    for block in re.finditer(
        r'name:\s*"(\w+)",\s*display_name:\s*"([^"]+)",\s*env_var:\s*"(\w+)",\s*'
        r'base_url:\s*"([^"]+)",\s*docs:\s*"([^"]+)"',
        model_rs.read_text(encoding="utf-8"),
    ):
        info = dict(zip(fields, block.groups()))
        info["models"] = sorted(m for m in prices if m.startswith(info["name"] + "/"))
        found.append(info)
    return found


def read_leaderboard(limit: int = 5) -> tuple[list[dict[str, Any]], str]:
    """Top models across every committed benchmark result, best score per model."""
    best: dict[str, dict[str, Any]] = {}
    dataset = ""
    for path in sorted(glob.glob(str(ROOT / "benchmark/results/*.json"))):
        try:
            run = json.loads(Path(path).read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        ds = run.get("dataset", {})
        dataset = f"{ds.get('name', '?')} v{ds.get('version', '?')} · {ds.get('documents', '?')} documents"
        for m in run.get("models", []):
            s = m.get("summary", {})
            row = {
                "model": m.get("model", "?"),
                "overall": s.get("overall"),
                "p50": s.get("latency_p50_ms"),
                "cost": s.get("cost_per_1k_pages_usd"),
            }
            if row["overall"] is None:
                continue
            prev = best.get(row["model"])
            if prev is None or row["overall"] > prev["overall"]:
                best[row["model"]] = row
    rows = sorted(best.values(), key=lambda r: -r["overall"])[:limit]
    return rows, dataset


def leaderboard_card(site: Site) -> str:
    rows, dataset = read_leaderboard()
    if not rows:
        return ""
    cells = "".join(
        "<tr>"
        f"<td>{i + 1}</td>"
        f"<td><code>{html.escape(r['model'])}</code></td>"
        f"<td><strong>{r['overall']:.2f}</strong></td>"
        f"<td>{r['p50']:,} ms</td>"
        f"<td>${r['cost']:.2f}</td>"
        "</tr>"
        for i, r in enumerate(rows)
    )
    return f"""<section class="card">
<h2 id="leaderboard-strip">Open benchmark<a class="headerlink" href="#leaderboard-strip">#</a></h2>
<p>Accuracy, latency and cost, measured through the same client you would use. Exact ground truth,
deterministic metrics, no LLM judge.</p>
<div class="table-wrap"><table>
<thead><tr><th>#</th><th>Model</th><th>Overall</th><th>p50 latency</th><th>$/1k pages</th></tr></thead>
<tbody>{cells}</tbody></table></div>
<p class="meta">{html.escape(dataset)} — <a href="{site.url("benchmark/leaderboard")}">full leaderboard</a>
· <a href="{site.url("benchmark")}">methodology and caveats</a></p>
</section>"""


def hero(site: Site, models: list[str]) -> str:
    fallback = models[0] if models else "reducto/standard"
    default = "reducto/standard" if "reducto/standard" in models else fallback
    swap = html.escape(json.dumps(models or [default]), quote=True)
    span = f'<span id="swap" data-models="{swap}">{default}</span>'
    return f"""<section class="hero">
<h1>{html.escape(site.title)}</h1>
<p class="lede"><strong>{html.escape(site.tagline)}</strong> Rust core, Python SDK, CLI, and an open
benchmark that ranks providers on accuracy, latency and cost.</p>
<div class="codewrap"><pre><code class="language-python">import liteocr
doc = liteocr.parse(&quot;invoice.pdf&quot;, model=&quot;{span}&quot;)
print(doc.markdown, doc.cost_usd)</code></pre></div>
<p class="meta">Switch providers by changing one string. Same request, same response shape, same errors.</p>
<div class="cta">
<a class="btn primary" href="{site.url("getting-started")}">Get started</a>
<code>pip install liteocr</code>
<a class="btn" href="{site.url("benchmark/leaderboard")}">Leaderboard</a>
<a class="btn" href="{site.repo}">GitHub</a>
</div>
</section>"""


def agents_card(site: Site) -> str:
    return f"""<section class="card">
<h2 id="for-agents">For agents<a class="headerlink" href="#for-agents">#</a></h2>
<p>Every page on this site is also served as plain markdown at <code>&lt;page&gt;/index.md</code>, and is
linked from each HTML page with <code>&lt;link rel="alternate" type="text/markdown"&gt;</code>. Three entry
points are meant for you:</p>
<ul>
<li><a href="{site.base}llms.txt"><code>/llms.txt</code></a> — the site map, with a one-line
description of every page.</li>
<li><a href="{site.base}llms-full.txt"><code>/llms-full.txt</code></a> — every page concatenated,
in nav order.</li>
<li><a href="{site.url("project/for-agents")}"><code>/project/for-agents/</code></a> — the working agreement
for contributing to this repository: branch, required checks, and where each kind of change goes.</li>
</ul>
</section>"""


def provider_cards(site: Site, providers: list[dict[str, Any]], by_slug: dict[str, Page]) -> str:
    if not providers:
        return ""
    cards = []
    for p in providers:
        slug = f"providers/{p['name']}"
        if slug not in by_slug:
            continue
        models = "".join(f"<code>{html.escape(m.split('/', 1)[1])}</code> " for m in p["models"])
        cards.append(
            f'<div class="card"><h3><a href="{site.url(slug)}">{html.escape(p["display_name"])}</a></h3>'
            f"<p><code>{html.escape(p['env_var'])}</code></p><p>{models}</p>"
            f'<p class="meta"><a href="{html.escape(p["docs"])}">Vendor docs</a></p></div>'
        )
    return f'<div class="grid3">{"".join(cards)}</div>' if cards else ""


# ------------------------------------------------------------------------------------- template


def nav_html(site: Site, pages: list[Page], current: str) -> str:
    out: list[str] = ["<nav>"]
    section = None
    for p in pages:
        if p.section != section:
            if section is not None:
                out.append("</ul>")
            section = p.section
            if section:
                out.append(f"<h4>{html.escape(section)}</h4>")
            out.append("<ul>")
        aria = ' aria-current="page"' if p.slug == current else ""
        out.append(f'<li><a href="{site.url(p.slug)}"{aria}>{html.escape(p.nav_title)}</a></li>')
    out.append("</ul></nav>")
    return "".join(out)


def page_html(site: Site, page: Page, pages: list[Page]) -> str:
    nav = nav_html(site, pages, page.slug)
    toc = render_toc(page.toc)
    aside = (
        f'<aside class="toc"><nav><strong>On this page</strong>{toc}</nav></aside>'
        if toc
        else "<aside></aside>"
    )
    title = page.title if page.home else f"{page.title} · {site.title}"
    canonical = site.absolute(site.url(page.slug))
    md_url = site.url(page.slug, md=True)
    edit = site.blob(page.source)
    alt = (
        f'<link rel="alternate" type="text/markdown" href="{md_url}" '
        f'title="{html.escape(page.title)} (Markdown)">'
        if page.has_md
        else ""
    )
    md_link = f'<span class="sep"><a href="{md_url}">View as Markdown</a></span>' if page.has_md else ""
    return f"""<!doctype html>
<html lang="en" data-base="{site.base}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)}</title>
<meta name="description" content="{html.escape(page.description, quote=True)}">
<link rel="canonical" href="{canonical}">
{alt}
<link rel="icon" href="{FAVICON}">
<link rel="stylesheet" href="{site.base}style.css">
<script>(function(){{try{{var t=localStorage.getItem('liteocr-theme');
if(t)document.documentElement.setAttribute('data-theme',t);}}catch(e){{}}}})();</script>
</head>
<body>
<header class="top"><div class="topin">
<a class="brand" href="{site.base}">LiteOCR<span class="hide-sm">docs</span></a>
<div class="spacer"></div>
<div class="searchwrap">
<input id="search" type="search" placeholder="Search docs" autocomplete="off"
 aria-label="Search documentation">
<div id="results" role="listbox" hidden></div>
</div>
<span class="kbd hide-sm">/</span>
<a class="plain hide-sm" href="{site.base}llms.txt">llms.txt</a>
<a class="plain hide-sm" href="{site.repo}">GitHub</a>
{THEME_BUTTON}
</div></header>
<details class="menu"><summary>Documentation menu</summary><div class="side">{nav}</div></details>
<div class="layout">
<aside class="side">{nav}</aside>
<main>
<article>
{page.body_html}
<footer>
<span>{site.title} · {site.license}</span>
<span class="sep"><a href="{site.repo}">GitHub</a></span>
<span class="sep"><a href="{edit}">Edit this page</a></span>
{md_link}
</footer>
</article>
{aside}
</main>
</div>
<script src="{site.base}app.js" defer></script>
</body>
</html>
"""


# ---------------------------------------------------------------------------------------- build


def strip_badges(text: str) -> tuple[str, str]:
    """Split the README into (everything before the badge block, everything after it)."""
    lines = text.splitlines()
    first = next((i for i, ln in enumerate(lines) if ln.startswith("[![")), None)
    if first is None:
        return "", text
    last = first
    while last + 1 < len(lines) and (lines[last + 1].startswith("[![") or not lines[last + 1].strip()):
        if lines[last + 1].startswith("[!["):
            last += 1
        else:
            break
    return "\n".join(lines[:first]).rstrip(), "\n".join(lines[last + 1 :]).lstrip("\n")


HTML_FRAG_RE = re.compile(r'href="([^"#]*)#([^"]+)"')
MD_FRAG_RE = re.compile(r"\]\(([^)\s#]*)#([^)\s]+)\)")


def resolve_fragments(content: str, self_url: str, ids: dict[str, set[str]], pattern: re.Pattern[str]) -> str:
    """Expand shorthand anchors (``#adr-9``) to the heading id the site actually generated."""

    def repl(m: re.Match[str]) -> str:
        url, frag = m.group(1), m.group(2)
        known = ids.get(url or self_url)
        if known is None or frag in known:
            return m.group(0)
        match = sorted(i for i in known if i.startswith(frag + "-"))
        if not match:
            return m.group(0)
        return m.group(0).replace(f"#{frag}", f"#{match[0]}")

    return pattern.sub(repl, content)


def build(base: str, out_dir: Path, site_url: str) -> tuple[Site, list[Page]]:
    site, pages = load_nav(WEB / "nav.json", base, site_url)
    by_source = {p.source: p for p in pages}
    by_slug = {p.slug: p for p in pages}
    providers = read_providers()
    models = [m for p in providers for m in p["models"]]
    md = make_md()

    if out_dir.exists():
        shutil.rmtree(out_dir)
    out_dir.mkdir(parents=True)

    for page in pages:
        src = ROOT / page.source
        if not src.exists():
            raise SystemExit(f"missing source for '{page.title}': {page.source}")
        raw = src.read_text(encoding="utf-8")
        if page.home:
            _, raw = strip_badges(raw)
        page.text = rewrite_links(raw, page.source, site, by_source, md_mode=True)
        body = rewrite_links(raw, page.source, site, by_source, md_mode=False)

        md.reset()
        rendered = wrap_tables(md.convert(body))
        page.toc = list(getattr(md, "toc_tokens", []))

        if page.home:
            page.body_html = hero(site, models) + leaderboard_card(site) + agents_card(site) + rendered
            page.toc = [
                {"id": "leaderboard-strip", "name": "Open benchmark", "children": []},
                {"id": "for-agents", "name": "For agents", "children": []},
                *page.toc,
            ]
        elif page.provider_index:
            head, sep, rest = rendered.partition("</h1>")
            page.body_html = head + sep + provider_cards(site, providers, by_slug) + rest
        else:
            page.body_html = rendered

    # Second pass: expand shorthand anchors now that every page's heading ids are known.
    ids: dict[str, set[str]] = {}
    for page in pages:
        found = set(ID_RE.findall(page.body_html))
        ids[site.url(page.slug)] = found
        ids[site.url(page.slug, md=True)] = found

    for page in pages:
        page.body_html = resolve_fragments(page.body_html, site.url(page.slug), ids, HTML_FRAG_RE)
        page.text = resolve_fragments(page.text, site.url(page.slug, md=True), ids, MD_FRAG_RE)
        target = out_dir if not page.slug else out_dir / page.slug
        target.mkdir(parents=True, exist_ok=True)
        (target / "index.html").write_text(page_html(site, page, pages), encoding="utf-8")
        (target / "index.md").write_text(page.text.rstrip() + "\n", encoding="utf-8")

    write_extras(site, pages, out_dir)
    return site, pages


def write_extras(site: Site, pages: list[Page], out: Path) -> None:
    for asset in ("style.css", "app.js"):
        shutil.copyfile(WEB / "assets" / asset, out / asset)

    index = [{"t": p.title, "u": site.url(p.slug), "h": flat_headings(p.toc)} for p in pages]
    (out / "search.json").write_text(json.dumps(index, separators=(",", ":")), encoding="utf-8")

    def entry(p: Page) -> str:
        return f"- [{p.title}]({site.url(p.slug, md=True)}): {p.description}"

    docs = [p for p in pages if not p.optional]
    optional = [p for p in pages if p.optional]
    llms = [
        f"# {site.title}",
        "",
        f"> {site.summary}",
        "",
        f"Source: {site.repo} ({site.license}). Every page below is also available as HTML at the same "
        "URL without the trailing `index.md`.",
        "",
        "## Docs",
        "",
        *[entry(p) for p in docs],
        "",
        "## Optional",
        "",
        *[entry(p) for p in optional],
        f"- [llms-full.txt]({site.base}llms-full.txt): every page on this site concatenated, in nav order.",
        "",
    ]
    (out / "llms.txt").write_text("\n".join(llms), encoding="utf-8")

    full = [f"# {site.title} — full documentation", "", f"> {site.summary}", "", f"Source: {site.repo}", ""]
    for p in pages:
        body = H1_RE.sub("", p.text, count=1).strip()
        full += [
            "---",
            "",
            f"# {p.title}",
            "",
            f"<!-- source: {p.source} | url: {site.absolute(site.url(p.slug))} -->",
            "",
            body,
            "",
        ]
    (out / "llms-full.txt").write_text("\n".join(full), encoding="utf-8")

    urls = "".join(f"<url><loc>{site.absolute(site.url(p.slug))}</loc></url>" for p in pages)
    (out / "sitemap.xml").write_text(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        f'<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{urls}</urlset>\n',
        encoding="utf-8",
    )
    sitemap = site.absolute(site.base + "sitemap.xml")
    (out / "robots.txt").write_text(f"User-agent: *\nAllow: /\n\nSitemap: {sitemap}\n", encoding="utf-8")

    notfound = Page(
        slug="404",
        title="Page not found",
        nav_title="404",
        source="website/build.py",
        description="That page does not exist on the LiteOCR documentation site.",
        section="",
        has_md=False,
        body_html=(
            "<h1>Page not found</h1><p>That URL is not part of the LiteOCR documentation.</p>"
            f'<p><a href="{site.base}">Back to the home page</a> · '
            f'<a href="{site.url("getting-started")}">Getting started</a> · '
            f'<a href="{site.base}llms.txt">llms.txt</a></p>'
        ),
    )
    (out / "404.html").write_text(page_html(site, notfound, pages), encoding="utf-8")


# ---------------------------------------------------------------------------------- link check


HREF_RE = re.compile(r'(?:href|src)="([^"]+)"')
ID_RE = re.compile(r'id="([^"]+)"')


def check(site: Site, out: Path) -> int:
    """Verify that every internal link (and fragment) resolves to something in dist/."""
    ids: dict[Path, set[str]] = {}
    problems: list[str] = []
    files = sorted(out.rglob("*.html"))
    for f in files:
        ids[f] = set(ID_RE.findall(f.read_text(encoding="utf-8")))
    for f in files:
        rel = f.relative_to(out)
        for href in HREF_RE.findall(f.read_text(encoding="utf-8")):
            if href.startswith(("http://", "https://", "mailto:", "data:", "//")):
                continue
            path, _, frag = href.partition("#")
            target = f
            if path:
                if not path.startswith(site.base):
                    problems.append(f"{rel}: link '{href}' does not start with base URL '{site.base}'")
                    continue
                local = path[len(site.base) :]
                target = out / (local + "index.html" if local.endswith("/") or not local else local)
                if not target.exists():
                    problems.append(f"{rel}: link '{href}' -> missing {target.relative_to(out)}")
                    continue
            if frag and target.suffix == ".html" and frag not in ids.get(target, set()):
                problems.append(f"{rel}: fragment '#{frag}' not found in {target.relative_to(out)}")
    for p in problems:
        print(f"  broken: {p}", file=sys.stderr)
    print(f"link check: {len(files)} pages, {len(problems)} problem(s)")
    return 1 if problems else 0


# ----------------------------------------------------------------------------------------- main


def dir_size(path: Path) -> int:
    return sum(f.stat().st_size for f in path.rglob("*") if f.is_file())


def main(argv: Optional[list[str]] = None) -> int:
    ap = argparse.ArgumentParser(description="Build the LiteOCR documentation site.")
    ap.add_argument("--base-url", default="/", help="URL prefix the site is served from (default: /)")
    ap.add_argument("--out", default=str(WEB / "dist"), help="output directory (default: website/dist)")
    ap.add_argument(
        "--site-url",
        default="https://ajinkyashejul.github.io/liteocr",
        help="public base URL of the deployed site, used for canonical links and the sitemap",
    )
    ap.add_argument("--check", action="store_true", help="verify every internal link after building")
    args = ap.parse_args(argv)

    base = "/" + args.base_url.strip("/") + "/" if args.base_url.strip("/") else "/"
    out = Path(args.out).resolve()
    site, pages = build(base, out, args.site_url.rstrip("/"))

    files = sum(1 for f in out.rglob("*") if f.is_file())
    size = f"{dir_size(out) / 1024:.0f} KB"
    print(f"built {len(pages)} pages ({files} files, {size}) -> {os.path.relpath(out, ROOT)}")
    return check(site, out) if args.check else 0


if __name__ == "__main__":
    raise SystemExit(main())
