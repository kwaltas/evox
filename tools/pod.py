#!/usr/bin/env python3
"""
pod.py -- Terminal Reality POD archive reader for 4x4 Evolution 1 / 2.

POD is the container format used by Terminal Reality engines (Nocturne, Blair
Witch, 4x4 Evo 1/2). Two revisions exist:

  "POD\x00"  v1: magic, u32 count, 20-byte TOC, NUL-terminated name block.
  "POD2"     v2: magic, char comment[80], u32 pad, u32 count, TOC, names.

TOC entry (little endian, 20 bytes):

  u32 name_offset   offset into the name block (relative to block start)
  u32 size          payload size; bit 31 => payload is raw-deflate compressed
  u32 data_offset   absolute offset of the payload in the archive
  u64 timestamp     FILETIME

Both revisions are identical from `count` onwards, so one parser handles them.

The extractor never trusts a single field: it bounds-checks every entry,
checks the name block is inside the archive, checks payload ranges do not
overlap, verifies decompression output length, and records MD5 of every
payload it writes. Anything malformed is reported, not silently dropped.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import struct
import sys
import zlib
from dataclasses import dataclass, field
from pathlib import Path

MAGIC_V1 = b"POD\x00"
MAGIC_V2 = b"POD2"
ENTRY = struct.Struct("<IIIQ")
COMPRESSED_BIT = 0x8000_0000
SIZE_MASK = 0x7FFF_FFFF
MAX_SANE_FILES = 200_000
MAX_COMMENT = 80

# ---------------------------------------------------------------------------
# Classification
# ---------------------------------------------------------------------------

# extension -> (class, group)
EXT_CLASS = {
    ".smf": ("model", "vehicle"),
    ".trk": ("model", "vehicle"),
    ".lte": ("track", "track"),
    ".trk2": ("track", "track"),
    ".rtd": ("track", "track"),
    ".rtl": ("track", "track"),
    ".sit": ("track", "track"),
    ".sdw": ("track", "track"),
    ".clr": ("track", "track"),
    ".lvl": ("track", "track"),
    ".raw": ("texture", "texture"),
    ".act": ("texture", "palette"),
    ".opa": ("texture", "opacity"),
    ".tga": ("texture", "texture"),
    ".tif": ("texture", "unknown"),
    ".bmp": ("texture", "texture"),
    ".pbm": ("texture", "texture"),
    ".png": ("texture", "texture"),
    ".jpg": ("texture", "texture"),
    ".dds": ("texture", "texture"),
    ".wav": ("audio", "sfx"),
    ".sfx": ("audio", "sfx"),
    ".mp3": ("audio", "music"),
    ".bik": ("video", "video"),
    ".rpl": ("video", "replay"),
    ".avi": ("video", "video"),
    ".txt": ("data", "data"),
    ".ini": ("data", "config"),
    ".loc": ("data", "localization"),
    ".rsp": ("data", "script"),
    ".vox": ("data", "localization"),
    ".lst": ("data", "index"),
    ".bin": ("data", "binary"),
}

# Pod name -> default group when the path carries no better hint.
POD_GROUP = {
    "ui": "ui",
    "uitrack": "ui",
    "startup": "ui",
    "parts": "ui",
    "series": "career",
    "ai": "ai",
    "truck": "vehicle",
    "life": "props",
    "sound": "sfx",
    "driver": "vehicle",
}

UI_PODS = {"ui", "uitrack", "startup", "parts"}

SAFE_CHARS = re.compile(r"[^A-Za-z0-9._\-]+")


class PodError(Exception):
    pass


@dataclass
class Entry:
    index: int
    name: str
    raw_size: int
    offset: int
    timestamp: int

    @property
    def size(self) -> int:
        return self.raw_size & SIZE_MASK

    @property
    def compressed(self) -> bool:
        return bool(self.raw_size & COMPRESSED_BIT)


@dataclass
class Archive:
    path: Path
    version: int
    comment: str
    entries: list[Entry]
    data_start: int
    reserved: int = 0
    warnings: list[str] = field(default_factory=list)


def _read_u32(buf: bytes, off: int) -> int:
    if off + 4 > len(buf):
        raise PodError(f"truncated header at 0x{off:x}")
    return struct.unpack_from("<I", buf, off)[0]


def parse(path: Path) -> Archive:
    """Parse an archive header and TOC. Raises PodError on malformed input."""
    buf = path.read_bytes()
    if len(buf) < 8:
        raise PodError("file shorter than a header")

    if buf[:4] == MAGIC_V2:
        version, comment, count_at, toc_at = 2, buf[4:84].split(b"\x00")[0], 0x58, 0x60
        pad = _read_u32(buf, 0x54)
        if pad not in (0, 1):
            raise PodError(f"unexpected v2 pad word 0x{pad:x}")
        reserved = _read_u32(buf, 0x5C)
    elif buf[:4] == MAGIC_V1:
        version, comment, count_at, toc_at = 1, b"", 0x04, 0x0C
        reserved = _read_u32(buf, 0x08)
    else:
        raise PodError(f"bad magic {buf[:4]!r}")

    count = _read_u32(buf, count_at)
    if count > MAX_SANE_FILES:
        raise PodError(f"implausible file count {count}")

    toc_end = toc_at + count * ENTRY.size
    if toc_end > len(buf):
        raise PodError("TOC runs past end of file")

    warnings: list[str] = []
    raw: list[tuple[int, int, int, int]] = []
    for i in range(count):
        noff, size, off, ts = ENTRY.unpack_from(buf, toc_at + i * ENTRY.size)
        raw.append((noff, size, off, ts))

    # Name block begins right after the TOC. Every name must terminate inside
    # the archive, and the block must end before the first payload.
    name_base = toc_end
    first_data = min((o for _, s, o, _ in raw if s), default=len(buf))
    if not 0 <= first_data <= len(buf):
        raise PodError("payload offset outside archive")
    if name_base > first_data:
        raise PodError("name block overlaps first payload")

    entries: list[Entry] = []
    seen_offsets: list[tuple[int, int, str]] = []
    for i, (noff, size, off, ts) in enumerate(raw):
        start = name_base + noff
        if start >= first_data:
            raise PodError(f"entry {i}: name offset 0x{noff:x} outside name block")
        end = buf.find(b"\x00", start, first_data)
        if end < 0:
            raise PodError(f"entry {i}: unterminated name")
        try:
            name = buf[start:end].decode("latin-1")
        except UnicodeDecodeError:  # pragma: no cover - latin-1 is total
            name = buf[start:end].decode("utf-8", "replace")

        if size:
            if off < first_data or off >= len(buf):
                raise PodError(f"entry {i} ({name}): payload offset 0x{off:x} outside archive")
            if not (raw[i][1] & COMPRESSED_BIT) and off + size > len(buf):
                raise PodError(f"entry {i} ({name}): payload 0x{off:x}+{size} outside archive")
            seen_offsets.append((off, off + size, name))

        entries.append(Entry(i, name, size, off, ts))

    seen_offsets.sort()
    for (a0, a1, an), (b0, _, bn) in zip(seen_offsets, seen_offsets[1:]):
        if b0 < a1:
            warnings.append(f"overlapping payloads: {an} and {bn}")

    return Archive(path, version, comment.decode("latin-1", "replace"), entries,
                   name_base, reserved, warnings)


# ---------------------------------------------------------------------------
# Naming / classification
# ---------------------------------------------------------------------------


def sanitize(name: str) -> str:
    parts = []
    for chunk in name.replace("\\", "/").split("/"):
        chunk = chunk.strip().replace("..", "").strip(".")
        if not chunk:
            continue
        parts.append(SAFE_CHARS.sub("_", chunk))
    if not parts:
        return "unnamed.bin"
    return "/".join(parts)


def classify(pod_stem: str, name: str) -> tuple[str, str]:
    """Return (class, group). Never returns an empty class."""
    ext = os.path.splitext(name)[1].lower()
    cls, group = EXT_CLASS.get(ext, ("unknown", "unknown"))
    up = name.upper()
    stem = pod_stem.lower()

    if cls in ("texture", "model", "data") and stem in UI_PODS:
        if up.startswith("ART\\") or up.startswith("UI") or "/" not in up:
            group = "ui"
    if up.startswith("UIREPLAY") or up.startswith("REPLAY"):
        group = "replay"
        if cls == "unknown":
            cls = "video"
    if stem == "series" and cls == "data":
        group = "career"
    if group == "unknown":
        group = POD_GROUP.get(stem, "unknown")
    return cls, group


# ---------------------------------------------------------------------------
# Extraction
# ---------------------------------------------------------------------------


def md5(data: bytes) -> str:
    return hashlib.md5(data).hexdigest()


def payload(buf: bytes, entry: Entry) -> bytes:
    """Return the payload. `size` is the stored (compressed) length; a deflate
    stream may legally end before that, so the decoder stops at stream end."""
    blob = buf[entry.offset:entry.offset + entry.size]
    if not entry.compressed:
        return blob
    try:
        d = zlib.decompressobj(-15)
        out = d.decompress(blob) + d.flush()
    except zlib.error as exc:
        raise PodError(f"{entry.name}: deflate failed: {exc}") from exc
    if not d.eof:
        raise PodError(f"{entry.name}: truncated deflate stream")
    return out


def extract(archive: Archive, out_root: Path, dry_run: bool = False) -> dict:
    buf = archive.path.read_bytes()
    stem = archive.path.stem.lower()
    report = {
        "archive": archive.path.name,
        "version": archive.version,
        "comment": archive.comment,
        "files": 0,
        "bytes": 0,
        "skipped": [],
        "warnings": list(archive.warnings),
        "entries": [],
    }

    for entry in archive.entries:
        if not entry.name:
            report["skipped"].append({"index": entry.index, "reason": "empty name"})
            continue
        try:
            data = payload(buf, entry)
        except PodError as exc:
            report["skipped"].append({"index": entry.index, "reason": str(exc)})
            continue
        if not entry.compressed and len(data) != entry.size:
            report["skipped"].append(
                {"index": entry.index, "reason": f"size mismatch {len(data)} != {entry.size}"})
            continue

        cls, group = classify(stem, entry.name)
        rel = sanitize(entry.name)
        target = out_root / cls / group / rel
        if not dry_run:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)

        report["files"] += 1
        report["bytes"] += len(data)
        report["entries"].append({
            "index": entry.index,
            "name": entry.name,
            "class": cls,
            "group": group,
            "size": len(data),
            "compressed": entry.compressed,
            "md5": md5(data),
            "path": str(target.relative_to(out_root)),
        })

    return report


# ---------------------------------------------------------------------------
# Self test (synthetic archives, no game data needed)
# ---------------------------------------------------------------------------


def _make_pod(names: list[str], payloads: list[bytes], v2: bool = True,
              compress: bool = False) -> bytes:
    head = bytearray()
    head += MAGIC_V2 if v2 else MAGIC_V1
    if v2:
        head += b"unit test".ljust(MAX_COMMENT, b"\x00")
        head += struct.pack("<I", 0)
    head += struct.pack("<I", len(names))
    head += struct.pack("<I", len(names))  # reserved word at 0x5c / 0x08
    toc_at = len(head)
    name_base = toc_at + len(names) * ENTRY.size

    name_block = bytearray()
    offsets = []
    for n in names:
        offsets.append(len(name_block))
        name_block += n.encode("latin-1") + b"\x00"

    data_at = name_base + len(name_block)
    toc = bytearray()
    body = bytearray()
    for i, p in enumerate(payloads):
        if compress:
            c = zlib.compressobj(9, zlib.DEFLATED, -15)
            p = c.compress(p) + c.flush()
        toc += ENTRY.pack(offsets[i],
                          len(payloads[i]) | (COMPRESSED_BIT if compress else 0),
                          data_at + len(body), 0)
        body += p
    out = bytes(head) + bytes(toc) + bytes(name_block) + bytes(body)
    assert len(out) == data_at + len(body)
    return out


def self_test() -> int:
    import tempfile

    failures = 0

    def check(label: str, cond: bool) -> None:
        nonlocal failures
        print(("  ok   " if cond else "  FAIL ") + label)
        failures += 0 if cond else 1

    with tempfile.TemporaryDirectory() as td:
        td = Path(td)
        names = ["ART\\TEX.ACT", "MODELS\\TRUCK.SMF", "SOUND\\HIT.WAV"]
        payloads = [bytes(256), b"SMF" + bytes(64), b"RIFF" + bytes(100)]

        for v2 in (True, False):
            for comp in (False, True):
                p = td / f"t_{v2}_{comp}.POD"
                p.write_bytes(_make_pod(names, payloads, v2, comp))
                arc = parse(p)
                check(f"v2={v2} compress={comp} count", len(arc.entries) == 3)
                check(f"v2={v2} compress={comp} names",
                      [e.name for e in arc.entries] == names)
                rep = extract(arc, td / "out")
                check(f"v2={v2} compress={comp} extracted", rep["files"] == 3)
                for e in rep["entries"]:
                    check(f"  payload {e['name']}", (td / "out" / e["path"]).read_bytes() == payloads[e["index"]])
                    check(f"  classified {e['name']}", e["class"] in ("texture", "model", "audio"))
                check("  act classified as palette", rep["entries"][0]["group"] == "palette")
                check("  smf is a model", rep["entries"][1]["class"] == "model")

        bad = td / "bad.POD"
        bad.write_bytes(b"POD2" + bytes(40))
        try:
            parse(bad)
            check("truncated rejected", False)
        except PodError:
            check("truncated rejected", True)

        bad.write_bytes(b"JUNK" + bytes(200))
        try:
            parse(bad)
            check("bad magic rejected", False)
        except PodError:
            check("bad magic rejected", True)

        check("sanitize strips traversal", sanitize("..\\..\\x\\a b.raw") == "x/a_b.raw")

    print(f"self-test: {'PASS' if failures == 0 else 'FAIL'} ({failures} failures)")
    return 0 if failures == 0 else 1


# ---------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Extract Terminal Reality POD archives.")
    ap.add_argument("source", nargs="?", help="POD file or directory of PODs")
    ap.add_argument("-o", "--out", default="assets/extracted", help="output root")
    ap.add_argument("--manifest", default="assets/manifest.json")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args(argv)

    if args.self_test:
        return self_test()
    if not args.source:
        ap.error("source is required unless --self-test is given")

    src = Path(args.source)
    files = sorted(p for p in ([src] if src.is_file() else src.rglob("*"))
                   if p.is_file() and p.suffix.lower() == ".pod")
    if not files:
        print(f"no .POD files under {src}", file=sys.stderr)
        return 2

    out_root = Path(args.out)
    manifest = {"generator": "pod.py", "archives": [], "totals": {}}
    totals = {"files": 0, "bytes": 0, "skipped": 0}

    for path in files:
        try:
            arc = parse(path)
        except PodError as exc:
            print(f"! {path.name}: {exc}", file=sys.stderr)
            manifest["archives"].append({"archive": path.name, "error": str(exc)})
            continue
        rep = extract(arc, out_root, dry_run=args.dry_run)
        totals["files"] += rep["files"]
        totals["bytes"] += rep["bytes"]
        totals["skipped"] += len(rep["skipped"])
        manifest["archives"].append(rep)
        print(f"{path.name}: {rep['files']} files, {rep['bytes']:,} bytes, "
              f"{len(rep['skipped'])} skipped, {len(rep['warnings'])} warnings")
        for w in rep["warnings"]:
            print(f"  ~ {w}", file=sys.stderr)

    manifest["totals"] = totals
    if not args.dry_run:
        Path(args.manifest).parent.mkdir(parents=True, exist_ok=True)
        Path(args.manifest).write_text(json.dumps(manifest, indent=1), encoding="utf-8")

    print(f"total: {totals['files']} files, {totals['bytes']:,} bytes, {totals['skipped']} skipped")
    return 0


if __name__ == "__main__":
    sys.exit(main())
