#!/usr/bin/env python3
"""
evo2_ui.py -- dump the interface archive into a reviewable UI atlas.

Extracts UI.POD (or a folder), keeps the interface-relevant ART, emits PNGs
plus `ui.json` describing each element's intended role. Used to audit menu
fidelity and to feed the shop tabs without re-deriving names by hand.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import pod  # noqa: E402
from art_convert import convert  # noqa: E402

UI_PODS = {"ui.pod", "uitrack.pod", "startup.pod", "parts.pod"}
ROLE = [
    (re.compile(r"^ART\\EDBTN(HLP|TM|LD|AE|BE|CE|CI|EP|PE|SV|TD|TE|TST|TM|VE|OI|NM)$"), "button"),
    (re.compile(r"^ART\\NAV"), "nav"),
    (re.compile(r"^ART\\ARROW"), "arrow"),
    (re.compile(r"^ART\\(GARAGE|SHOWROOM|TRUCK\d)"), "garage"),
    (re.compile(r"^ART\\VMU_"), "hud"),
    (re.compile(r"^ART\\(FNT|4X4FONT)"), "font"),
    (re.compile(r"^ART\\(LOADING|EVOLOGO|TRILOGO|LOCHEV|LOGOMIT|LOGOFORD)"), "branding"),
    (re.compile(r"^ART\\(CKBOX|CURSOR|LCD_EVO|TICK|NEEDLE|ODOM)"), "widget"),
    (re.compile(r"^ART\\(.*SKY|CLOUD|SUN|RAIN|SNOW)"), "sky"),
]


def role_for(name: str) -> str:
    for rx, role in ROLE:
        if rx.match(name.upper()):
            return role
    return "art"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("source", help="POD archive or directory containing PODs")
    ap.add_argument("-o", "--out", default="assets/ui", help="output root")
    ap.add_argument("--all", action="store_true", help="keep every ART entry, not just interface art")
    args = ap.parse_args(argv)

    src = Path(args.source)
    files = [src] if src.is_file() else [p for p in src.rglob("*") if p.suffix.lower() == ".pod"]
    files = [p for p in files if p.name.lower() in UI_PODS or args.all]
    if not files:
        print("no interface archives found", file=sys.stderr)
        return 2

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    index = {"generator": "evo2_ui.py", "elements": [], "skipped": []}

    for path in files:
        try:
            arc = pod.parse(path)
            buf = arc.path.read_bytes()
        except pod.PodError as exc:
            print(f"! {path.name}: {exc}", file=sys.stderr)
            continue
        stems: dict[str, str] = {}
        for entry in arc.entries:
            upper = entry.name.upper()
            if not upper.startswith("ART\\") or not upper.endswith((".RAW", ".ACT", ".OPA")):
                continue
            stem = Path(entry.name.replace("\\", "/")).stem
            dest = out / role_for(entry.name) / f"{stem}.png"
            dest.parent.mkdir(parents=True, exist_ok=True)
            data = pod.payload(buf, entry)
            dest.with_suffix(entry.name[-4:].lower()).write_bytes(data)
            stems[stem] = role_for(entry.name)

        for stem, role in stems.items():
            dest = out / role / f"{stem}.png"
            act = dest.with_suffix(".act")
            opa = dest.with_suffix(".opa")
            raw = dest.with_suffix(".raw")
            if not raw.exists():
                continue
            if not act.exists():
                index["skipped"].append({"name": stem, "reason": "no palette"})
                continue
            convert(raw.name, dest, act, opa if opa.exists() else None)
            index["elements"].append({
                "stem": stem,
                "role": role,
                "group": "ui",
                "path": str(dest.relative_to(out)),
            })
    (out / "ui.json").write_text(json.dumps(index, indent=1))
    print(f"{len(index['elements'])} elements -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
