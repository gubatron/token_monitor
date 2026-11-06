"""Tests for check_parity.py. Run: python3 -m unittest discover -s windows/scripts/parity/tests"""

from __future__ import annotations

import io
import sys
import tempfile
import textwrap
import unittest
from contextlib import redirect_stdout
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import check_parity as cp  # noqa: E402

ID_A = "TokenMonTests/Core/ATests.swift::ATests::testA"
ID_B = "TokenMonTests/Core/ATests.swift::ATests::testB"
HEADER = "| Swift test | Behavior | Workstream | Status | Rust test | Notes |\n|---|---|---|---|---|---|\n"


class CheckParityTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        swift = self.root / "TokenMonTests/Core/ATests.swift"
        swift.parent.mkdir(parents=True)
        swift.write_text("final class ATests: XCTestCase {\n func testA() {}\n func testB() {}\n}\n")
        rust = self.root / "windows/crates/tokenmon-core/tests/a.rs"
        rust.parent.mkdir(parents=True)
        rust.write_text("#[test]\nfn a_works() {}\n#[tokio::test]\nasync fn b_works() {}\n")
        self.ledger = self.root / "windows/docs/PARITY.md"
        self.ledger.parent.mkdir(parents=True)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def run_check(self, body: str) -> cp.Report:
        self.ledger.write_text("# Ledger\n\n" + textwrap.dedent(body))
        return cp.check(self.root, self.ledger)

    def row(self, swift_id: str, status: str = "todo", rust: str = "", notes: str = "", ws: str = "ws-core-net") -> str:
        return f"| `{swift_id}` | does a thing | {ws} | {status} | {rust} | {notes} |\n"

    def test_todo_rows_pass(self) -> None:
        report = self.run_check(HEADER + self.row(ID_A) + self.row(ID_B))
        self.assertTrue(report.ok, report)

    def test_missing_swift_test_fails(self) -> None:
        report = self.run_check(HEADER + self.row(ID_A))
        self.assertFalse(report.ok)
        self.assertEqual(report.missing, [ID_B])

    def test_stale_row_fails(self) -> None:
        gone = "TokenMonTests/Core/ATests.swift::ATests::testGone"
        report = self.run_check(HEADER + self.row(ID_A) + self.row(ID_B) + self.row(gone))
        self.assertFalse(report.ok)
        self.assertEqual([r.id for r in report.stale], [gone])

    def test_duplicate_row_fails(self) -> None:
        report = self.run_check(HEADER + self.row(ID_A) + self.row(ID_B) + self.row(ID_A))
        self.assertEqual(report.duplicates, [ID_A])
        self.assertFalse(report.ok)

    def test_ported_row_needs_an_existing_rust_test(self) -> None:
        ok = self.run_check(
            HEADER
            + self.row(ID_A, "ported", "`crates/tokenmon-core/tests/a.rs::a_works`")
            + self.row(ID_B, "ported", "`crates/tokenmon-core/tests/a.rs::a_works`<br>`crates/tokenmon-core/tests/a.rs::b_works`")
        )
        self.assertTrue(ok.ok, ok.errors)

        no_ref = self.run_check(HEADER + self.row(ID_A, "ported") + self.row(ID_B))
        self.assertIn("names no Rust test", "\n".join(no_ref.errors))

        no_file = self.run_check(HEADER + self.row(ID_A, "ported", "crates/x/tests/nope.rs::a_works") + self.row(ID_B))
        self.assertIn("does not exist", "\n".join(no_file.errors))

        no_fn = self.run_check(HEADER + self.row(ID_A, "ported", "crates/tokenmon-core/tests/a.rs::a_work") + self.row(ID_B))
        self.assertIn("no `fn a_work`", "\n".join(no_fn.errors))

        bad_shape = self.run_check(HEADER + self.row(ID_A, "ported", "a_works") + self.row(ID_B))
        self.assertIn("is not `<path>.rs::<fn>`", "\n".join(bad_shape.errors))

        escape = self.run_check(HEADER + self.row(ID_A, "ported", "../windows/crates/tokenmon-core/tests/a.rs::a_works") + self.row(ID_B))
        self.assertIn("inside windows/", "\n".join(escape.errors))

    def test_na_needs_a_reason(self) -> None:
        report = self.run_check(HEADER + self.row(ID_A, "n/a") + self.row(ID_B, "n/a", notes="macOS-only: Keychain"))
        self.assertEqual(len(report.errors), 1)
        self.assertIn("without a reason", report.errors[0])

    def test_invalid_status_and_workstream(self) -> None:
        report = self.run_check(HEADER + self.row(ID_A, "done") + self.row(ID_B, ws="core"))
        text = "\n".join(report.errors)
        self.assertIn("status `done`", text)
        self.assertIn("workstream `core`", text)

    def test_wrong_cell_count_is_reported(self) -> None:
        report = self.run_check(HEADER + f"| `{ID_A}` | a | b | ws-x | todo | | |\n" + self.row(ID_B))
        self.assertIn("expected 6 cells", "\n".join(report.errors))

    def test_escaped_pipe_stays_in_one_cell(self) -> None:
        body = HEADER + f"| `{ID_A}` | `a \\| b` | ws-core-net | todo | | x \\| y |\n" + self.row(ID_B)
        self.assertTrue(self.run_check(body).ok)

    def test_feature_rows_are_checked_but_not_matched_to_swift(self) -> None:
        features = (
            "| ID | Feature | Spec | Workstream | Status | Rust test | Notes |\n|---|---|---|---|---|---|---|\n"
            "| `FEAT-x` | thing | 01 | ws-shell | ported | `crates/tokenmon-core/tests/a.rs::missing` | |\n"
        )
        report = self.run_check(HEADER + self.row(ID_A) + self.row(ID_B) + "\n" + features)
        self.assertEqual(report.stale, [])
        self.assertIn("FEAT-x", "\n".join(report.errors))

    def test_rows_outside_status_tables_are_ignored(self) -> None:
        other = "| ID | Area |\n|---|---|\n| `D1` | something |\n"
        self.assertTrue(self.run_check(HEADER + self.row(ID_A) + self.row(ID_B) + "\n" + other).ok)

    def test_summary_and_exit_code(self) -> None:
        self.ledger.write_text(HEADER + self.row(ID_A, "ported", "crates/tokenmon-core/tests/a.rs::a_works"))
        summary_file = self.root / "summary.md"
        out = io.StringIO()
        with redirect_stdout(out):
            code = cp.main(["--repo", str(self.root), "--summary", str(summary_file)])
        self.assertEqual(code, 1)
        text = out.getvalue()
        self.assertIn("Result: FAIL", text)
        self.assertIn("Missing from the ledger (1)", text)
        self.assertIn(f"`{ID_B}`", text)
        self.assertIn("| ws-core-net | 1 | 1 | 0 | 0 | 100.0% |", text)
        self.assertIn("Rows to paste", text)
        self.assertEqual(summary_file.read_text(), text)

    def test_print_missing_rows(self) -> None:
        self.ledger.write_text(HEADER + self.row(ID_A))
        out = io.StringIO()
        with redirect_stdout(out):
            code = cp.main(["--repo", str(self.root), "--print-missing-rows"])
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue().strip(), f"| `{ID_B}` | TODO: one-line behavior | ws-parity | todo | | |")

    def test_pass_summary(self) -> None:
        self.ledger.write_text(HEADER + self.row(ID_A) + self.row(ID_B, "n/a", notes="mac"))
        out = io.StringIO()
        with redirect_stdout(out):
            code = cp.main(["--repo", str(self.root)])
        self.assertEqual(code, 0)
        self.assertIn("Result: PASS", out.getvalue())
        self.assertIn("| all | 2 | 0 | 1 | 1 | 0.0% |", out.getvalue())


if __name__ == "__main__":
    unittest.main()
