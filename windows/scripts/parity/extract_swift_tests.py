#!/usr/bin/env python3
"""Extract every macOS (Swift) test so the Windows parity ledger can track it.

Two sources are scanned, relative to the repository root:

* ``TokenMonTests/**/*.swift``: every XCTest method. A method counts when it is
  an instance method named ``test...`` that takes no parameters, is declared
  directly inside a class (or an extension of a class) that inherits from
  ``XCTestCase`` (directly or through another test class), and is not
  ``private``, ``fileprivate`` or ``static``. These are the methods XCTest runs.
  Swift Testing tests count too: every function marked ``@Test`` (in any
  type, or at file scope), whatever its name or parameters. A file that has an
  ``@Test`` the scanner cannot attribute to a function is an error, so a new
  kind of test can never slip past the ledger unnoticed.
* ``Tests/Manual/CoreTestsMain.swift``: every ``assertTrue(<condition>, "<label>")``
  check in the command-line core smoke test (``make test-core``). Two checks
  whose labels give the same ID are an error: each check needs its own row.

Each test gets a stable ID:

* ``TokenMonTests/Providers/Grok/GrokUsageParsingTests.swift::GrokUsageParsingTests::testParseFixtureJSON``
* ``TokenMonTests/Core/PaceTests.swift::PaceTests::resetMovesWithPeriod`` (a
  Swift Testing ``@Test``; the middle part is the enclosing type, or
  ``global`` for a function at file scope)
* ``Tests/Manual/CoreTestsMain.swift::CoreTestsMain::grpc_usedpercent`` (the
  label, lower-cased, with runs of other characters folded to ``_``)

The scanner is a small Swift lexer, not a regex over raw text: string literals
(including multi-line, raw and interpolated ones) and comments are blanked out
before braces are counted, so a ``{`` or ``func test`` inside a string or a
comment never confuses it.

Python 3 standard library only. Run ``--help`` for the output formats.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from dataclasses import asdict, dataclass
from pathlib import Path

XCTEST_ROOT = "TokenMonTests"
CORE_MAIN = "Tests/Manual/CoreTestsMain.swift"
CORE_CLASS = "CoreTestsMain"
GLOBAL_SCOPE = "global"
SWIFT_TESTING_ATTR = re.compile(r"@Test\b")


@dataclass(frozen=True)
class SwiftTest:
    """One Swift test (an XCTest method or a CoreTestsMain check)."""

    id: str
    path: str
    cls: str
    name: str
    line: int
    kind: str  # "xctest", "swift-testing" or "core-check"


# ---------------------------------------------------------------------------
# Lexing
# ---------------------------------------------------------------------------


def blank_swift(source: str, keep_strings: bool = False) -> str:
    """Return ``source`` with comments and string-literal contents replaced by spaces.

    The result has the same length and the same newlines as the input, so
    offsets and line numbers still line up. String delimiters are kept as
    ``"`` so a later pass can still see that a literal was there.
    Interpolated code inside ``\\( ... )`` is kept, because it is code.
    With ``keep_strings`` only comments are blanked (for body digests, where a
    changed expected string is a changed test).
    """
    out = list(source)
    n = len(source)

    def blank(start: int, end: int) -> None:
        for k in range(start, min(end, n)):
            if out[k] != "\n":
                out[k] = " "

    def blank_literal(start: int, end: int) -> None:
        if not keep_strings:
            blank(start, end)

    def skip_code(i: int, stop_at_paren: bool) -> int:
        """Scan code from ``i``; return the index after the closing ``)`` when
        ``stop_at_paren`` (interpolation), else ``n``."""
        depth = 0
        while i < n:
            c = source[i]
            if source.startswith("//", i):
                j = source.find("\n", i)
                j = n if j == -1 else j
                blank(i, j)
                i = j
                continue
            if source.startswith("/*", i):
                j = skip_block_comment(i)
                blank(i, j)
                i = j
                continue
            if c == "#" or c == '"':
                j = skip_string(i)
                if j is not None:
                    i = j
                    continue
            if stop_at_paren:
                if c == "(":
                    depth += 1
                elif c == ")":
                    if depth == 0:
                        return i + 1
                    depth -= 1
            i += 1
        return n

    def skip_block_comment(i: int) -> int:
        # Swift block comments nest.
        depth = 0
        while i < n:
            if source.startswith("/*", i):
                depth += 1
                i += 2
            elif source.startswith("*/", i):
                depth -= 1
                i += 2
                if depth == 0:
                    return i
            else:
                i += 1
        return n

    def skip_string(i: int) -> int | None:
        """If a string literal starts at ``i``, blank its contents and return
        the index after it; otherwise return ``None``."""
        hashes = 0
        j = i
        while j < n and source[j] == "#":
            hashes += 1
            j += 1
        if j >= n or source[j] != '"':
            return None
        multiline = source.startswith('"""', j)
        open_len = 3 if multiline else 1
        close = ('"""' if multiline else '"') + "#" * hashes
        escape = "\\" + "#" * hashes
        k = j + open_len
        content_start = k
        while k < n:
            if source.startswith(escape, k):
                after = k + len(escape)
                if after < n and source[after] == "(":
                    blank_literal(content_start, k)
                    k = skip_code(after + 1, stop_at_paren=True)
                    content_start = k
                    continue
                k = after + 1
                continue
            if source.startswith(close, k):
                blank_literal(content_start, k)
                return k + len(close)
            if not multiline and source[k] == "\n":
                # Unterminated single-line literal: stop at the line end.
                blank_literal(content_start, k)
                return k
            k += 1
        blank_literal(content_start, n)
        return n

    skip_code(0, stop_at_paren=False)
    return "".join(out)


