#!/usr/bin/env python3
"""Zero-drift guard: a macOS change must come with Windows parity or a tracking issue.

The macOS app is ``TokenMon/**`` (sources, ``Resources/``, Info.plist and
entitlements), ``TokenMonTests/**``, ``Tests/**``, and the files that decide how
it is built and released: ``project.yml``, ``Makefile``, ``Scripts/**`` and
``TokenMon.xcodeproj/**``. Docs, README and CI-only files do not count.

A change (pull request or push) is checked with two rules:

A. **Changed Swift tests.** For every Swift test file the change touches, the
   guard digests each test body at the base and at the head (comments and
   formatting ignored; code and string literals count; for ``make test-core``
   checks, the ``assertTrue`` arguments). When the body of a test whose ledger
   row was ``ported`` changed, the change must re-confirm the port: edit that
   row in ``windows/docs/PARITY.md``, or edit one of the Rust test files the
   row names. Renaming a test, or editing a row that is ``todo`` or ``n/a``,
   needs nothing here (the ledger job checks IDs).

B. **Other macOS changes** (app code, resources, build files, and test-file
   code outside test bodies such as helpers, fixtures and ``setUp``) must come
   with a change to ``windows/crates/**`` or ``windows/docs/PARITY.md``, or
   carry a ``Windows-Parity: <reason>`` trailer saying why Windows needs
   nothing (for example a macOS-only menu-bar tweak). A ``windows/`` README or
   docs edit is not enough.

Either rule is also met by an **open** issue labeled ``windows-parity`` that
the change references (``#123``, ``owner/repo#123`` or an issue URL, in the PR
title/body or a commit message), or that names one of the change's commit SHAs
(7 or more hex characters). The SHA route is how a push that cannot be amended
any more (for example an upstream faulknerpearce/token_monitor merge on
``master``) is acknowledged after the fact: open the issue, then re-run the job.

The workflow passes the changed paths, the text and the SHAs as files, and the
base and head commits; the guard reads both revisions of the touched Swift
files and of the ledger with ``git show``. The verdict depends only on those
inputs and the open issues, prints a Markdown summary, and exits 1 on a
violation.

Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_parity  # noqa: E402
import extract_swift_tests  # noqa: E402

# Directories (trailing /) and top-level files that make up the macOS app.
MAC_PREFIXES = ("TokenMon/", "TokenMonTests/", "Tests/", "Scripts/", "TokenMon.xcodeproj/")
MAC_FILES = ("project.yml", "Makefile")
WINDOWS_PREFIX = "windows/"
LEDGER = "windows/docs/PARITY.md"
# A windows/ change that can carry a port: code and tests, or the ledger.
WINDOWS_PORT_PREFIXES = ("windows/crates/", LEDGER)
LABEL = "windows-parity"
TRAILER = re.compile(r"^[ \t]*Windows-Parity:[ \t]*(\S.*?)[ \t]*$", re.MULTILINE | re.IGNORECASE)
MAX_LISTED = 50

Issue = dict  # the GitHub REST issue object (number, title, body, html_url, ...)
IssueFetcher = Callable[[str], list[Issue]]
Reader = Callable[[str, str], "str | None"]  # (revision, path) -> text, or None when absent


@dataclass
class TestChange:
    """A Swift test whose body differs between base and head."""

    id: str
    row_line: int  # the head ledger row
    rust: list[str]  # the Rust tests the row names
    confirmed_by: str = ""  # "" when the change did not re-confirm the port


@dataclass
class Result:
    mac_paths: list[str]
    windows_paths: list[str]
    references: list[int]
    test_changes: list[TestChange] = field(default_factory=list)
    other_mac_paths: list[str] = field(default_factory=list)  # rule B
    windows_port_paths: list[str] = field(default_factory=list)
    trailer: str = ""
    notes: list[str] = field(default_factory=list)
    matched_by_reference: list[Issue] = field(default_factory=list)
    matched_by_sha: list[tuple[Issue, str]] = field(default_factory=list)
    checked_issues: bool = False
    error: str = ""

    @property
    def unconfirmed_tests(self) -> list[TestChange]:
        return [t for t in self.test_changes if not t.confirmed_by]

    @property
    def other_changes_unported(self) -> bool:
        return bool(self.other_mac_paths) and not self.windows_port_paths and not self.trailer

    @property
    def needs_parity(self) -> bool:
        return bool(self.unconfirmed_tests) or self.other_changes_unported

    @property
    def ok(self) -> bool:
        if not self.needs_parity:
            return True
        return not self.error and bool(self.matched_by_reference or self.matched_by_sha)


def is_mac_path(path: str) -> bool:
    return path.startswith(MAC_PREFIXES) or path in MAC_FILES


def is_swift_test_file(path: str) -> bool:
    return (path.startswith(extract_swift_tests.XCTEST_ROOT + "/") and path.endswith(".swift")) or (
        path == extract_swift_tests.CORE_MAIN
    )


def classify(paths: list[str]) -> tuple[list[str], list[str]]:
    """Split changed paths into (macOS paths, windows/ paths)."""
    clean = [p.strip() for p in paths if p.strip()]
    mac = [p for p in clean if is_mac_path(p)]
    win = [p for p in clean if p.startswith(WINDOWS_PREFIX)]
    return mac, win


def find_references(text: str, repo: str) -> list[int]:
    """Issue numbers in ``text`` that point at ``repo`` (``#N``, ``repo#N`` or an issue URL)."""
    owner_repo = re.escape(repo)
    patterns = (
        r"(?<![\w/#&])#(\d+)\b",
        rf"(?<![\w/]){owner_repo}#(\d+)\b",
        rf"https?://github\.com/{owner_repo}/issues/(\d+)\b",
    )
    found: list[int] = []
    for pattern in patterns:
        for match in re.finditer(pattern, text, flags=re.IGNORECASE):
            number = int(match.group(1))
            if number not in found:
                found.append(number)
    return sorted(found)


def find_trailer(text: str) -> str:
    """The reason in the first ``Windows-Parity: <reason>`` line, or ``""``."""
    match = TRAILER.search(text)
    return match.group(1) if match else ""


def mentioned_sha(issue: Issue, shas: list[str]) -> str | None:
    """The first commit in ``shas`` that ``issue`` mentions by a 7+ character prefix."""
    text = f"{issue.get('title') or ''}\n{issue.get('body') or ''}".lower()
    tokens = set(re.findall(r"\b[0-9a-f]{7,40}\b", text))
    for sha in shas:
        sha = sha.strip().lower()
        if sha and any(sha.startswith(token) for token in tokens):
            return sha
    return None


def github_fetcher(api_url: str, token: str) -> IssueFetcher:
    def fetch(repo: str) -> list[Issue]:
        issues: list[Issue] = []
        page = 1
        while True:
            url = f"{api_url}/repos/{repo}/issues?state=open&labels={LABEL}&per_page=100&page={page}"
            request = urllib.request.Request(
                url,
                headers={
                    "Accept": "application/vnd.github+json",
                    "Authorization": f"Bearer {token}",
                    "X-GitHub-Api-Version": "2022-11-28",
                    "User-Agent": "tokenmon-drift-guard",
                },
            )
            with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310 (fixed https API URL)
                batch = json.load(response)
            # The issues endpoint also lists pull requests; only issues count.
            issues.extend(item for item in batch if "pull_request" not in item)
            if len(batch) < 100 or page >= 20:
                return issues
            page += 1

    return fetch


def git_reader(repo_root: Path) -> Reader:
    def read(rev: str, path: str) -> str | None:
        if not rev:
            return None
        proc = subprocess.run(  # noqa: S603 (fixed argv, no shell)
            ["git", "-C", str(repo_root), "show", f"{rev}:{path}"],
            capture_output=True,
            check=False,
        )
        return proc.stdout.decode("utf-8", errors="replace") if proc.returncode == 0 else None

    return read


def _rows(text: str | None) -> dict[str, check_parity.Row]:
    rows, _ = check_parity.parse_ledger(text or "")
    return {row.id: row for row in rows}


def analyze_mac_changes(mac: list[str], changed: set[str], base: str, head: str, read: Reader, result: Result) -> None:
    """Fill ``result.test_changes``, ``other_mac_paths`` and ``notes`` (rules A and B)."""
    base_rows: dict[str, check_parity.Row] | None = None
    head_rows: dict[str, check_parity.Row] = {}
    for path in mac:
        if not is_swift_test_file(path):
            result.other_mac_paths.append(path)
            continue
        old, new = read(base, path), read(head, path)
        if new is None:
            result.other_mac_paths.append(path)  # deleted: the ledger job lists the stale rows
            continue
        try:
            old_bodies, old_rest = extract_swift_tests.body_digests(path, old) if old is not None else ({}, None)
            new_bodies, new_rest = extract_swift_tests.body_digests(path, new)
        except ValueError as err:
            result.notes.append(f"`{path}` could not be scanned ({err}); treated as a non-test change")
            result.other_mac_paths.append(path)
            continue
        if old_rest != new_rest:
            result.other_mac_paths.append(path)
        modified = [i for i, digest in new_bodies.items() if i in old_bodies and old_bodies[i] != digest]
        if not modified:
            continue
        if base_rows is None:
            base_rows, head_rows = _rows(read(base, LEDGER)), _rows(read(head, LEDGER))
        for test_id in modified:
            before, after = base_rows.get(test_id), head_rows.get(test_id)
            if before is None or before.status != "ported" or after is None:
                continue  # nothing was ported, or the row is gone (the ledger job reports it)
            refs = check_parity.rust_refs(after.rust)
            change = TestChange(test_id, after.line, refs)
            if after.text != before.text:
                change.confirmed_by = f"its row (PARITY.md:{after.line}) changed"
            else:
                touched = [r for r in refs if f"{WINDOWS_PREFIX}{r.split('::')[0]}" in changed]
                if touched:
                    change.confirmed_by = f"`windows/{touched[0].split('::')[0]}` changed"
            result.test_changes.append(change)


def evaluate(
    changed: list[str],
    text: str,
    shas: list[str],
    repo: str,
    fetch: IssueFetcher | None,
    *,
    base: str = "",
    head: str = "",
    read: Reader | None = None,
) -> Result:
    mac, win = classify(changed)
    result = Result(mac_paths=mac, windows_paths=win, references=find_references(text, repo))
    result.windows_port_paths = [p for p in win if p.startswith(WINDOWS_PORT_PREFIXES)]
    result.trailer = find_trailer(text)
    analyze_mac_changes(mac, set(win), base, head, read or (lambda _rev, _path: None), result)
    if not result.needs_parity:
        return result
    if fetch is None:
        result.error = "no GitHub token, so the open windows-parity issues could not be listed"
        return result
    try:
        open_issues = fetch(repo)
    except (urllib.error.URLError, OSError, ValueError) as err:
        result.error = f"listing open {LABEL} issues failed: {err}"
        return result
    result.checked_issues = True
    by_number = {issue["number"]: issue for issue in open_issues}
    result.matched_by_reference = [by_number[n] for n in result.references if n in by_number]
    for issue in open_issues:
        sha = mentioned_sha(issue, shas)
        if sha:
            result.matched_by_sha.append((issue, sha))
    return result


def _issue_link(issue: Issue) -> str:
    return f"[#{issue['number']}]({issue.get('html_url', '')}) {issue.get('title', '')}".strip()


def _listed(items: list[str]) -> list[str]:
    out = items[:MAX_LISTED]
    if len(items) > MAX_LISTED:
        out.append(f"- … and {len(items) - MAX_LISTED} more")
    return out


def render_summary(result: Result, event: str, base: str, head: str, repo: str) -> str:
    out = ["## Windows parity drift guard", ""]
    if not result.mac_paths:
        verdict, reason = "PASS", (
            "No macOS app files (`TokenMon/`, `TokenMonTests/`, `Tests/`, `Scripts/`, "
            "`TokenMon.xcodeproj/`, `project.yml`, `Makefile`) changed."
        )
    elif not result.needs_parity:
        verdict, reason = "PASS", "macOS files changed, and the change carries its Windows parity (details below)."
    elif result.ok:
        verdict, reason = "PASS", "macOS files changed without full Windows parity, and an open `windows-parity` issue tracks the port."
    else:
        verdict, reason = "FAIL", "**Windows parity needed:** see *Why it failed* below."
    out.extend([f"**Result: {verdict}.** {reason}", ""])

    refs = ", ".join(f"#{n}" for n in result.references) or "none"
    matched = [_issue_link(i) + " (referenced)" for i in result.matched_by_reference]
    matched += [_issue_link(i) + f" (mentions `{sha[:12]}`)" for i, sha in result.matched_by_sha]
    confirmed = len(result.test_changes) - len(result.unconfirmed_tests)
    out.extend(
        [
            "| Check | Value |",
            "|---|---|",
            f"| Event | `{event}` |",
            f"| Compared | `{base[:12]}`…`{head[:12]}` |",
            f"| macOS paths changed | {len(result.mac_paths)} |",
            f"| `windows/` paths changed | {len(result.windows_paths)} ({len(result.windows_port_paths)} under `windows/crates/` or the ledger) |",
            f"| Ported Swift tests whose body changed | {len(result.test_changes)} ({confirmed} re-confirmed) |",
            f"| Other macOS changes (app, build, test helpers) | {len(result.other_mac_paths)} |",
            f"| `Windows-Parity:` trailer | {result.trailer or 'none'} |",
            f"| Issue references found | {refs} |",
            f"| Open `{LABEL}` issues matched | {'<br>'.join(matched) if matched else ('none' if result.checked_issues else 'not checked')} |",
            "",
        ]
    )
    if result.error:
        out.extend([f"Error: {result.error}.", ""])
    for note in result.notes:
        out.extend([f"Note: {note}.", ""])

    if verdict == "FAIL":
        out.extend(["### Why it failed", ""])
        if result.unconfirmed_tests:
            out.extend(
                [
                    "**Rule A: these Swift tests changed, but their Windows port was not re-confirmed.** "
                    "Each row is `ported`, so the Rust test may still assert the old behavior. Check the Rust "
                    "test against the new Swift body, then update the Rust test, or the row (for example its "
                    "Behavior or Notes) if the Rust test already matches. Touching either one re-confirms it.",
                    "",
                ]
            )
            out.extend(
                _listed(
                    [
                        f"- `{t.id}`: row PARITY.md:{t.row_line}, Rust "
                        + (", ".join(f"`{r}`" for r in t.rust) or "none")
                        for t in result.unconfirmed_tests
                    ]
                )
            )
            out.append("")
        if result.other_changes_unported:
            out.extend(
                [
                    "**Rule B: macOS app, build or test-helper files changed, but nothing under "
                    "`windows/crates/` or `windows/docs/PARITY.md` did.** A `windows/` README or docs edit "
                    "does not count.",
                    "",
                ]
            )
            out.extend(_listed([f"- `{p}`" for p in result.other_mac_paths]))
            out.append("")
        out.extend(
            [
                "### How to fix",
                "",
                "Every macOS change needs Windows parity (CONTRIBUTING.md, \"Windows parity (zero drift)\"). Either:",
                "",
                "1. **Port it** in the same change: update the Rust code and tests under `windows/crates/` and the "
                "rows in `windows/docs/PARITY.md`;",
                "2. **Say why Windows needs nothing** (rule B only): add a `Windows-Parity: <reason>` line to the "
                "pull request description, or to a commit message for a direct push; or",
                f"3. **Track it**: open an issue in `{repo}` labeled `{LABEL}` describing what the Windows port must "
                "do, then reference it (`Refs #123`) in the pull request description, or in a commit message for "
                "a direct push.",
                "",
                "For a push that is already on the branch and cannot be amended (for example an upstream",
                "faulknerpearce/token_monitor merge arriving on `master`), open the `windows-parity` issue, put this",
                f"commit SHA (`{head[:12]}`) and the upstream PR link in its description, and re-run this job.",
                "",
            ]
        )
    elif result.test_changes or result.other_mac_paths:
        out.extend(["### Why it passed", ""])
        out.extend(_listed([f"- `{t.id}`: {t.confirmed_by or 'tracked by the matched issue'}" for t in result.test_changes]))
        if result.other_mac_paths:
            if result.windows_port_paths:
                why = f"`windows/` port changes ({len(result.windows_port_paths)} paths)"
            elif result.trailer:
                why = f"the `Windows-Parity:` trailer ({result.trailer})"
            else:
                why = "the matched issue"
            out.append(f"- {len(result.other_mac_paths)} other macOS path(s): covered by {why}")
        out.append("")
    if result.mac_paths:
        out.append("<details><summary>macOS paths that triggered the guard</summary>")
        out.append("")
        out.extend(_listed([f"- `{p}`" for p in result.mac_paths]))
        out.extend(["", "</details>", ""])
    return "\n".join(out) + "\n"


def _read(path: Path | None) -> str:
    return path.read_text(encoding="utf-8", errors="replace") if path else ""


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--changed-files", type=Path, required=True, help="file with one changed path per line")
    parser.add_argument("--text-file", type=Path, help="PR title and body, or the pushed commit messages")
    parser.add_argument("--shas-file", type=Path, help="file with the change's commit SHAs, one per line")
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""), help="owner/name")
    parser.add_argument("--repo-root", type=Path, default=Path("."), help="git checkout to read revisions from")
    parser.add_argument("--event", default=os.environ.get("GITHUB_EVENT_NAME", "local"))
    parser.add_argument("--base", default="", help="base commit (the merge base for a pull request)")
    parser.add_argument("--head", default="", help="head commit")
    parser.add_argument("--summary", type=Path, help="also append the Markdown summary to this file")
    args = parser.parse_args(argv)

    token = os.environ.get("GITHUB_TOKEN", "")
    api_url = os.environ.get("GITHUB_API_URL", "https://api.github.com")
    fetch = github_fetcher(api_url, token) if token and args.repo else None
    result = evaluate(
        _read(args.changed_files).splitlines(),
        _read(args.text_file),
        _read(args.shas_file).split(),
        args.repo,
        fetch,
        base=args.base,
        head=args.head,
        read=git_reader(args.repo_root),
    )
    summary = render_summary(result, args.event, args.base, args.head, args.repo)
    sys.stdout.write(summary)
    if args.summary:
        with args.summary.open("a", encoding="utf-8") as handle:
            handle.write(summary)
    if not result.ok:
        print(
            "::error title=Windows parity needed::macOS files changed without their Windows parity "
            f"(re-confirmed ledger rows for changed Swift tests, a windows/ port, a Windows-Parity trailer, or an "
            f"open {LABEL} issue). See the job summary for the exact tests and paths."
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
