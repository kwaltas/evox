# Legal

## Clean-room boundary

EvoX ships **code and derived metadata only**. No mesh, texture, sound, video,
or level data is redistributed. The user's own disc supplies those.

What is derived and therefore kept in git:

- `assets/pods_evo2.md5` — 52 MD5 digests of the retail 4x4 Evolution 2 archives
- `assets/vehicles/_selftest.trk` — one vehicle definition, as a parser fixture
- format grammar (documented in `docs/MODDING.md`)

## Trademarks

"4x4 Evolution" and manufacturer names belong to their owners. The port is
unaffiliated and unendorsed. The title screen credits Terminal Reality and
the original year.

## First boot

First boot uses the built-in Rust extractor (or `tools/setup_evo2.py` for
scripted intake):

1. reads only the POD archives from the ISO or folder you select,
2. checks each against the embedded `pods_evo2.md5` manifest (mismatches
   require explicit confirmation),
3. writes a new tree under the per-user data directory,
4. records the source path in `install.json` for support.

No network access, no telemetry to a third party. Nothing is uploaded.

## Distribution

Redistributing the compiled binary is fine under MIT. If you also redistribute
an `assets/extracted` folder, that folder is the copyrighted part — mark it as
game data, not code, in your bundle.

## Engine provenance

The implementation is original Rust. Behaviour is calibrated against observed
output and published format documentation, not decompiled source. Where a
constant comes from the retail data (spring rates, torque curves), it is loaded
from the user's files rather than hardcoded, so the legal surface stays thin.

## Contributions

By contributing you agree to license code under MIT. Do not paste GPL
decompiler output into the tree without attribution; note the tool (Ghidra,
IDA) and the function in the PR.
