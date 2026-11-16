#!/usr/bin/env python3
"""
telemetry/bridge.py -- read live physics values from the original 4x4 Evo 2
binary inside a VM and stream them as JSON lines.

Two strategies:

  --scan   search committed memory for the float signature of a known speed
           value (slow but needs no offsets)
  --offsets offsets.json   module base + static offsets (fast, stable)

Only Windows guests are supported (ReadProcessMemory). Run inside the VM.

offsets.json:
  {"module": "4x4.exe", "fields": {"speed": 0x1A2B3C, "rpm": 0x1A2B40, ...},
   "scale": {"speed": 0.01, "rpm": 1.0}}

Windows path conventions are used because the target is a Win32 binary running
under Wine or a Windows VM; run this script with the Windows Python build, or
under Wine's python.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wt
import json
import struct
import sys
import time
from pathlib import Path

PROCESS_VM_READ = 0x0010
PROCESS_QUERY_INFORMATION = 0x0400
TH32CS_SNAPMODULE = 0x00000008


class MODULEENTRY32(ctypes.Structure):
    _fields_ = [
        ("dwSize", wt.DWORD),
        ("th32ModuleID", wt.DWORD),
        ("th32ProcessID", wt.DWORD),
        ("GlblcntUsage", wt.DWORD),
        ("ProccntUsage", wt.DWORD),
        ("modBaseAddr", ctypes.POINTER(ctypes.c_byte)),
        ("modBaseSize", wt.DWORD),
        ("hModule", wt.HMODULE),
        ("szModule", ctypes.c_char * 256),
        ("szExePath", ctypes.c_char * 260),
    ]


class Bridge:
    def __init__(self, pid: int):
        self.k32 = ctypes.windll.kernel32
        self.pid = pid
        self.handle = self.k32.OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, False, pid)
        if not self.handle:
            raise OSError(f"OpenProcess({pid}) failed: {ctypes.GetLastError()}")
        self.base = self._module_base()

    def _module_base(self) -> int:
        snap = self.k32.CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, self.pid)
        entry = MODULEENTRY32()
        entry.dwSize = ctypes.sizeof(MODULEENTRY32)
        base = 0
        if self.k32.Module32First(snap, ctypes.byref(entry)):
            while True:
                if entry.szModule.lower().endswith(b"4x4.exe"):
                    base = ctypes.cast(entry.modBaseAddr, ctypes.c_void_p).value or 0
                    break
                if not self.k32.Module32Next(snap, ctypes.byref(entry)):
                    break
        self.k32.CloseHandle(snap)
        return base

    def read(self, addr: int, n: int) -> bytes | None:
        buf = ctypes.create_string_buffer(n)
        got = ctypes.c_size_t()
        ok = self.k32.ReadProcessMemory(self.handle, ctypes.c_void_p(addr), buf, n, ctypes.byref(got))
        return buf.raw[: got.value] if ok else None

    def read_f32(self, addr: int) -> float | None:
        raw = self.read(addr, 4)
        return struct.unpack("<f", raw)[0] if raw and len(raw) == 4 else None


def scan_for(bridge: Bridge, needle: float, span: int = 0x2000000) -> int | None:
    """Find the first occurrence of a float in the module's mapped range."""
    target = struct.pack("<f", needle)
    step = 1 << 20
    for off in range(0, span, step):
        block = bridge.read(bridge.base + off, step + 4)
        if not block:
            continue
        i = block.find(target)
        if i >= 0:
            return bridge.base + off + i
    return None


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--pid", type=int, required=True)
    ap.add_argument("--hz", type=float, default=60.0)
    ap.add_argument("--json", default="offsets.json")
    ap.add_argument("--scan", type=float, help="scan for this speed value instead of using offsets")
    ap.add_argument("--out", default="-")
    args = ap.parse_args(argv)

    bridge = Bridge(args.pid)
    if not bridge.base:
        print("4x4.exe module not found in process", file=sys.stderr)
        return 2

    fields: dict[str, int] = {}
    scale: dict[str, float] = {}
    if args.scan is not None:
        found = scan_for(bridge, args.scan)
        if found is None:
            print(f"value {args.scan} not found in module range", file=sys.stderr)
            return 3
        fields["speed"] = found
        print(f"found speed at +0x{found - bridge.base:x}", file=sys.stderr)
    else:
        cfg = json.loads(Path(args.json).read_text())
        fields = {k: int(v) for k, v in cfg["fields"].items()}
        scale = {k: float(v) for k, v in cfg.get("scale", {}).items()}

    sink = sys.stdout if args.out == "-" else open(args.out, "w", encoding="utf-8")
    interval = 1.0 / max(args.hz, 1.0)
    t0 = time.time()
    try:
        while True:
            rec = {"t": round(time.time() - t0, 4)}
            for name, off in fields.items():
                v = bridge.read_f32(bridge.base + off)
                if v is not None:
                    rec[name] = v * scale.get(name, 1.0)
            sink.write(json.dumps(rec) + "\n")
            sink.flush()
            time.sleep(interval)
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