# ---------------------------------------------------------------------------
# XCTest discovery
# ---------------------------------------------------------------------------

TYPE_DECL = re.compile(
    r"\b(class|struct|enum|actor|extension|protocol)\s+([A-Za-z_][\w.]*)([^{]*)$", re.S
)
FUNC_DECL = re.compile(
    r"\bfunc\s+([A-Za-z_]\w*)\s*(<[^>{]*>)?\s*\(([^)]*)\)[^{]*$", re.S
)
MODIFIERS_BEFORE_FUNC = re.compile(r"((?:@\w+(?:\([^)]*\))?\s+|\w+\s+)*)func\s+\w+[^{]*$", re.S)


@dataclass
class _Scope:
    kind: str  # "type" or "other"
    name: str = ""
    keyword: str = ""
    candidate: "Candidate | None" = None


@dataclass
class Candidate:
    """A test-shaped function found in one file (see ``scan_swift_file``)."""

    owner: str
    name: str
    line: int
    kind: str
    start: int  # offset of the `func` keyword (the digest covers signature and body)
    end: int = -1  # offset just past the body's ``}``


def _header(code: str, brace: int) -> str:
    """The declaration text in front of the ``{`` at ``brace``."""
    start = max(code.rfind("{", 0, brace), code.rfind("}", 0, brace), code.rfind(";", 0, brace))
    return code[start + 1 : brace]


def _inherits(clause: str) -> list[str]:
    clause = clause.split(" where ")[0]
    if ":" not in clause:
        return []
    return [part.strip().split("<")[0].split(".")[-1] for part in clause.split(":", 1)[1].split(",")]


def scan_swift_file(code: str) -> tuple[list[Candidate], dict[str, list[str]]]:
    """Scan blanked Swift code.

    Returns ``(candidates, supertypes)``. Each candidate has an owner (type
    name), a function name, a line, the body's offsets, and a kind:

    * kind ``"xctest"``: a parameterless, non-private, non-static
      ``func test...`` declared directly in a class or extension (it is a test
      when that class turns out to be an ``XCTestCase``);
    * kind ``"swift-testing"``: any function marked ``@Test``, declared in a
      type or at file scope (``type_name`` is ``global`` then).

    ``supertypes`` maps each class declared in the file to its inheritance list.
    """
    stack: list[_Scope] = []
    candidates: list[Candidate] = []
    supertypes: dict[str, list[str]] = {}
    for idx, ch in enumerate(code):
        if ch == "}":
            if stack:
                closed = stack.pop()
                if closed.candidate is not None:
                    closed.candidate.end = idx + 1
            continue
        if ch != "{":
            continue
        header = _header(code, idx)
        func = FUNC_DECL.search(header)
        type_match = None if func else TYPE_DECL.search(header)
        if type_match:
            keyword, name = type_match.group(1), type_match.group(2).split(".")[-1]
            if keyword == "class":
                supertypes.setdefault(name, []).extend(_inherits(type_match.group(3)))
            stack.append(_Scope("type", name, keyword))
            continue
        found: Candidate | None = None
        if func:
            decl = idx - len(header) + func.start()
            line = code.count("\n", 0, decl) + 1
            name = func.group(1)
            in_type = bool(stack) and stack[-1].kind == "type"
            if SWIFT_TESTING_ATTR.search(header[: func.start()]) and (in_type or not stack):
                owner = stack[-1].name if in_type else GLOBAL_SCOPE
                found = Candidate(owner, name, line, "swift-testing", decl)
            elif in_type and stack[-1].keyword in ("class", "extension"):
                params = func.group(3).strip()
                mods_match = MODIFIERS_BEFORE_FUNC.search(header)
                mods = mods_match.group(1).split() if mods_match else []
                if (
                    name.startswith("test")
                    and not params
                    and not {"private", "fileprivate", "static", "class"} & set(mods)
                ):
                    found = Candidate(stack[-1].name, name, line, "xctest", decl)
        if found:
            candidates.append(found)
        stack.append(_Scope("other", candidate=found))
    return candidates, supertypes


