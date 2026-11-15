#!/usr/bin/env python3
"""List the DLLs a Windows executable imports, and fail on the VC++ runtime.

    pe_imports.py list  TokenMon.exe   # one DLL name per line (imports + delay-loads)
    pe_imports.py check TokenMon.exe   # exit 1 if it needs the VC++ Redistributable

TokenMon.exe links the C runtime statically (windows/.cargo/config.toml), so
it runs on a clean Windows install. `check` keeps it that way: it fails when
the binary imports a DLL that ships only with the Visual C++ Redistributable
(vcruntime*, msvcp*, concrt*, vccorlib*, vcomp*). The Universal CRT
(ucrtbase.dll, api-ms-win-crt-*) is part of Windows 10 and later, so it is
allowed.

Standard library only; reads the PE headers directly, so it runs on any OS.
"""

from __future__ import annotations

import argparse
import re
import struct
import sys
from pathlib import Path

# DLLs that are not part of Windows and come only from the VC++ Redistributable.
REDIST_ONLY = re.compile(r"^(vcruntime|msvcp|concrt|vccorlib|vcomp)\d.*\.dll$", re.IGNORECASE)

IMPORT_DIR = 1
DELAY_IMPORT_DIR = 13


class PeError(ValueError):
    pass


def _u16(data: bytes, off: int) -> int:
    return struct.unpack_from("<H", data, off)[0]


def _u32(data: bytes, off: int) -> int:
    return struct.unpack_from("<I", data, off)[0]


def _cstr(data: bytes, off: int) -> str:
    end = data.find(b"\0", off)
    if end < 0:
        raise PeError("unterminated DLL name")
    return data[off:end].decode("ascii", "replace")


def imported_dlls(data: bytes) -> list[str]:
    """DLL names from the import and delay-import tables, in table order."""
    try:
        if data[:2] != b"MZ":
            raise PeError("not an MZ executable")
        pe = _u32(data, 0x3C)
        if data[pe : pe + 4] != b"PE\0\0":
            raise PeError("missing PE signature")
        coff = pe + 4
        n_sections = _u16(data, coff + 2)
        opt_size = _u16(data, coff + 16)
        opt = coff + 20
        magic = _u16(data, opt)
        if magic == 0x20B:  # PE32+
            n_dirs_off, dirs_off = opt + 108, opt + 112
        elif magic == 0x10B:  # PE32
            n_dirs_off, dirs_off = opt + 92, opt + 96
        else:
            raise PeError(f"unknown optional header magic {magic:#x}")
        n_dirs = _u32(data, n_dirs_off)

        sections = []
        sec = opt + opt_size
        for i in range(n_sections):
            h = sec + 40 * i
            vsize, va, raw_size, raw_ptr = struct.unpack_from("<IIII", data, h + 8)
            sections.append((va, max(vsize, raw_size), raw_ptr))

        def offset(rva: int) -> int:
            for va, size, raw_ptr in sections:
                if va <= rva < va + size:
                    return rva - va + raw_ptr
            raise PeError(f"RVA {rva:#x} is outside every section")

        def directory(index: int) -> int:
            if index >= n_dirs:
                return 0
            return _u32(data, dirs_off + 8 * index)

        names: list[str] = []
        rva = directory(IMPORT_DIR)
        if rva:
            d = offset(rva)
            while True:
                desc = struct.unpack_from("<IIIII", data, d)
                if not any(desc):
                    break
                names.append(_cstr(data, offset(desc[3])))
                d += 20
        rva = directory(DELAY_IMPORT_DIR)
        if rva:
            d = offset(rva)
            while True:
                desc = struct.unpack_from("<8I", data, d)
                if not any(desc):
                    break
                names.append(_cstr(data, offset(desc[1])))
                d += 32
        return names
    except struct.error as e:
        raise PeError(f"truncated executable: {e}") from None


def redist_only(names: list[str]) -> list[str]:
    return [n for n in names if REDIST_ONLY.match(n)]


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("command", choices=["list", "check"])
    p.add_argument("exe", type=Path)
    a = p.parse_args(argv)
    try:
        names = imported_dlls(a.exe.read_bytes())
    except (OSError, PeError) as e:
        print(f"pe_imports: {a.exe}: {e}", file=sys.stderr)
        return 2
    if a.command == "list":
        print("\n".join(names))
        return 0
    bad = redist_only(names)
    if bad:
        print(
            f"{a.exe.name} needs the Visual C++ Redistributable: {', '.join(bad)}.\n"
            "It should link the C runtime statically (target-feature=+crt-static in "
            "windows/.cargo/config.toml); check that RUSTFLAGS is not set, since it "
            "replaces those flags.",
            file=sys.stderr,
        )
        return 1
    print(f"{a.exe.name}: no VC++ Redistributable DLLs among {len(names)} imports")
    return 0


if __name__ == "__main__":
    sys.exit(main())
