"""Tests for drift_guard.py. Run: python3 -m unittest discover -s windows/scripts/parity/tests"""

from __future__ import annotations

import io
import os
import subprocess
import sys
import tempfile
import unittest
import urllib.error
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import drift_guard as dg  # noqa: E402

REPO = "gubatron/token_monitor"
SHA = "488be7c4adf545b98cbc3857ffa93ed3bafca1c7"
PARITY_ISSUE = {"number": 42, "title": "Port the Grok fix", "body": "", "html_url": "https://github.com/x/42"}


def fetcher(issues: list[dict]):
    calls: list[str] = []

    def fetch(repo: str) -> list[dict]:
        calls.append(repo)
        return issues

    fetch.calls = calls  # type: ignore[attr-defined]
    return fetch


class ClassifyAndReferenceTests(unittest.TestCase):
    def test_classify(self) -> None:
        mac, win = dg.classify(
            ["TokenMon/App/A.swift", "TokenMonTests/X.swift", "Tests/Manual/CoreTestsMain.swift",
             "windows/docs/PARITY.md", "README.md", "TokenMon.xcodeproj/project.pbxproj", "",
             "TokenMon/Resources/Info.plist", "project.yml", "Makefile", "Scripts/notarize.sh",
             "windows/Makefile", "Docs/x.md", "docs/Makefile", ".github/workflows/ci.yml"]
        )
        self.assertEqual(
            mac,
            ["TokenMon/App/A.swift", "TokenMonTests/X.swift", "Tests/Manual/CoreTestsMain.swift",
             "TokenMon.xcodeproj/project.pbxproj", "TokenMon/Resources/Info.plist", "project.yml", "Makefile",
             "Scripts/notarize.sh"],
        )
        self.assertEqual(win, ["windows/docs/PARITY.md", "windows/Makefile"])

    def test_find_references(self) -> None:
        text = (
            "Refs #12 and #7, gubatron/token_monitor#99, "
            "https://github.com/gubatron/token_monitor/issues/100 but not "
            "faulknerpearce/token_monitor#51, abc#5, &#38; or #12 again"
        )
        self.assertEqual(dg.find_references(text, REPO), [7, 12, 99, 100])

    def test_mentioned_sha_needs_seven_characters(self) -> None:
        self.assertEqual(dg.mentioned_sha({"body": "upstream merge 488be7c"}, [SHA]), SHA)
        self.assertEqual(dg.mentioned_sha({"title": f"port {SHA.upper()}"}, [SHA]), SHA)
        self.assertIsNone(dg.mentioned_sha({"body": "488be7"}, [SHA]))
        self.assertIsNone(dg.mentioned_sha({"body": None, "title": None}, [SHA]))


class EvaluateTests(unittest.TestCase):
    def test_no_macos_change_passes_without_api(self) -> None:
        fetch = fetcher([])
        result = dg.evaluate(["README.md", "windows/x"], "", [], REPO, fetch)
        self.assertTrue(result.ok)
        self.assertEqual(fetch.calls, [])

    def test_macos_with_windows_change_passes_without_api(self) -> None:
        fetch = fetcher([])
        result = dg.evaluate(["TokenMon/A.swift", "windows/crates/a.rs"], "", [], REPO, fetch)
        self.assertTrue(result.ok)
        self.assertEqual(fetch.calls, [])

    def test_macos_only_without_issue_fails(self) -> None:
        result = dg.evaluate(["TokenMon/A.swift"], "fix: thing", [SHA], REPO, fetcher([]))
        self.assertTrue(result.needs_parity)
        self.assertFalse(result.ok)

    def test_reference_to_open_labeled_issue_passes(self) -> None:
        result = dg.evaluate(["TokenMonTests/A.swift"], "Refs #42", [SHA], REPO, fetcher([PARITY_ISSUE]))
        self.assertTrue(result.ok)
        self.assertEqual([i["number"] for i in result.matched_by_reference], [42])

    def test_reference_to_other_issue_fails(self) -> None:
        # #41 is closed, unlabeled, or a pull request: the listing of open windows-parity issues omits it.
        result = dg.evaluate(["Tests/Manual/C.swift"], "Refs #41", [SHA], REPO, fetcher([PARITY_ISSUE]))
        self.assertFalse(result.ok)

    def test_issue_mentioning_the_commit_passes(self) -> None:
        issue = dict(PARITY_ISSUE, body=f"Upstream merge {SHA[:10]} needs porting")
        result = dg.evaluate(["TokenMon/A.swift"], "Merge pull request #49 from upstream", [SHA], REPO, fetcher([issue]))
        self.assertTrue(result.ok)
        self.assertEqual(result.matched_by_sha[0][1], SHA)

    def test_missing_token_or_api_error_fails_closed(self) -> None:
        self.assertFalse(dg.evaluate(["TokenMon/A.swift"], "Refs #42", [], REPO, None).ok)

        def broken(repo: str) -> list[dict]:
            raise urllib.error.URLError("boom")

        result = dg.evaluate(["TokenMon/A.swift"], "Refs #42", [], REPO, broken)
        self.assertFalse(result.ok)
        self.assertIn("boom", result.error)


