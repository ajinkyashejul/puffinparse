"""``python -m benchmark.adapters <name> [--limit N] [--out DIR] [--cache DIR] [--no-download]``."""

from __future__ import annotations

import argparse
import sys
from collections import Counter
from pathlib import Path
from typing import Any, Optional

from .base import default_cache_dir, human_size, registry

REPO_ROOT = Path(__file__).resolve().parents[2]


def _format_stat(value: Any) -> str:
    if isinstance(value, Counter):
        return ", ".join(f"{k}={v}" for k, v in sorted(value.items(), key=lambda kv: -kv[1]))
    return str(value)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m benchmark.adapters",
        description="Convert a public OCR benchmark into a PuffinParse dataset manifest.",
    )
    parser.add_argument("adapter", nargs="?", help=f"one of: {', '.join(sorted(registry))}")
    parser.add_argument("--list", action="store_true", help="list the registered adapters and exit")
    parser.add_argument("--limit", type=int, default=None, help="cap the number of documents built")
    parser.add_argument("--out", type=Path, default=None, help="output dataset directory")
    parser.add_argument("--cache", type=Path, default=None, help="download cache directory")
    parser.add_argument("--seed", type=int, default=1234, help="selection seed (default: 1234)")
    parser.add_argument("--no-download", action="store_true", help="reuse the cache, never fetch")
    return parser


def main(argv: Optional[list[str]] = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    if args.list or not args.adapter:
        for name in sorted(registry):
            cls = registry[name]
            up = f" [{cls.upstream.repo_id}@{cls.upstream.revision[:12]}]" if cls.upstream else ""
            print(f"{name:12} {cls.license or '-':14} -> {cls.default_out}{up}")
        return 0 if args.list else 2

    if args.adapter not in registry:
        parser.error(f"unknown adapter {args.adapter!r}; known: {', '.join(sorted(registry))}")

    cache_dir = (args.cache or default_cache_dir()).expanduser().resolve()
    adapter = registry[args.adapter](cache_dir=cache_dir)
    out_dir = (args.out or (REPO_ROOT / adapter.default_out)).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    if not args.no_download:
        adapter.download(cache_dir)
    manifest = adapter.build(out_dir, limit=args.limit, seed=args.seed)

    print(f"\n{manifest.name} v{manifest.version} -> {out_dir}")
    print(f"  documents: {len(manifest.documents)} {manifest.counts_by_kind()}")
    for key, value in sorted(adapter.stats.items()):
        if key == "committed_bytes":
            print(f"  {key}: {human_size(int(value))}")
        else:
            print(f"  {key}: {_format_stat(value)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
