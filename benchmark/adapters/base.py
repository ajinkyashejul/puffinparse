"""Framework for turning a public OCR benchmark into a LiteOCR dataset manifest.

An *adapter* knows how to fetch one upstream benchmark at a pinned revision and rewrite it
into ``benchmark/datasets/<name>/manifest.json`` (see ``docs/benchmarks/adapters.md``).

The shared pieces live here:

* :class:`Adapter` — the interface every adapter implements.
* :class:`Doc` / :class:`Manifest` — the manifest data model, including the ``kind`` extension
  (``"transcript"`` for markdown ground truth, ``"rules"`` for machine-checkable assertions).
* :class:`Rule` — the one rule schema every rule-based upstream benchmark is converted into.
* :func:`html_table_to_markdown` — HTML table → GitHub-markdown table, flattening
  ``colspan`` / ``rowspan`` by repeating cells.
* :func:`pdf_page_count` — page count via ``pypdf`` when it imports, otherwise a byte scan.
* :func:`sha256_file` / :func:`write_manifest` / :func:`write_rules` — deterministic output.

Nothing here imports a provider SDK or touches the network; downloading is each adapter's job.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import unicodedata
import zlib
from abc import ABC, abstractmethod
from collections.abc import Iterable, Sequence
from dataclasses import asdict, dataclass, field
from functools import lru_cache
from html.parser import HTMLParser
from pathlib import Path
from typing import Any, Optional

__all__ = [
    "RULE_TYPES",
    "Adapter",
    "Doc",
    "Manifest",
    "Rule",
    "TableConversion",
    "Upstream",
    "default_cache_dir",
    "html_table_to_markdown",
    "pdf_page_count",
    "register",
    "registry",
    "sha256_bytes",
    "sha256_file",
    "slugify",
    "write_manifest",
    "write_rules",
]

#: Manifest ``kind`` values. ``transcript`` is the default and keeps the pre-existing behaviour
#: (``truth`` is markdown, scored by ``liteocr_core::bench``); ``rules`` points at a rule file.
KIND_TRANSCRIPT = "transcript"
KIND_RULES = "rules"

#: The closed set of assertion types in the common rule schema.
RULE_TYPES = ("present", "absent", "order", "table_cell", "bag_of_sentences")


# --------------------------------------------------------------------------------------------
# Hashing / ids
# --------------------------------------------------------------------------------------------


def default_cache_dir() -> Path:
    """Where adapters cache upstream downloads (never inside the repository).

    Honours ``LITEOCR_BENCH_CACHE`` first, then ``XDG_CACHE_HOME``, then ``~/.cache``.
    """
    env = os.environ.get("LITEOCR_BENCH_CACHE")
    if env:
        return Path(env).expanduser()
    xdg = os.environ.get("XDG_CACHE_HOME")
    base = Path(xdg).expanduser() if xdg else Path.home() / ".cache"
    return base / "liteocr" / "benchmarks"


def sha256_bytes(data: bytes) -> str:
    """SHA-256 of ``data`` as lowercase hex."""
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path, chunk: int = 1 << 20) -> str:
    """SHA-256 of a file, streamed so large PDFs do not need to fit in memory."""
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        while True:
            block = fh.read(chunk)
            if not block:
                break
            h.update(block)
    return h.hexdigest()


_SLUG_STRIP = re.compile(r"[^a-z0-9]+")


def slugify(text: str, max_len: int = 72) -> str:
    """Filesystem- and ``--filter``-safe id: ASCII, lowercase, ``_``-separated.

    Non-ASCII characters are transliterated where possible and dropped otherwise, so ids stay
    stable across platforms. The upstream id is always preserved separately in
    ``Doc.source_id``.
    """
    norm = unicodedata.normalize("NFKD", text)
    ascii_only = norm.encode("ascii", "ignore").decode("ascii").lower()
    slug = _SLUG_STRIP.sub("_", ascii_only).strip("_")
    if len(slug) > max_len:
        slug = slug[:max_len].rstrip("_")
    return slug or "doc"


# --------------------------------------------------------------------------------------------
# Manifest model
# --------------------------------------------------------------------------------------------


@dataclass(frozen=True)
class Upstream:
    """Where a dataset came from, pinned hard enough to reproduce it."""

    repo_id: str
    revision: str
    url: str
    repo_type: str = "dataset"

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass
class Rule:
    """One machine-checkable assertion about a document's parsed markdown.

    The schema is deliberately small so that several upstream rule vocabularies (ParseBench's
    ``text_content.jsonl``, olmOCR-bench's ``present`` / ``absent`` / ``order`` / ``table``
    tests) all land in it. See ``docs/benchmarks/adapters.md`` for the mapping tables.
    """

    id: str
    type: str
    source: str
    text: Optional[str] = None
    before: Optional[str] = None
    after: Optional[str] = None
    cell: Optional[dict[str, Any]] = None
    sentences: Optional[list[str]] = None
    threshold: Optional[float] = None
    case_sensitive: bool = False
    #: Upstream fuzzy-match allowance (olmOCR-bench ``max_diffs``: Levenshtein edits tolerated).
    #: Recorded for provenance and for a future fuzzy scorer; the current Rust scorer matches
    #: exactly, which is *stricter* than upstream for ``present`` / ``order`` / ``table_cell``
    #: and *looser* for ``absent`` whenever this is > 0.
    max_diffs: Optional[int] = None

    def __post_init__(self) -> None:
        if self.type not in RULE_TYPES:
            raise ValueError(f"unknown rule type {self.type!r}; expected one of {RULE_TYPES}")

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {"id": self.id, "type": self.type}
        for key in ("text", "before", "after", "cell", "sentences", "threshold"):
            value = getattr(self, key)
            if value is not None:
                out[key] = value
        out["case_sensitive"] = self.case_sensitive
        if self.max_diffs is not None:
            out["max_diffs"] = self.max_diffs
        out["source"] = self.source
        return out


@dataclass
class Doc:
    """One manifest document.

    ``id``, ``file``, ``truth``, ``pages``, ``category`` and ``tags`` are the fields the Rust
    CLI already reads (``liteocr-cli/src/bench.rs``, ``ManifestDoc``). Everything else is an
    additive extension that serde ignores today.

    ``truth`` is always emitted, even for ``kind == "rules"`` documents where it is the empty
    string, because the current ``ManifestDoc`` declares it without ``#[serde(default)]`` and a
    missing key would fail to deserialise the whole manifest.
    """

    id: str
    file: str
    truth: str = ""
    pages: int = 1
    category: str = ""
    tags: list[str] = field(default_factory=list)
    kind: str = KIND_TRANSCRIPT
    rules: Optional[str] = None
    source_id: Optional[str] = None
    upstream_path: Optional[str] = None
    sha256: Optional[str] = None
    license: Optional[str] = None
    attribution: Optional[str] = None
    #: Where the page originally came from (olmOCR-bench records one URL per test).
    source_url: Optional[str] = None
    #: SHA-256 of the truth file, for datasets whose truth is generated at fetch time and not
    #: committed (OmniDocBench), so a local build can be verified against the manifest.
    truth_sha256: Optional[str] = None

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "id": self.id,
            "file": self.file,
            "truth": self.truth,
            "pages": self.pages,
            "category": self.category,
            "tags": list(self.tags),
            "kind": self.kind,
        }
        for key in (
            "rules",
            "source_id",
            "upstream_path",
            "sha256",
            "truth_sha256",
            "source_url",
            "license",
            "attribution",
        ):
            value = getattr(self, key)
            if value is not None:
                out[key] = value
        return out


@dataclass
class Manifest:
    """``benchmark/datasets/<name>/manifest.json``."""

    name: str
    version: str
    description: str
    license: str
    documents: list[Doc] = field(default_factory=list)
    generator: Optional[str] = None
    upstream: Optional[Upstream] = None
    attribution: Optional[str] = None
    sources: Optional[list[dict[str, Any]]] = None
    notes: Optional[list[str]] = None

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "name": self.name,
            "version": self.version,
            "description": self.description,
            "license": self.license,
        }
        if self.generator:
            out["generator"] = self.generator
        if self.upstream is not None:
            out["upstream"] = self.upstream.to_dict()
        if self.attribution:
            out["attribution"] = self.attribution
        if self.sources is not None:
            out["sources"] = self.sources
        if self.notes:
            out["notes"] = self.notes
        out["documents"] = [d.to_dict() for d in sorted(self.documents, key=lambda d: d.id)]
        return out

    def counts_by_kind(self) -> dict[str, int]:
        counts: dict[str, int] = {}
        for d in self.documents:
            counts[d.kind] = counts.get(d.kind, 0) + 1
        return counts


def write_json(path: Path, payload: Any) -> str:
    """Write pretty, stable JSON with a trailing newline; return its SHA-256."""
    path.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(payload, indent=2, ensure_ascii=False, sort_keys=False) + "\n"
    data = text.encode("utf-8")
    path.write_bytes(data)
    return sha256_bytes(data)


def write_manifest(path: Path, manifest: Manifest) -> str:
    """Serialise ``manifest`` to ``path``; return the SHA-256 of the bytes written."""
    return write_json(path, manifest.to_dict())


def write_rules(path: Path, rules: Sequence[Rule]) -> str:
    """Serialise a rule list to ``path``; return the SHA-256 of the bytes written."""
    return write_json(path, [r.to_dict() for r in rules])


# --------------------------------------------------------------------------------------------
# HTML table → GitHub markdown
# --------------------------------------------------------------------------------------------


@dataclass
class TableConversion:
    """Result of :func:`html_table_to_markdown`."""

    markdown: str
    merged_cells: bool
    rows: int
    cols: int

    @property
    def tags(self) -> list[str]:
        """Tags the caller should merge into the document, e.g. ``["merged-cells"]``."""
        return ["merged-cells"] if self.merged_cells else []


@dataclass
class _Cell:
    text: list[str] = field(default_factory=list)
    header: bool = False
    colspan: int = 1
    rowspan: int = 1

    def rendered(self) -> str:
        return _clean_cell("".join(self.text))


_WS = re.compile(r"\s+")


def _clean_cell(text: str) -> str:
    """Collapse whitespace and escape the markdown cell separator.

    Backslashes are kept verbatim: LaTeX in cells (``$\\delta$``) must reach the scorer as the
    parser would print it, and the Rust table reader only unescapes ``\\|``.
    """
    return _WS.sub(" ", text).strip().replace("|", "\\|")


def _span(value: Optional[str]) -> int:
    try:
        n = int(str(value).strip())
    except (TypeError, ValueError):
        return 1
    return n if 1 <= n <= 64 else 1


class _TableParser(HTMLParser):
    """Minimal, forgiving ``<table>`` reader.

    Inline markup (``<strong>``, ``<em>``, ``<sup>``, …) is dropped and only its text kept,
    because GitHub-markdown table cells are scored as plain text after normalisation.
    """

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.rows: list[list[_Cell]] = []
        self._row: Optional[list[_Cell]] = None
        self._cell: Optional[_Cell] = None

    def handle_starttag(self, tag: str, attrs: list[tuple[str, Optional[str]]]) -> None:
        if tag == "tr":
            self._close_cell()
            self._row = []
        elif tag in ("td", "th"):
            self._close_cell()
            if self._row is None:
                self._row = []
            a = dict(attrs)
            self._cell = _Cell(
                header=(tag == "th"),
                colspan=_span(a.get("colspan")),
                rowspan=_span(a.get("rowspan")),
            )
        elif tag in ("br", "p", "li", "div") and self._cell is not None:
            self._cell.text.append(" ")

    def handle_startendtag(self, tag: str, attrs: list[tuple[str, Optional[str]]]) -> None:
        if tag == "br" and self._cell is not None:
            self._cell.text.append(" ")

    def handle_endtag(self, tag: str) -> None:
        if tag in ("td", "th"):
            self._close_cell()
        elif tag == "tr" or tag == "table":
            self._close_cell()
            self._close_row()

    def handle_data(self, data: str) -> None:
        if self._cell is not None:
            self._cell.text.append(data)

    def _close_cell(self) -> None:
        if self._cell is not None:
            if self._row is None:
                self._row = []
            self._row.append(self._cell)
            self._cell = None

    def _close_row(self) -> None:
        if self._row:
            self.rows.append(self._row)
        self._row = None

    def close(self) -> None:
        super().close()
        self._close_cell()
        self._close_row()


def html_table_to_markdown(html: str) -> TableConversion:
    """Convert one or more HTML tables into a GitHub-markdown pipe table.

    ``colspan`` / ``rowspan`` cannot be expressed in markdown, so a spanning cell is **repeated**
    into every grid position it covers and :attr:`TableConversion.merged_cells` is set, which
    callers turn into a ``"merged-cells"`` tag. That is a real fidelity loss on hierarchical
    headers: a scorer comparing against this truth measures cell *content*, not table structure.

    Rows shorter than the widest row are padded with empty cells. The first grid row becomes the
    markdown header row (markdown has no headerless table).
    """
    parser = _TableParser()
    parser.feed(html)
    parser.close()
    if not parser.rows:
        return TableConversion(markdown="", merged_cells=False, rows=0, cols=0)

    grid: dict[tuple[int, int], str] = {}
    merged = False
    max_col = 0
    for r, row in enumerate(parser.rows):
        c = 0
        for cell in row:
            while (r, c) in grid:
                c += 1
            if cell.colspan > 1 or cell.rowspan > 1:
                merged = True
            value = cell.rendered()
            for dr in range(cell.rowspan):
                for dc in range(cell.colspan):
                    grid[(r + dr, c + dc)] = value
            c += cell.colspan
            max_col = max(max_col, c)

    n_rows = max(r for r, _ in grid) + 1
    n_cols = max(max_col, max(c for _, c in grid) + 1)
    matrix = [[grid.get((r, c), "") for c in range(n_cols)] for r in range(n_rows)]

    lines = ["| " + " | ".join(matrix[0]) + " |", "| " + " | ".join(["---"] * n_cols) + " |"]
    lines.extend("| " + " | ".join(row) + " |" for row in matrix[1:])
    return TableConversion(markdown="\n".join(lines) + "\n", merged_cells=merged, rows=n_rows, cols=n_cols)


# --------------------------------------------------------------------------------------------
# PDF page counting
# --------------------------------------------------------------------------------------------

#: A page object in the raw byte stream, captured with its object number so that an
#: incrementally-updated PDF (the same ``1 0 obj`` written twice) is not double counted.
_PAGE_OBJ = re.compile(rb"(\d+)\s+\d+\s+obj\b(?:(?!endobj).){0,4000}?/Type\s*/Page(?![sA-Za-z])", re.S)
_PAGE = re.compile(rb"/Type\s*/Page(?![sA-Za-z])")
#: ``/Count`` inside a page-tree node, in either field order. Restricting it to a dictionary that
#: also says ``/Type /Pages`` keeps ``/Type /Outlines /Count n`` out of the answer.
_PAGES_COUNT = (
    re.compile(rb"/Type\s*/Pages(?:(?!endobj).){0,2000}?/Count\s+(\d+)", re.S),
    re.compile(rb"/Count\s+(\d+)(?:(?!endobj).){0,2000}?/Type\s*/Pages", re.S),
)
_STREAM_START = re.compile(rb"stream\r?\n")


@lru_cache(maxsize=1)
def _pdf_reader() -> Any:
    """``pypdf.PdfReader`` if it imports cleanly, else ``None`` (cached: the import is noisy).

    pypdf pulls in ``cryptography``, whose Rust extension can raise a ``pyo3`` ``PanicException``
    — a ``BaseException``, not an ``Exception`` — when the installed wheel does not match the
    interpreter. The import is therefore guarded broadly on purpose: a broken optional
    dependency must degrade to the byte-scan fallback, never abort a build.
    """
    try:
        from pypdf import PdfReader
    except BaseException:  # a broken optional dependency must not abort a build - see docstring
        return None
    return PdfReader


def _pypdf_page_count(path: Path) -> Optional[int]:
    """Page count via pypdf, or ``None`` if pypdf is unusable in this environment."""
    reader = _pdf_reader()
    if reader is None:
        return None
    try:
        return len(reader(str(path)).pages)
    except Exception:
        return None


def _inflate_streams(data: bytes) -> list[bytes]:
    """Every FlateDecode stream we can inflate — modern PDFs hide page dicts in object streams."""
    out: list[bytes] = []
    pos = 0
    while True:
        m = _STREAM_START.search(data, pos)
        if m is None:
            return out
        end = data.find(b"endstream", m.end())
        if end < 0:
            return out
        pos = end + len(b"endstream")
        try:
            out.append(zlib.decompress(data[m.end() : end]))
        except zlib.error:
            continue


def _scan_page_count(data: bytes) -> int:
    """Page count from raw PDF bytes, without a PDF library.

    Preference order: the ``/Count`` of the root page tree (correct even when the page objects
    live in a compressed object stream), then the number of *distinct* page objects.
    """
    blobs = [data, *_inflate_streams(data)]
    counts = [int(n) for blob in blobs for rx in _PAGES_COUNT for n in rx.findall(blob)]
    if counts:
        return max(counts)
    objects = {m.group(1) for m in _PAGE_OBJ.finditer(data)}
    extra = sum(len(_PAGE.findall(blob)) for blob in blobs[1:])
    return len(objects) + extra


def pdf_page_count(path: Path) -> int:
    """Number of pages in a PDF.

    Uses ``pypdf`` when it is importable, otherwise falls back to :func:`_scan_page_count`.
    Returns at least 1 so a manifest never records a zero-page document.
    """
    if path.suffix.lower() != ".pdf":
        return 1
    n = _pypdf_page_count(path)
    if n and n > 0:
        return n
    return max(_scan_page_count(path.read_bytes()), 1)


# --------------------------------------------------------------------------------------------
# Adapter interface + registry
# --------------------------------------------------------------------------------------------


class Adapter(ABC):
    """Base class for every benchmark adapter.

    Subclasses set :attr:`name`, :attr:`license`, :attr:`upstream` and :attr:`default_out`, then
    implement :meth:`download` and :meth:`build`. ``build`` must be deterministic for a given
    ``(limit, seed)`` so re-running it produces a byte-identical manifest.
    """

    #: Dataset name, also the ``python -m benchmark.adapters <name>`` argument.
    name: str = ""
    #: SPDX identifier for the redistributable part of the upstream data.
    license: str = ""
    #: Pinned upstream location.
    upstream: Optional[Upstream] = None
    #: Default output directory, relative to the repository root.
    default_out: str = ""
    #: One-line description written into the manifest.
    description: str = ""

    def __init__(self, cache_dir: Optional[Path] = None) -> None:
        #: Free-form counters an adapter fills in during :meth:`build`, printed by the CLI.
        self.stats: dict[str, Any] = {}
        #: Where :meth:`download` put the upstream data; :meth:`build` reads from here.
        self.cache_dir: Path = Path(cache_dir) if cache_dir is not None else default_cache_dir()

    @abstractmethod
    def download(self, cache_dir: Path) -> Path:
        """Fetch the upstream data into ``cache_dir`` and return the snapshot root."""

    @abstractmethod
    def build(self, out_dir: Path, limit: Optional[int] = None, seed: int = 1234) -> Manifest:
        """Write ``out_dir`` (manifest + any committed files) and return the manifest."""

    # -- helpers available to subclasses ------------------------------------------------------

    def log(self, message: str) -> None:
        print(f"[{self.name}] {message}")

    def bump(self, key: str, n: int = 1) -> None:
        self.stats[key] = int(self.stats.get(key, 0)) + n


registry: dict[str, type[Adapter]] = {}


def register(cls: type[Adapter]) -> type[Adapter]:
    """Class decorator that adds an adapter to the CLI registry."""
    if not cls.name:
        raise ValueError(f"{cls.__name__} must set a name")
    registry[cls.name] = cls
    return cls


def total_size(paths: Iterable[Path]) -> int:
    """Sum of file sizes in bytes."""
    return sum(p.stat().st_size for p in paths if p.is_file())


def human_size(n: int) -> str:
    """``1234567`` → ``"1.2 MB"``."""
    for unit, scale in (("MB", 1 << 20), ("KB", 1 << 10)):
        if n >= scale:
            return f"{n / scale:.1f} {unit}"
    return f"{n} B"