TEST_FILE = "TokenMonTests/Core/PaceTests.swift"
PACE_ID = f"{TEST_FILE}::PaceTests::testPace"
OTHER_ID = f"{TEST_FILE}::PaceTests::testOther"
RUST = "crates/tokenmon-core/tests/test_pace.rs"
SWIFT_OLD = """import XCTest
final class PaceTests: XCTestCase {
    func testPace() {
        XCTAssertEqual(pace(1), 0.5) // half
    }
    func testOther() { XCTAssertEqual(label(), "a b") }
    private func helper() -> Int { 1 }
}
"""


def ledger(pace_status: str = "ported", pace_notes: str = "", other_status: str = "ported") -> str:
    return (
        "| Swift test | Behavior | Workstream | Status | Rust test | Notes |\n|---|---|---|---|---|---|\n"
        f"| `{PACE_ID}` | pace | ws-core-usage | {pace_status} | `{RUST}::test_pace` | {pace_notes} |\n"
        f"| `{OTHER_ID}` | other | ws-core-usage | {other_status} | `{RUST}::test_other` | |\n"
    )


def reader(base: dict[str, str], head: dict[str, str]):
    def read(rev: str, path: str) -> str | None:
        return {"BASE": base, "HEAD": head}.get(rev, {}).get(path)

    return read