def _unattributed_swift_testing(rel: str, code: str, candidates: list[Candidate]) -> None:
    """Fail when ``@Test`` appears more often than the scanner found ``@Test`` functions."""
    marks = [code.count("\n", 0, m.start()) + 1 for m in SWIFT_TESTING_ATTR.finditer(code)]
    found = sum(c.kind == "swift-testing" for c in candidates)
    if len(marks) != found:
        raise ValueError(
            f"{rel}: found {len(marks)} `@Test` attribute(s) (lines {', '.join(map(str, marks))}) but could "
            f"extract only {found} Swift Testing function(s); declare each `@Test func` directly in a type or "
            "at file scope, or teach windows/scripts/parity/extract_swift_tests.py the new form"
        )


def _is_test_case(name: str, supertypes: dict[str, list[str]], seen: set[str] | None = None) -> bool:
    if name == "XCTestCase":
        return True
    seen = seen or set()
    if name in seen:
        return False
    seen.add(name)
    return any(_is_test_case(parent, supertypes, seen) for parent in supertypes.get(name, []))


def extract_xctests(repo: Path) -> list[SwiftTest]:
    root = repo / XCTEST_ROOT
    if not root.is_dir():
        return []
    per_file: list[tuple[str, list[Candidate]]] = []
    supertypes: dict[str, list[str]] = {}
    for path in sorted(root.rglob("*.swift")):
        rel = path.relative_to(repo).as_posix()
        code = blank_swift(path.read_text(encoding="utf-8"))
        candidates, file_supers = scan_swift_file(code)
        _unattributed_swift_testing(rel, code, candidates)
        for cls, parents in file_supers.items():
            supertypes.setdefault(cls, []).extend(parents)
        per_file.append((rel, candidates))
    tests: list[SwiftTest] = []
    for rel, candidates in per_file:
        for c in candidates:
            if c.kind == "swift-testing" or _is_test_case(c.owner, supertypes):
                tests.append(SwiftTest(f"{rel}::{c.owner}::{c.name}", rel, c.owner, c.name, c.line, c.kind))
    return tests


# ---------------------------------------------------------------------------
# CoreTestsMain checks
# ---------------------------------------------------------------------------

STRING_LITERAL = re.compile(r'^"((?:[^"\\]|\\.)*)"$', re.S)


def slugify(label: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "_", label.lower()).strip("_")
    return slug or "check"


def _call_arguments(source: str, code: str, open_paren: int) -> tuple[list[str], int]:
    """Split the arguments of the call whose ``(`` is at ``open_paren``.

    Uses the blanked ``code`` to find top-level commas and the closing paren,
    and returns the raw ``source`` slices (so string labels are readable)."""
    depth = 0
    args: list[str] = []
    start = open_paren + 1
    i = open_paren
    while i < len(code):
        c = code[i]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                args.append(source[start:i].strip())
                return args, i
        elif c == "," and depth == 1:
            args.append(source[start:i].strip())
            start = i + 1
        i += 1
    return args, len(code)


