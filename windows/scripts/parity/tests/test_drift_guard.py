"""Tests for drift_guard.py. Run: python3 -m unittest discover -s windows/scripts/parity/tests"""

from __future__ import annotations

import io
import os
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
        self.assertIn("Parity issue needed", summary)
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
