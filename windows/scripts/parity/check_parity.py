#!/usr/bin/env python3
"""Check the Windows parity ledger (windows/docs/PARITY.md) against the Swift tests.

Fails (exit 1) when:

* a Swift test extracted by ``extract_swift_tests.py`` has no ledger row
  (missing), a ledger row names a Swift test that no longer exists (stale), or
  an ID appears more than once (duplicate);
* a row's status is not one of ``todo``, ``ported``, ``n/a``, or its
  workstream is not ``ws-<name>``;
* a row marked ``ported`` does not name a Rust test, or names one that does not
  exist: the file is missing, or it has no ``fn <name>`` carrying a test
  attribute (``#[test]``, ``#[tokio::test]``, ``#[rstest]``, ``#[test_case]``,
  or another ``#[...::test]``; ``#[test]`` fns inside ``proptest!`` count), or
  the test is ``#[ignore]``d. Comments and string literals are skipped, so a
  helper fn or a fn named in a comment never counts;
* a row marked ``n/a`` gives no reason in its Notes column.

``todo`` rows pass: the ledger is allowed to be incomplete, never wrong.

It prints a Markdown summary (counts by status and by workstream, percent
ported, and every problem) to stdout, and appends it to ``--summary FILE``
when given (CI passes ``$GITHUB_STEP_SUMMARY``).

Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import functools
import re
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import extract_swift_tests  # noqa: E402

STATUSES = ("todo", "ported", "n/a")
WORKSTREAM = re.compile(r"^ws-[a-z0-9]+(?:-[a-z0-9]+)*$")
ROW_ID = re.compile(r"^`([^`]+)`$")
CELL_SPLIT = re.compile(r"(?<!\\)\|")
RUST_REF = re.compile(r"^(?P<path>[\w./-]+\.rs)::(?P<fn>[A-Za-z_]\w*)$")
MAX_LISTED = 300
# The last path segment of an attribute that makes a fn a test.
TEST_ATTRIBUTES = {"test", "rstest", "test_case", "test_log", "wasm_bindgen_test"}
FN_MODIFIERS = re.compile(r"(?:\b(?:pub(?:\s*\([^)]*\))?|async|unsafe|const)\s+)*$")
CHAR_LITERAL = re.compile(r"'(?:\\(?:u\{[0-9A-Fa-f]{1,6}\}|x[0-9A-Fa-f]{2}|.)|[^\\'\n])'")

# Best guess of the owning workstream for --print-missing-rows, by path prefix.
# The first match wins; a human still checks the row before committing it.
WORKSTREAM_GUESS = (
    ("TokenMonTests/App/", "ws-app"),
    ("TokenMonTests/Core/Networking/", "ws-core-net"),
    ("TokenMonTests/Core/Polling/", "ws-core-polling"),
    ("TokenMonTests/Core/Storage/", "ws-core-storage"),
    ("TokenMonTests/Core/Usage/", "ws-core-usage"),
    ("TokenMonTests/Core/Utilities/", "ws-core-usage"),
    ("TokenMonTests/Features/Overview/", "ws-overview"),
    ("TokenMonTests/Features/Settings/", "ws-settings"),
    ("TokenMonTests/Features/Update/", "ws-update"),
    ("TokenMonTests/Platform/Auth/CredentialStore", "ws-secrets"),
    ("TokenMonTests/Platform/Auth/WebKitCookieCapture", "ws-webauth"),
    ("TokenMonTests/Platform/Auth/", "ws-foundation"),
    ("TokenMonTests/Platform/SystemWakeGate", "ws-power"),
    ("TokenMonTests/Platform/ThresholdNotifier", "ws-settings"),
    ("TokenMonTests/Providers/Grokbot/", "ws-grokbot"),
    ("TokenMonTests/Providers/Grok/DailyBudget", "ws-core-usage"),
    ("TokenMonTests/Providers/Grok/", "ws-grok"),
    ("TokenMonTests/Providers/Cursor/", "ws-cursor"),
    ("TokenMonTests/Providers/Claude/", "ws-claude"),
    ("TokenMonTests/Providers/ChatGPT/", "ws-chatgpt"),
    ("TokenMonTests/Providers/OpenCode/", "ws-opencode"),
    ("TokenMonTests/Providers/OpenRouter/", "ws-openrouter"),
    ("TokenMonTests/Providers/Registry/", "ws-ui-panels"),
    ("TokenMonTests/UI/MenuBar/MenuBarStatusRenderer", "ws-tray"),
    ("TokenMonTests/UI/SignIn/", "ws-webauth"),
    ("TokenMonTests/UI/", "ws-ui-kit"),
    ("Tests/Manual/", "ws-grok"),
)


@dataclass
class Row:
    id: str
    line: int
    kind: str  # "swift" or "feature"
    workstream: str
    status: str
    rust: str
    notes: str
    text: str = ""  # the whole row, stripped (the drift guard compares revisions)


@dataclass
class Report:
    rows: list[Row] = field(default_factory=list)
    swift_ids: list[str] = field(default_factory=list)
    missing: list[str] = field(default_factory=list)
    stale: list[Row] = field(default_factory=list)
    duplicates: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return not (self.missing or self.stale or self.duplicates or self.errors)


def split_cells(line: str) -> list[str]:
    body = line.strip()
    if body.startswith("|"):
        body = body[1:]
    if body.endswith("|") and not body.endswith("\\|"):
        body = body[:-1]
    return [cell.strip() for cell in CELL_SPLIT.split(body)]


def parse_ledger(text: str) -> tuple[list[Row], list[str]]:
    """Parse every ledger table row.

    A table counts when its header has ``Status`` and ``Rust test`` columns. A
    row counts when its first cell is a single backticked ID: Swift IDs
    contain ``.swift::``; anything else is a feature row.
    """
    rows: list[Row] = []
    errors: list[str] = []
    header: dict[str, int] | None = None
    lines = text.splitlines()
    for number, line in enumerate(lines, start=1):
        stripped = line.strip()
        if not stripped.startswith("|"):
            header = None
            continue
        cells = split_cells(stripped)
        if all(re.fullmatch(r":?-{3,}:?", c) for c in cells if c):
            continue
        lowered = [c.lower() for c in cells]
        if "status" in lowered and "rust test" in lowered:
            header = {name: idx for idx, name in enumerate(lowered)}
            continue
        if header is None:
            continue
        match = ROW_ID.match(cells[0]) if cells else None
        if not match:
            continue
        if len(cells) != len(header):
            errors.append(f"PARITY.md:{number}: expected {len(header)} cells, found {len(cells)} (unescaped `|`?)")
            continue

        def cell(name: str) -> str:
            idx = header.get(name)
            return cells[idx] if idx is not None else ""

        row_id = match.group(1)
        rows.append(
            Row(
                id=row_id,
                line=number,
                kind="swift" if ".swift::" in row_id else "feature",
                workstream=cell("workstream"),
                status=cell("status"),
                rust=cell("rust test"),
                notes=cell("notes"),
                text=stripped,
            )
        )
    return rows, errors


def rust_refs(cell: str) -> list[str]:
    parts = re.split(r"<br\s*/?>|,", cell)
    return [p.strip().strip("`").strip() for p in parts if p.strip().strip("`").strip()]


def blank_rust(source: str) -> str:
    """Return ``source`` with comments and the contents of string and char
    literals replaced by spaces (same length, same newlines)."""
    out = list(source)
    n = len(source)

    def blank(start: int, end: int) -> None:
        for k in range(start, min(end, n)):
            if out[k] != "\n":
                out[k] = " "

    i = 0
    while i < n:
        c = source[i]
        if source.startswith("//", i):
            j = source.find("\n", i)
            j = n if j == -1 else j
            blank(i, j)
            i = j
        elif source.startswith("/*", i):
            depth, j = 0, i
            while j < n:
                if source.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif source.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                    if depth == 0:
                        break
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c == "r" and re.match(r'r#*"', source[i:i + 260]) and not (i and (source[i - 1].isalnum() or source[i - 1] == "_")):
            hashes = re.match(r'r(#*)"', source[i:]).group(1)
            start = i + 2 + len(hashes)
            end = source.find('"' + hashes, start)
            end = n if end == -1 else end
            blank(start, end)
            i = end + 1 + len(hashes)
        elif c == '"':
            j = i + 1
            while j < n and source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            blank(i + 1, j)
            i = j + 1
        elif c == "'":
            m = CHAR_LITERAL.match(source, i)
            if m:
                blank(i + 1, m.end() - 1)
                i = m.end()
            else:
                i += 1  # a lifetime
        else:
            i += 1
    return "".join(out)


def _attributes_before(code: str, end: int) -> list[str]:
    """The ``#[...]`` attributes (inner text) directly in front of ``code[:end]``,
    skipping ``pub``/``async``/``unsafe``/``const`` and whitespace."""
    modifiers = FN_MODIFIERS.search(code, 0, end)
    i = modifiers.start() if modifiers else end
    attrs: list[str] = []
    while True:
        while i > 0 and code[i - 1].isspace():
            i -= 1
        if i == 0 or code[i - 1] != "]":
            return attrs
        depth, j = 0, i - 1
        while j >= 0:
            if code[j] == "]":
                depth += 1
            elif code[j] == "[":
                depth -= 1
                if depth == 0:
                    break
            j -= 1
        if j < 1 or code[j - 1] != "#":
            return attrs
        attrs.append(code[j + 1 : i - 1].strip())
        i = j - 1


def _attribute_name(attr: str) -> str:
    return re.split(r"[\s(=]", attr, maxsplit=1)[0].split("::")[-1]


@functools.lru_cache(maxsize=None)
def _blanked_file(path: Path) -> str:
    return blank_rust(path.read_text(encoding="utf-8", errors="replace"))


def find_test_fn(source: str, name: str, *, blanked: bool = False) -> str | None:
    """Look for a test fn called ``name`` in Rust ``source``.

    Returns None when one exists and is not ignored; otherwise why not."""
    code = source if blanked else blank_rust(source)
    found_plain = False
    for match in re.finditer(rf"\bfn\s+{re.escape(name)}\s*[(<]", code):
        attrs = _attributes_before(code, match.start())
        names = {_attribute_name(a) for a in attrs}
        if not names & TEST_ATTRIBUTES:
            found_plain = True
            continue
        if "ignore" in names:
            return f"`fn {name}` is `#[ignore]`d, so it does not run; mark the row `todo` until it does"
        return None
    if found_plain:
        return f"`fn {name}` has no test attribute (`#[test]`, `#[tokio::test]`, ...); point at the test, not a helper"
    return f"no `fn {name}`"


def check_rust_ref(ref: str, windows_root: Path) -> str | None:
    """Return an error message, or None when ``ref`` names an existing Rust test."""
    match = RUST_REF.match(ref)
    if not match:
        return f"`{ref}` is not `<path>.rs::<fn>`"
    rel = Path(match.group("path"))
    if rel.is_absolute() or ".." in rel.parts:
        return f"`{ref}` must be a path inside windows/"
    path = windows_root / rel
    if not path.is_file():
        if rel.parts[:1] == ("windows",):
            # Strict on purpose: one canonical form keeps the ledger greppable.
            return (f"`{ref}`: paths are relative to windows/, so drop the leading "
                    f"`windows/` (use `{Path(*rel.parts[1:]).as_posix()}`)")
        return f"`{ref}`: file windows/{rel.as_posix()} does not exist"
    problem = find_test_fn(_blanked_file(path.resolve()), match.group("fn"), blanked=True)
    if problem:
        return f"`{ref}`: {problem} in windows/{rel.as_posix()}"
    return None


def check(repo: Path, ledger_path: Path) -> Report:
    report = Report()
    windows_root = repo / "windows"
    _blanked_file.cache_clear()
    report.swift_ids = [t.id for t in extract_swift_tests.extract_all(repo)]
    rows, errors = parse_ledger(ledger_path.read_text(encoding="utf-8"))
    report.rows = rows
    report.errors.extend(errors)

    counts = Counter(r.id for r in rows)
    report.duplicates = sorted(i for i, n in counts.items() if n > 1)
    ledger_swift = {r.id for r in rows if r.kind == "swift"}
    extracted = set(report.swift_ids)
    report.missing = [i for i in report.swift_ids if i not in ledger_swift]
    report.stale = [r for r in rows if r.kind == "swift" and r.id not in extracted]

    for row in rows:
        where = f"PARITY.md:{row.line} `{row.id}`"
        if row.status not in STATUSES:
            report.errors.append(f"{where}: status `{row.status}` is not one of {', '.join(STATUSES)}")
        if not WORKSTREAM.match(row.workstream):
            report.errors.append(f"{where}: workstream `{row.workstream}` is not `ws-<name>`")
        if row.status == "ported":
            refs = rust_refs(row.rust)
            if not refs:
                report.errors.append(f"{where}: marked ported but names no Rust test")
            for ref in refs:
                problem = check_rust_ref(ref, windows_root)
                if problem:
                    report.errors.append(f"{where}: {problem}")
        if row.status == "n/a" and not row.notes.strip():
            report.errors.append(f"{where}: marked n/a without a reason in Notes")
    return report


def guess_workstream(swift_id: str) -> str:
    for prefix, ws in WORKSTREAM_GUESS:
        if swift_id.startswith(prefix):
            return ws
    return "ws-parity"


def missing_rows(report: Report) -> list[str]:
    return [
        f"| `{i}` | TODO: one-line behavior | {guess_workstream(i)} | todo | | |" for i in report.missing
    ]


def _capped(items: list[str]) -> list[str]:
    if len(items) <= MAX_LISTED:
        return items
    return items[:MAX_LISTED] + [f"- … and {len(items) - MAX_LISTED} more"]


def render_summary(report: Report) -> str:
    swift_rows = [r for r in report.rows if r.kind == "swift" and r.id in set(report.swift_ids)]
    feature_rows = [r for r in report.rows if r.kind == "feature"]
    out: list[str] = ["## Windows parity ledger", ""]
    if report.ok:
        out.append("**Result: PASS.** Every Swift test has exactly one ledger row, and every `ported` row points at an existing Rust test.")
    else:
        out.append("**Result: FAIL.** See the problems below; `windows/docs/PARITY.md` explains how to fix each kind.")
    out.append("")

    def status_table(rows: list[Row], label: str) -> None:
        by_status = Counter(r.status for r in rows)
        applicable = len(rows) - by_status["n/a"]
        pct = 100.0 * by_status["ported"] / applicable if applicable else 100.0
        out.extend(
            [
                f"| {label} | Total | ported | todo | n/a | % ported (excl. n/a) |",
                "|---|---:|---:|---:|---:|---:|",
                f"| all | {len(rows)} | {by_status['ported']} | {by_status['todo']} | {by_status['n/a']} | {pct:.1f}% |",
                "",
            ]
        )

    out.append(f"Swift tests extracted: **{len(report.swift_ids)}** (XCTest methods plus `make test-core` checks).")
    out.append("")
    status_table(swift_rows, "Swift tests")

    by_ws: dict[str, Counter] = defaultdict(Counter)
    for row in swift_rows + feature_rows:
        by_ws[row.workstream][row.status] += 1
    out.extend(
        [
            "### By workstream (Swift tests and `FEAT-` rows)",
            "",
            "| Workstream | Total | ported | todo | n/a | % ported (excl. n/a) |",
            "|---|---:|---:|---:|---:|---:|",
        ]
    )
    for ws in sorted(by_ws):
        c = by_ws[ws]
        total = sum(c.values())
        applicable = total - c["n/a"]
        pct = 100.0 * c["ported"] / applicable if applicable else 100.0
        out.append(f"| {ws} | {total} | {c['ported']} | {c['todo']} | {c['n/a']} | {pct:.1f}% |")
    out.append("")
    if feature_rows:
        out.append(f"Features without Swift tests (`FEAT-` rows): **{len(feature_rows)}**.")
        out.append("")
        status_table(feature_rows, "Features")

    def section(title: str, items: list[str], hint: str) -> None:
        if not items:
            return
        out.extend([f"### {title} ({len(items)})", "", hint, ""])
        out.extend(_capped(items))
        out.append("")

    section(
        "Missing from the ledger",
        [f"- `{i}`" for i in report.missing],
        "These Swift tests have no row. Port them or add a `todo` row "
        "(`python3 windows/scripts/parity/check_parity.py --print-missing-rows` prints them).",
    )
    section(
        "Stale ledger rows",
        [f"- `{r.id}` (PARITY.md:{r.line})" for r in report.stale],
        "These rows name Swift tests that no longer exist. Delete them, or rename them if the test was renamed.",
    )
    section(
        "Duplicate IDs",
        [f"- `{i}`" for i in report.duplicates],
        "Each ID must appear exactly once.",
    )
    section("Invalid rows", [f"- {e}" for e in report.errors], "Fix these rows in `windows/docs/PARITY.md`.")
    if report.missing:
        out.extend(["<details><summary>Rows to paste for the missing tests</summary>", "", "```markdown"])
        out.extend(missing_rows(report))
        out.extend(["```", "", "</details>", ""])
    return "\n".join(out) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    repo_default = extract_swift_tests.default_repo_root()
    parser.add_argument("--repo", type=Path, default=repo_default, help="repository root")
    parser.add_argument("--ledger", type=Path, help="ledger path (default: <repo>/windows/docs/PARITY.md)")
    parser.add_argument("--summary", type=Path, help="also append the Markdown summary to this file")
    parser.add_argument(
        "--print-missing-rows",
        action="store_true",
        help="print `todo` ledger rows for the missing Swift tests instead of the summary",
    )
    args = parser.parse_args(argv)
    ledger = args.ledger or args.repo / "windows" / "docs" / "PARITY.md"
    try:
        report = check(args.repo, ledger)
    except (OSError, ValueError) as err:
        print(f"error: {err}", file=sys.stderr)
        return 2
    if args.print_missing_rows:
        print("\n".join(missing_rows(report)))
        return 0
    summary = render_summary(report)
    sys.stdout.write(summary)
    if args.summary:
        with args.summary.open("a", encoding="utf-8") as handle:
            handle.write(summary)
    return 0 if report.ok else 1


if __name__ == "__main__":
    sys.exit(main())
