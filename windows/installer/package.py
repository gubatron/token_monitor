#!/usr/bin/env python3
"""Release packaging helpers for TokenMon for Windows (stdlib only).

Used by .github/workflows/windows-release.yml; tested by tests/test_package.py
on any OS.

    package.py version [--windows-dir DIR]
        Print the release version: tokenmon-app's version, which inherits
        [workspace.package] version from windows/Cargo.toml.
    package.py check-tag TAG [--windows-dir DIR]
        Fail unless TAG is windows-v<version>.
    package.py asset-names VERSION
        Print key=value lines (msi, zip, sums) for $GITHUB_OUTPUT.
    package.py zip --exe PATH --license PATH --readme PATH --out PATH
        Build the portable zip: TokenMon.exe, README.txt and LICENSE.txt at
        the top level (the in-app updater only accepts a top-level exe), with
        fixed timestamps so the same inputs give the same bytes.
    package.py validate-zip PATH
        Check an existing zip against the same layout rules.
    package.py sums --out PATH FILE...
        Write a SHA256SUMS file (`sha256sum -c` format) for FILE...
    package.py summary --version V --signed yes|no FILE...
        Print a Markdown table of the release assets (name, size, SHA-256).
"""

from __future__ import annotations

import argparse
import hashlib
import re
import sys
import tomllib
import zipfile
from pathlib import Path

TAG_PREFIX = "windows-v"
ASSET_PREFIX = "tokenmon-windows-"
EXE_NAME = "TokenMon.exe"
APP_CRATE = "crates/tokenmon-app/Cargo.toml"
# 1980-01-01, the earliest time a zip entry can carry.
ZIP_EPOCH = (1980, 1, 1, 0, 0, 0)
SEMVER = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")


class PackagingError(Exception):
    """A release input is missing or malformed."""


def read_version(windows_dir: Path) -> str:
    """tokenmon-app's version, resolving `version.workspace = true`."""
    app = tomllib.loads((windows_dir / APP_CRATE).read_text(encoding="utf-8"))
    version = app.get("package", {}).get("version")
    if isinstance(version, dict) and version.get("workspace") is True:
        workspace = tomllib.loads((windows_dir / "Cargo.toml").read_text(encoding="utf-8"))
        version = workspace.get("workspace", {}).get("package", {}).get("version")
    if not isinstance(version, str) or not SEMVER.match(version):
        raise PackagingError(f"tokenmon-app version {version!r} is not MAJOR.MINOR.PATCH")
    major, minor, patch = (int(p) for p in version.split("."))
    # Windows Installer ProductVersion limits (cargo-wix enforces the same).
    if major > 255 or minor > 255 or patch > 65534:
        raise PackagingError(f"version {version} exceeds the MSI limits 255.255.65534")
    return version


def check_tag(tag: str, version: str) -> None:
    """Raise unless `tag` is exactly `windows-v<version>`."""
    expected = TAG_PREFIX + version
    if tag != expected:
        raise PackagingError(
            f"Tag {tag} does not match the crate version {version}. "
            f"Bump [workspace.package] version in windows/Cargo.toml, or tag {expected}."
        )


def asset_names(version: str) -> dict[str, str]:
    """Release asset file names. All start with `tokenmon-windows-`, the
    only names the Windows updater considers and the macOS updater ignores."""
    base = f"{ASSET_PREFIX}{version}"
    return {"msi": f"{base}-x64.msi", "zip": f"{base}-x64.zip", "sums": f"{base}-SHA256SUMS.txt"}


def build_zip(exe: Path, license_file: Path, readme: Path, out: Path) -> list[str]:
    """Write the portable zip and return its entry names."""
    entries = [(EXE_NAME, exe), ("README.txt", readme), ("LICENSE.txt", license_file)]
    for _, src in entries:
        if not src.is_file():
            raise PackagingError(f"{src} does not exist")
    out.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
        for name, src in entries:
            info = zipfile.ZipInfo(name, date_time=ZIP_EPOCH)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.create_system = 0  # MS-DOS: no Unix mode bits, no symlink flag
            info.external_attr = 0x20  # FILE_ATTRIBUTE_ARCHIVE
            zf.writestr(info, src.read_bytes())
    return validate_zip(out)


