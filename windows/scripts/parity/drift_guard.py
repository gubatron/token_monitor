#!/usr/bin/env python3
"""Zero-drift guard: a macOS change must come with Windows parity or a tracking issue.

A change (pull request or push) that touches the macOS app must also do one of
these. The macOS app is ``TokenMon/**`` (sources, ``Resources/``, Info.plist and
entitlements), ``TokenMonTests/**``, ``Tests/**``, and the files that decide how
it is built and released: ``project.yml``, ``Makefile``, ``Scripts/**`` and
``TokenMon.xcodeproj/**``. Docs, README and CI-only files do not count.

1. touch ``windows/**`` (the port is updated in the same change), or
2. reference an **open** issue in this repository labeled ``windows-parity``,
   as ``#123``, ``owner/repo#123`` or an issue URL, in the PR title/body (pull
   requests) or in a commit message (pushes), or
3. be named by such an issue: an open ``windows-parity`` issue whose title or
   body contains one of the change's commit SHAs (7 or more hex characters).
   This is how a push that cannot be amended any more (for example an upstream
   faulknerpearce/token_monitor merge on ``master``) is acknowledged after the
   fact: open the issue, then re-run the job.

The workflow computes the inputs with git and passes them as files; this script
classifies them, asks the GitHub API for the open ``windows-parity`` issues
(only when it has to), prints a Markdown summary and exits 1 on a violation.

Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

# Directories (trailing /) and top-level files that make up the macOS app.
MAC_PREFIXES = ("TokenMon/", "TokenMonTests/", "Tests/", "Scripts/", "TokenMon.xcodeproj/")
MAC_FILES = ("project.yml", "Makefile")
WINDOWS_PREFIX = "windows/"
LABEL = "windows-parity"
MAX_LISTED = 50

Issue = dict  # the GitHub REST issue object (number, title, body, html_url, ...)
IssueFetcher = Callable[[str], list[Issue]]


@dataclass
class Result:
    mac_paths: list[str]
    windows_paths: list[str]
    references: list[int]
    matched_by_reference: list[Issue] = field(default_factory=list)
    matched_by_sha: list[tuple[Issue, str]] = field(default_factory=list)
    checked_issues: bool = False
    error: str = ""

    @property
    def needs_parity(self) -> bool:
        return bool(self.mac_paths) and not self.windows_paths

    @property
    def ok(self) -> bool:
        if not self.needs_parity:
            return True
        return not self.error and bool(self.matched_by_reference or self.matched_by_sha)


def classify(paths: list[str]) -> tuple[list[str], list[str]]:
    """Split changed paths into (macOS paths, windows/ paths)."""
    clean = [p.strip() for p in paths if p.strip()]
    mac = [p for p in clean if p.startswith(MAC_PREFIXES) or p in MAC_FILES]
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


def evaluate(
    changed: list[str], text: str, shas: list[str], repo: str, fetch: IssueFetcher | None
) -> Result:
    mac, win = classify(changed)
    result = Result(mac_paths=mac, windows_paths=win, references=find_references(text, repo))
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


def render_summary(result: Result, event: str, base: str, head: str, repo: str) -> str:
    out = ["## Windows parity drift guard", ""]
    if not result.needs_parity:
        verdict = "PASS"
        reason = (
            "macOS sources changed together with `windows/`."
            if result.mac_paths
            else "No macOS app files (`TokenMon/`, `TokenMonTests/`, `Tests/`, `Scripts/`, `TokenMon.xcodeproj/`, `project.yml`, `Makefile`) changed."
        )
    elif result.ok:
        verdict = "PASS"
        reason = "macOS sources changed without `windows/`, and an open `windows-parity` issue tracks the port."
    else:
        verdict = "FAIL"
        reason = "**Parity issue needed:** macOS sources changed, `windows/` did not, and no open `windows-parity` issue tracks it."
    out.extend([f"**Result: {verdict}.** {reason}", ""])

    refs = ", ".join(f"#{n}" for n in result.references) or "none"
    matched = [_issue_link(i) + " (referenced)" for i in result.matched_by_reference]
    matched += [_issue_link(i) + f" (mentions `{sha[:12]}`)" for i, sha in result.matched_by_sha]
    out.extend(
        [
            "| Check | Value |",
            "|---|---|",
            f"| Event | `{event}` |",
            f"| Compared | `{base[:12]}`…`{head[:12]}` |",
            f"| macOS paths changed | {len(result.mac_paths)} |",
            f"| `windows/` paths changed | {len(result.windows_paths)} |",
            f"| Issue references found | {refs} |",
            f"| Open `{LABEL}` issues matched | {'<br>'.join(matched) if matched else ('none' if result.checked_issues else 'not checked')} |",
            "",
        ]
    )
    if result.error:
        out.extend([f"Error: {result.error}.", ""])
    if result.mac_paths:
        listed = result.mac_paths[:MAX_LISTED]
        out.append("<details><summary>macOS paths that triggered the guard</summary>")
        out.append("")
        out.extend(f"- `{p}`" for p in listed)
        if len(result.mac_paths) > MAX_LISTED:
            out.append(f"- … and {len(result.mac_paths) - MAX_LISTED} more")
        out.extend(["", "</details>", ""])
    if verdict == "FAIL":
        out.extend(
            [
                "### How to fix",
                "",
                "Every macOS change needs Windows parity (CONTRIBUTING.md, \"Windows parity (zero drift)\"). Either:",
                "",
                "1. **Port it** in the same change: update `windows/` (the Rust code, its tests, and the rows in `windows/docs/PARITY.md`); or",
                f"2. **Track it**: open an issue in `{repo}` labeled `{LABEL}` describing what the Windows port must do, then reference it",
                "   (`Refs #123`) in the pull request description, or in a commit message for a direct push.",
                "",
                "For a push that is already on the branch and cannot be amended (for example an upstream",
                "faulknerpearce/token_monitor merge arriving on `master`), open the `windows-parity` issue, put this",
                f"commit SHA (`{head[:12]}`) and the upstream PR link in its description, and re-run this job.",
                "",
            ]
        )
    return "\n".join(out) + "\n"


def _read(path: Path | None) -> str:
    return path.read_text(encoding="utf-8", errors="replace") if path else ""


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--changed-files", type=Path, required=True, help="file with one changed path per line")
    parser.add_argument("--text-file", type=Path, help="PR title and body, or the pushed commit messages")
    parser.add_argument("--shas-file", type=Path, help="file with the change's commit SHAs, one per line")
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""), help="owner/name")
    parser.add_argument("--event", default=os.environ.get("GITHUB_EVENT_NAME", "local"))
    parser.add_argument("--base", default="")
    parser.add_argument("--head", default="")
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
    )
    summary = render_summary(result, args.event, args.base, args.head, args.repo)
    sys.stdout.write(summary)
    if args.summary:
        with args.summary.open("a", encoding="utf-8") as handle:
            handle.write(summary)
    if not result.ok:
        print(
            "::error title=Windows parity needed::macOS sources changed without windows/ and without an open "
            f"{LABEL} issue. Port the change under windows/, or open a {LABEL} issue and reference it "
            "(see the job summary)."
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
