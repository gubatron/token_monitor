#!/usr/bin/env python3
"""Turn cargo-fuzz (libFuzzer) runs into step-summary Markdown.

Subcommands:

    fuzz_report.py row --target T --log run.log --corpus-dir DIR
        --corpus-before N --seconds S --exit-code C [--crash FILE] --out row.json
        Parse one target's libFuzzer log (run with -print_final_stats=1) into a
        JSON row: runs, exec/s, corpus size, coverage gained, crash input.

    fuzz_report.py table --title TITLE [--note TEXT] row.json...
        Print the Markdown summary for all rows: one table
        (target | runs | exec/s | corpus | new coverage | crashes) and, for each
        crash, the input as hex and base64 plus the panic excerpt and the
        command that reproduces it. Exit 1 if any target crashed or errored.

    fuzz_report.py crash-md row.json [--run-url URL]
        Print one crash as Markdown (the body of the nightly crash issue).

The crash input is printed in full (up to 4 KiB as hex, 64 KiB as base64)
so a developer can reproduce it without downloading any artifact.
Standard library only.
"""

from __future__ import annotations

import argparse
import base64
import json
import re
import sys
from pathlib import Path

HEX_LIMIT = 4096
B64_LIMIT = 64 * 1024
EXCERPT_LINES = 30

STAT_RE = re.compile(r"^stat::(\w+):\s+(\d+)", re.M)
# "#2\tINITED cov: 18 ft: 18 corp: 1/1b exec/s: 0 rss: 36Mb"
PROGRESS_RE = re.compile(r"^#(\d+)\s+(\w+)\s+cov: (\d+) ft: (\d+)(?: corp: (\d+)/)?(?:.*?exec/s: (\d+))?", re.M)
ARTIFACT_RE = re.compile(r"Test unit written to (\S+)")
KINDS = ("crash", "leak", "timeout", "oom", "slow-unit")


def count_files(directory: Path) -> int:
    return sum(1 for p in directory.iterdir() if p.is_file()) if directory.is_dir() else 0


def parse_log(text: str) -> dict:
    stats = {name: int(value) for name, value in STAT_RE.findall(text)}
    progress = PROGRESS_RE.findall(text)
    inited = next((p for p in progress if p[1] == "INITED"), None)
    last = progress[-1] if progress else None
    runs = stats.get("number_of_executed_units", int(last[0]) if last else 0)
    exec_s = stats.get("average_exec_per_sec", 0)
    if not exec_s and last and last[5]:
        exec_s = int(last[5])
    return {
        "runs": runs,
        "exec_per_sec": exec_s,
        "new_units": stats.get("new_units_added", 0),
        "peak_rss_mb": stats.get("peak_rss_mb", 0),
        "cov_start": int(inited[2]) if inited else None,
        "cov_end": int(last[2]) if last else None,
        "ft_start": int(inited[3]) if inited else None,
        "ft_end": int(last[3]) if last else None,
    }


def crash_excerpt(text: str) -> str:
    """The panic message (or sanitizer report) that explains the crash."""
    lines = text.splitlines()
    start = next(
        (i for i, line in enumerate(lines)
         if "panicked at" in line or "ERROR: libFuzzer" in line or "ERROR: AddressSanitizer" in line),
        None,
    )
    if start is None:
        return ""
    picked = []
    for line in lines[start:]:
        # Raw stack frames are noise next to the panic message; keep a few.
        if re.match(r"^\s+#\d+ 0x", line) and sum(1 for p in picked if re.match(r"^\s+#\d+ 0x", p)) >= 8:
            continue
        picked.append(line)
        if line.startswith("SUMMARY:") or len(picked) >= EXCERPT_LINES:
            break
    return "\n".join(picked)


def crash_kind(path: Path) -> str:
    """libFuzzer names artifacts `<kind>-<sha1>`; tmin output is `minimized-from-<sha1>`."""
    return next((kind for kind in KINDS if path.name.startswith(kind + "-")), "crash")


def hex_lines(data: bytes) -> str:
    shown = data[:HEX_LIMIT]
    rows = [shown[i:i + 32].hex() for i in range(0, len(shown), 32)]
    if len(data) > HEX_LIMIT:
        rows.append(f"... ({len(data) - HEX_LIMIT} more bytes; see base64)")
    return "\n".join(rows) if rows else "(empty input)"


def cmd_row(args) -> int:
    text = Path(args.log).read_text(encoding="utf-8", errors="replace") if Path(args.log).is_file() else ""
    row = {
        "target": args.target,
        "seconds": args.seconds,
        "exit_code": args.exit_code,
        "corpus_before": args.corpus_before,
        "corpus_after": count_files(Path(args.corpus_dir)),
        **parse_log(text),
        "crash": None,
    }
    # libFuzzer reports 0 exec/s when it stops on an early crash; use wall time.
    if not row["exec_per_sec"] and row["runs"] and args.elapsed > 0:
        row["exec_per_sec"] = int(row["runs"] / args.elapsed)
    crash_path = Path(args.crash) if args.crash else None
    if crash_path is None:
        match = ARTIFACT_RE.findall(text)
        if match and Path(match[-1]).is_file():
            crash_path = Path(match[-1])
    if crash_path is not None and crash_path.is_file():
        data = crash_path.read_bytes()
        row["crash"] = {
            "kind": crash_kind(Path(args.original_crash) if args.original_crash else crash_path),
            "file": crash_path.name,
            "size": len(data),
            "minimized": bool(args.original_crash) and Path(args.original_crash) != crash_path,
            "hex": hex_lines(data),
            "base64": base64.b64encode(data[:B64_LIMIT]).decode("ascii"),
            "truncated": len(data) > B64_LIMIT,
            "excerpt": crash_excerpt(text),
        }
        row["status"] = "crash"
    elif args.exit_code != 0:
        row["status"] = "error"
        row["error_excerpt"] = "\n".join(text.splitlines()[-25:])
    else:
        row["status"] = "ok"
    Path(args.out).write_text(json.dumps(row, indent=2) + "\n", encoding="utf-8")
    return 0


