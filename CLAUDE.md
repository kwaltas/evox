# CLAUDE.md

Short entry point for Claude Code sessions. The full rules live in
[`AGENTS.md`](AGENTS.md); the specification lives in
[`docs/PORT_REFERENCE.md`](docs/PORT_REFERENCE.md).

## What this repo is

A clean-room Rust port of 4x4 Evolution 1/2. Retail data, the engine source and
a full Ghidra decompile are on this machine and are the ground truth for every
format, constant and behaviour. Clean-room means: no copied source, but every
number traceable to a reference.

## The five things that matter most

1. Cite ground truth (`docs/PORT_REFERENCE.md` has the map and the `.TRK`
   offset table) before writing physics, format or UI code.
2. Reference trees are read-only, including
   `./4x4evo2-work/decomp_4x42_full.c`.
3. No placeholder UI and no silent fallbacks — missing assets must be loud.
4. Convert imperial file units once at load; the sim is SI at a fixed 60 Hz.
5. Finish with `cargo fmt --check`, `cargo test --workspace`, `cargo clippy
   --workspace --all-targets -- -D warnings`, and `cargo build --release`.
6. **Never sign off on UI, camera, or physics changes using unit tests alone;
   always run `EVOX_AUTODRIVE=1` + `EVOX_SHOT` and visually inspect the
   rendered frames and telemetry.**

## Layout

`game/src/` (bin `evox`): `physics.rs`, `terrain.rs`, `assets.rs`, `audio.rs`,
`vehicle.rs`, `render.rs`, `ui/`. `server/` is the master server,
`leaderboard/` the times service, `tools/` the asset pipeline, `docs/` the
written spec.
