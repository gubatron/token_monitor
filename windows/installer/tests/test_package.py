"""Tests for windows/installer/package.py (release version, zip, SHA256SUMS).

    python3 -I -m unittest discover -s windows/installer/tests -v
"""

from __future__ import annotations

import hashlib
import importlib.util
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE.parent / "package.py"
WINDOWS = HERE.parent.parent

_spec = importlib.util.spec_from_file_location("package", SCRIPT)
assert _spec and _spec.loader
package = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(package)


def write_workspace(root: Path, workspace_version: str, app_version: str | None = None) -> None:
    (root / "crates/tokenmon-app").mkdir(parents=True)
    (root / "Cargo.toml").write_text(
        f'[workspace]\nmembers = ["crates/tokenmon-app"]\n\n[workspace.package]\nversion = "{workspace_version}"\n'
    )
    version_line = 'version.workspace = true' if app_version is None else f'version = "{app_version}"'
    (root / "crates/tokenmon-app/Cargo.toml").write_text(
        f'[package]\nname = "tokenmon-app"\n{version_line}\n'
    )


class VersionTests(unittest.TestCase):
    def test_reads_the_real_workspace(self):
        version = package.read_version(WINDOWS)
        self.assertRegex(version, r"^\d+\.\d+\.\d+$")

    def test_inherits_workspace_version(self):
        with tempfile.TemporaryDirectory() as d:
            write_workspace(Path(d), "1.10.1")
            self.assertEqual(package.read_version(Path(d)), "1.10.1")

    def test_crate_version_overrides_workspace(self):
        with tempfile.TemporaryDirectory() as d:
            write_workspace(Path(d), "1.0.0", app_version="2.3.4")
            self.assertEqual(package.read_version(Path(d)), "2.3.4")

    def test_prerelease_and_bad_versions_are_refused(self):
        for bad in ("1.2", "1.2.3-beta.1", "1.2.3+4", "01.2.3", "256.0.0", "1.256.0", "1.0.65535"):
            with self.subTest(bad=bad), tempfile.TemporaryDirectory() as d:
                write_workspace(Path(d), bad)
                with self.assertRaises(package.PackagingError):
                    package.read_version(Path(d))

    def test_msi_limits_are_inclusive(self):
        with tempfile.TemporaryDirectory() as d:
            write_workspace(Path(d), "255.255.65534")
            self.assertEqual(package.read_version(Path(d)), "255.255.65534")


class TagTests(unittest.TestCase):
    def test_matching_tag_passes(self):
        package.check_tag("windows-v1.2.3", "1.2.3")

    def test_mismatches_fail_with_a_fix(self):
        for tag in ("v1.2.3", "windows-v1.2.4", "windows-1.2.3", "Windows-v1.2.3", "windows-v1.2.3-rc1"):
            with self.subTest(tag=tag):
                with self.assertRaises(package.PackagingError) as cm:
                    package.check_tag(tag, "1.2.3")
                self.assertIn("windows-v1.2.3", str(cm.exception))

    def test_cli_check_tag_exit_codes(self):
        with tempfile.TemporaryDirectory() as d:
            write_workspace(Path(d), "0.4.0")
            ok = subprocess.run([sys.executable, "-I", str(SCRIPT), "check-tag", "windows-v0.4.0",
                                 "--windows-dir", d], capture_output=True, text=True)
            self.assertEqual(ok.returncode, 0, ok.stderr)
            bad = subprocess.run([sys.executable, "-I", str(SCRIPT), "check-tag", "windows-v0.4.1",
                                  "--windows-dir", d], capture_output=True, text=True)
            self.assertEqual(bad.returncode, 1)
            self.assertIn("does not match", bad.stderr)


class AssetNameTests(unittest.TestCase):
    def test_names_carry_the_windows_prefix(self):
        names = package.asset_names("1.2.3")
        self.assertEqual(names, {
            "msi": "tokenmon-windows-1.2.3-x64.msi",
            "zip": "tokenmon-windows-1.2.3-x64.zip",
            "sums": "tokenmon-windows-1.2.3-SHA256SUMS.txt",
        })
        for name in names.values():
            self.assertTrue(name.startswith("tokenmon-windows-"))

    def test_sums_file_is_never_taken_for_an_update_asset(self):
        # The updater picks tokenmon-windows-*.zip / *.msi; the sums file must match neither.
        sums = package.asset_names("1.2.3")["sums"]
        self.assertFalse(sums.endswith((".zip", ".msi")))


class ZipTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        self.exe = self.dir / "tokenmon.exe"
        self.exe.write_bytes(b"MZ" + bytes(range(256)) * 64)
        self.license = self.dir / "LICENSE"
        self.license.write_text("MIT License\n")
        self.readme = self.dir / "README.txt"
        self.readme.write_text("readme\n")

    def tearDown(self):
        self.tmp.cleanup()

    def build(self, name="a.zip"):
        out = self.dir / "dist" / name
        return out, package.build_zip(self.exe, self.license, self.readme, out)

    def test_layout_is_flat_with_the_exe_renamed(self):
        out, names = self.build()
        self.assertEqual(names, ["TokenMon.exe", "README.txt", "LICENSE.txt"])
        with zipfile.ZipFile(out) as zf:
            self.assertEqual(zf.read("TokenMon.exe"), self.exe.read_bytes())
            self.assertEqual(zf.read("LICENSE.txt"), b"MIT License\n")
            for info in zf.infolist():
                self.assertEqual(info.compress_type, zipfile.ZIP_DEFLATED)
                self.assertEqual(info.date_time, (1980, 1, 1, 0, 0, 0))

    def test_same_inputs_give_identical_bytes(self):
        a, _ = self.build("a.zip")
        b, _ = self.build("b.zip")
        self.assertEqual(a.read_bytes(), b.read_bytes())

    def test_missing_input_is_an_error(self):
        self.readme.unlink()
        with self.assertRaises(package.PackagingError):
            self.build()

    def write_zip(self, entries):
        out = self.dir / "bad.zip"
        with zipfile.ZipFile(out, "w") as zf:
            for name, data, attr in entries:
                info = zipfile.ZipInfo(name)
                if attr is not None:
                    info.external_attr = attr
                zf.writestr(info, data)
        return out

    def test_validate_refuses_layouts_the_updater_refuses(self):
        cases = {
            "nested exe": [("TokenMon/TokenMon.exe", b"MZ", None)],
            "no exe": [("README.txt", b"x", None)],
            "traversal": [("TokenMon.exe", b"MZ", None), ("../evil.txt", b"x", None)],
            "backslash": [("TokenMon.exe", b"MZ", None), ("a\\b.txt", b"x", None)],
            "drive": [("TokenMon.exe", b"MZ", None), ("C:evil", b"x", None)],
            "symlink": [("TokenMon.exe", b"MZ", None), ("link", b"TokenMon.exe", (0o120777 << 16))],
            "duplicate": [("TokenMon.exe", b"MZ", None), ("tokenmon.EXE", b"MZ", None)],
        }
        for label, entries in cases.items():
            with self.subTest(label):
                with self.assertRaises(package.PackagingError):
                    package.validate_zip(self.write_zip(entries))


class SumsAndSummaryTests(unittest.TestCase):
    def test_sums_match_sha256sum_format(self):
        with tempfile.TemporaryDirectory() as d:
            b = Path(d) / "b.msi"
            a = Path(d) / "a.zip"
            b.write_bytes(b"bbb")
            a.write_bytes(b"aaa")
            out = Path(d) / "SUMS.txt"
            text = package.write_sums(out, [b, a])
            self.assertEqual(text, (
                f"{hashlib.sha256(b'aaa').hexdigest()}  a.zip\n"
                f"{hashlib.sha256(b'bbb').hexdigest()}  b.msi\n"
            ))
            self.assertEqual(out.read_bytes(), text.encode("ascii"))

    def test_same_basename_twice_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "x").mkdir()
            one = Path(d) / "f.zip"
            two = Path(d) / "x" / "f.zip"
            one.write_bytes(b"1")
            two.write_bytes(b"2")
            with self.assertRaises(package.PackagingError):
                package.write_sums(Path(d) / "S", [one, two])

    def test_summary_lists_size_and_digest(self):
        with tempfile.TemporaryDirectory() as d:
            f = Path(d) / "tokenmon-windows-1.0.0-x64.zip"
            f.write_bytes(b"x" * 2048)
            md = package.summary("1.0.0", False, [f])
            self.assertIn("windows-v1.0.0", md)
            self.assertIn("**unsigned**", md)
            self.assertIn(f"| `{f.name}` | 0.00 MiB (2048 bytes) | `{hashlib.sha256(b'x' * 2048).hexdigest()}` |", md)
            self.assertIn("signed", package.summary("1.0.0", True, [f]))


if __name__ == "__main__":
    unittest.main()
