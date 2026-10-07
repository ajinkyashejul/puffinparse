#!/usr/bin/env python3
"""Build the PuffinParse product website.

The site has two halves:

* the **landing page** at ``/``, rendered from the template in ``website/landing/index.html`` with
  live repository data (the provider registry in ``crates/puffinparse-core/src/model.rs`` and the
  committed benchmark results) injected at build time;
* the **documentation** under ``/docs/``, driven by the site map in ``website/nav.json``: every
  listed markdown file (existing repo docs plus the pages under ``website/pages/``) is rendered to
  a pretty URL, with the agent-friendly companions ``<page>/index.md``, ``llms.txt``,
  ``llms-full.txt``, ``search.json``, ``sitemap.xml``, ``robots.txt`` and ``404.html``.

    python website/build.py                     # -> website/dist, base URL "/"
    python website/build.py --base-url /preview # hosted under a subpath
    python website/build.py --check             # build, then verify every internal link
    python website/build.py --write-redirects   # refresh the /<slug> -> /docs/<slug> map in vercel.json

The only third-party dependency is ``markdown`` (``pip install markdown``); everything else is
standard library. No syntax highlighter is used, so code blocks stay plain ``<pre><code>``.
"""

from __future__ import annotations

import argparse
import glob
import html
import importlib.util
import json
import os
import posixpath
import re
import shutil
import sys
import urllib.parse
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional

import markdown

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "website"

# The benchmark results viewer is a second, self-contained static app (its own build
# script, its own ~10 MB of data) mounted below the site at this path.
BENCH_PREFIX = "benchmark-results"
BENCH_BUILD = ROOT / "benchmark" / "site" / "build.py"

# Brand (docs/DESIGN.md, Brand). The favicon is the mark file itself (it carries its own dark-mode
# tile); headers inline MARK so the tile follows the theme toggle through --mark-tile.
FAVICON = "data:image/svg+xml," + urllib.parse.quote(
    (WEB / "assets" / "mark.svg").read_text(encoding="utf-8").strip(), safe=" =:/'"
)
_TILE = "var(--mark-tile,#14171c)"
MARK = (
    '<svg class="mark" viewBox="0 0 32 32" aria-hidden="true" focusable="false">'
    f'<rect x="1" y="1" width="30" height="30" rx="7.5" fill="{_TILE}"/>'
    '<ellipse cx="13.2" cy="17.2" rx="6.6" ry="7.4" fill="#fff"/>'
    f'<circle cx="14.6" cy="14.4" r="1.55" fill="{_TILE}"/>'
    '<path d="M19 9.6 28.6 17.2 19 24.6Z" fill="var(--scan,#e95c20)" stroke="var(--scan,#e95c20)" '
    'stroke-width="1.6" stroke-linejoin="round"/>'
    f'<rect x="19" y="14.6" width="7.2" height="1.5" fill="{_TILE}"/>'
    f'<rect x="19" y="18.4" width="5.6" height="1.5" fill="{_TILE}"/></svg>'
)
OG_IMAGE = "og.png"  # 1200x630, rendered by website/og/render.py and committed


def puffin(width: int, extra_class: str = "") -> str:
    """The mascot, inlined (so --mascot-body follows the theme toggle) at a given CSS width."""
    svg = (WEB / "assets" / "puffin.svg").read_text(encoding="utf-8")
    svg = svg[svg.index("<svg") :].strip()
    cls = f"puffin {extra_class}".strip()
    return svg.replace("<svg ", f'<svg class="{cls}" width="{width}" ', 1)


