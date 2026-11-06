#!/usr/bin/env python3
"""Aggregate the jobs of a workflow run into one step-summary table.

    NEEDS_JSON='${{ toJSON(needs) }}' render_ci_summary.py \
        --jobs jobs.json --run-url URL [--self "Summary"] [--skip-note TEXT]

``--jobs`` is the output of `gh api repos/<repo>/actions/runs/<id>/jobs`
(``{"jobs": [...]}``); it gives one row per job and matrix leg, with its
duration and a link to it. ``NEEDS_JSON`` is the `needs` context; it decides
the exit status, so this job can be the single required check:

* exit 1 when any needed job failed or was cancelled,
* skipped jobs pass (they are skipped when there is nothing to check yet).

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


def render(needs: dict, jobs: list[dict], run_url: str, self_name: str, skip_note: str) -> tuple[str, bool]:
    failed = sorted(name for name, job in needs.items() if job.get("result") in ("failure", "cancelled"))
    out = ["## Windows CI summary", ""]
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
    args = parser.parse_args(argv)

    needs = json.loads(os.environ.get("NEEDS_JSON") or "{}")
    jobs: list[dict] = []
    if args.jobs and args.jobs.is_file() and args.jobs.stat().st_size:
        try:
            jobs = json.loads(args.jobs.read_text(encoding="utf-8")).get("jobs", [])
        except ValueError:
            jobs = []
    markdown, failed = render(needs, jobs, args.run_url, args.self_name, args.skip_note)
    sys.stdout.write(markdown)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
