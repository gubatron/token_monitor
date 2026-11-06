#!/usr/bin/env python3
"""List the cargo-fuzz targets of a fuzz crate as a JSON array.

    fuzz_targets.py windows/fuzz/Cargo.toml

Prints `targets=<json array>` and `count=<n>` lines, for $GITHUB_OUTPUT. The
targets are the crate's `[[bin]]` entries, which is what `cargo fuzz list`
prints, but this needs no nightly toolchain or cargo-fuzz install, so the
nightly workflow can build its job matrix before installing anything.
A missing file means no targets. Standard library only (Python 3.11+).
"""

from __future__ import annotations

import json
import sys
import tomllib
from pathlib import Path


def targets(manifest: Path) -> list[str]:
    if not manifest.is_file():
        return []
    data = tomllib.loads(manifest.read_text(encoding="utf-8"))
    names = [b["name"] for b in data.get("bin", []) if isinstance(b, dict) and b.get("name")]
    return sorted(set(names))


def main(argv: list[str]) -> int:
    if len(argv) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    found = targets(Path(argv[0]))
    print(f"targets={json.dumps(found)}")
    print(f"count={len(found)}")
    return 0


if __name__ == "__main__":
    # Step summaries are UTF-8; Windows runners default stdout to cp1252
    # (and `python -I` ignores PYTHONIOENCODING).
    sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main(sys.argv[1:]))