def social_meta(site: Site, title: str, description: str, url: str) -> str:
    """Open Graph / Twitter card tags; the image needs an absolute URL, so none without site_url."""
    image = site.absolute(site.base + OG_IMAGE) if site.site_url else ""
    tags = [
        ("property", "og:type", "website"),
        ("property", "og:site_name", site.title),
        ("property", "og:title", title),
        ("property", "og:description", description),
        ("property", "og:url", url),
        ("name", "twitter:card", "summary_large_image" if image else "summary"),
    ]
    if image:
        tags += [
            ("property", "og:image", image),
            ("property", "og:image:width", "1200"),
            ("property", "og:image:height", "630"),
            ("name", "twitter:image", image),
        ]
    return "\n".join(f'<meta {k}="{v}" content="{html.escape(c, quote=True)}">' for k, v, c in tags)


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
    docs_prefix: str = "docs"
    nav_links: dict[str, list[dict[str, str]]] = field(default_factory=dict)

    @property
    def docs_base(self) -> str:
        """Where the documentation lives. The landing page owns ``base`` itself."""
        return f"{self.base}{self.docs_prefix}/" if self.docs_prefix else self.base

    @property
    def viewer_url(self) -> str:
        """The benchmark results viewer, mounted beside the docs rather than inside them."""
        return f"{self.base}{BENCH_PREFIX}/"

    def blob(self, path: str, tree: bool = False) -> str:
        kind = "tree" if tree else "blob"
        return f"{self.repo}/{kind}/{self.branch}/{path}"

    def url(self, slug: str, md: bool = False) -> str:
        u = self.docs_base if not slug else f"{self.docs_base}{slug}/"
        return u + "index.md" if md else u

    def absolute(self, rel: str) -> str:
        """Public URL for a base-relative path. ``site_url`` and ``base`` are independent:
        ``base`` is how links are written in the HTML, ``site_url`` is where the site is served."""
        if not self.site_url:
            return rel
        path = rel[len(self.base) :] if rel.startswith(self.base) else rel.lstrip("/")
        return f"{self.site_url.rstrip('/')}/{path}"


def load_nav(path: Path, base: str, site_url: str, docs_prefix: str = "docs") -> tuple[Site, list[Page]]:
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
        docs_prefix=docs_prefix.strip("/"),
    )
    pages: list[Page] = []
    for section in data["sections"]:
        # A section may carry plain links to things that are not docs pages (the benchmark
        # results viewer). They appear in the sidebar only: no markdown, no llms.txt entry.
        for link in section.get("links", []):
            site.nav_links.setdefault(section["title"], []).append(
                {"title": link["title"], "url": link["url"].lstrip("/")}
            )
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


MODES = ("parse", "ocr", "extract")

RS_FIELDS = ("name", "display_name", "env_var", "base_url", "docs")
RS_FIELD_RE = {n: re.compile(rf'\b{n}:\s*"([^"]*)"') for n in RS_FIELDS}
RS_MODEL_RE = re.compile(
    r'model:\s*"([^"]+)",\s*description:\s*"([^"]*)",\s*default:\s*(true|false),\s*modes:\s*([^,\n]+),'
)
STATUS_ROW_RE = re.compile(r"^\|.*?\[`(?P<file>[\w.-]+)\.md`\].*\|\s*(?P<status>[^|]+?)\s*\|\s*$", re.M)


def _rs_modes(expr: str) -> list[str]:
    """``PARSE_OCR`` / ``Mode::ALL`` / ``&[Mode::Extract]`` -> ordered lowercase mode names."""
    expr = expr.strip()
    if expr == "PARSE_OCR":
        return ["parse", "ocr"]
    if expr == "Mode::ALL":
        return list(MODES)
    found = {m.lower() for m in re.findall(r"Mode::(\w+)", expr)}
    return [m for m in MODES if m in found]


def read_provider_status() -> dict[str, str]:
    """Verification status per provider, from the status column of ``docs/providers/README.md``."""
    readme = ROOT / "docs/providers/README.md"
    if not readme.exists():
        return {}
    return {m["file"]: m["status"] for m in STATUS_ROW_RE.finditer(readme.read_text(encoding="utf-8"))}


