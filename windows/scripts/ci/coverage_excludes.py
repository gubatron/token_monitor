#!/usr/bin/env python3
"""The coverage exclusion list (windows/.config/coverage-excludes.txt).

    coverage_excludes.py regex [--file F] [--no-non-product]
        Print the --ignore-filename-regex for cargo llvm-cov: test, bench,
        fuzz and example code plus every listed glue file.
    coverage_excludes.py check [--file F] [--root <windows dir>]
        The guard run by the lint job. Fails (exit 1) when an entry has no
        reason or matches no file, or when a matched file is not entirely
        cfg(windows) or is longer than its line budget (400 lines unless the
        entry says max-lines=N).
    coverage_excludes.py list [--file F] [--root <windows dir>]
        Print the entries and the files they match as a Markdown table.

Entry format: `<path regex>  [max-lines=N]  # <reason>`. The regex matches the
path relative to windows/, anchored at both ends, with `/` matching either
separator. Standard library only; render_coverage.py imports this module.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

DEFAULT_FILE = Path(".config/coverage-excludes.txt")
DEFAULT_MAX_LINES = 400
SEP = r"[/\\]"
# Test, bench, fuzz and example code is not product code.
NON_PRODUCT = r"(^|[/\\])(tests|benches|fuzz|examples)[/\\]"
# Syntax Python's re accepts but llvm-cov's POSIX ERE engine does not.
_NOT_ERE = re.compile(r"\(\?|\\[dDwWsSbB]")
_ENTRY = re.compile(r"^(?P<regex>\S+)(?:\s+max-lines=(?P<max>\d+))?\s+#\s*(?P<reason>.*)$")
_CFG_WINDOWS = re.compile(r"cfg\(\s*(windows|target_os\s*=\s*\"windows\")\s*\)")
_ATTR_LINE = re.compile(r"^\s*#\[.*\]\s*$")
_INNER_CFG = re.compile(r"^\s*#!\[\s*cfg\(\s*(windows|target_os\s*=\s*\"windows\")\s*\)\s*\]")


class ExcludesError(ValueError):
    """The exclusion file is malformed."""


@dataclass(frozen=True)
class Entry:
    pattern: str
    reason: str
    max_lines: int
    line_no: int

    @property
    def regex(self) -> str:
        """The entry as an llvm-cov / Python regex over a full file path."""
        return f"(^|{SEP})({self.pattern.replace('/', SEP)})$"

    def matches(self, path: str) -> bool:
        return re.search(self.regex, path) is not None


def parse(text: str) -> list[Entry]:
    """Parses the exclusion file; raises ExcludesError on a malformed line."""
    entries: list[Entry] = []
    errors: list[str] = []
    for no, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        m = _ENTRY.match(line)
        if not m or not m.group("reason").strip():
            errors.append(f"line {no}: expected '<path regex> [max-lines=N] # <reason>': {line}")
            continue
        pattern = m.group("regex")
        if _NOT_ERE.search(pattern):
            errors.append(f"line {no}: {pattern}: llvm-cov's regex engine is POSIX ERE (no (?, \\d, \\w, \\s, \\b)")
            continue
        if re.search(r"\[[^\]]*/", pattern):
            errors.append(f"line {no}: {pattern}: no '/' inside [...]")
            continue
        try:
            re.compile(pattern)
        except re.error as e:
            errors.append(f"line {no}: {pattern}: {e}")
            continue
        max_lines = int(m.group("max")) if m.group("max") else DEFAULT_MAX_LINES
        entries.append(Entry(pattern, m.group("reason").strip(), max_lines, no))
    if errors:
        raise ExcludesError("\n".join(errors))
    return entries


def load(path: Path) -> list[Entry]:
    return parse(path.read_text(encoding="utf-8"))


def ignore_regex(entries: list[Entry], non_product: bool = True) -> str:
    """One alternation for `cargo llvm-cov report --ignore-filename-regex`."""
    parts = ([NON_PRODUCT] if non_product else []) + [e.regex for e in entries]
    return "|".join(parts)


def is_excluded(entries: list[Entry], path: str) -> bool:
    return any(e.matches(path) for e in entries)


# --- the guard -------------------------------------------------------------

def _decl_is_windows(parent_text: str, name: str) -> bool | None:
    """Whether `mod name;` in `parent_text` carries cfg(windows).

    None when the file declares no such module.
    """
    lines = parent_text.splitlines()
    decl = re.compile(rf"^\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+{re.escape(name)}\s*;")
    for i, line in enumerate(lines):
        if not decl.match(line):
            continue
        attrs = [line.split("mod", 1)[0]]
        j = i - 1
        # Only whole-line attributes and doc comments belong to this item;
        # `#[cfg(windows)] mod other;` above it is another item.
        while j >= 0 and (_ATTR_LINE.match(lines[j]) or lines[j].strip().startswith("///")):
            attrs.append(lines[j])
            j -= 1
        return any(_CFG_WINDOWS.search(a) for a in attrs if "#[" in a)
    return None


def windows_only(root: Path, rel: PurePosixPath, _depth: int = 0) -> tuple[bool, str]:
    """Whether the module in `rel` (relative to root) only compiles on Windows.

    True when the file starts with `#![cfg(windows)]`, when its `mod`
    declaration carries `#[cfg(windows)]`, or when its parent module is
    itself Windows-only. Returns the verdict and why.
    """
    path = root / rel
    text = path.read_text(encoding="utf-8")
    if any(_INNER_CFG.match(line) for line in text.splitlines()[:40]):
        return True, f"{rel} has #![cfg(windows)]"
    if _depth > 16:
        return False, "module nesting too deep"
    if rel.stem == "mod":
        name, container = rel.parent.name, rel.parent.parent
    else:
        name, container = rel.stem, rel.parent
    if rel.stem in ("lib", "main") and rel.parent.name == "src":
        return False, f"{rel} is a crate root without #![cfg(windows)]"
    candidates = [container / "mod.rs", container.parent / f"{container.name}.rs"]
    if container.name == "src":
        candidates += [container / "lib.rs", container / "main.rs"]
    for parent in candidates:
        if not (root / parent).is_file():
            continue
        verdict = _decl_is_windows((root / parent).read_text(encoding="utf-8"), name)
        if verdict is None:
            continue
        if verdict:
            return True, f"declared #[cfg(windows)] mod {name} in {parent}"
        inherited, why = windows_only(root, parent, _depth + 1)
        if inherited:
            return True, f"inside Windows-only {parent} ({why})"
        return False, f"`mod {name};` in {parent} has no #[cfg(windows)] and {why}"
    return False, f"no `mod {name};` declaration found for {rel}"


def source_files(root: Path) -> list[PurePosixPath]:
    return sorted(PurePosixPath(p.relative_to(root).as_posix()) for p in root.rglob("*.rs")
                  if "target" not in p.relative_to(root).parts)


def check(root: Path, entries: list[Entry]) -> list[str]:
    """Every problem with the exclusion list against the tree at `root`."""
    problems: list[str] = []
    files = source_files(root)
    for e in entries:
        hits = [f for f in files if e.matches(str(f))]
        if not hits:
            problems.append(f"line {e.line_no}: {e.pattern} matches no file (stale entry? remove it)")
        for f in hits:
            if re.search(NON_PRODUCT, str(f)):
                problems.append(f"line {e.line_no}: {f} is test/bench/fuzz code; it is excluded already")
                continue
            ok, why = windows_only(root, f)
            if not ok:
                problems.append(f"line {e.line_no}: {f} is not entirely cfg(windows): {why}")
            n = len((root / f).read_text(encoding="utf-8").splitlines())
            if n > e.max_lines:
                problems.append(
                    f"line {e.line_no}: {f} has {n} lines, over its budget of {e.max_lines}: move logic into a "
                    "portable tested module instead of growing an excluded adapter"
                )
    return problems


def table(root: Path, entries: list[Entry]) -> str:
    files = source_files(root)
    out = ["| excluded file | lines / budget | reason |", "|---|---:|---|"]
    for e in entries:
        for f in [f for f in files if e.matches(str(f))] or [PurePosixPath(e.pattern + " (no match)")]:
            p = root / f
            n = len(p.read_text(encoding="utf-8").splitlines()) if p.is_file() else 0
            out.append(f"| `{f}` | {n} / {e.max_lines} | {e.reason.replace('|', chr(92) + '|')} |")
    return "\n".join(out) + "\n"


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("command", choices=("regex", "check", "list"))
    parser.add_argument("--root", type=Path, default=Path("."), help="the windows/ directory")
    parser.add_argument("--file", type=Path, help=f"default: <root>/{DEFAULT_FILE}")
    parser.add_argument("--no-non-product", action="store_true",
                        help="regex: leave out the tests/benches/fuzz/examples pattern")
    args = parser.parse_args(argv)
    path = args.file or args.root / DEFAULT_FILE
    try:
        entries = load(path)
    except (OSError, ExcludesError) as e:
        print(f"{path}: {e}", file=sys.stderr)
        return 1
    if args.command == "regex":
        print(ignore_regex(entries, non_product=not args.no_non_product))
        return 0
    if args.command == "list":
        sys.stdout.write(table(args.root, entries))
        return 0
    problems = check(args.root, entries)
    for p in problems:
        print(f"::error file=windows/{DEFAULT_FILE.as_posix()},title=coverage excludes::{p}", file=sys.stderr)
    if problems:
        return 1
    print(f"coverage excludes OK: {len(entries)} entries, every file cfg(windows)-only and within budget")
    return 0


if __name__ == "__main__":
    # UTF-8 and "\n" on Windows too: a "\r" would end up inside the regex
    # captured by `ignore=$(...)` in bash.
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    sys.exit(main(sys.argv[1:]))
