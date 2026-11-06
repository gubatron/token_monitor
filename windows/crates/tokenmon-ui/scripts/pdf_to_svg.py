#!/usr/bin/env python3
"""Convert the Grok mark (`MenuBarIcon.pdf`, a single filled path of m/l/c ops)
to `ui/assets/logos/grok.svg`.

Usage: pdf_to_svg.py <MenuBarIcon.pdf> <out.svg>

The PDF content stream is uncompressed and uses only `m`, `l`, `c` and `f`, so
no PDF library is needed. PDF's y axis points up; the SVG flips it back.
"""
import re
import sys


def main(src: str, dst: str) -> None:
    data = open(src, "rb").read()
    stream = re.search(rb"stream\r?\n(.*?)endstream", data, re.S).group(1).decode()
    subpaths, cur = [], []
    for line in stream.split("\n"):
        tok = line.split()
        if not tok:
            continue
        op = tok[-1]
        if op == "m":
            if cur:
                subpaths.append(" ".join(cur) + " Z")
                cur = []
            cur.append("M" + " ".join(tok[:2]))
        elif op == "l":
            cur.append("L" + " ".join(tok[:2]))
        elif op == "c":
            cur.append("C" + " ".join(tok[:6]))
    if cur:
        subpaths.append(" ".join(cur) + " Z")
    svg = (
        '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">\n'
        '<path fill="black" transform="translate(0 16) scale(1 -1)" d="%s"/>\n</svg>\n'
        % " ".join(subpaths)
    )
    open(dst, "w").write(svg)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