def scan_core_checks(source: str) -> list[Candidate]:
    """Every ``assertTrue`` check in CoreTestsMain ``source``, named by its label slug.

    The span covers the call's arguments (the condition and the label)."""
    code = blank_swift(source)
    checks: list[Candidate] = []
    first_line: dict[str, int] = {}
    for match in re.finditer(r"\bassertTrue\s*\(", code):
        if re.search(r"\bfunc\s*$", code[: match.start()]):
            continue  # the helper's own declaration
        args, close = _call_arguments(source, code, match.end() - 1)
        label_match = STRING_LITERAL.match(args[-1]) if args else None
        if not label_match:
            raise ValueError(
                f"{CORE_MAIN}:{code.count(chr(10), 0, match.start()) + 1}: assertTrue needs a "
                "string-literal label as its last argument so the parity ledger can name it"
            )
        slug = slugify(label_match.group(1))
        line = code.count("\n", 0, match.start()) + 1
        if slug in first_line:
            # Numbering the second one would let two checks share a row by
            # accident (and renumber when one is removed). Make the author pick.
            raise ValueError(
                f"{CORE_MAIN}:{line}: assertTrue label {label_match.group(1)!r} gives the ID `{slug}`, "
                f"already used by the check on line {first_line[slug]}; give each check a distinct label"
            )
        first_line[slug] = line
        checks.append(Candidate(CORE_CLASS, slug, line, "core-check", match.start(), close + 1))
    return checks


def extract_core_checks(repo: Path) -> list[SwiftTest]:
    path = repo / CORE_MAIN
    if not path.is_file():
        return []
    return [
        SwiftTest(f"{CORE_MAIN}::{CORE_CLASS}::{c.name}", CORE_MAIN, CORE_CLASS, c.name, c.line, "core-check")
        for c in scan_core_checks(path.read_text(encoding="utf-8"))
    ]


# ---------------------------------------------------------------------------
# Body digests (for the drift guard)
# ---------------------------------------------------------------------------


def _normalize(code: str) -> str:
    """Drop formatting: whitespace runs vanish, except one space between two
    word characters (so ``let x`` stays two tokens)."""
    collapsed = re.sub(r"\s+", " ", code).strip()
    return re.sub(r"(?<=\W) | (?=\W)", "", collapsed)


def _digest(code: str) -> str:
    return hashlib.sha256(_normalize(code).encode("utf-8")).hexdigest()[:16]


def body_digests(rel: str, source: str) -> tuple[dict[str, str], str]:
    """Digest every test-shaped body in one Swift file.

    Returns ``({test ID: digest}, rest digest)``. Comments and formatting do
    not change a digest; code and string literals do. The rest digest covers
    everything outside the test bodies (helpers, fixtures, ``setUp``,
    signatures), which can change what several tests check at once.

    It works on one file's text, so it can digest an old revision. It does not
    check that a class really is an ``XCTestCase`` (that needs the other
    files); callers match the IDs against the ledger instead.
    """
    code = blank_swift(source)
    commentless = blank_swift(source, keep_strings=True)
    if rel == CORE_MAIN:
        found = scan_core_checks(source)
    else:
        found, _ = scan_swift_file(code)
    digests: dict[str, str] = {}
    rest: list[str] = []
    cursor = 0
    for c in sorted(found, key=lambda c: c.start):
        if c.start < cursor:
            continue  # nested inside a body already digested
        end = c.end if c.end > c.start else len(source)
        test_id = f"{rel}::{c.owner}::{c.name}"
        body = _digest(commentless[c.start : end])
        # Same name twice (overloads in extensions): one digest for both.
        digests[test_id] = body if test_id not in digests else _digest(digests[test_id] + body)
        rest.append(commentless[cursor : c.start])
        cursor = end
    rest.append(commentless[cursor:])
    return digests, _digest(" ".join(rest))


def extract_all(repo: Path) -> list[SwiftTest]:
    tests = extract_xctests(repo) + extract_core_checks(repo)
    seen: set[str] = set()
    for test in tests:
        if test.id in seen:
            raise ValueError(f"duplicate Swift test ID {test.id}")
        seen.add(test.id)
    return tests


def default_repo_root() -> Path:
    return Path(__file__).resolve().parents[3]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--repo", type=Path, default=default_repo_root(), help="repository root")
    parser.add_argument(
        "--format",
        choices=("ids", "json", "count"),
        default="ids",
        help="ids: one ID per line (default); json: full records; count: totals only",
    )
    args = parser.parse_args(argv)
    try:
        tests = extract_all(args.repo)
    except ValueError as err:
        print(f"error: {err}", file=sys.stderr)
        return 2
    if args.format == "ids":
        for test in tests:
            print(test.id)
    elif args.format == "json":
        json.dump([asdict(t) for t in tests], sys.stdout, indent=2)
        print()
    else:
        kinds = {kind: sum(t.kind == kind for t in tests) for kind in ("xctest", "swift-testing", "core-check")}
        print(" ".join(f"{k}={v}" for k, v in kinds.items()) + f" total={len(tests)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