def fence(text: str, lang: str = "text") -> str:
    longest = max((len(m) for m in re.findall(r"`+", text)), default=0)
    ticks = "`" * max(3, longest + 1)
    return f"{ticks}{lang}\n{text}\n{ticks}"


def new_coverage(row: dict) -> str:
    if row.get("cov_start") is None or row.get("cov_end") is None:
        return "n/a"
    gained = row["cov_end"] - row["cov_start"]
    return f"+{gained} edges ({row['cov_start']} → {row['cov_end']}), +{row.get('new_units', 0)} inputs"


def crash_markdown(row: dict, run_url: str = "") -> str:
    crash = row["crash"]
    target = row["target"]
    lines = [
        f"**`{target}`**: {crash['kind']} on a {crash['size']}-byte input"
        + (" (minimized with `cargo fuzz tmin`)" if crash.get("minimized") else ""),
        "",
    ]
    if crash.get("excerpt"):
        lines += ["Panic / sanitizer report:", "", fence(crash["excerpt"]), ""]
    lines += ["Input as hex (32 bytes per line):", "", fence(crash["hex"]), ""]
    lines += ["Input as base64" + (" (first 64 KiB)" if crash.get("truncated") else "") + ":", "",
              fence(crash["base64"]), ""]
    lines += [
        "Reproduce from `windows/`:",
        "",
        fence(
            f"echo '{crash['base64'] if len(crash['base64']) <= 400 else '<base64 above>'}' "
            f"| base64 -d > crash.bin\ncargo +nightly fuzz run {target} crash.bin",
            "sh",
        ),
        "",
        "Then turn it into a regression test (skills/rust-quality-pipeline §4: "
        "minimized input under `tests/fixtures/fuzz/<target>/` plus a stable-toolchain test).",
    ]
    if run_url:
        lines += ["", f"Run: {run_url}"]
    return "\n".join(lines) + "\n"


def cmd_table(args) -> int:
    rows = [json.loads(Path(p).read_text(encoding="utf-8")) for p in args.rows]
    rows.sort(key=lambda r: r["target"])
    out = [f"### {args.title}", ""]
    if args.note:
        out += [args.note, ""]
    if not rows:
        out.append("No fuzz targets yet: `cargo fuzz list` returned nothing, so there was nothing to run.")
        sys.stdout.write("\n".join(out) + "\n")
        return 0
    out += ["| target | runs | exec/s | corpus size | new coverage | crashes |", "|---|---:|---:|---:|---|---:|"]
    bad = 0
    for r in rows:
        crashes = 1 if r["status"] == "crash" else 0
        if r["status"] == "crash":
            mark = f"**1** ({r['crash']['kind']}) :x:"
        elif r["status"] == "error":
            mark = f"run failed (exit {r['exit_code']}) :x:"
        else:
            mark = "0"
        bad += crashes or r["status"] == "error"
        corpus = f"{r['corpus_after']} (was {r['corpus_before']})"
        out.append(
            f"| `{r['target']}` | {r['runs']:,} | {r['exec_per_sec']:,} | {corpus} | {new_coverage(r)} | {mark} |"
        )
    seconds = {r["seconds"] for r in rows}
    out += ["", f"Each target ran for {', '.join(str(s) for s in sorted(seconds))} s starting from its committed corpus."]
    for r in rows:
        if r["status"] == "crash":
            out += ["", f"#### Crash in `{r['target']}`", "", crash_markdown(r, args.run_url).rstrip()]
        elif r["status"] == "error":
            out += ["", f"#### `{r['target']}` did not run", "", fence(r.get("error_excerpt", ""))]
    sys.stdout.write("\n".join(out) + "\n")
    return 1 if bad else 0


def cmd_crash_md(args) -> int:
    row = json.loads(Path(args.row).read_text(encoding="utf-8"))
    if not row.get("crash"):
        return 1
    sys.stdout.write(crash_markdown(row, args.run_url))
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="cmd", required=True)

    row = sub.add_parser("row")
    row.add_argument("--target", required=True)
    row.add_argument("--log", required=True)
    row.add_argument("--corpus-dir", required=True)
    row.add_argument("--corpus-before", type=int, required=True)
    row.add_argument("--seconds", type=int, required=True)
    row.add_argument("--exit-code", type=int, required=True)
    row.add_argument("--elapsed", type=float, default=0.0, help="wall-clock seconds the run took")
    row.add_argument("--crash", help="crash input to report (the minimized one when tmin succeeded)")
    row.add_argument("--original-crash", help="the artifact libFuzzer wrote, when --crash is its minimized form")
    row.add_argument("--out", required=True)
    row.set_defaults(func=cmd_row)

    table = sub.add_parser("table")
    table.add_argument("--title", default="Fuzz")
    table.add_argument("--note", default="")
    table.add_argument("--run-url", default="")
    table.add_argument("rows", nargs="*")
    table.set_defaults(func=cmd_table)

    crash = sub.add_parser("crash-md")
    crash.add_argument("row")
    crash.add_argument("--run-url", default="")
    crash.set_defaults(func=cmd_crash_md)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    # Step summaries are UTF-8; Windows runners default stdout to cp1252
    # (and `python -I` ignores PYTHONIOENCODING).
    sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main(sys.argv[1:]))
