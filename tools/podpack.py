#!/usr/bin/env python3
"""
podpack.py -- build a POD archive from a directory (modding SDK).

Round-trips with tools/pod.py: names are stored verbatim relative to the input
root with backslashes, sorted for reproducible archives. `--compress` uses raw
deflate and sets the format's high size bit.
"""

from __future__ import annotations

import argparse
import struct
import sys
import time
import zlib
from pathlib import Path

MAGIC = b"POD2"
ENTRY = struct.Struct("<IIIQ")
COMPRESSED_BIT = 0x8000_0000


def build(root: Path, compress: bool, comment: str) -> bytes:
    files = sorted(
        (p for p in root.rglob("*") if p.is_file()),
        key=lambda p: p.relative_to(root).as_posix().lower(),
    )
    if not files:
        raise SystemExit("no input files")
    if len(files) > 60_000:
        raise SystemExit("too many files for one archive")

    names = [p.relative_to(root).as_posix().replace("/", "\\") for p in files]
    for n in names:
        if any(ord(c) > 255 for c in n):
            raise SystemExit(f"non-latin-1 name: {n}")
        if len(n) > 255:
            raise SystemExit(f"name too long: {n}")

    count = len(files)
    comment_b = comment.encode("latin-1", "replace")[:80].ljust(80, b"\x00")
    toc_at = 0x60
    name_base = toc_at + count * ENTRY.size

    name_block = bytearray()
    offsets = []
    for n in names:
        offsets.append(len(name_block))
        name_block += n.encode("latin-1") + b"\x00"

    data_at = name_base + len(name_block)
    toc = bytearray()
    body = bytearray()
    now = int(time.time() * 10_000_000) + 116_444_736_000_000_000  # FILETIME-ish

    for i, p in enumerate(files):
        payload = p.read_bytes()
        stored = payload
        flag = 0
        if compress and payload:
            c = zlib.compressobj(9, zlib.DEFLATED, -15)
            stored = c.compress(payload) + c.flush()
            if len(stored) < len(payload):
                flag = COMPRESSED_BIT
            else:
                stored = payload
        toc += ENTRY.pack(offsets[i], len(stored) | flag, data_at + len(body), now)
        body += stored

    header = bytearray()
    header += MAGIC
    header += comment_b
    header += struct.pack("<I", 0)
    header += struct.pack("<I", count)
    header += struct.pack("<I", 0)
    out = bytes(header) + bytes(toc) + bytes(name_block) + bytes(body)
    assert len(header) == toc_at
    return out


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("input", help="directory to pack")
    ap.add_argument("output")
    ap.add_argument("--compress", action="store_true")
    ap.add_argument("--comment", default="EvoX mod")
    args = ap.parse_args(argv)
    data = build(Path(args.input), args.compress, args.comment)
    Path(args.output).write_bytes(data)
    print(f"wrote {args.output}: {len(data):,} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
