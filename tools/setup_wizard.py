#!/usr/bin/env python3
"""
setup_wizard.py -- first-boot asset intake from the user's own copy of 4x4
Evolution.

Finds POD archives, verifies them against `assets/pods.md5`, extracts through
tools/pod.py, optionally bakes UI art to PNG, and writes `install.json`.
Nothing copyrighted enters git.

Exit codes: 0 ok, 1 missing source, 2 no archives, 3 verification failure,
4 extraction failure.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
POD_TOOL = ROOT / "tools" / "pod.py"
EXPECTED = ROOT / "assets" / "pods.md5"


def md5(path: Path) -> str:
    h = hashlib.md5()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def load_expected() -> dict[str, str]:
    out = {}
    if not EXPECTED.exists():
        return out
    for line in EXPECTED.read_text().splitlines():
        parts = line.split()
        if len(parts) == 2:
            out[parts[1].lower()] = parts[0].lower()
    return out


def find_archives(src: Path) -> list[Path]:
    if src.is_file():
        return [src] if src.suffix.lower() == ".pod" else []
    out = []
    for p in src.rglob("*"):
        if p.is_file() and p.suffix.lower() == ".pod" and p.stat().st_size > 64:
            with p.open("rb") as f:
                if f.read(4) in (b"POD\x00", b"POD2"):
                    out.append(p)
    return sorted(out)


def human(n: int) -> str:
    return f"{n / 1048576:.1f} MiB"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--source", help="CD root, ISO mount, or extracted game folder")
    ap.add_argument("--out", default=str(ROOT / "assets" / "extracted"))
    ap.add_argument("--yes", action="store_true", help="no prompts")
    ap.add_argument("--force", action="store_true", help="re-extract over existing output")
    ap.add_argument("--skip-verify", action="store_true", help="accept unknown/MD5-mismatched archives")
    ap.add_argument("--bake-ui", action="store_true", help="also convert ART to PNG via art_convert.py")
    args = ap.parse_args(argv)

    source = args.source
    if not source and not args.yes:
        source = input("Path to your 4x4 Evolution CD / ISO / install folder: ").strip()
    if not source:
        print("error: --source is required", file=sys.stderr)
        return 1
    src = Path(source).expanduser()
    if not src.exists():
        print(f"error: {src} does not exist", file=sys.stderr)
        return 1

    archives = find_archives(src)
    if not archives:
        print("error: no POD archives found (mount the CD or point at the installed game)", file=sys.stderr)
        return 2

    out = Path(args.out).expanduser()
    if out.exists() and any(out.iterdir()) and not args.force:
        print(f"output {out} already has data; pass --force to replace")
    else:
        if out.exists() and args.force:
            shutil.rmtree(out)
        out.mkdir(parents=True, exist_ok=True)

    expected = load_expected()
    total = sum(p.stat().st_size for p in archives)
    print(f"found {len(archives)} archives, {human(total)} in {src}")
    if not args.yes:
        print(f"extracting to {out}")
        if input("continue? [Y/n] ").strip().lower() in ("n", "no"):
            return 1

    failures = []
    unknown = []
    for p in archives:
        name = p.name.lower()
        if args.skip_verify or not expected:
            continue
        want = expected.get(name)
        if want is None:
            unknown.append(p.name)
            continue
        got = md5(p)
        if got != want:
            failures.append(f"{p.name}: md5 {got} != {want}")
        else:
            print(f"  verified {p.name}")
    if unknown:
        print(f"warning: {len(unknown)} archives not in pods.md5: {', '.join(unknown[:6])}")
    if failures and not args.skip_verify:
        for f in failures:
            print("  " + f, file=sys.stderr)
        print("verification failed; pass --skip-verify to override", file=sys.stderr)
        return 3

    cmd = [sys.executable, str(POD_TOOL), str(src), "-o", str(out.parent), "--manifest", str(out.parent / "manifest.json")]
    # pod.py writes into <out>/<class>/..., so point it at a staging dir and
    # then flatten. Simpler: let it write under the parent and rename.
    staging = out.parent / "_intake"
    cmd = [sys.executable, str(POD_TOOL), str(src), "-o", str(staging), "--manifest", str(out.parent / "manifest.json")]
    print("extracting...")
    rc = subprocess.call(cmd)
    if rc != 0:
        print("extractor failed", file=sys.stderr)
        return 4
    for child in staging.iterdir():
        target = out / child.name
        if target.exists():
            shutil.rmtree(target)
        shutil.move(str(child), str(target))
    staging.rmdir()

    if args.bake_ui:
        subprocess.call([sys.executable, str(ROOT / "tools" / "art_convert.py"), "--in", str(out), "--out", str(out.parent / "ui")])

    install = {
        "source": str(src),
        "archives": len(archives),
        "bytes": total,
        "verified": not failures,
        "unknown_archives": unknown,
        "baked_ui": bool(args.bake_ui),
    }
    (out.parent / "install.json").write_text(json.dumps(install, indent=1))
    print(f"done: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
