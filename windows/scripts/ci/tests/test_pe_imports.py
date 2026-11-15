"""Tests for pe_imports.py against small PE images built in memory.

    python3 -m unittest discover -s windows/scripts/ci/tests -v
"""

from __future__ import annotations

import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

import pe_imports  # noqa: E402

SECTION_VA = 0x1000
SECTION_RAW = 0x200


def build_pe(imports: list[str], delay: list[str] = (), pe32_plus: bool = True) -> bytes:
    """A minimal PE with one section holding the import tables and names."""
    # Section body: import descriptors, delay descriptors, then the names.
    n_imp, n_delay = len(imports), len(delay)
    imp_size = 20 * (n_imp + 1) if n_imp else 0
    delay_size = 32 * (n_delay + 1) if n_delay else 0
    names_off = imp_size + delay_size
    blob = b""
    name_rvas = []
    for n in list(imports) + list(delay):
        name_rvas.append(SECTION_VA + names_off + len(blob))
        blob += n.encode() + b"\0"
    body = b""
    for i in range(n_imp):
        body += struct.pack("<IIIII", 0, 0, 0, name_rvas[i], 0)
    if n_imp:
        body += b"\0" * 20
    for i in range(n_delay):
        body += struct.pack("<8I", 1, name_rvas[n_imp + i], 0, 0, 0, 0, 0, 0)
    if n_delay:
        body += b"\0" * 32
    body += blob

    n_dirs = 16
    opt_fixed = 112 if pe32_plus else 96
    opt_size = opt_fixed + 8 * n_dirs
    opt = bytearray(opt_size)
    struct.pack_into("<H", opt, 0, 0x20B if pe32_plus else 0x10B)
    struct.pack_into("<I", opt, opt_fixed - 4, n_dirs)
    if n_imp:
        struct.pack_into("<II", opt, opt_fixed + 8 * 1, SECTION_VA, imp_size)
    if n_delay:
        struct.pack_into("<II", opt, opt_fixed + 8 * 13, SECTION_VA + imp_size, delay_size)

    pe_off = 0x40
    coff = struct.pack("<HHIIIHH", 0x8664, 1, 0, 0, 0, opt_size, 0x22)
    section = struct.pack(
        "<8sIIIIIIHHI", b".idata", len(body), SECTION_VA, len(body), SECTION_RAW, 0, 0, 0, 0, 0
    )
    head = bytearray(SECTION_RAW)
    head[0:2] = b"MZ"
    struct.pack_into("<I", head, 0x3C, pe_off)
    hdr = b"PE\0\0" + coff + bytes(opt) + section
    head[pe_off : pe_off + len(hdr)] = hdr
    return bytes(head) + body


class ImportedDlls(unittest.TestCase):
    def test_lists_imports_and_delay_loads_in_order(self):
        data = build_pe(["KERNEL32.dll", "USER32.dll"], delay=["WebView2Loader.dll"])
        self.assertEqual(
            pe_imports.imported_dlls(data), ["KERNEL32.dll", "USER32.dll", "WebView2Loader.dll"]
        )

    def test_pe32_header(self):
        data = build_pe(["KERNEL32.dll"], pe32_plus=False)
        self.assertEqual(pe_imports.imported_dlls(data), ["KERNEL32.dll"])

    def test_no_import_table(self):
        self.assertEqual(pe_imports.imported_dlls(build_pe([])), [])

    def test_rejects_non_pe(self):
        with self.assertRaises(pe_imports.PeError):
            pe_imports.imported_dlls(b"#!/bin/sh\n")
        with self.assertRaises(pe_imports.PeError):
            pe_imports.imported_dlls(build_pe(["KERNEL32.dll"])[:0x50])


class RedistOnly(unittest.TestCase):
    def test_flags_vc_redist_dlls_only(self):
        names = [
            "KERNEL32.dll",
            "VCRUNTIME140.dll",
            "vcruntime140_1.dll",
            "MSVCP140.dll",
            "api-ms-win-crt-runtime-l1-1-0.dll",
            "ucrtbase.dll",
            "msvcrt.dll",
        ]
        self.assertEqual(
            pe_imports.redist_only(names), ["VCRUNTIME140.dll", "vcruntime140_1.dll", "MSVCP140.dll"]
        )


class Cli(unittest.TestCase):
    def run_cli(self, data: bytes, command: str) -> subprocess.CompletedProcess:
        with tempfile.TemporaryDirectory() as d:
            exe = Path(d) / "TokenMon.exe"
            exe.write_bytes(data)
            return subprocess.run(
                [sys.executable, "-I", str(SCRIPTS / "pe_imports.py"), command, str(exe)],
                capture_output=True,
                text=True,
            )

    def test_check_passes_without_redist(self):
        r = self.run_cli(build_pe(["KERNEL32.dll", "api-ms-win-crt-heap-l1-1-0.dll"]), "check")
        self.assertEqual(r.returncode, 0, r.stderr)

    def test_check_fails_on_vcruntime(self):
        r = self.run_cli(build_pe(["KERNEL32.dll", "VCRUNTIME140.dll"]), "check")
        self.assertEqual(r.returncode, 1)
        self.assertIn("VCRUNTIME140.dll", r.stderr)

    def test_list(self):
        r = self.run_cli(build_pe(["KERNEL32.dll"]), "list")
        self.assertEqual(r.stdout.split(), ["KERNEL32.dll"])

    def test_unreadable_input_exits_2(self):
        r = self.run_cli(b"nope", "check")
        self.assertEqual(r.returncode, 2)


if __name__ == "__main__":
    unittest.main()
