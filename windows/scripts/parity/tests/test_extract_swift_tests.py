"""Tests for extract_swift_tests.py. Run: python3 -m unittest discover -s windows/scripts/parity/tests"""

from __future__ import annotations

import io
import sys
import tempfile
import textwrap
import unittest
from contextlib import redirect_stdout
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import extract_swift_tests as ex  # noqa: E402


def write(root: Path, rel: str, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(text), encoding="utf-8")


class BlankSwiftTests(unittest.TestCase):
    def test_keeps_length_and_newlines(self) -> None:
        src = 'let a = "x{y}"\n// func testNo() {\n/* { */ let b = 1\n'
        out = ex.blank_swift(src)
        self.assertEqual(len(out), len(src))
        self.assertEqual(out.count("\n"), src.count("\n"))
        self.assertNotIn("{", out)
        self.assertNotIn("testNo", out)

    def test_nested_block_comments(self) -> None:
        out = ex.blank_swift("/* a /* { */ } */ let x = 1")
        self.assertNotIn("{", out)
        self.assertNotIn("}", out)
        self.assertIn("let x = 1", out)

    def test_multiline_and_raw_strings(self) -> None:
        src = 'let a = """\n  { "q" }\n  """\nlet b = #"say "{" "#\nlet c = 2'
        out = ex.blank_swift(src)
        self.assertNotIn("{", out)
        self.assertIn("let c = 2", out)

    def test_interpolation_code_is_kept_and_inner_strings_blanked(self) -> None:
        out = ex.blank_swift('let s = "a\\(dict["{"] ?? f(1))b"; let t = 1')
        self.assertIn("dict[", out)
        self.assertIn("f(1)", out)
        self.assertNotIn("{", out)
        self.assertIn("let t = 1", out)

    def test_escaped_quote_does_not_end_string(self) -> None:
        out = ex.blank_swift('let s = "a\\"{"; let t = 1')
        self.assertNotIn("{", out)
        self.assertIn("let t = 1", out)


class ExtractXCTestsTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def names(self) -> list[str]:
        return [t.id for t in ex.extract_all(self.root)]

    def test_finds_test_methods_with_stable_ids(self) -> None:
        write(
            self.root,
            "TokenMonTests/A/FooTests.swift",
            """
            import XCTest
            @MainActor
            final class FooTests: XCTestCase {
                override func setUp() { super.setUp() }
                func testOne() { XCTAssertTrue(true) }
                func testTwo() async throws {
                    let s = "func testInString() {"
                    func testNestedHelper() {}
                }
                // func testCommented() {}
                func helper() {}
            }
            """,
        )
        self.assertEqual(
            self.names(),
            [
                "TokenMonTests/A/FooTests.swift::FooTests::testOne",
                "TokenMonTests/A/FooTests.swift::FooTests::testTwo",
            ],
        )

    def test_skips_private_static_and_parameterised(self) -> None:
        write(
            self.root,
            "TokenMonTests/BarTests.swift",
            """
            class BarTests: XCTestCase {
                private func testPrivate() {}
                fileprivate func testFilePrivate() {}
                static func testStatic() {}
                func testWithArg(_ x: Int) {}
                @MainActor func testAttributed() {}
            }
            """,
        )
        self.assertEqual(self.names(), ["TokenMonTests/BarTests.swift::BarTests::testAttributed"])

    def test_extensions_and_inherited_test_cases(self) -> None:
        write(self.root, "TokenMonTests/Base.swift", "class BaseCase: XCTestCase {}\n")
        write(
            self.root,
            "TokenMonTests/Sub.swift",
            """
            final class SubTests: BaseCase {
                func testA() {}
            }
            extension SubTests {
                func testFromExtension() {}
            }
            struct NotATestCase {
                func testIgnored() {}
            }
            final class Helper: NSObject {
                func testAlsoIgnored() {}
            }
            """,
        )
        self.assertEqual(
            self.names(),
            [
                "TokenMonTests/Sub.swift::SubTests::testA",
                "TokenMonTests/Sub.swift::SubTests::testFromExtension",
            ],
        )

    def test_multiple_classes_in_one_file(self) -> None:
        write(
            self.root,
            "TokenMonTests/Two.swift",
            """
            final class OneTests: XCTestCase { func testX() {} }
            final class TwoTests: XCTestCase {
                func testX() {}
                var closure: () -> Void = { }
                func testY() { let x = [1].map { $0 } ; _ = x }
            }
            """,
        )
        self.assertEqual(
            self.names(),
            [
                "TokenMonTests/Two.swift::OneTests::testX",
                "TokenMonTests/Two.swift::TwoTests::testX",
                "TokenMonTests/Two.swift::TwoTests::testY",
            ],
        )

    def test_reports_line_numbers(self) -> None:
        write(self.root, "TokenMonTests/L.swift", "class LTests: XCTestCase {\n\n    func testLine() {}\n}\n")
        (test,) = ex.extract_all(self.root)
        self.assertEqual(test.line, 3)


class ExtractSwiftTestingTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def test_finds_test_functions_in_suites_and_at_file_scope(self) -> None:
        write(
            self.root,
            "TokenMonTests/Core/PaceTests.swift",
            """
            import Testing

            @Suite("Pace")
            struct PaceTests {
                @Test func resetMovesWithPeriod() { #expect(1 == 1) }
                @Test("Display name", arguments: [1, 2])
                func parameterised(value: Int) async throws {}
                func helper() {}
                let s = "@Test func notATest() {"
            }

            @Test
            func freeStanding() {}

            extension PaceTests {
                @MainActor @Test func inExtension() {}
            }
            """,
        )
        tests = ex.extract_all(self.root)
        self.assertEqual(
            [t.id for t in tests],
            [
                "TokenMonTests/Core/PaceTests.swift::PaceTests::resetMovesWithPeriod",
                "TokenMonTests/Core/PaceTests.swift::PaceTests::parameterised",
                "TokenMonTests/Core/PaceTests.swift::global::freeStanding",
                "TokenMonTests/Core/PaceTests.swift::PaceTests::inExtension",
            ],
        )
        self.assertEqual({t.kind for t in tests}, {"swift-testing"})
        self.assertEqual(tests[1].line, 8)  # the `func` line

    def test_mixed_with_xctest(self) -> None:
        write(
            self.root,
            "TokenMonTests/Mixed.swift",
            """
            final class OldTests: XCTestCase { func testA() {} }
            struct NewTests { @Test func b() {} }
            """,
        )
        self.assertEqual(
            [(t.name, t.kind) for t in ex.extract_all(self.root)], [("testA", "xctest"), ("b", "swift-testing")]
        )

    def test_unattributable_test_attribute_fails_loudly(self) -> None:
        write(
            self.root,
            "TokenMonTests/Odd.swift",
            """
            struct OddTests {
                func outer() {
                    @Test func nested() {}
                }
            }
            """,
        )
        with self.assertRaisesRegex(ValueError, r"Odd.swift: found 1 `@Test`.*lines 4"):
            ex.extract_all(self.root)


class ExtractCoreChecksTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def test_each_assert_label_becomes_a_check(self) -> None:
        write(
            self.root,
            "Tests/Manual/CoreTestsMain.swift",
            """
            @main struct CoreTestsMain {
                static func main() throws {
                    func assertTrue(_ cond: Bool, _ msg: String) throws {}
                    try assertTrue(a == 1, "Used Percent")
                    try assertTrue(list.contains(where: { $0.id == "chat", }),
                                   "grpc chat")
                }
            }
            """,
        )
        ids = [t.id for t in ex.extract_all(self.root)]
        prefix = "Tests/Manual/CoreTestsMain.swift::CoreTestsMain::"
        self.assertEqual(ids, [prefix + "used_percent", prefix + "grpc_chat"])

    def test_labels_that_collapse_to_one_id_are_an_error(self) -> None:
        write(
            self.root,
            "Tests/Manual/CoreTestsMain.swift",
            """
            func assertTrue(_ c: Bool, _ m: String) {}
            try assertTrue(a == 1, "Used Percent")
            try assertTrue(b, "used percent")
            """,
        )
        with self.assertRaisesRegex(ValueError, r"CoreTestsMain.swift:4: .*`used_percent`.*line 3"):
            ex.extract_all(self.root)

    def test_non_literal_label_is_an_error(self) -> None:
        write(
            self.root,
            "Tests/Manual/CoreTestsMain.swift",
            "func assertTrue(_ c: Bool, _ m: String) {}\ntry assertTrue(x, label)\n",
        )
        with self.assertRaises(ValueError):
            ex.extract_all(self.root)

    def test_cli_count(self) -> None:
        write(self.root, "TokenMonTests/C.swift", "class CTests: XCTestCase { func testA() {} }\n")
        out = io.StringIO()
        with redirect_stdout(out):
            self.assertEqual(ex.main(["--repo", str(self.root), "--format", "count"]), 0)
        self.assertEqual(out.getvalue().strip(), "xctest=1 swift-testing=0 core-check=0 total=1")


class RealRepositoryTests(unittest.TestCase):
    def test_extracts_the_repository_suite(self) -> None:
        tests = ex.extract_all(ex.default_repo_root())
        self.assertGreater(len(tests), 0)
        self.assertTrue(any(t.kind == "core-check" for t in tests))
        self.assertEqual(len({t.id for t in tests}), len(tests))


if __name__ == "__main__":
    unittest.main()
