#!/usr/bin/env python3
"""
verify_manifest.py -- re-hash an extraction and compare with manifest.json.

Detects truncated writes, disk corruption, or partial copies. Exit code is the
number of mismatches.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path


def md5(p: Path) -> str:
    h = hashlib.md5()
    with p.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("root", help="extraction root")
    ap.add_argument("manifest", help="manifest.json produced by pod.py")
    args = ap.parse_args(argv)
    root = Path(args.root)
    manifest = json.loads(Path(args.manifest).read_text())
    bad = 0
    checked = 0
    for archive in manifest.get("archives", []):
        for e in archive.get("entries", []):
            checked += 1
            p = root / e["path"]
            if not p.exists():
                print(f"missing {e['path']}")
                bad += 1
            elif md5(p) != e["md5"]:
                print(f"corrupt {e['path']}")
                bad += 1
    print(f"{checked} files checked, {bad} bad")
    return bad


if __name__ == "__main__":
    sys.exit(main())