def read_registry() -> list[dict[str, Any]]:
    """Providers, models and modes parsed straight out of the Rust registry, so nothing can drift.

    ``model.rs`` is regular enough to read with a small parser: one ``ProviderInfo`` block per
    provider, each holding a list of ``ModelInfo`` entries with a ``modes`` expression.
    """
    model_rs = ROOT / "crates/puffinparse-core/src/model.rs"
    if not model_rs.exists():
        return []
    _, _, body = model_rs.read_text(encoding="utf-8").partition("pub const PROVIDERS")
    status = read_provider_status()
    found: list[dict[str, Any]] = []
    for chunk in body.split("ProviderInfo {")[1:]:
        info: dict[str, Any] = {}
        for key, pattern in RS_FIELD_RE.items():
            m = pattern.search(chunk)
            info[key] = m.group(1) if m else ""
        if not info["name"]:
            continue
        info["slug"] = info["name"].replace("_", "-")  # google_documentai -> google-documentai
        models = [
            {
                "model": model,
                "qualified": f"{info['name']}/{model}",
                "description": desc,
                "default": default == "true",
                "modes": _rs_modes(modes),
            }
            for model, desc, default, modes in RS_MODEL_RE.findall(chunk)
        ]
        info["models"] = models
        info["modes"] = [m for m in MODES if any(m in mod["modes"] for mod in models)]
        info["status"] = status.get(info["slug"], "")
        info["verified"] = info["status"].startswith("live-verified")
        info["local"] = info["status"].startswith("verified locally")
        found.append(info)
    return found


def read_providers() -> list[dict[str, Any]]:
    """Registry providers with the priced model list used by the docs provider cards."""
    pricing_json = ROOT / "crates/puffinparse-core/src/pricing.json"
    if not pricing_json.exists():
        return []
    prices = {k for k in json.loads(pricing_json.read_text(encoding="utf-8")) if "/" in k}
    providers = read_registry()
    for p in providers:
        p["priced"] = sorted(m["qualified"] for m in p["models"] if m["qualified"] in prices)
    return providers


def read_leaderboard(limit: int = 5) -> tuple[list[dict[str, Any]], str]:
    """Top models of the headline run: the newest run on the newest ``combined-vN`` dataset.

    Scores are only comparable within one dataset, so rows are never mixed across result files
    (mixing let the easy synthetic set's 100s outrank every real-document score). Falls back to the
    newest run of any dataset when no combined run exists.
    """
    runs: list[dict[str, Any]] = []
    for path in sorted(glob.glob(str(ROOT / "benchmark/results/*.json"))):
        try:
            runs.append(json.loads(Path(path).read_text(encoding="utf-8")))
        except (OSError, ValueError):
            continue
    if not runs:
        return [], ""

    def rank(run: dict[str, Any]) -> tuple[int, int, str]:
        name = str(run.get("dataset", {}).get("name", ""))
        match = re.fullmatch(r"combined-v(\d+)", name)
        return (1 if match else 0, int(match.group(1)) if match else 0, str(run.get("created_at", "")))

    run = max(runs, key=rank)
    ds = run.get("dataset", {})
    dataset = f"{ds.get('name', '?')} v{ds.get('version', '?')} · {ds.get('documents', '?')} documents"
    rows = []
    for m in run.get("models", []):
        s = m.get("summary", {})
        if s.get("overall") is None:
            continue
        rows.append(
            {
                "model": m.get("model", "?"),
                "overall": s.get("overall"),
                "p50": s.get("latency_p50_ms"),
                "cost": s.get("cost_per_1k_pages_usd"),
            }
        )
    rows.sort(key=lambda r: -r["overall"])
    return rows[:limit], dataset


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
· <a href="{site.url("benchmark")}">methodology and caveats</a>
· <a href="{site.viewer_url}">results viewer</a></p>
</section>"""


def hero(site: Site, models: list[str]) -> str:
    fallback = models[0] if models else "reducto/standard"
    default = "reducto/standard" if "reducto/standard" in models else fallback
    swap = html.escape(json.dumps(models or [default]), quote=True)
    span = f'<span id="swap" data-models="{swap}">{default}</span>'
    return f"""<section class="hero">
