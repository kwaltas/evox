#!/usr/bin/env python3
"""
setup_evo2.py -- first-boot asset intake from the user's own copy of 4x4
Evolution 2.

Evox is 4x4 Evo 2 only: this is the single intake tool. Point it at your
retail ISO, a mounted CD, the installer's `.cab` pair, or an already
extracted folder of POD files, and it rebuilds `assets/extracted/`.

Pipeline:
  1. resolve the source (mount the ISO on macOS, unshield the cabinet)
  2. find every POD2 archive and check its header
  3. extract through tools/pod.py into `assets/extracted/`
  4. optionally bake UI art to PNG, and record a local checksum manifest

Nothing copyrighted enters git. Exit codes: 0 ok, 1 bad source,
2 no archives, 3 verification failure, 4 extraction failure.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
POD_TOOL = ROOT / "tools" / "pod.py"
EXPECTED = ROOT / "assets" / "pods_evo2.md5"

# Archives every retail 4x4 Evo 2 disc carries. Missing ones mean either an
# incomplete rip or data from the wrong game.
REQUIRED = ("startup.pod", "truck.pod", "ui.pod", "sound.pod")


def md5(path: Path) -> str:
    h = hashlib.md5()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def load_expected() -> dict[str, str]:
    out: dict[str, str] = {}
    if not EXPECTED.exists():
        return out
    for line in EXPECTED.read_text().splitlines():
        parts = line.split()
        if len(parts) == 2:
            out[parts[1].lower()] = parts[0].lower()
    return out


def find_archives(src: Path) -> list[Path]:
    """POD2 archives under `src`. Extension case varies on retail discs."""
    if src.is_file():
        return [src] if src.suffix.lower() == ".pod" else []
    out = []
    for p in src.rglob("*"):
        if not p.is_file() or p.suffix.lower() != ".pod" or p.stat().st_size <= 64:
            continue
        with p.open("rb") as f:
            if f.read(4) in (b"POD2", b"POD\x00"):
                out.append(p)
    return sorted(out)


def run(cmd: list[str]) -> int:
    return subprocess.call(cmd)


def mount_iso(iso: Path) -> Path | None:
    """Attach a retail ISO read-only and return its mount point."""
    if sys.platform != "darwin":
        print(f"note: mount {iso} yourself and pass the mount point", file=sys.stderr)
        return None
    out = subprocess.run(
        ["hdiutil", "attach", "-readonly", "-nobrowse", str(iso)],
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        print(out.stderr.strip(), file=sys.stderr)
        return None
    for line in out.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) >= 3 and parts[-1].startswith("/Volumes/"):
            return Path(parts[-1])
    return None


def unmount(point: Path) -> None:
    if sys.platform == "darwin":
        subprocess.call(["hdiutil", "detach", str(point), "-quiet"])


def expand_cab(src: Path, tmp: Path) -> Path | None:
    """Retail discs ship the game inside an InstallShield cabinet.

    A disc has several cabinets (`_sys1`, `_user1`, `data1`); only `data1`
    holds the game. Try them largest-first and keep the first one that
    actually yields POD archives.
    """
    hdrs = [p for p in src.iterdir() if p.suffix.lower() == ".hdr"] if src.is_dir() else []
    if not hdrs:
        return None
    unshield = shutil.which("unshield")
    if not unshield:
        print("error: this disc stores the game in a .cab; install unshield", file=sys.stderr)
        return None
    # `data*.hdr` first, then the rest by size (the game cabinet is the big one).
    hdrs.sort(key=lambda p: (not p.stem.lower().startswith("data"), -p.stat().st_size))
    for hdr in hdrs:
        dest = tmp / hdr.stem
        print(f"unpacking cabinet {hdr.name} (unshield)...")
        if run([unshield, "x", "-d", str(dest), str(hdr)]) != 0:
            continue
        if find_archives(dest):
            return dest
        shutil.rmtree(dest, ignore_errors=True)
    return None


def human(n: int) -> str:
    return f"{n / 1048576:.1f} MiB"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--source", help="ISO, mounted CD, folder with .cab files, or folder of PODs")
    ap.add_argument("--out", default=str(ROOT / "assets" / "extracted"))
    ap.add_argument("--yes", action="store_true", help="no prompts")
    ap.add_argument("--force", action="store_true", help="replace an existing asset tree")
    ap.add_argument("--skip-verify", action="store_true", help="accept unknown or checksum-mismatched archives")
    ap.add_argument("--record-md5", action="store_true", help="write assets/pods_evo2.md5 from this copy")
    ap.add_argument("--bake-ui", action="store_true", help="also convert UI art to PNG")
    args = ap.parse_args(argv)

    source = args.source
    if not source and not args.yes:
        source = input("Path to your 4x4 Evolution 2 ISO, CD or extracted folder: ").strip()
    if not source:
        print("error: --source is required", file=sys.stderr)
        return 1
    src = Path(source).expanduser()
    if not src.exists():
        print(f"error: {src} does not exist", file=sys.stderr)
        return 1

    tmp = Path(tempfile.mkdtemp(prefix="evox-evo2-"))
    mounted: Path | None = None
    try:
        scan = src
        if src.is_file() and src.suffix.lower() == ".iso":
            print(f"mounting {src.name}...")
            mounted = mount_iso(src)
            if mounted is None:
                print("error: could not mount the ISO", file=sys.stderr)
                return 1
            scan = mounted

        archives = find_archives(scan)
        if not archives:
            where = expand_cab(scan, tmp / "cab")
            if where is None:
                print("error: no POD2 archives found (mount the CD or point at the install folder)", file=sys.stderr)
                return 2
            archives = find_archives(where)
            scan = where
            if not archives:
                print("error: the cabinet has no POD archives", file=sys.stderr)
                return 2

        names = {p.name.lower() for p in archives}
        missing = [n for n in REQUIRED if n not in names]
        if missing:
            print(f"warning: expected archives missing: {', '.join(missing)}")
            print("warning: this does not look like a complete 4x4 Evolution 2 disc")

        out = Path(args.out).expanduser()
        if out.exists() and any(out.iterdir()):
            if not args.force:
                print(f"output {out} already has data; pass --force to replace")
            else:
                shutil.rmtree(out)
        out.mkdir(parents=True, exist_ok=True)

        expected = load_expected()
        total = sum(p.stat().st_size for p in archives)
        print(f"found {len(archives)} POD2 archives, {human(total)} in {scan}")
        if not args.yes:
            print(f"extracting to {out}")
            if input("continue? [Y/n] ").strip().lower() in ("n", "no"):
                return 1

        failures: list[str] = []
        unknown: list[str] = []
        recorded: list[str] = []
        for p in archives:
            got = md5(p)
            recorded.append(f"{got}  {p.name}")
            if args.skip_verify or not expected:
                continue
            want = expected.get(p.name.lower())
            if want is None:
                unknown.append(p.name)
            elif got != want:
                failures.append(f"{p.name}: md5 {got} != {want}")
        if unknown:
            print(f"warning: {len(unknown)} archives not in pods_evo2.md5")
        if failures:
            for f in failures:
                print("  " + f, file=sys.stderr)
            if not args.skip_verify:
                print("verification failed; pass --skip-verify to override", file=sys.stderr)
                return 3

        staging = tmp / "intake"
        print("extracting...")
        cmd = [sys.executable, str(POD_TOOL), str(scan), "-o", str(staging), "--manifest", str(out.parent / "manifest.json")]
        if run(cmd) != 0:
            print("extractor failed", file=sys.stderr)
            return 4
        for child in staging.iterdir():
            target = out / child.name
            if target.exists():
                shutil.rmtree(target)
            shutil.move(str(child), str(target))

        if args.bake_ui:
            run([sys.executable, str(ROOT / "tools" / "art_convert.py"), "--in", str(out), "--out", str(out.parent / "ui")])

        if args.record_md5:
            EXPECTED.write_text("\n".join(sorted(recorded)) + "\n")
            print(f"wrote {EXPECTED}")

        install = {
            "game": "4x4-evo-2",
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
    finally:
        if mounted is not None:
            unmount(mounted)
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
