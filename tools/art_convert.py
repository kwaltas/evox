#!/usr/bin/env python3
"""
art_convert.py -- convert Terminal Reality indexed ART (RAW + ACT + OPA) to PNG.

RAW is 8-bit indexed pixels, ACT a 768-byte VGA palette, OPA a same-size alpha
mask. Dimensions are inferred from the payload length; unknown sizes are
skipped rather than guessed.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:  # pragma: no cover
    Image = None

KNOWN = {
    65536: (256, 256),
    16384: (128, 128),
    4096: (64, 64),
    1024: (32, 32),
    256: (16, 16),
    307200: (640, 480),
    76096: (232, 328),
    57024: (264, 216),
    38704: (248, 156),
    29040: (220, 132),
    1536: (32, 48),
}


def dims(n: int) -> tuple[int, int] | None:
    if n in KNOWN:
        return KNOWN[n]
    side = int(n**0.5)
    if side * side == n:
        return side, side
    return None


def convert(name: str, dest: Path, act: Path | None, opa: Path | None) -> bool:
    if Image is None:
        print("pillow is required: pip install pillow", file=sys.stderr)
        return False
    raw_path = dest.with_suffix(".raw")
    if not raw_path.exists():
        # Caller may only hold the payload; try sibling naming.
        raw_path = dest.parent / (dest.stem + ".RAW")
    try:
        pixels = raw_path.read_bytes()
    except OSError:
        return False
    if act is None or not act.exists():
        return False
    palette = bytearray(act.read_bytes()[:768])
    if len(palette) < 768:
        palette.extend(b"\x00" * (768 - len(palette)))
    size = dims(len(pixels))
    if size is None:
        print(f"skip {name}: unknown size {len(pixels)}", file=sys.stderr)
        return False
    alpha = opa.read_bytes() if opa and opa.exists() else b"\xff" * len(pixels)
    if len(alpha) < len(pixels):
        alpha = alpha + b"\xff" * (len(pixels) - len(alpha))
    img = Image.frombytes("P", size, pixels)
    img.putpalette(bytes(palette))
    img = img.convert("RGBA")
    img.putalpha(Image.frombytes("L", size, alpha))
    dest.parent.mkdir(parents=True, exist_ok=True)
    img.save(dest)
    return True


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--in", dest="root", required=True, help="extraction root")
    ap.add_argument("--out", required=True, help="PNG output root")
    ap.add_argument("--limit", type=int, default=0)
    args = ap.parse_args(argv)
    if Image is None:
        print("pillow is required", file=sys.stderr)
        return 2
    root = Path(args.root)
    out = Path(args.out)
    n = 0
    for raw in sorted(root.rglob("*.RAW")) + sorted(root.rglob("*.raw")):
        act = raw.with_suffix(".ACT")
        if not act.exists():
            act = raw.with_suffix(".act")
        opa = raw.with_suffix(".OPA")
        if not opa.exists():
            opa = raw.with_suffix(".opa")
        rel = raw.relative_to(root).with_suffix(".png")
        if convert(str(raw), out / rel, act, opa):
            # convert() reads <dest>.raw, so stage payloads next to the PNG.
            n += 1
        if args.limit and n >= args.limit:
            break
    print(f"converted {n} textures")
    return 0


if __name__ == "__main__":
    sys.exit(main())
