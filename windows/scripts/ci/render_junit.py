#!/usr/bin/env python3
"""Render a cargo-nextest JUnit report as GitHub step-summary Markdown.

    render_junit.py --junit junit.xml [--list list.json] [--title "Tests (ubuntu)"]

Prints Markdown to stdout:

* one line with the totals,
* a table of passed / failed / skipped per crate,
* for each failing test, its name and an excerpt of its output.

nextest leaves ignored tests out of the JUnit file, so skipped counts come
from `cargo nextest list --message-format json` (``--list``), which marks
every `#[ignore]` test with ``"ignored": true``.

The exit status is always 0 unless the input cannot be read: the test step
itself is the gate, this script only reports.
Standard library only, so it runs on any runner without installing anything.
"""

from __future__ import annotations

import argparse
import json
import sys
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from pathlib import Path

# Keep the summary readable and well below GitHub's 1 MiB step-summary cap.
MAX_FAILURES_SHOWN = 30
MAX_EXCERPT_LINES = 60
MAX_EXCERPT_CHARS = 6000


@dataclass
class CrateCounts:
    passed: int = 0
    failed: int = 0
    skipped: int = 0
    flaky: int = 0
    seconds: float = 0.0


@dataclass
class Failure:
    crate: str
    suite: str
    name: str
    message: str
    output: str


@dataclass
class Report:
    crates: dict[str, CrateCounts] = field(default_factory=dict)
    failures: list[Failure] = field(default_factory=list)

    def crate(self, name: str) -> CrateCounts:
        return self.crates.setdefault(name, CrateCounts())


def crate_of(binary_id: str) -> str:
    """nextest binary ids look like `pkg`, `pkg::test_bin` or `pkg::bin/name`."""
    return binary_id.split("::", 1)[0] or "(unknown)"


def _float(value: str | None) -> float:
    try:
        return float(value or 0)
    except ValueError:
        return 0.0


def parse_junit(path: Path, report: Report) -> None:
    root = ET.parse(path).getroot()
    suites = [root] if root.tag == "testsuite" else root.iter("testsuite")
    for suite in suites:
        suite_name = suite.get("name", "")
        counts = report.crate(crate_of(suite_name))
        for case in suite.iter("testcase"):
            counts.seconds += _float(case.get("time"))
            problem = case.find("failure")
            if problem is None:
                problem = case.find("error")
            if case.find("skipped") is not None:
                counts.skipped += 1
            elif problem is not None:
                counts.failed += 1
                report.failures.append(
                    Failure(
                        crate=crate_of(suite_name),
                        suite=suite_name,
                        name=case.get("name", "?"),
                        message=problem.get("message", "") or "",
                        output=_case_output(case, problem),
                    )
                )
            else:
                counts.passed += 1
                # A test that failed and then passed on retry is flaky.
                if case.find("flakyFailure") is not None or case.find("rerunFailure") is not None:
                    counts.flaky += 1


def _case_output(case: ET.Element, problem: ET.Element) -> str:
    parts = []
    for text in (
        problem.text,
        _text(case.find("system-out")),
        _text(case.find("system-err")),
    ):
        text = (text or "").strip("\n")
        if text and text not in parts:
            parts.append(text)
    return "\n\n".join(parts)


def _text(element: ET.Element | None) -> str:
    return "" if element is None else (element.text or "")


def apply_ignored(list_path: Path, report: Report) -> None:
    """Count `#[ignore]` tests per crate from `cargo nextest list` JSON."""
    data = json.loads(list_path.read_text(encoding="utf-8"))
    for suite in data.get("rust-suites", {}).values():
        crate = suite.get("package-name") or crate_of(suite.get("binary-id", ""))
        ignored = sum(1 for case in suite.get("testcases", {}).values() if case.get("ignored"))
        if ignored:
            report.crate(crate).skipped += ignored