class ParityRulesTests(unittest.TestCase):
    """Rule A (changed ported test bodies) and rule B (other macOS changes)."""

    def run_guard(
        self, new_swift: str | None, changed_extra: list[str] = (), text: str = "", head_ledger: str | None = None,
        issues: list[dict] | None = None, base_ledger: str | None = None,
    ) -> dg.Result:
        base = {TEST_FILE: SWIFT_OLD, dg.LEDGER: base_ledger or ledger()}
        head = {dg.LEDGER: head_ledger or base_ledger or ledger()}
        if new_swift is not None:
            head[TEST_FILE] = new_swift
        changed = [TEST_FILE, *changed_extra]
        if head_ledger is not None:
            changed.append(dg.LEDGER)
        return dg.evaluate(
            changed, text, [SHA], REPO, fetcher(issues or []), base="BASE", head="HEAD", read=reader(base, head)
        )

    def test_changed_expected_value_needs_its_row_reconfirmed(self) -> None:
        # The issue's PR 2: an assertion value changes, plus an unrelated windows/ edit.
        result = self.run_guard(SWIFT_OLD.replace("0.5", "0.6"), ["windows/README.md", "windows/crates/x/src/lib.rs"])
        self.assertEqual([t.id for t in result.unconfirmed_tests], [PACE_ID])
        self.assertEqual(result.other_mac_paths, [])
        self.assertFalse(result.ok)
        summary = dg.render_summary(result, "pull_request", "BASE", "HEAD", REPO)
        self.assertIn("Rule A", summary)
        self.assertIn(f"`{PACE_ID}`: row PARITY.md:3, Rust `{RUST}::test_pace`", summary)
        self.assertNotIn(OTHER_ID, summary)

    def test_changed_string_literal_is_a_change(self) -> None:
        result = self.run_guard(SWIFT_OLD.replace('"a b"', '"a  b!"'))
        self.assertEqual([t.id for t in result.unconfirmed_tests], [OTHER_ID])

    def test_comments_and_formatting_are_not_changes(self) -> None:
        reformatted = SWIFT_OLD.replace("// half", "// exactly a half").replace(
            "XCTAssertEqual(pace(1), 0.5)", "XCTAssertEqual(\n            pace(1),\n            0.5\n        )"
        )
        result = self.run_guard(reformatted)
        self.assertEqual(result.test_changes, [])
        self.assertEqual(result.other_mac_paths, [])
        self.assertTrue(result.ok)

    def test_touching_the_row_reconfirms(self) -> None:
        result = self.run_guard(SWIFT_OLD.replace("0.5", "0.6"), head_ledger=ledger(pace_notes="0.6 since #80"))
        self.assertEqual(len(result.test_changes), 1)
        self.assertIn("PARITY.md:3", result.test_changes[0].confirmed_by)
        self.assertTrue(result.ok)

    def test_touching_the_other_row_does_not(self) -> None:
        head = ledger().replace("| other |", "| other thing |")
        result = self.run_guard(SWIFT_OLD.replace("0.5", "0.6"), head_ledger=head)
        self.assertEqual([t.id for t in result.unconfirmed_tests], [PACE_ID])
        self.assertFalse(result.ok)

    def test_changing_the_named_rust_test_reconfirms(self) -> None:
        result = self.run_guard(SWIFT_OLD.replace("0.5", "0.6"), [f"windows/{RUST}"])
        self.assertIn(RUST, result.test_changes[0].confirmed_by)
        self.assertTrue(result.ok)

    def test_todo_and_na_rows_need_nothing(self) -> None:
        result = self.run_guard(SWIFT_OLD.replace("0.5", "0.6"), base_ledger=ledger(pace_status="todo"))
        self.assertEqual(result.test_changes, [])
        self.assertTrue(result.ok)

    def test_added_test_is_left_to_the_ledger_job(self) -> None:
        added = SWIFT_OLD.replace("    private func", "    func testNew() { XCTAssertTrue(true) }\n    private func")
        result = self.run_guard(added)
        self.assertEqual(result.test_changes, [])
        self.assertTrue(result.ok)

    def test_helper_change_is_rule_b(self) -> None:
        result = self.run_guard(SWIFT_OLD.replace("{ 1 }", "{ 2 }"))
        self.assertEqual(result.test_changes, [])
        self.assertEqual(result.other_mac_paths, [TEST_FILE])
        self.assertFalse(result.ok)

    def test_issue_reference_covers_rule_a(self) -> None:
        result = self.run_guard(SWIFT_OLD.replace("0.5", "0.6"), text="Refs #42", issues=[PARITY_ISSUE])
        self.assertTrue(result.ok)
        self.assertIn("tracked by the matched issue", dg.render_summary(result, "push", "BASE", "HEAD", REPO))

    def test_trailer_does_not_cover_rule_a(self) -> None:
        result = self.run_guard(SWIFT_OLD.replace("0.5", "0.6"), text="Windows-Parity: macOS-only")
        self.assertFalse(result.ok)

    def test_core_check_condition_change(self) -> None:
        core = "Tests/Manual/CoreTestsMain.swift"
        core_id = f"{core}::CoreTestsMain::used_percent"
        old = 'func assertTrue(_ c: Bool, _ m: String) {}\nlet x = 1\ntry assertTrue(x == 64, "used percent")\n'
        base_ledger = (
            "| Swift test | Behavior | Workstream | Status | Rust test | Notes |\n|---|---|---|---|---|---|\n"
            f"| `{core_id}` | 64 | ws-grok | ported | `{RUST}::t` | |\n"
        )
        read = reader({core: old, dg.LEDGER: base_ledger}, {core: old.replace("64", "65"), dg.LEDGER: base_ledger})
        result = dg.evaluate([core], "", [], REPO, fetcher([]), base="BASE", head="HEAD", read=read)
        self.assertEqual([t.id for t in result.unconfirmed_tests], [core_id])
        self.assertEqual(result.other_mac_paths, [])
        # A change outside the checks (the setup) is rule B.
        read = reader({core: old, dg.LEDGER: base_ledger}, {core: old.replace("x = 1", "x = 2"), dg.LEDGER: base_ledger})
        result = dg.evaluate([core], "", [], REPO, fetcher([]), base="BASE", head="HEAD", read=read)
        self.assertEqual((result.test_changes, result.other_mac_paths), ([], [core]))


class OtherMacChangesTests(unittest.TestCase):
    def test_windows_docs_do_not_count(self) -> None:
        # The issue's PR 1: a new pace rule plus a README typo fix.
        result = dg.evaluate(
            ["TokenMon/Core/Usage/DailyBudgetPace.swift", "windows/README.md", "windows/docs/spec/01.md"],
            "", [SHA], REPO, fetcher([]),
        )
        self.assertTrue(result.other_changes_unported)
        self.assertFalse(result.ok)

    def test_windows_code_or_ledger_counts(self) -> None:
        for path in ("windows/crates/tokenmon-core/src/pace.rs", "windows/docs/PARITY.md"):
            result = dg.evaluate(["TokenMon/Core/Usage/DailyBudgetPace.swift", path], "", [SHA], REPO, fetcher([]))
            self.assertTrue(result.ok, path)

    def test_trailer_counts(self) -> None:
        text = "fix(menubar): spacing\n\nWindows-Parity: macOS-only NSStatusItem padding\n"
        result = dg.evaluate(["TokenMon/UI/MenuBar/X.swift"], text, [SHA], REPO, fetcher([]))
        self.assertEqual(result.trailer, "macOS-only NSStatusItem padding")
        self.assertTrue(result.ok)
        self.assertEqual(dg.find_trailer("Windows-Parity:   \n"), "")


