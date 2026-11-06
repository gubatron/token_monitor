#!/usr/bin/env python3
"""Aggregate the jobs of a workflow run into one step-summary table.

    NEEDS_JSON='${{ toJSON(needs) }}' render_ci_summary.py \
        --jobs jobs.json --run-url URL [--self "Summary"] [--skip-note TEXT]

``--jobs`` is the output of `gh api repos/<repo>/actions/runs/<id>/jobs`
(``{"jobs": [...]}``); it gives one row per job and matrix leg, with its
duration and a link to it. ``NEEDS_JSON`` is the `needs` context; it decides
the exit status, so this job can be the single required check:

* exit 1 when any needed job failed or was cancelled,
* skipped jobs pass (they are skipped when there is nothing to check yet),
* jobs named in ``--supersedable`` pass when cancelled: on windows-port a
  newer push cancels them (job-level concurrency) so only the newest commit
  pays for coverage, fuzzing and the release build.

The header names the commit that was tested (``--sha`` and the
COMMIT_SUBJECT environment variable), since many runs are in flight at once.

Standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from datetime import datetime
from pathlib import Path

ICON = {
    "success": ":white_check_mark: success",
    "failure": ":x: failure",
    "cancelled": ":no_entry_sign: cancelled",
    "skipped": ":fast_forward: skipped",
    "timed_out": ":x: timed out",
    "neutral": "neutral",
    "action_required": "action required",
}


def parse_time(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def duration(job: dict) -> str:
    start, end = parse_time(job.get("started_at")), parse_time(job.get("completed_at"))
    if not start or not end:
        return ""
    secs = max(0, int((end - start).total_seconds()))
    return f"{secs // 60}m {secs % 60:02d}s" if secs >= 60 else f"{secs}s"


def commit_line(args) -> str:
    if not args.sha:
        return ""
    short = args.sha[:7]
    link = f"[`{short}`]({args.server_url}/{args.repo}/commit/{args.sha})" if args.repo else f"`{short}`"
    subject = (os.environ.get("COMMIT_SUBJECT") or "").strip().splitlines()
    subject_text = f": {subject[0][:120]}" if subject else ""
    where = " ".join(x for x in (f"`{args.ref}`" if args.ref else "", f"({args.event})" if args.event else "") if x)
    return f"Tested commit {link}{subject_text} {where}".rstrip()


def render(needs: dict, jobs: list[dict], run_url: str, self_name: str, skip_note: str,
           supersedable: set[str] | None = None, header: str = "") -> tuple[str, bool]:
    supersedable = supersedable or set()
    failed = sorted(
        name for name, job in needs.items()
        if job.get("result") == "failure"
        or (job.get("result") == "cancelled" and name not in supersedable)
    )
    superseded = sorted(n for n, j in needs.items() if j.get("result") == "cancelled" and n in supersedable)
    out = ["## Windows CI summary", ""]
    if header:
        out += [header, ""]
    if failed:
        out.append(f"**{len(failed)} job(s) need attention:** " + ", ".join(f"`{n}`" for n in failed))
    else:
        out.append("**All jobs passed or had nothing to check yet.**")
    out.append("")

    rows = [j for j in jobs if j.get("name") != self_name]
    if rows:
        out += ["| job | result | duration |", "|---|---|---|"]
        for job in sorted(rows, key=lambda j: j.get("name", "")):
            result = job.get("conclusion") or job.get("status") or "unknown"
            name = job.get("name", "?").replace("|", "\\|")
            link = job.get("html_url")
            label = f"[{name}]({link})" if link else name
            out.append(f"| {label} | {ICON.get(result, result)} | {duration(job)} |")
    else:
        # No API data (e.g. token without actions: read): fall back to `needs`.
        out += ["| job | result |", "|---|---|"]
        for name in sorted(needs):
            result = needs[name].get("result", "unknown")
            out.append(f"| {name} | {ICON.get(result, result)} |")

    if superseded:
        out += ["", "Superseded by a newer push (only the newest commit runs them): "
                + ", ".join(f"`{n}`" for n in superseded) + "."]
    if skip_note and any(j.get("result") == "skipped" for j in needs.values()):
        out += ["", skip_note]
    out += ["", f"Each job's own summary (tests, coverage, fuzzing, build size) is further down this page: {run_url}"]
    return "\n".join(out) + "\n", bool(failed)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--jobs", type=Path)
    parser.add_argument("--run-url", default="")
    parser.add_argument("--self", dest="self_name", default="Summary")
    parser.add_argument("--skip-note", default="")
    parser.add_argument("--supersedable", default="", help="comma-separated job ids whose cancellation passes")
    parser.add_argument("--sha", default="")
    parser.add_argument("--ref", default="")
    parser.add_argument("--event", default="")
    parser.add_argument("--server-url", default="https://github.com")
    parser.add_argument("--repo", default="")
    args = parser.parse_args(argv)

    needs = json.loads(os.environ.get("NEEDS_JSON") or "{}")
    jobs: list[dict] = []
    if args.jobs and args.jobs.is_file() and args.jobs.stat().st_size:
        try:
            jobs = json.loads(args.jobs.read_text(encoding="utf-8")).get("jobs", [])
        except ValueError:
            jobs = []
    supersedable = {x.strip() for x in args.supersedable.split(",") if x.strip()}
    markdown, failed = render(needs, jobs, args.run_url, args.self_name, args.skip_note,
                              supersedable, commit_line(args))
    sys.stdout.write(markdown)
    return 1 if failed else 0


if __name__ == "__main__":
    # Step summaries are UTF-8; Windows runners default stdout to cp1252
    # (and `python -I` ignores PYTHONIOENCODING).
    sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main(sys.argv[1:]))
