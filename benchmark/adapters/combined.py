"""Combined dataset builder — union several committed datasets into one manifest.

``benchmark/datasets/combined-v1/manifest.json`` is a *view*: it copies no bytes. Every document
keeps living in its own dataset directory and is referenced relatively, e.g.

    {"id": "synthetic/plain_001", "file": "../synthetic-v1/docs/plain_001.png",
     "truth": "../synthetic-v1/truth/plain_001.md"}

The Rust CLI joins ``--dataset`` with each path (``bench.rs``, ``run_doc``) and does not
normalise or reject ``..``, so this loads and runs unchanged.

Adding a source is one entry in :data:`SOURCES`: a name, the directory, and the id prefix.
Per ADR-10 the combined score is only meaningful next to the per-dataset scores, so the manifest
records a ``sources`` array with each source's name, version, license, document count and the
SHA-256 of the source manifest it was built from.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Optional

from .base import (
    KIND_TRANSCRIPT,
    Adapter,
    Doc,
    Manifest,
    register,
    sha256_file,
    write_manifest,
)

VERSION = "1.0.0"


@dataclass(frozen=True)
class Source:
    """One dataset folded into the combined view."""

    #: Short name used as the id prefix, e.g. ``synthetic`` → ``synthetic/plain_001``.
    prefix: str
    #: Directory name under ``benchmark/datasets/``.
    directory: str
    #: Human label for the ``sources`` array.
    name: str


#: Order matters only for readability; documents are sorted by id in the output.
SOURCES = (
    Source(prefix="synthetic", directory="synthetic-v1", name="synthetic-v1"),
    Source(prefix="parsebench", directory="parsebench", name="parsebench"),
)


def _relative(from_dir: Path, to_dir: Path, rel_path: str) -> str:
    """Path of ``to_dir/rel_path`` expressed relative to ``from_dir``, with ``/`` separators."""
    import os.path

    target = (to_dir / rel_path).as_posix()
    return os.path.relpath(target, from_dir.as_posix()).replace("\\", "/")


@register
class CombinedAdapter(Adapter):
    """Builds ``benchmark/datasets/combined-v1`` from the datasets already in the repo."""

    name = "combined"
    license = "mixed (see `sources`)"
    default_out = "benchmark/datasets/combined-v1"
    description = "Union of every committed LiteOCR benchmark dataset, referenced in place."

    def download(self, cache_dir: Path) -> Path:
        """Nothing to download: every source is already in the working tree."""
        return cache_dir

    def build(self, out_dir: Path, limit: Optional[int] = None, seed: int = 1234) -> Manifest:
        datasets_dir = out_dir.parent
        documents: list[Doc] = []
        sources: list[dict[str, Any]] = []
        for source in SOURCES:
            src_dir = datasets_dir / source.directory
            manifest_path = src_dir / "manifest.json"
            if not manifest_path.is_file():
                self.log(f"skipping {source.name}: no {manifest_path}")
                self.bump("sources_missing")
                continue
            raw = json.loads(manifest_path.read_text(encoding="utf-8"))
            docs = raw.get("documents") or []
            for entry in docs:
                documents.append(self._fold(out_dir, src_dir, source, entry, raw))
            entry_summary: dict[str, Any] = {
                "name": raw.get("name", source.name),
                "version": raw.get("version", ""),
                "license": raw.get("license", ""),
                "path": _relative(out_dir, src_dir, "").rstrip("/") or ".",
                "documents": len(docs),
                "manifest_sha256": sha256_file(manifest_path),
            }
            if raw.get("attribution"):
                entry_summary["attribution"] = raw["attribution"]
            sources.append(entry_summary)
            self.bump("sources_included")
            self.log(f"{source.name}: {len(docs)} documents")

        if limit is not None:
            documents = sorted(documents, key=lambda d: d.id)[:limit]

        kinds: dict[str, int] = {}
        for d in documents:
            kinds[d.kind] = kinds.get(d.kind, 0) + 1
        self.stats["documents"] = len(documents)
        self.stats["by_kind"] = kinds

        manifest = Manifest(
            name="combined-v1",
            version=VERSION,
            description=(
                "Union of the committed LiteOCR benchmark datasets (synthetic-v1 and the "
                "redistributable ParseBench subset). No bytes are copied: every document is "
                "referenced relatively in its own dataset directory, so the per-dataset and "
                "combined runs score exactly the same files."
            ),
            license="mixed — per-document `license`, summarised in `sources`",
            documents=documents,
            generator="benchmark/adapters/combined.py",
            sources=sources,
            notes=[
                "Document ids are `<source>/<upstream id>`; use `--filter synthetic` or "
                "`--filter parsebench` to run one source.",
                "`file` and `truth` are relative to this directory and start with `../`.",
                'Documents with `kind: "rules"` have no markdown truth. A transcript-only '
                "scorer must skip them; the current Rust CLI reports them as errors instead "
                "(see docs/benchmarks/adapters.md).",
                "A combined score mixes licences, difficulty and document kinds — always read "
                "it next to the per-dataset scores.",
            ],
        )
        write_manifest(out_dir / "manifest.json", manifest)
        return manifest

    def _fold(
        self,
        out_dir: Path,
        src_dir: Path,
        source: Source,
        entry: dict[str, Any],
        raw: dict[str, Any],
    ) -> Doc:
        doc_id = entry["id"]
        truth = entry.get("truth") or ""
        rules = entry.get("rules")
        doc = Doc(
            id=f"{source.prefix}/{doc_id}",
            file=_relative(out_dir, src_dir, entry["file"]),
            truth=_relative(out_dir, src_dir, truth) if truth else "",
            pages=int(entry.get("pages", 1)),
            category=entry.get("category", ""),
            tags=sorted({*(entry.get("tags") or []), source.prefix}),
            kind=entry.get("kind", KIND_TRANSCRIPT),
            rules=_relative(out_dir, src_dir, rules) if rules else None,
            source_id=doc_id,
            sha256=entry.get("sha256"),
            license=entry.get("license") or raw.get("license"),
            attribution=entry.get("attribution") or raw.get("attribution"),
        )
        return doc