class GitIntegrationTests(unittest.TestCase):
    """The guard reads both revisions with `git show`, as in CI."""

    def git(self, *args: str) -> str:
        env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t", GIT_COMMITTER_NAME="t",
                   GIT_COMMITTER_EMAIL="t@t", GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
        proc = subprocess.run(["git", "-C", str(self.root), *args], capture_output=True, text=True, env=env, check=True)
        return proc.stdout.strip()

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.git("init", "-q")
        (self.root / "TokenMonTests/Core").mkdir(parents=True)
        (self.root / "windows/docs").mkdir(parents=True)
        (self.root / TEST_FILE).write_text(SWIFT_OLD)
        (self.root / dg.LEDGER).write_text(ledger())
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "base")
        self.base = self.git("rev-parse", "HEAD")

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def test_reads_revisions_from_git(self) -> None:
        (self.root / TEST_FILE).write_text(SWIFT_OLD.replace("0.5", "0.6"))
        self.git("commit", "-q", "-am", "change")
        head = self.git("rev-parse", "HEAD")
        changed = self.git("diff", "--name-only", self.base, head).splitlines()
        read = dg.git_reader(self.root)
        self.assertIsNone(read(self.base, "nope.swift"))
        self.assertIsNone(read("", TEST_FILE))
        result = dg.evaluate(changed, "", [head], REPO, fetcher([]), base=self.base, head=head, read=read)
        self.assertEqual([t.id for t in result.unconfirmed_tests], [PACE_ID])

        (self.root / dg.LEDGER).write_text(ledger(pace_notes="now 0.6"))
        self.git("commit", "-q", "-am", "confirm")
        head = self.git("rev-parse", "HEAD")
        changed = self.git("diff", "--name-only", self.base, head).splitlines()
        result = dg.evaluate(changed, "", [head], REPO, fetcher([]), base=self.base, head=head, read=read)
        self.assertTrue(result.ok)


class MainTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def run_main(self, changed: str, text: str, issues: list[dict] | None) -> tuple[int, str, str]:
        (self.dir / "changed.txt").write_text(changed)
        (self.dir / "text.txt").write_text(text)
        (self.dir / "shas.txt").write_text(SHA + "\n")
        summary = self.dir / "summary.md"
        args = [
            "--changed-files", str(self.dir / "changed.txt"),
            "--text-file", str(self.dir / "text.txt"),
            "--shas-file", str(self.dir / "shas.txt"),
            "--repo", REPO, "--event", "push", "--base", "a" * 40, "--head", SHA,
            "--summary", str(summary),
        ]
        env = {"GITHUB_TOKEN": "t"} if issues is not None else {}
        out = io.StringIO()
        with mock.patch.dict(os.environ, env, clear=False), \
                mock.patch.object(dg, "github_fetcher", return_value=fetcher(issues or [])), \
                redirect_stdout(out):
            if issues is None:
                os.environ.pop("GITHUB_TOKEN", None)
            code = dg.main(args)
        return code, out.getvalue(), summary.read_text()

    def test_failure_summary_explains_the_fix(self) -> None:
        code, out, summary = self.run_main("TokenMon/App/A.swift\n", "fix: x", [])
        self.assertEqual(code, 1)
        self.assertIn("Result: FAIL", summary)
        self.assertIn("Windows parity needed", summary)
        self.assertIn("Rule B", summary)
        self.assertIn("How to fix", summary)
        self.assertIn(SHA[:12], summary)
        self.assertIn("`TokenMon/App/A.swift`", summary)
        self.assertIn("::error title=Windows parity needed::", out)

    def test_pass_summary(self) -> None:
        code, _, summary = self.run_main("TokenMon/App/A.swift\n", "Refs #42", [PARITY_ISSUE])
        self.assertEqual(code, 0)
        self.assertIn("Result: PASS", summary)
        self.assertIn("[#42]", summary)

    def test_no_token_fails_when_api_is_needed(self) -> None:
        code, _, summary = self.run_main("TokenMon/App/A.swift\n", "Refs #42", None)
        self.assertEqual(code, 1)
        self.assertIn("no GitHub token", summary)

    def test_no_token_is_fine_when_not_needed(self) -> None:
        code, _, summary = self.run_main("README.md\n", "", None)
        self.assertEqual(code, 0)
        self.assertIn("No macOS app files", summary)


if __name__ == "__main__":
    unittest.main()
