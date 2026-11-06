"""Tests for the windows-ci report scripts, run against recorded tool output.

    python3 -m unittest discover -s windows/scripts/ci/tests -v

The fixtures are real output from a two-crate sample workspace (crates
`alpha` and `beta`, paths rewritten to /work/windows):
  junit-fail.xml / junit-pass.xml  cargo-nextest 0.9 JUnit, one failing test
  list.json                        cargo nextest list --message-format json
  cov.json                         cargo llvm-cov report --json --summary-only
  fuzz-crash.log / fuzz-ok.log     cargo fuzz run ... -print_final_stats=1
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPTS = HERE.parent
FIX = HERE / "fixtures"


def run(script: str, *args: str, env: dict | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, "-I", str(SCRIPTS / script), *args],
        capture_output=True, text=True, env={**os.environ, **(env or {})}, check=False,
    )


class RenderJunitTest(unittest.TestCase):
    def test_failing_run_lists_crates_counts_and_failure_output(self):
        r = run("render_junit.py", "--junit", str(FIX / "junit-fail.xml"), "--list", str(FIX / "list.json"),
                "--title", "Tests (ubuntu-latest)")
        self.assertEqual(r.returncode, 0, r.stderr)
        out = r.stdout
        self.assertIn("### Tests (ubuntu-latest)", out)
        self.assertIn("**FAILED**: 3 passed, 1 failed, 1 skipped in 2 crate(s)", out)
        self.assertIn("| `alpha` :x: | 2 | 1 | 1 |", out)
        self.assertIn("| `beta` | 1 | 0 | 0 |", out)
        self.assertIn("- `alpha::test_alpha` **fails_on_demand**", out)
        self.assertIn("assertion `left == right` failed: math broke", out)
        # Output text is fenced, so markup in it is not interpreted.
        self.assertIn("some stdout context <tag> & | pipe", out)

    def test_passing_run(self):
        r = run("render_junit.py", "--junit", str(FIX / "junit-pass.xml"), "--list", str(FIX / "list.json"))
        self.assertIn("**passed**: 4 passed, 0 failed, 1 skipped", r.stdout)
        self.assertNotIn("Failing tests", r.stdout)

    def test_missing_report_is_explained_not_fatal(self):
        r = run("render_junit.py", "--junit", "/nonexistent/junit.xml")
        self.assertEqual(r.returncode, 0)
        self.assertIn("No JUnit report", r.stdout)

    def test_backticks_in_output_get_a_longer_fence(self):
        with tempfile.TemporaryDirectory() as tmp:
            xml = Path(tmp) / "j.xml"
            xml.write_text(
                '<testsuites><testsuite name="c::t"><testcase name="x">'
                '<failure message="m">```\ninside\n```</failure></testcase></testsuite></testsuites>'
            )
            r = run("render_junit.py", "--junit", str(xml))
        self.assertIn("````text\n```\ninside\n```\n````", r.stdout)


class RenderCoverageTest(unittest.TestCase):
    def cov(self, *extra: str, floors: str | None = None, baseline: dict | None = None):
        with tempfile.TemporaryDirectory() as tmp:
            args = ["--json", str(FIX / "cov.json"), "--root", "/work/windows",
                    "--write-summary", str(Path(tmp) / "summary.json"), "--title", "Coverage", *extra]
            if floors is not None:
                (Path(tmp) / "floors.toml").write_text(floors)
                args += ["--floors", str(Path(tmp) / "floors.toml")]
            if baseline is not None:
                (Path(tmp) / "base.json").write_text(json.dumps(baseline))
                args += ["--baseline", str(Path(tmp) / "base.json"), "--baseline-url", "https://x/runs/1"]
            r = run("render_coverage.py", *args)
            summary = json.loads((Path(tmp) / "summary.json").read_text())
        return r, summary

    def test_tables_and_summary_json(self):
        r, summary = self.cov()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("| lines | 11 | 20 | 55.00% | n/a |", r.stdout)
        self.assertIn("| `alpha` | 47.06% | 45.45% | 66.67% | n/a |", r.stdout)
        self.assertIn("| `beta` | 100.00% |", r.stdout)
        self.assertIn("<details><summary>Per file (2 files", r.stdout)
        self.assertIn("| `crates/alpha/src/lib.rs` | 47.06% (8/17) |", r.stdout)
        self.assertIn("no baseline yet", r.stdout)
        self.assertEqual(summary["schema"], 1)
        self.assertEqual(summary["totals"]["lines"], {"covered": 11, "count": 20, "percent": 55.0})
        self.assertEqual(sorted(summary["crates"]), ["alpha", "beta"])

    def test_small_drop_passes_and_shows_delta(self):
        _, base = self.cov()
        base["totals"]["lines"]["percent"] = 55.4
        r, _ = self.cov(baseline=base)
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("| lines | 11 | 20 | 55.00% | -0.40 |", r.stdout)
        self.assertIn("**Delta gate** (OK)", r.stdout)
        self.assertIn("[previous windows-port run](https://x/runs/1)", r.stdout)

    def test_drop_over_half_a_point_fails(self):
        _, base = self.cov()
        base["totals"]["lines"]["percent"] = 60.0
        r, _ = self.cov(baseline=base)
        self.assertEqual(r.returncode, 1)
        self.assertIn(":x: FAIL", r.stdout)
        self.assertIn("dropped 5.00 points", r.stdout)

    def test_floors(self):
        r, _ = self.cov(floors='[total]\nlines = 50\n[crates.alpha]\nlines = 80\n[crates.ghost]\nlines = 10\n')
        self.assertEqual(r.returncode, 1)
        self.assertIn("55.00% (floor 50%)", r.stdout)
        self.assertIn("47.06% (floor 80% :x:)", r.stdout)
        self.assertIn("crate alpha lines coverage 47.06% is below its floor 80.00%", r.stdout)
        self.assertIn("| `ghost` | not measured |", r.stdout)

    def test_committed_floors_file_parses_and_passes(self):
        floors = (SCRIPTS.parent.parent / ".config" / "coverage-floors.toml").read_text()
        r, _ = self.cov(floors=floors)
        self.assertEqual(r.returncode, 0, r.stdout)

    def test_windows_paths_are_made_relative(self):
        data = json.loads((FIX / "cov.json").read_text())
        for f in data["data"][0]["files"]:
            f["filename"] = "D:\\a\\repo\\windows\\" + f["filename"].split("/work/windows/")[1].replace("/", "\\")
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "cov.json"
            p.write_text(json.dumps(data))
            r = run("render_coverage.py", "--json", str(p), "--root", "D:\\a\\repo\\windows")
        self.assertIn("| `crates/alpha/src/lib.rs` |", r.stdout)
        self.assertIn("| `alpha` |", r.stdout)


class FuzzReportTest(unittest.TestCase):
    def row(self, log: str, exit_code: int, crash: bytes | None = None) -> dict:
        with tempfile.TemporaryDirectory() as tmp:
            corpus = Path(tmp) / "corpus"
            corpus.mkdir()
            for i in range(3):
                (corpus / f"in{i}").write_bytes(b"x" * i)
            args = ["row", "--target", "parse_alpha", "--log", str(FIX / log), "--corpus-dir", str(corpus),
                    "--corpus-before", "1", "--seconds", "60", "--exit-code", str(exit_code),
                    "--elapsed", "4", "--out", str(Path(tmp) / "row.json")]
            if crash is not None:
                (Path(tmp) / "crash-abc").write_bytes(crash)
                (Path(tmp) / "minimized-from-abc").write_bytes(crash[:4])
                args += ["--crash", str(Path(tmp) / "minimized-from-abc"),
                         "--original-crash", str(Path(tmp) / "crash-abc")]
            r = run("fuzz_report.py", *args)
            self.assertEqual(r.returncode, 0, r.stderr)
            return json.loads((Path(tmp) / "row.json").read_text())

    def table(self, *rows: dict) -> subprocess.CompletedProcess:
        with tempfile.TemporaryDirectory() as tmp:
            paths = []
            for i, row in enumerate(rows):
                p = Path(tmp) / f"{i}.json"
                p.write_text(json.dumps(row))
                paths.append(str(p))
            return run("fuzz_report.py", "table", "--title", "Fuzz smoke", *paths)

    def test_clean_run(self):
        row = self.row("fuzz-ok.log", 0)
        self.assertEqual(row["status"], "ok")
        self.assertEqual(row["runs"], 10980852)
        self.assertEqual(row["exec_per_sec"], 1830142)
        self.assertEqual((row["cov_start"], row["cov_end"]), (14, 14))
        self.assertEqual((row["corpus_before"], row["corpus_after"]), (1, 3))
        r = self.table(row)
        self.assertEqual(r.returncode, 0)
        self.assertIn("| `parse_alpha` | 10,980,852 | 1,830,142 | 3 (was 1) | +0 edges (14 → 14), +0 inputs | 0 |",
                      r.stdout)

    def test_crash_is_reported_with_hex_and_base64(self):
        row = self.row("fuzz-crash.log", 1, crash=b"FUZ!extra")
        self.assertEqual(row["status"], "crash")
        self.assertEqual(row["crash"]["hex"], "46555a21")
        self.assertEqual(row["crash"]["base64"], "RlVaIQ==")
        self.assertTrue(row["crash"]["minimized"])
        # libFuzzer said 0 exec/s; the wall-clock fallback fills it in.
        self.assertEqual(row["exec_per_sec"], 36527 // 4)
        self.assertEqual((row["cov_start"], row["cov_end"]), (18, 31))
        r = self.table(row)
        self.assertEqual(r.returncode, 1)
        self.assertIn("**1** (crash) :x:", r.stdout)
        self.assertIn("#### Crash in `parse_alpha`", r.stdout)
        self.assertIn("panicked at /work/windows/crates/alpha/src/lib.rs:4:13", r.stdout)
        self.assertIn("echo 'RlVaIQ==' | base64 -d > crash.bin", r.stdout)
        self.assertIn("minimized with `cargo fuzz tmin`", r.stdout)

    def test_crash_found_from_log_when_no_file_given(self):
        row = self.row("fuzz-crash.log", 1)
        # The artifact path in the log does not exist here, so it is an error row.
        self.assertEqual(row["status"], "error")
        self.assertEqual(self.table(row).returncode, 1)

    def test_no_targets(self):
        r = self.table()
        self.assertEqual(r.returncode, 0)
        self.assertIn("No fuzz targets yet", r.stdout)

    def test_build_failure_is_not_reported_as_no_targets(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "build-failed.log"
            log.write_text("error: duplicate key\n  --> crates/x/Cargo.toml:25:1\n")
            r = run("fuzz_report.py", "table", "--build-failed-log", str(log))
        self.assertEqual(r.returncode, 1)
        self.assertIn("did not build", r.stdout)
        self.assertIn("error: duplicate key", r.stdout)
        self.assertNotIn("No fuzz targets yet", r.stdout)

    def test_crash_md_for_issue_body(self):
        row = self.row("fuzz-crash.log", 1, crash=bytes(range(40)))
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "row.json"
            p.write_text(json.dumps(row))
            r = run("fuzz_report.py", "crash-md", str(p), "--run-url", "https://x/runs/9")
        self.assertEqual(r.returncode, 0)
        self.assertIn("00010203", r.stdout)
        self.assertIn("Run: https://x/runs/9", r.stdout)

    def test_long_input_hex_is_capped(self):
        row = self.row("fuzz-crash.log", 1, crash=b"\xff" * 5000)
        self.assertEqual(row["crash"]["size"], 4)  # the minimized file is reported
        with tempfile.TemporaryDirectory() as tmp:
            big = Path(tmp) / "crash-big"
            big.write_bytes(b"\xff" * 5000)
            out = Path(tmp) / "row.json"
            run("fuzz_report.py", "row", "--target", "t", "--log", str(FIX / "fuzz-crash.log"), "--corpus-dir", tmp,
                "--corpus-before", "0", "--seconds", "60", "--exit-code", "1", "--crash", str(big), "--out", str(out))
            crash = json.loads(out.read_text())["crash"]
        self.assertIn("(904 more bytes; see base64)", crash["hex"])
        self.assertEqual(crash["kind"], "crash")


class CiSummaryTest(unittest.TestCase):
    def summary(self, needs: dict, jobs: dict | None = None):
        with tempfile.TemporaryDirectory() as tmp:
            args = ["--run-url", "https://x/runs/1", "--skip-note", "skipped: no workspace yet"]
            if jobs is not None:
                (Path(tmp) / "jobs.json").write_text(json.dumps(jobs))
                args += ["--jobs", str(Path(tmp) / "jobs.json")]
            return run("render_ci_summary.py", *args, env={"NEEDS_JSON": json.dumps(needs)})

    def test_failure_fails_and_is_named(self):
        r = self.summary({"lint": {"result": "success"}, "test": {"result": "failure"}})
        self.assertEqual(r.returncode, 1)
        self.assertIn("**1 job(s) need attention:** `test`", r.stdout)

    def test_skipped_passes_with_note(self):
        r = self.summary({"lint": {"result": "skipped"}, "detect": {"result": "success"}})
        self.assertEqual(r.returncode, 0)
        self.assertIn("skipped: no workspace yet", r.stdout)

    def test_superseded_heavy_jobs_pass_and_commit_is_named(self):
        with tempfile.TemporaryDirectory() as tmp:
            r = run("render_ci_summary.py", "--supersedable", "coverage,windows-build", "--sha", "a" * 40,
                    "--ref", "windows-port", "--event", "push", "--repo", "o/r",
                    env={"NEEDS_JSON": json.dumps({"test": {"result": "success"},
                                                   "coverage": {"result": "cancelled"}}),
                         "COMMIT_SUBJECT": "feat(x): thing\n\nbody"})
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("Tested commit [`aaaaaaa`](https://github.com/o/r/commit/" + "a" * 40 + "): feat(x): thing "
                      "`windows-port` (push)", r.stdout)
        self.assertIn("Superseded by a newer push (only the newest commit runs them): `coverage`.", r.stdout)

    def test_cancelled_lint_still_fails(self):
        r = self.summary({"lint": {"result": "cancelled"}})
        self.assertEqual(r.returncode, 1)

    def test_jobs_api_rows_with_links_and_durations(self):
        jobs = {"jobs": [
            {"name": "Tests (windows-latest)", "conclusion": "success", "html_url": "https://x/j/2",
             "started_at": "2026-11-06T10:00:00Z", "completed_at": "2026-11-06T10:03:05Z"},
            {"name": "Summary", "conclusion": None, "status": "in_progress"},
        ]}
        r = self.summary({"test": {"result": "success"}}, jobs)
        self.assertIn("| [Tests (windows-latest)](https://x/j/2) | :white_check_mark: success | 3m 05s |", r.stdout)
        self.assertNotIn("| Summary", r.stdout)


FAKE_GH = r"""#!/usr/bin/env bash
# Stand-in for `gh api`: serves $FAKE_ARTIFACTS through jq, and a zip per artifact.
set -euo pipefail
[ "$1" = api ] || exit 9
endpoint=$2; shift 2
case "$endpoint" in
  */zip) cat "$FAKE_ZIP" ;;
  *) [ "${1:-}" = --jq ] && jq -r "$2" "$FAKE_ARTIFACTS" ;;