<h1>{html.escape(site.title)}</h1>
<p class="lede"><strong>{html.escape(site.tagline)}</strong> Rust core, Python and TypeScript SDKs,
CLI, gateway, and an open benchmark that ranks providers on accuracy, latency and cost.</p>
<div class="codewrap"><pre><code class="language-python">import puffinparse
doc = puffinparse.parse(&quot;invoice.pdf&quot;, model=&quot;{span}&quot;)
print(doc.markdown, doc.cost_usd)</code></pre></div>
<p class="meta">Switch providers by changing one string. Same request, same response shape, same errors.</p>
<div class="cta">
<a class="btn primary" href="{site.url("getting-started")}">Get started</a>
<code>pip install puffinparse</code>
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
<li><a href="{site.url("project/contributing")}"><code>/project/contributing/</code></a> — how to contribute:
required checks and where each kind of change goes.</li>
</ul>
</section>"""


def provider_cards(site: Site, providers: list[dict[str, Any]], by_slug: dict[str, Page]) -> str:
    if not providers:
        return ""
    cards = []
    for p in providers:
        slug = f"providers/{p['slug']}"
        if slug not in by_slug:
            continue
        models = "".join(f"<code>{html.escape(m.split('/', 1)[1])}</code> " for m in p["priced"])
        cards.append(
            f'<div class="card"><h3><a href="{site.url(slug)}">{html.escape(p["display_name"])}</a></h3>'
            f"<p><code>{html.escape(p['env_var'])}</code></p><p>{models}</p>"
            f'<p class="meta"><a href="{html.escape(p["docs"])}">Vendor docs</a></p></div>'
        )
    return f'<div class="grid3">{"".join(cards)}</div>' if cards else ""


# -------------------------------------------------------------------------------------- landing

# The four model strings the "switch in one line" tab strip offers. Only the string changes.
SWITCH_MODELS = ("reducto/r-1", "extend/parse_performance", "mistral/ocr-latest", "gemini/2.5-flash")

# What the hero animation "types out" once the scan bar has swept the document.
SCAN_OUTPUT = (
    "{",
    '  "markdown": "# ACME Industries\\n\\n## Invoice …",',
    '  "pages": [{"blocks": [',
    '    {"type": "title",',
    '     "bbox": [0.08, 0.07, 0.62, 0.11]},',
    '    {"type": "table",',
    '     "bbox": [0.08, 0.41, 0.92, 0.63]}',
    "  ]}],",
    '  "usage": {"pages": 2},',
    '  "latency_ms": 2848,',
    '  "cost_usd": 0.0300',
    "}",
)

LANDING_SWITCH_CODE = """import puffinparse

doc = puffinparse.parse(&quot;invoice.pdf&quot;, model={model})

doc.markdown          # same unified markdown
doc.pages[0].blocks   # same typed blocks, same normalised boxes
doc.usage.pages       # same billed page count
doc.cost_usd          # same field, priced from the model string"""


def landing_switch_tabs() -> str:
    """CSS-only tab strip. Panels are stacked in one grid cell, so switching shifts nothing."""
    inputs, labels, panels = [], [], []
    for i, model in enumerate(SWITCH_MODELS):
        checked = " checked" if i == 0 else ""
        inputs.append(f'<input class="lp-tabin" type="radio" name="lp-switch" id="lp-tab{i}"{checked}>')
        labels.append(f'<label class="lp-tab" for="lp-tab{i}">{html.escape(model)}</label>')
        code = LANDING_SWITCH_CODE.format(
            model=f'<mark class="lp-diff">&quot;{html.escape(model)}&quot;</mark>'
        )
        panels.append(
            f'<div class="lp-panel lp-code"><pre><code class="language-python">{code}</code></pre></div>'
        )
    return (
        '<div class="lp-tabs">'
        + "".join(inputs)
        + f'<div class="lp-tablist" role="tablist">{"".join(labels)}</div>'
        + f'<div class="lp-panels">{"".join(panels)}</div>'
        + "</div>"
    )


