#!/usr/bin/env python3
"""Render cargo-llvm-cov results as step-summary Markdown and gate on them.

    render_coverage.py --json cov.json --root <windows dir>
        [--floors windows/.config/coverage-floors.toml]
        [--baseline previous/coverage-summary.json] [--baseline-url URL]
        [--write-summary coverage-summary.json] [--max-drop 0.5]
        [--title "Coverage (ubuntu-latest)"] [--commit SHA]

Input is `cargo llvm-cov report --json --summary-only` output. The script
prints Markdown to stdout with:

* total line / region / function coverage, and the delta against the
  baseline (the coverage-summary.json of the latest windows-port run),
* a per-crate table (files under `crates/<name>/` belong to crate `<name>`),
* a per-file table inside a collapsed <details> block.

It writes a small coverage-summary.json (``--write-summary``) that the next
run uses as its baseline, so nobody has to download lcov to see a delta.

Exit status 1 when a gate fails:

* total line coverage dropped more than ``--max-drop`` points vs the baseline,
* total or any crate's line/region coverage is below its floor in the floors
  file.

Standard library only (tomllib needs Python 3.11+).
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath

SCHEMA = 1
METRICS = ("lines", "regions", "functions")


@dataclass
class Tally:
    covered: int = 0
    count: int = 0

    def add(self, covered: int, count: int) -> None:
        self.covered += covered
        self.count += count

    @property
    def percent(self) -> float:
        # Nothing measurable counts as fully covered, like llvm-cov's own report.
        return 100.0 if self.count == 0 else 100.0 * self.covered / self.count

    def to_json(self) -> dict:
        return {"covered": self.covered, "count": self.count, "percent": round(self.percent, 4)}


@dataclass
class Unit:
    """Coverage of one file, one crate, or the total."""

    tallies: dict[str, Tally] = field(default_factory=lambda: {m: Tally() for m in METRICS})

    def add_summary(self, summary: dict) -> None:
        for m in METRICS:
            s = summary.get(m, {})
            self.tallies[m].add(int(s.get("covered", 0)), int(s.get("count", 0)))

    def pct(self, metric: str) -> float:
        return self.tallies[metric].percent

    def to_json(self) -> dict:
        return {m: t.to_json() for m, t in self.tallies.items()}


def relative(filename: str, root: str) -> str:
    """Workspace-relative POSIX path; works for Linux and Windows llvm-cov paths."""
    path = filename.replace("\\", "/")
    base = root.replace("\\", "/").rstrip("/") + "/"
    if path.lower().startswith(base.lower()):
        return path[len(base):]
    return path


def crate_of(rel: str) -> str:
    parts = PurePosixPath(rel).parts
    if len(parts) >= 2 and parts[0] == "crates":
        return parts[1]
    if parts and parts[0] not in ("", "/") and len(parts) > 1:
        return parts[0]
    return "(other)"


def load(cov_path: Path, root: str) -> tuple[Unit, dict[str, Unit], dict[str, Unit]]:
    data = json.loads(cov_path.read_text(encoding="utf-8"))
    entry = data["data"][0]
    total, crates, files = Unit(), {}, {}
    for f in entry.get("files", []):
        rel = relative(f["filename"], root)
        unit = Unit()
        unit.add_summary(f["summary"])
        files[rel] = unit
        crates.setdefault(crate_of(rel), Unit()).add_summary(f["summary"])
        total.add_summary(f["summary"])
    return total, crates, files


def load_floors(path: Path | None) -> dict:
    if path is None or not path.is_file():
        return {"total": {}, "crates": {}}
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    return {"total": data.get("total", {}), "crates": data.get("crates", {})}


def load_baseline(path: Path | None) -> dict | None:
    if path is None or not path.is_file():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (ValueError, OSError):
        return None
    return data if data.get("schema") == SCHEMA else None


def fmt_pct(value: float) -> str:
    return f"{value:.2f}%"


def fmt_delta(now: float, before: float | None) -> str:
    if before is None:
        return "new"
    delta = now - before
    if abs(delta) < 0.005:
        return "±0.00"
    return f"{delta:+.2f}"


def md_cell(text: str) -> str:
    return text.replace("|", "\\|")


@dataclass
class Gate:
    failures: list[str] = field(default_factory=list)

    def floor(self, label: str, metric: str, actual: float, floor) -> str:
        if floor is None:
            return ""
        floor = float(floor)
        if actual + 1e-9 < floor:
            self.failures.append(f"{label} {metric} coverage {actual:.2f}% is below its floor {floor:.2f}%")
            return f" (floor {floor:g}% :x:)"
        return f" (floor {floor:g}%)"


def render(args, total: Unit, crates: dict[str, Unit], files: dict[str, Unit],
           floors: dict, baseline: dict | None) -> tuple[str, Gate]:
    gate = Gate()
    out = [f"### {args.title}", ""]

    if not files:
        out.append("No source files were measured (the workspace has no code or tests yet).")
        return "\n".join(out) + "\n", gate

    base_totals = (baseline or {}).get("totals", {})
    base_crates = (baseline or {}).get("crates", {})

    def base_pct(source: dict, metric: str):
        value = source.get(metric, {}).get("percent")
        return None if value is None else float(value)

    # Totals table.
    out += ["| metric | covered | total | % | Δ vs baseline |", "|---|---:|---:|---:|---:|"]
    for m in METRICS:
        t = total.tallies[m]
        note = gate.floor("total", m, t.percent, floors["total"].get(m)) if m != "functions" else ""
        delta = fmt_delta(t.percent, base_pct(base_totals, m)) if baseline else "n/a"
        out.append(f"| {m} | {t.covered} | {t.count} | {fmt_pct(t.percent)}{note} | {delta} |")

    # Delta gate on total line coverage.
    out.append("")
    if baseline:
        before = base_pct(base_totals, "lines")
        src = args.baseline_label or baseline.get("source") or "previous windows-port run"
        if args.baseline_url:
            src = f"[{src}]({args.baseline_url})"
        if before is not None:
            drop = before - total.pct("lines")
            verdict = "OK"
            if drop > args.max_drop + 1e-9:
                verdict = ":x: FAIL"
                gate.failures.append(
                    f"total line coverage dropped {drop:.2f} points "
                    f"({before:.2f}% -> {total.pct('lines'):.2f}%), more than the allowed {args.max_drop}"
                )
            out.append(
                f"**Delta gate** ({verdict}): lines {fmt_pct(total.pct('lines'))} vs {fmt_pct(before)} "
                f"in {src}; a drop of more than {args.max_drop} points fails."
            )
    else:
        out.append("**Delta gate**: no baseline yet (first run on windows-port, or its artifact expired); skipped.")

    # Per-crate table.
    out += ["", "#### Per crate", "", "| crate | lines | regions | functions | Δ lines |",
            "|---|---:|---:|---:|---:|"]
    for name in sorted(crates):
        c = crates[name]
        crate_floors = floors["crates"].get(name, {})
        cells = []
        for m in METRICS:
            note = gate.floor(f"crate {name}", m, c.pct(m), crate_floors.get(m)) if m != "functions" else ""
            cells.append(f"{fmt_pct(c.pct(m))}{note}")
        delta = fmt_delta(c.pct("lines"), base_pct(base_crates.get(name, {}), "lines")) if baseline else "n/a"
        out.append(f"| `{md_cell(name)}` | " + " | ".join(cells) + f" | {delta} |")
    for name in sorted(set(floors["crates"]) - set(crates)):
        out.append(f"| `{md_cell(name)}` | not measured | | | |")

    # Per-file table, collapsed.
    out += ["", f"<details><summary>Per file ({len(files)} files, lowest line coverage first)</summary>", "",
            "| file | lines | regions | functions |", "|---|---:|---:|---:|"]
    for rel in sorted(files, key=lambda r: (files[r].pct("lines"), r)):
        f = files[rel]
        lt = f.tallies["lines"]
        out.append(
            f"| `{md_cell(rel)}` | {fmt_pct(lt.percent)} ({lt.covered}/{lt.count}) | "
            f"{fmt_pct(f.pct('regions'))} | {fmt_pct(f.pct('functions'))} |"
        )
    out += ["", "</details>"]

    if gate.failures:
        out += ["", "#### Coverage gate failed", ""] + [f"- {msg}" for msg in gate.failures]
    return "\n".join(out) + "\n", gate


def summary_json(total: Unit, crates: dict[str, Unit], commit: str | None, source: str | None) -> dict:
    return {
        "schema": SCHEMA,
        "commit": commit,
        "source": source,
        "totals": total.to_json(),
        "crates": {name: unit.to_json() for name, unit in sorted(crates.items())},
    }


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--json", type=Path, required=True, help="cargo llvm-cov report --json --summary-only")
    parser.add_argument("--root", required=True, help="workspace root the paths are made relative to")
    parser.add_argument("--floors", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--baseline-url", default="")
    parser.add_argument("--baseline-label", default="", help="how the baseline is named in the delta line")
    parser.add_argument("--write-summary", type=Path)
    parser.add_argument("--max-drop", type=float, default=0.5)
    parser.add_argument("--title", default="Coverage")
    parser.add_argument("--commit")
    parser.add_argument("--source", help="label stored in the summary, e.g. 'run 123 (abc1234)'")
    args = parser.parse_args(argv)

    root = str(Path(args.root).resolve()) if Path(args.root).exists() else args.root
    total, crates, files = load(args.json, root)
    markdown, gate = render(args, total, crates, files, load_floors(args.floors), load_baseline(args.baseline))
    sys.stdout.write(markdown)
    if args.write_summary:
        args.write_summary.write_text(
            json.dumps(summary_json(total, crates, args.commit, args.source), indent=2) + "\n",
            encoding="utf-8",
        )
    for msg in gate.failures:
        print(f"::error title=coverage gate::{msg}", file=sys.stderr)
    return 1 if gate.failures else 0


if __name__ == "__main__":
    # Step summaries are UTF-8; Windows runners default stdout to cp1252
    # (and `python -I` ignores PYTHONIOENCODING).
    sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main(sys.argv[1:]))
