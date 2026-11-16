#!/usr/bin/env python3
"""
telemetry/delta.py -- compare original-binary telemetry with EvoX output.

Both inputs are JSON lines with at least `t` and `speed`. Reports per-channel
RMSE, peak error, and a signed tuning suggestion (raise/lower) so Phase 10A
calibration is a loop rather than eyeballing.

Exit code is 1 when speed RMSE exceeds --tolerance.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

CHANNELS = ("speed", "rpm", "slip", "suspension")


def load(path: Path) -> list[dict]:
    out = []
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            out.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return out


def align(a: list[dict], b: list[dict]) -> list[tuple[dict, dict]]:
    b.sort(key=lambda r: r.get("t", 0.0))
    pairs = []
    j = 0
    for row in a:
        t = row.get("t", 0.0)
        while j + 1 < len(b) and abs(b[j + 1].get("t", 0.0) - t) <= abs(b[j].get("t", 0.0) - t):
            j += 1
        if b and abs(b[j].get("t", 0.0) - t) < 0.05:
            pairs.append((row, b[j]))
    return pairs


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("reference", type=Path)
    ap.add_argument("candidate", type=Path)
    ap.add_argument("--tolerance", type=float, default=1.5, help="max speed RMSE")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args(argv)

    pairs = align(load(args.reference), load(args.candidate))
    if len(pairs) < 20:
        print(f"not enough aligned samples ({len(pairs)}); check clocks and --hz", file=sys.stderr)
        return 2

    report = {"samples": len(pairs), "channels": {}}
    for ch in CHANNELS:
        errs = []
        for ref, cand in pairs:
            if ch in ref and ch in cand:
                errs.append(cand[ch] - ref[ch])
        if not errs:
            continue
        rmse = math.sqrt(sum(e * e for e in errs) / len(errs))
        bias = sum(errs) / len(errs)
        report["channels"][ch] = {
            "rmse": round(rmse, 4),
            "bias": round(bias, 4),
            "peak": round(max(errs, key=abs), 4),
            "suggest": "raise" if bias < 0 else "lower",
        }

    if args.json:
        print(json.dumps(report, indent=1))
    else:
        print(f"{len(pairs)} aligned samples")
        for ch, r in report["channels"].items():
            print(f"{ch:>10}: rmse {r['rmse']:.3f}  bias {r['bias']:+.3f}  peak {r['peak']:+.3f}  -> {r['suggest']}")

    speed = report["channels"].get("speed")
    return 1 if speed and speed["rmse"] > args.tolerance else 0


if __name__ == "__main__":
    sys.exit(main())