def landing_providers(site: Site, providers: list[dict[str, Any]]) -> str:
    cards = []
    for p in providers:
        chips = "".join(f'<span class="lp-chip lp-chip-{m}">{m}</span>' for m in p["modes"])
        count = len(p["models"])
        if p["verified"]:
            state, label = "live", "live-verified"
        elif p["local"]:
            state, label = "local", "verified locally"
        else:
            state, label = "docs", "docs-only"
        cards.append(
            f'<a class="lp-provider" href="{site.url("providers/" + p["slug"])}">'
            f'<span class="lp-pid">{html.escape(p["name"])}</span>'
            f'<span class="lp-pname">{html.escape(p["display_name"])}</span>'
            f'<span class="lp-modes">{chips}</span>'
            f'<span class="lp-pfoot"><span class="lp-count">{count} '
            f"<span>model{'' if count == 1 else 's'}</span></span>"
            f'<span class="lp-status lp-status-{state}">{label}</span></span></a>'
        )
    return "".join(cards)


def landing_leaderboard(site: Site, limit: int = 7) -> tuple[str, str]:
    rows, dataset = read_leaderboard(limit)
    if not rows:
        return "", ""
    cells = "".join(
        f'<tr style="--row:{i}">'
        f'<td class="lp-rank">{i + 1}</td>'
        f"<td><code>{html.escape(r['model'])}</code></td>"
        f'<td class="lp-num"><strong>{r["overall"]:.2f}</strong></td>'
        f'<td class="lp-num">{r["p50"]:,}<span class="lp-unit"> ms</span></td>'
        f'<td class="lp-num">${r["cost"]:.2f}</td>'
        "</tr>"
        for i, r in enumerate(rows)
    )
    return cells, dataset


def landing_html(site: Site, providers: list[dict[str, Any]]) -> str:
    template = (WEB / "landing" / "index.html").read_text(encoding="utf-8")
    models = [m["qualified"] for p in providers for m in p["models"]]
    rows, dataset = landing_leaderboard(site)
    description = (
        f"{site.tagline} Parse, OCR and schema extraction across {len(providers)} providers and "
        f"{len(models)} models with one Python call, one response shape, and an open benchmark."
    )
    values = {
        "BASE": site.base,
        "DOCS": site.docs_base,
        "REPO": site.repo,
        "RESULTS": site.blob("benchmark/results", tree=True),
        "FAVICON": FAVICON,
        "MARK": MARK,
        "PUFFIN_HERO": puffin(115, "lp-puffin"),
        "SOCIAL": social_meta(site, f"{site.title}: {site.tagline}", description, site.absolute(site.base)),
        "CANONICAL": site.absolute(site.base),
        "DESCRIPTION": html.escape(description, quote=True),
        "THEME_BUTTON": THEME_BUTTON,
        "MODELS": html.escape(json.dumps(models), quote=True),
        "SCAN_LINES": html.escape(json.dumps(list(SCAN_OUTPUT)), quote=True),
        "SCAN_STATIC": html.escape("\n".join(SCAN_OUTPUT)),
        "DEFAULT_MODEL": models[0] if models else "reducto/standard",
        "N_PROVIDERS": str(len(providers)),
        "N_MODELS": str(len(models)),
        "N_MODES": str(len(MODES)),
        "N_VERIFIED": str(sum(1 for p in providers if p["verified"])),
        "N_LOCAL": str(sum(1 for p in providers if p["local"])),
        "N_DOCS_ONLY": str(sum(1 for p in providers if not p["verified"] and not p["local"])),
        "URL_ISSUE_VERIFY": f"{site.repo}/issues/10",
        "SWITCH_TABS": landing_switch_tabs(),
        "PROVIDER_CARDS": landing_providers(site, providers),
        "LEADERBOARD_ROWS": rows,
        "LEADERBOARD_DATASET": html.escape(dataset),
        "URL_DOCS": site.docs_base,
        "URL_START": site.url("getting-started"),
        "URL_PROVIDERS": site.url("providers"),
        "URL_BENCH": site.url("benchmark"),
        "URL_LEADERBOARD": site.url("benchmark/leaderboard"),
        "URL_VIEWER": site.viewer_url,
        "URL_COMPAT": site.url("project/compat"),
        "URL_PYTHON": site.url("python"),
        "URL_SPEC": site.url("project/spec"),
    }
    for key, value in values.items():
        template = template.replace("{{" + key + "}}", value)
    left = re.findall(r"\{\{(\w+)\}\}", template)
    if left:
        raise SystemExit(f"landing template: unsubstituted placeholder(s): {sorted(set(left))}")
    return template