def validate_zip(path: Path) -> list[str]:
    """Check the layout the in-app updater accepts (tokenmon-platform
    install::archive): TokenMon.exe at the top level, no folders, no
    symlinks, no path tricks. Returns the entry names."""
    with zipfile.ZipFile(path) as zf:
        names = zf.namelist()
        for info in zf.infolist():
            name = info.filename
            if "/" in name or "\\" in name or ":" in name or name in ("", ".", ".."):
                raise PackagingError(f"zip entry {name!r} is not a plain top-level file")
            if info.is_dir() or (info.external_attr >> 16) & 0o170000 == 0o120000:
                raise PackagingError(f"zip entry {name!r} is a folder or a symlink")
        if EXE_NAME not in names:
            raise PackagingError(f"zip has no top-level {EXE_NAME}")
        if len(set(n.lower() for n in names)) != len(names):
            raise PackagingError("zip has duplicate entries")
        bad = zf.testzip()
        if bad is not None:
            raise PackagingError(f"zip entry {bad!r} is corrupt")
    return names


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def write_sums(out: Path, files: list[Path]) -> str:
    """Write `<sha256>  <basename>` lines sorted by name; return the text."""
    ordered = sorted(files, key=lambda f: f.name)
    names = [f.name for f in ordered]
    if len(set(names)) != len(names):
        raise PackagingError("two files share a name")
    lines = [f"{sha256_file(f)}  {f.name}" for f in ordered]
    text = "\n".join(lines) + "\n"
    out.write_text(text, encoding="ascii", newline="\n")
    return text


def summary(version: str, signed: bool, files: list[Path]) -> str:
    """Markdown for $GITHUB_STEP_SUMMARY."""
    out = [
        f"### TokenMon for Windows {version}",
        "",
        f"Tag `{TAG_PREFIX}{version}` · Authenticode: "
        + ("signed" if signed else "**unsigned** (no signing secrets; SmartScreen will warn)"),
        "",
        "| asset | size | SHA-256 |",
        "|---|---:|---|",
    ]
    for f in files:
        size = f.stat().st_size
        out.append(f"| `{f.name}` | {size / 1048576:.2f} MiB ({size} bytes) | `{sha256_file(f)}` |")
    return "\n".join(out) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    default_dir = Path(__file__).resolve().parent.parent

    p = sub.add_parser("version")
    p.add_argument("--windows-dir", type=Path, default=default_dir)
    p = sub.add_parser("check-tag")
    p.add_argument("tag")
    p.add_argument("--windows-dir", type=Path, default=default_dir)
    p = sub.add_parser("asset-names")
    p.add_argument("version")
    p = sub.add_parser("zip")
    p.add_argument("--exe", type=Path, required=True)
    p.add_argument("--license", type=Path, required=True)
    p.add_argument("--readme", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p = sub.add_parser("validate-zip")
    p.add_argument("path", type=Path)
    p = sub.add_parser("sums")
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("files", type=Path, nargs="+")
    p = sub.add_parser("summary")
    p.add_argument("--version", required=True)
    p.add_argument("--signed", choices=["yes", "no"], required=True)
    p.add_argument("files", type=Path, nargs="+")
    args = parser.parse_args(argv)

    try:
        if args.cmd == "version":
            print(read_version(args.windows_dir))
        elif args.cmd == "check-tag":
            check_tag(args.tag, read_version(args.windows_dir))
            print(f"{args.tag} matches the crate version.")
        elif args.cmd == "asset-names":
            for key, value in asset_names(args.version).items():
                print(f"{key}={value}")
        elif args.cmd == "zip":
            names = build_zip(args.exe, args.license, args.readme, args.out)
            print(f"{args.out}: {', '.join(names)}")
        elif args.cmd == "validate-zip":
            print(f"{args.path}: {', '.join(validate_zip(args.path))}")
        elif args.cmd == "sums":
            sys.stdout.write(write_sums(args.out, args.files))
        elif args.cmd == "summary":
            sys.stdout.write(summary(args.version, args.signed == "yes", args.files))
    except (PackagingError, OSError, zipfile.BadZipFile, tomllib.TOMLDecodeError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
