#!/usr/bin/env python3
"""Delete the test executables an earlier run left in the coverage target dir.

    prune_stale_test_binaries.py <llvm-cov-target-dir>

Why: `cargo llvm-cov report` hands *every* executable under the target dir to
llvm-cov, not just the ones the current run built. The CI cache restores
`target/llvm-cov-target` (the cache key only changes with Cargo.lock), and
`cargo llvm-cov` cleans only the workspace targets that still exist, so
integration-test binaries of test files that were since renamed, merged or
deleted survive. llvm-cov then reads their coverage maps, which describe the
source as it was when they were built: every line that has since moved or
changed shows up as an uncovered line, and total coverage drifts down run after
run. The Ubuntu job read 85.5% (Windows 94-97%) with identical covered-line
counts and 10-85 extra "uncovered" lines per file, all at stale line numbers.

Removing the test binaries before the run is safe: cargo rebuilds a missing
test binary, and the third-party rlibs (the expensive part of the cache) are
untouched. Proc-macro libraries (.so, .dylib, .dll) and everything with an
extension other than .exe are kept.
"""

from __future__ import annotations

import sys
from pathlib import Path


def is_test_binary(path: Path) -> bool:
    """Whether `path` (a file in `<profile>/deps`) is a test executable cargo links."""
    if not path.is_file():
        return False
    if path.suffix.lower() == ".exe":
        return True
    # Unix executables have no extension: `test_auth-1a2b3c4d5e6f7a8b`.
    return path.suffix == "" and "." not in path.name


def prune(target_dir: Path) -> list[Path]:
    """Deletes every test executable under `<target_dir>/*/deps`; returns them."""
    removed: list[Path] = []
    for deps in sorted(target_dir.glob("*/deps")):
        for entry in sorted(deps.iterdir()):
            if is_test_binary(entry):
                entry.unlink()
                removed.append(entry)
    return removed


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    removed = prune(Path(argv[1]))
    print(f"pruned {len(removed)} stale test binaries from {argv[1]}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