# ------------------------------------------------------------------------------------- template


def nav_html(site: Site, pages: list[Page], current: str) -> str:
    grouped: list[tuple[str, list[Page]]] = []
    for p in pages:
        if not grouped or grouped[-1][0] != p.section:
            grouped.append((p.section, []))
        grouped[-1][1].append(p)

    out: list[str] = ["<nav>"]
    for section, items in grouped:
        if section:
            out.append(f"<h4>{html.escape(section)}</h4>")
        out.append("<ul>")
        for p in items:
            aria = ' aria-current="page"' if p.slug == current else ""
            out.append(f'<li><a href="{site.url(p.slug)}"{aria}>{html.escape(p.nav_title)}</a></li>')
        for link in site.nav_links.get(section, []):
            out.append(f'<li><a href="{site.base}{link["url"]}">{html.escape(link["title"])}</a></li>')
        out.append("</ul>")
    out.append("</nav>")
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
{social_meta(site, title, page.description, canonical)}
<link rel="stylesheet" href="{site.base}tokens.css">
<link rel="stylesheet" href="{site.base}style.css">
<script>(function(){{try{{var t=localStorage.getItem('puffinparse-theme');
if(t)document.documentElement.setAttribute('data-theme',t);}}catch(e){{}}}})();</script>
</head>
<body>
<header class="top"><div class="topin">
<a class="brand" href="{site.base}">{MARK}PuffinParse</a>
<a class="brand-sub hide-sm" href="{site.docs_base}">docs</a>
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


def build(base: str, out_dir: Path, site_url: str, docs_prefix: str = "docs") -> tuple[Site, list[Page]]:
    site, pages = load_nav(WEB / "nav.json", base, site_url, docs_prefix)
    by_source = {p.source: p for p in pages}
    by_slug = {p.slug: p for p in pages}
    providers = read_providers()
    models = [m["qualified"] for p in providers for m in p["models"]]
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
        target = out_dir / site.url(page.slug)[len(site.base) :]
        target.mkdir(parents=True, exist_ok=True)
        (target / "index.html").write_text(page_html(site, page, pages), encoding="utf-8")
        (target / "index.md").write_text(page.text.rstrip() + "\n", encoding="utf-8")

    (out_dir / "index.html").write_text(landing_html(site, providers), encoding="utf-8")
    write_extras(site, pages, out_dir)
    return site, pages


def write_extras(site: Site, pages: list[Page], out: Path) -> None:
    for asset in ("tokens.css", "style.css", "landing.css", "app.js", "mark.svg", "puffin.svg", OG_IMAGE):
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
        f"Source: {site.repo} ({site.license}). The documentation lives under `{site.docs_base}`; every "
        "page below is also available as HTML at the same URL without the trailing `index.md`.",
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

    locs = [site.absolute(site.base), *(site.absolute(site.url(p.slug)) for p in pages)]
    urls = "".join(f"<url><loc>{loc}</loc></url>" for loc in locs)
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
        description="That page does not exist on the PuffinParse documentation site.",
        section="",
        has_md=False,
        body_html=(
            f'<div class="notfound">{puffin(138)}</div>'
            "<h1>Page not found</h1><p>That URL is not part of the PuffinParse site. The documentation "
            f'moved under <a href="{site.docs_base}"><code>{site.docs_base}</code></a>.</p>'
            f'<p><a href="{site.base}">Home</a> · '
            f'<a href="{site.docs_base}">Documentation</a> · '
            f'<a href="{site.url("getting-started")}">Getting started</a> · '
            f'<a href="{site.base}llms.txt">llms.txt</a></p>'
        ),
    )
    (out / "404.html").write_text(page_html(site, notfound, pages), encoding="utf-8")