def excerpt(text: str) -> str:
    """The end of the output is where the panic message is; keep that part."""
    lines = text.splitlines()
    if len(lines) > MAX_EXCERPT_LINES:
        dropped = len(lines) - MAX_EXCERPT_LINES
        lines = [f"... ({dropped} earlier lines omitted)"] + lines[-MAX_EXCERPT_LINES:]
    out = "\n".join(lines)
    if len(out) > MAX_EXCERPT_CHARS:
        out = "... (truncated)\n" + out[-MAX_EXCERPT_CHARS:]
    return out


def fence(text: str, lang: str = "text") -> str:
    """A code fence longer than any backtick run inside the text."""
    longest = run = 0
    for ch in text:
        run = run + 1 if ch == "`" else 0
        longest = max(longest, run)
    ticks = "`" * max(3, longest + 1)
    return f"{ticks}{lang}\n{text}\n{ticks}"


def md_cell(text: str) -> str:
    return text.replace("|", "\\|").replace("\n", " ")


def render(report: Report, title: str) -> str:
    total = CrateCounts()
    for c in report.crates.values():
        total.passed += c.passed
        total.failed += c.failed
        total.skipped += c.skipped
        total.flaky += c.flaky
        total.seconds += c.seconds

    status = "FAILED" if total.failed else "passed"
    out = [f"### {title}", ""]
    if not report.crates:
        out.append("No tests ran (the workspace has no tests yet).")
        return "\n".join(out) + "\n"

    flaky = f", {total.flaky} flaky" if total.flaky else ""
    out.append(
        f"**{status}**: {total.passed} passed, {total.failed} failed, "
        f"{total.skipped} skipped{flaky} in {len(report.crates)} crate(s)"
    )
    out += ["", "| crate | passed | failed | skipped | time (s) |", "|---|---:|---:|---:|---:|"]
    for name in sorted(report.crates):
        c = report.crates[name]
        mark = " :x:" if c.failed else ""
        out.append(
            f"| `{md_cell(name)}`{mark} | {c.passed} | {c.failed} | {c.skipped} | {c.seconds:.2f} |"
        )
    out.append(
        f"| **total** | **{total.passed}** | **{total.failed}** | **{total.skipped}** "
        f"| **{total.seconds:.2f}** |"
    )

    if report.failures:
        out += ["", "#### Failing tests", ""]
        for f in report.failures[:MAX_FAILURES_SHOWN]:
            out.append(f"- `{md_cell(f.suite)}` **{md_cell(f.name)}**")
        if len(report.failures) > MAX_FAILURES_SHOWN:
            out.append(f"- ... and {len(report.failures) - MAX_FAILURES_SHOWN} more")
        out.append("")
        for f in report.failures[:MAX_FAILURES_SHOWN]:
            out += [
                "<details open>" if f is report.failures[0] else "<details>",
                f"<summary><code>{_html(f.suite)}</code> {_html(f.name)}</summary>",
                "",
                fence(excerpt(f.output or f.message or "(no output captured)")),
                "",
                "</details>",
                "",
            ]
    return "\n".join(out).rstrip() + "\n"


def _html(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--junit", type=Path, required=True)
    parser.add_argument("--list", type=Path, help="`cargo nextest list --message-format json` output")
    parser.add_argument("--title", default="Tests")
    args = parser.parse_args(argv)

    report = Report()
    if args.junit.is_file():
        parse_junit(args.junit, report)
    else:
        print(f"### {args.title}\n\nNo JUnit report at `{args.junit}`: the test run did not start "
              "(see the build log).")
        return 0
    if args.list and args.list.is_file() and args.list.stat().st_size:
        try:
            apply_ignored(args.list, report)
        except (ValueError, OSError) as err:  # reporting must not fail the job
            print(f"warning: could not read {args.list}: {err}", file=sys.stderr)
    sys.stdout.write(render(report, args.title))
    return 0


if __name__ == "__main__":
    # Step summaries are UTF-8; Windows runners default stdout to cp1252
    # (and `python -I` ignores PYTHONIOENCODING).
    sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main(sys.argv[1:]))
