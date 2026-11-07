"""Tests for prune_stale_test_binaries.py (stale test executables in the
cached coverage target dir must not reach llvm-cov).

    python3 -m unittest discover -s windows/scripts/ci/tests -v
"""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import prune_stale_test_binaries as prune  # noqa: E402


class PruneTest(unittest.TestCase):
    def test_removes_test_binaries_and_keeps_everything_else(self):
        with tempfile.TemporaryDirectory() as d:
            deps = Path(d) / "debug" / "deps"
            deps.mkdir(parents=True)
            gone = ["test_auth-1a2b3c4d5e6f7a8b", "ui_snapshots-00ff.exe"]
            kept = ["libserde-1.rlib", "libserde-1.rmeta", "serde-1.d",
                    "libmacros-1.so", "macros-1.dll", "test_auth-1a2b.pdb"]
            for name in gone + kept:
                (deps / name).write_bytes(b"x")
            (deps / "incremental").mkdir()
            removed = prune.prune(Path(d))
            self.assertEqual(sorted(p.name for p in removed), sorted(gone))
            self.assertEqual(sorted(p.name for p in deps.iterdir()), sorted(kept + ["incremental"]))

    def test_missing_target_dir_is_a_no_op(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(prune.prune(Path(d) / "nope"), [])


class WiredIntoCoverageTest(unittest.TestCase):
    """Both CI coverage legs and `make coverage` prune before measuring."""

    def test_workflow_and_makefile_call_the_script(self):
        root = HERE.parent.parent.parent
        wf = (root.parent / ".github" / "workflows" / "windows-ci.yml").read_text()
        self.assertIn("prune_stale_test_binaries.py target/llvm-cov-target", wf)
        mk = (root / "Makefile").read_text()
        self.assertIn("prune_stale_test_binaries.py target/llvm-cov-target", mk)


if __name__ == "__main__":
    unittest.main()