# ------------------------------------------------------------------ benchmark results viewer


def build_benchmark_viewer(site: Site, out: Path) -> bool:
    """Build ``benchmark/site`` into ``<out>/benchmark-results/``.

    The viewer has its own stdlib-only build script; it is imported and called in-process
    so the same interpreter (and the same optional Pillow) is used. A failure is reported
    and skipped rather than failing the whole site build — the docs must still deploy.
    """
    if not BENCH_BUILD.is_file():
        print(f"benchmark viewer: {BENCH_BUILD} not found, skipping", file=sys.stderr)
        return False
    spec = importlib.util.spec_from_file_location("puffinparse_benchmark_site_build", BENCH_BUILD)
    if spec is None or spec.loader is None:  # pragma: no cover - importlib contract
        print(f"benchmark viewer: cannot load {BENCH_BUILD}, skipping", file=sys.stderr)
        return False
    module = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(module)
        code = module.main(
            [
                "--out",
                str(out / BENCH_PREFIX),
                "--base-url",
                site.viewer_url,
                "--home-url",
                site.base,
                "--docs-url",
                site.docs_base,
            ]
        )
    except Exception as exc:  # the viewer is optional; never take the site down with it
        print(f"benchmark viewer: build failed ({exc}), skipping", file=sys.stderr)
        shutil.rmtree(out / BENCH_PREFIX, ignore_errors=True)
        return False
    if code != 0:
        print(f"benchmark viewer: build.py exited {code}, skipping", file=sys.stderr)
        shutil.rmtree(out / BENCH_PREFIX, ignore_errors=True)
        return False
    # The viewer is the page people share most; give it the same social card as the site.
    index = out / BENCH_PREFIX / "index.html"
    if index.is_file():
        tags = social_meta(
            site,
            f"{site.title} Benchmark",
            "Accuracy, latency and cost for every document-parsing provider, with every output inspectable.",
            site.absolute(site.viewer_url),
        )
        text = index.read_text(encoding="utf-8")
        index.write_text(text.replace("</head>", tags + "\n</head>", 1), encoding="utf-8")
    return True


# ------------------------------------------------------------------------------------ redirects

VERCEL_JSON = ROOT / "vercel.json"
# Former hostnames of the site; every path on them redirects to the same path on the canonical domain.
OLD_HOSTS = ("puffinparse.vercel.app", "liteocr.vercel.app")
CANONICAL_ORIGIN = "https://puffinparse.com"


def redirect_map(site: Site, pages: list[Page]) -> list[dict[str, Any]]:
    """Old root URLs -> their new home under the docs prefix.

    ``/`` is excluded: it belongs to the landing page now. ``/llms.txt`` and ``/llms-full.txt``
    stay at the site root and are not redirected either.
    """
    prefix = site.docs_prefix
    hosts = [
        {
            "source": "/:path*",
            "has": [{"type": "host", "value": host}],
            "destination": f"{CANONICAL_ORIGIN}/:path*",
            "permanent": True,
        }
        for host in OLD_HOSTS
    ]
    return hosts + [
        {"source": f"/{p.slug}/", "destination": f"/{prefix}/{p.slug}/", "permanent": True}
        for p in pages
        if p.slug
    ]


def write_redirects(site: Site, pages: list[Page], path: Path = VERCEL_JSON) -> int:
    """Rewrite the ``redirects`` array in vercel.json. Every other key is left untouched."""
    config = json.loads(path.read_text(encoding="utf-8"))
    config["redirects"] = redirect_map(site, pages)
    path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
    return len(config["redirects"])


