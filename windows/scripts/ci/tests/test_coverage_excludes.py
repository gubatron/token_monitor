"""Tests for coverage_excludes.py (the coverage exclusion guard) and for how
render_coverage.py applies the exclusions to the current run and the baseline.

    python3 -m unittest discover -s windows/scripts/ci/tests -v
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path, PurePosixPath

HERE = Path(__file__).resolve().parent
SCRIPTS = HERE.parent
WINDOWS = SCRIPTS.parent.parent
FIX = HERE / "fixtures"
sys.path.insert(0, str(SCRIPTS))

import coverage_excludes as ce  # noqa: E402


def run(script: str, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, "-I", str(SCRIPTS / script), *args],
                          capture_output=True, text=True, env=dict(os.environ), check=False)


def tree(root: Path, files: dict[str, str]) -> None:
    for rel, text in files.items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)


class ParseTest(unittest.TestCase):
    def test_entries_reasons_and_budgets(self):
        entries = ce.parse("# comment\n\ncrates/a/src/win\\.rs  # Win32 calls\n"
                           "crates/a/src/big\\.rs  max-lines=900  # frozen\n")
        self.assertEqual([(e.pattern, e.reason, e.max_lines, e.line_no) for e in entries],
                         [("crates/a/src/win\\.rs", "Win32 calls", 400, 3),
                          ("crates/a/src/big\\.rs", "frozen", 900, 4)])

    def test_missing_reason_is_an_error(self):
        for bad in ("crates/a/src/win\\.rs\n", "crates/a/src/win\\.rs  #   \n"):
            with self.assertRaisesRegex(ce.ExcludesError, "line 1"):
                ce.parse(bad)

    def test_non_ere_syntax_is_rejected(self):
        for bad in ("crates/(?:a|b)/x\\.rs", "crates/\\w+/x\\.rs", "crates/a[/]x\\.rs", "crates/(/x"):
            with self.assertRaises(ce.ExcludesError, msg=bad):
                ce.parse(f"{bad}  # reason\n")

    def test_regex_is_anchored_and_matches_both_separators(self):
        [e] = ce.parse("crates/a/src/win\\.rs  # r\n")
        self.assertTrue(e.matches("crates/a/src/win.rs"))
        self.assertTrue(e.matches("/home/runner/work/r/r/windows/crates/a/src/win.rs"))
        self.assertTrue(e.matches("D:\\a\\r\\windows\\crates\\a\\src\\win.rs"))
        self.assertFalse(e.matches("crates/a/src/win.rs.bak"))
        self.assertFalse(e.matches("crates/a/src/xwin.rs"))
        self.assertFalse(e.matches("crates/a/src/win/mod.rs"))

    def test_ignore_regex_combines_non_product_code_and_entries(self):
        entries = ce.parse("crates/a/src/win\\.rs  # r\n")
        rx = ce.ignore_regex(entries)
        for path in ("/w/crates/a/tests/t.rs", "D:\\w\\crates\\a\\benches\\b.rs", "/w/fuzz/src/bin/x.rs",
                     "/w/crates/a/src/win.rs"):
            self.assertRegex(path, rx)
        self.assertNotRegex("/w/crates/a/src/lib.rs", rx)
        self.assertNotIn("tests", ce.ignore_regex(entries, non_product=False))

    def test_committed_file_parses_and_passes_the_guard(self):
        entries = ce.load(WINDOWS / ".config" / "coverage-excludes.txt")
        self.assertTrue(entries)
        self.assertEqual(ce.check(WINDOWS, entries), [])


class WindowsOnlyTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        tree(self.root, {
            "crates/a/src/lib.rs": "pub mod portable;\n#[cfg(windows)]\npub mod shell;\n"
                                   "/// Docs.\n#[cfg(windows)]\n#[allow(unsafe_code)]\nmod attrs;\n"
                                   "#[cfg(target_os = \"windows\")] mod inline;\npub mod mixed;\n",
            "crates/a/src/portable.rs": "pub fn f() {}\n",
            "crates/a/src/shell.rs": "mod child;\n",
            "crates/a/src/shell/child.rs": "pub fn g() {}\n",
            "crates/a/src/attrs.rs": "",
            "crates/a/src/inline.rs": "",
            "crates/a/src/mixed.rs": "#[cfg(windows)]\nfn win() {}\nfn portable() {}\n",
            "crates/a/src/inner.rs": "//! Docs.\n#![cfg(windows)]\n",
            "crates/b/src/power/mod.rs": "#[cfg(windows)]\npub mod windows;\npub mod event;\n",
            "crates/b/src/power/windows.rs": "",
            "crates/b/src/power/event.rs": "",
            "crates/b/src/lib.rs": "pub mod power;\n",
            "crates/b/src/orphan.rs": "",
        })

    def tearDown(self):
        self.tmp.cleanup()

    def verdict(self, rel: str) -> bool:
        return ce.windows_only(self.root, PurePosixPath(rel))[0]

    def test_windows_only_modules(self):
        for rel in ("crates/a/src/shell.rs", "crates/a/src/shell/child.rs", "crates/a/src/attrs.rs",
                    "crates/a/src/inline.rs", "crates/a/src/inner.rs", "crates/b/src/power/windows.rs"):
            self.assertTrue(self.verdict(rel), rel)

    def test_portable_and_mixed_modules(self):
        for rel in ("crates/a/src/portable.rs", "crates/a/src/mixed.rs", "crates/a/src/lib.rs",
                    "crates/b/src/power/event.rs", "crates/b/src/power/mod.rs", "crates/b/src/orphan.rs"):
            self.assertFalse(self.verdict(rel), rel)

    def check(self, text: str) -> list[str]:
        return ce.check(self.root, ce.parse(text))

    def test_guard_passes_thin_windows_only_files(self):
        self.assertEqual(self.check("crates/a/src/shell\\.rs  # r\ncrates/a/src/shell/.*\\.rs  # r\n"
                                    "crates/b/src/power/windows\\.rs  # r\n"), [])

    def test_guard_rejects_portable_or_mixed_files(self):
        problems = self.check("crates/a/src/mixed\\.rs  # r\ncrates/b/src/power/.*\\.rs  # r\n")
        self.assertEqual(len(problems), 3, problems)
        self.assertTrue(all("not entirely cfg(windows)" in p for p in problems))
        self.assertIn("crates/a/src/mixed.rs", problems[0])

    def test_guard_rejects_stale_entries_and_test_code(self):
        tree(self.root, {"crates/a/tests/t.rs": ""})
        problems = self.check("crates/a/src/gone\\.rs  # r\ncrates/a/tests/t\\.rs  # r\n")
        self.assertIn("matches no file", problems[0])
        self.assertIn("test/bench/fuzz code", problems[1])

    def test_guard_enforces_the_line_budget(self):
        tree(self.root, {"crates/a/src/shell.rs": "mod child;\n" + "// x\n" * 400})
        [problem] = self.check("crates/a/src/shell\\.rs  # r\n")
        self.assertIn("401 lines, over its budget of 400", problem)
        self.assertEqual(self.check("crates/a/src/shell\\.rs  max-lines=401  # r\n"), [])

    def test_cli_check_exit_status(self):
        cfg = self.root / ".config"
        cfg.mkdir()
        (cfg / "coverage-excludes.txt").write_text("crates/a/src/shell\\.rs  # r\n")
        r = run("coverage_excludes.py", "check", "--root", str(self.root))
        self.assertEqual(r.returncode, 0, r.stderr)
        (cfg / "coverage-excludes.txt").write_text("crates/a/src/mixed\\.rs  # r\n")
        r = run("coverage_excludes.py", "check", "--root", str(self.root))
        self.assertEqual(r.returncode, 1)
        self.assertIn("::error", r.stderr)
        r = run("coverage_excludes.py", "regex", "--root", str(self.root))
        self.assertEqual(r.returncode, 0)
        self.assertIn("mixed", r.stdout)


class RenderWithExcludesTest(unittest.TestCase):
    """render_coverage.py with --excludes: the fixture's beta crate is 3/3
    lines covered and alpha 8/17, so excluding beta lowers the total."""

    def render(self, excludes: str, baseline: dict | None = None, baseline_cov: bool = False):
        with tempfile.TemporaryDirectory() as tmp:
            t = Path(tmp)
            (t / "ex.txt").write_text(excludes)
            args = ["--json", str(FIX / "cov.json"), "--root", "/work/windows", "--excludes", str(t / "ex.txt"),
                    "--write-summary", str(t / "summary.json")]
            if baseline is not None:
                (t / "base.json").write_text(json.dumps(baseline))
                args += ["--baseline", str(t / "base.json")]
            if baseline_cov:
                args += ["--baseline-cov", str(FIX / "cov.json")]
            r = run("render_coverage.py", *args)
            summary = json.loads((t / "summary.json").read_text()) if (t / "summary.json").exists() else None
        return r, summary

    def baseline_without_excludes(self) -> dict:
        r, summary = self.render("")
        self.assertEqual(r.returncode, 0, r.stderr)
        return summary

    def test_excluded_files_leave_every_number(self):
        r, summary = self.render("crates/beta/src/lib\\.rs  # glue\n")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotIn("`beta`", r.stdout.split("<details>")[0])
        self.assertEqual(summary["totals"]["lines"]["covered"], 8)
        self.assertEqual(summary["totals"]["lines"]["count"], 17)
        self.assertEqual(summary["excludes"], ["crates/beta/src/lib\\.rs"])
        self.assertIn("Excluded Windows-only glue (1 entries", r.stdout)
        self.assertIn("| `crates/beta/src/lib\\.rs` | 400 | glue |", r.stdout)

    def test_baseline_is_recomputed_like_for_like(self):
        # Excluding a fully covered crate drops the total from 55% to 47%;
        # compared with the old baseline that would fail the gate, but the
        # baseline is recomputed with the same exclusions, so the delta is 0.
        base = self.baseline_without_excludes()
        r, _ = self.render("crates/beta/src/lib\\.rs  # glue\n", baseline=base, baseline_cov=True)
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("**Delta gate** (OK): lines 47.06% vs 47.06%", r.stdout)
        self.assertIn("recomputed from its per-file data with the current 1 exclusion(s)", r.stdout)

    def test_changed_excludes_without_per_file_data_skip_the_gate_once(self):
        base = self.baseline_without_excludes()
        r, _ = self.render("crates/beta/src/lib\\.rs  # glue\n", baseline=base)
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("**Delta gate**: skipped. The exclusion list changed", r.stdout)

    def test_same_excludes_compare_normally(self):
        _, base = self.render("crates/beta/src/lib\\.rs  # glue\n")
        base["totals"]["lines"]["percent"] = 60.0
        r, _ = self.render("crates/beta/src/lib\\.rs  # glue\n", baseline=base)
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("dropped 12.94 points", r.stdout)

    def test_malformed_excludes_fail(self):
        r, _ = self.render("crates/beta/src/lib\\.rs\n")
        self.assertEqual(r.returncode, 1)
        self.assertIn("coverage excludes", r.stderr)


class WorkflowUsesTheListTest(unittest.TestCase):
    """The workflow and the Makefile build the ignore regex from the list."""

    def test_both_coverage_legs_and_make_use_the_script(self):
        wf = (WINDOWS.parent / ".github" / "workflows" / "windows-ci.yml").read_text()
        self.assertGreaterEqual(len(re.findall(r"coverage_excludes\.py regex", wf)), 1)
        self.assertIn("coverage_excludes.py check", wf)
        self.assertIn("--excludes .config/coverage-excludes.txt", wf)
        self.assertNotIn("COVERAGE_IGNORE:", wf)
        mk = (WINDOWS / "Makefile").read_text()
        self.assertTrue("coverage_excludes.py regex" in mk, "make coverage must use the list")


if __name__ == "__main__":
    unittest.main()