esac
"""


@unittest.skipIf(shutil.which("jq") is None or shutil.which("bash") is None, "needs jq and bash")
class FetchBaselineTest(unittest.TestCase):
    def fetch(self, artifacts: list[dict], branch: str | None = None) -> tuple[dict, Path]:
        tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp)
        (tmp / "bin").mkdir()
        gh = tmp / "bin" / "gh"
        gh.write_text(FAKE_GH)
        gh.chmod(0o755)
        (tmp / "artifacts.json").write_text(json.dumps({"artifacts": artifacts}))
        with zipfile.ZipFile(tmp / "a.zip", "w") as z:
            z.writestr("coverage-summary.json", '{"schema": 1}')
        out = tmp / "out.txt"
        env = {**os.environ, "PATH": f"{tmp / 'bin'}{os.pathsep}{os.environ['PATH']}",
               "FAKE_ARTIFACTS": str(tmp / "artifacts.json"), "FAKE_ZIP": str(tmp / "a.zip"),
               "GITHUB_OUTPUT": str(out), "GITHUB_REPOSITORY": "o/r", "GITHUB_RUN_ID": "50",
               "PYTHON": sys.executable}
        args = ["bash", str(SCRIPTS / "fetch_baseline.sh"), "cov", str(tmp / "dest")] + ([branch] if branch else [])
        subprocess.run(args, check=True, env=env, capture_output=True)
        return dict(line.split("=", 1) for line in out.read_text().splitlines()), tmp / "dest"

    @staticmethod
    def artifact(aid: int, run: int, branch: str, expired: bool = False) -> dict:
        return {"id": aid, "expired": expired,
                "workflow_run": {"id": run, "head_branch": branch, "head_sha": f"{run:040d}"}}

    def test_newest_matching_artifact_is_downloaded(self):
        got, dest = self.fetch([
            self.artifact(9, 50, "windows-port"),        # this run: skipped
            self.artifact(8, 49, "feature"),             # other branch: skipped
            self.artifact(7, 48, "windows-port", True),  # expired: skipped
            self.artifact(6, 47, "windows-port"),
            self.artifact(5, 46, "windows-port"),
        ])
        self.assertEqual(got["found"], "true")
        self.assertEqual(got["run_id"], "47")
        self.assertEqual(got["run_url"], "https://github.com/o/r/actions/runs/47")
        self.assertTrue((dest / "coverage-summary.json").is_file())

    def test_any_branch(self):
        got, _ = self.fetch([self.artifact(8, 49, "master")], branch="*")
        self.assertEqual(got["run_id"], "49")

    def test_nothing_found(self):
        got, _ = self.fetch([self.artifact(8, 49, "feature")])
        self.assertEqual(got, {"found": "false"})


class FuzzTargetsTest(unittest.TestCase):
    def test_bins_are_targets(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = Path(tmp) / "Cargo.toml"
            manifest.write_text('[package]\nname = "f"\n[[bin]]\nname = "parse_b"\npath = "x"\n'
                                '[[bin]]\nname = "parse_a"\npath = "y"\n')
            r = run("fuzz_targets.py", str(manifest))
            missing = run("fuzz_targets.py", str(Path(tmp) / "none.toml"))
        self.assertEqual(r.stdout, 'targets=["parse_a", "parse_b"]\ncount=2\n')
        self.assertEqual(missing.stdout, "targets=[]\ncount=0\n")


class DetectTest(unittest.TestCase):
    def detect(self, files: dict[str, str]) -> dict:
        with tempfile.TemporaryDirectory() as tmp:
            for name, text in files.items():
                p = Path(tmp) / name
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(text)
            out = Path(tmp) / "out.txt"
            subprocess.run(["bash", str(SCRIPTS / "detect.sh"), tmp], check=True,
                           env={**os.environ, "GITHUB_OUTPUT": str(out), "GITHUB_STEP_SUMMARY": os.devnull})
            return dict(line.split("=", 1) for line in out.read_text().splitlines())

    def test_empty(self):
        self.assertEqual(self.detect({}), {"workspace": "false", "locked": "", "deny": "false", "fuzz": "false",
                                           "toolchain": "stable"})

    def test_full(self):
        got = self.detect({"Cargo.toml": "", "Cargo.lock": "", "deny.toml": "", "fuzz/Cargo.toml": "",
                           "rust-toolchain.toml": '[toolchain]\nchannel = "1.91.0"\ncomponents = ["clippy"]\n'})
        self.assertEqual(got, {"workspace": "true", "locked": "--locked", "deny": "true", "fuzz": "true",
                               "toolchain": "1.91.0"})


if __name__ == "__main__":
    unittest.main()