def check_redirects(site: Site, pages: list[Page], out: Path, path: Path = VERCEL_JSON) -> list[str]:
    """The committed redirects must cover every docs slug and land on a page that exists."""
    if not path.exists():
        return [f"{path.name} is missing"]
    current = json.loads(path.read_text(encoding="utf-8")).get("redirects", [])
    expected = redirect_map(site, pages)
    problems = []
    if current != expected:
        problems.append(
            f"{path.name}: redirects are stale ({len(current)} entries, expected {len(expected)}) "
            "— run `python website/build.py --write-redirects`"
        )
    for r in expected:
        if r["destination"].startswith("http"):
            continue  # host redirects leave the site
        target = out / r["destination"].strip("/") / "index.html"
        if not target.exists():
            problems.append(f"{path.name}: redirect {r['source']} -> missing {r['destination']}")
    return problems


# ---------------------------------------------------------------------------------- link check


HREF_RE = re.compile(r'(?:href|src)="([^"]+)"')
ID_RE = re.compile(r'id="([^"]+)"')


def check(site: Site, pages: list[Page], out: Path) -> int:
    """Verify every internal link (and fragment) in the landing page and the docs, plus the
    redirect map that keeps the pre-``/docs/`` URLs working."""
    ids: dict[Path, set[str]] = {}
    problems: list[str] = []
    # The results viewer is a separate application with its own hash router: its pages are
    # not part of the docs link graph, so they are neither scanned nor followed into.
    viewer = out / BENCH_PREFIX
    files = [f for f in sorted(out.rglob("*.html")) if viewer not in f.parents]
    landing = out / "index.html"
    if not landing.exists():
        problems.append("index.html: the landing page was not generated")
    elif f'href="{site.docs_base}"' not in landing.read_text(encoding="utf-8"):
        problems.append(f"index.html: the landing page does not link to {site.docs_base}")
    if site.base == "/":
        problems.extend(check_redirects(site, pages, out))
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
                if local.split("/", 1)[0] == BENCH_PREFIX:
                    entry = out / (local + "index.html" if local.endswith("/") else local)
                    if viewer.is_dir() and not entry.exists():
                        problems.append(f"{rel}: link '{href}' -> missing {entry.relative_to(out)}")
                    continue
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
    ap = argparse.ArgumentParser(description="Build the PuffinParse documentation site.")
    ap.add_argument("--base-url", default="/", help="URL prefix the site is served from (default: /)")
    ap.add_argument("--out", default=str(WEB / "dist"), help="output directory (default: website/dist)")
    ap.add_argument(
        "--site-url",
        default="https://ajinkyashejul.github.io/puffinparse",
        help="public base URL of the deployed site, used for canonical links and the sitemap",
    )
    ap.add_argument(
        "--docs-prefix",
        default="docs",
        help="path the documentation is served under, below --base-url (default: docs)",
    )
    ap.add_argument("--check", action="store_true", help="verify every internal link after building")
    ap.add_argument(
        "--with-benchmark",
        dest="with_benchmark",
        action="store_true",
        default=True,
        help=f"also build benchmark/site into /{BENCH_PREFIX}/ (default)",
    )
    ap.add_argument(
        "--no-benchmark",
        dest="with_benchmark",
        action="store_false",
        help="skip the benchmark results viewer (docs and landing page only)",
    )
    ap.add_argument(
        "--write-redirects",
        action="store_true",
        help="rewrite the old-URL -> /docs/ redirect map in vercel.json, then exit",
    )
    args = ap.parse_args(argv)

    base = "/" + args.base_url.strip("/") + "/" if args.base_url.strip("/") else "/"
    out = Path(args.out).resolve()
    site, pages = build(base, out, args.site_url.rstrip("/"), args.docs_prefix)
    viewer = build_benchmark_viewer(site, out) if args.with_benchmark else False

    files = sum(1 for f in out.rglob("*") if f.is_file())
    size = f"{dir_size(out) / 1024 / 1024:.1f} MB"
    print(
        f"built the landing page + {len(pages)} docs pages under {site.docs_base}"
        + (f" + the results viewer under {site.viewer_url}" if viewer else "")
        + f" ({files} files, {size}) -> {os.path.relpath(out, ROOT)}"
    )
    if args.write_redirects:
        print(f"vercel.json: wrote {write_redirects(site, pages)} redirects")
    return check(site, pages, out) if args.check else 0


if __name__ == "__main__":
    raise SystemExit(main())
