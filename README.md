**Disclaimer:** *Evox is an unofficial, open-source engine re-implementation for 4x4 Evolution 1 & 2. This repository does NOT contain any original game assets, 3D models, audio, or copyrighted code. To play, users must legally own a copy of the original games and provide their own retail `.POD` files. Evox is not affiliated with Terminal Reality, GodGames, or any licensed vehicle manufacturer.*

---

# Evox

A clean-room, from-scratch reimplementation of the **4x4 Evolution** engine
(Terminal Reality, 2000) and the **4x4 Evolution 2** track/vehicle formats,
written in Rust on [macroquad](https://macroquad.rs/).

Evox ships **code only**. It reads the original `.POD` archives from your own
legally-owned copy of the games and rebuilds a working asset tree locally
("bring your own data"). Nothing extracted, decoded or decompiled from the
retail games is committed to this repository — no textures, models, audio,
`.POD` files or decompiled C++.

```
┌──────────────────┐   tools/setup_wizard.py   ┌───────────────────┐
│ your own 4x4 Evo │ ────────────────────────► │ assets/extracted  │
│ CD/ISO or folder │   local, MD5-checked      └─────────┬─────────┘
└──────────────────┘                                      │
                                               ┌──────────▼─────────┐
                                               │ evox (Rust/macroquad)│
                                               └─────────────────────┘
```

## Features

- Fixed-step (60 Hz) vehicle dynamics: sprung mass, Pacejka tyres, 4WD,
  low-range transfer case, damage-free suspension travel, hull/prop collision.
- Retail `.TRK` / `.SMF` / `.RAW` / `.ACT` / `.TEX` / `.LVL` / `.SIT` parsers.
- Height-field terrain with per-cell materials, scenery props and colliders.
- AI rivals driven by recorded racing lines, lap/checkpoint timing, minimap.
- Classic HUD, garage, parts shop, career series and quick race, using only
  artwork extracted from *your* copy of the game.
- Dynamic engine audio, surface SFX and music (when your data provides it).
- UDP netcode, master server and a signed time-trial leaderboard (alpha).
- In-game track editor foundation (free-fly camera, terrain brush, overlay save).

## How to Play (Asset Extraction)

You need **your own retail copies** of 4x4 Evolution 1 and/or 4x4 Evolution 2.
Evox does not download, bundle or ship any game data.

1. **Evo 1 PODs.** Create a `4x4data/out/Program_Executable_Files/` folder at
   the workspace root and place your retail **4x4 Evolution 1** `.POD` files
   inside.

2. **Evo 2 PODs.** Create a `4x4evo2-work/pods/` folder at the workspace root
   and place your retail **4x4 Evolution 2** `.POD` files inside (the tools
   expect the unpacked pod tree, e.g. `LEVELS/`, `DATA/`, `WORLD/`, `ART/`).

3. **Extract Evo 1.** Run:

   ```bash
   python3 tools/setup_wizard.py --source 4x4data/out/Program_Executable_Files
   ```

   This verifies each archive and extracts the Evo 1 base assets into
   `assets/extracted/`. Nothing is written back to your retail files.

4. **Merge Evo 2.** Extract and merge the Evo 2 vehicles and tracks safely
   (Evo 1 files always win on name collisions):

   ```bash
   python3 tools/evo2_trucks.py
   python3 tools/evo2_tracks.py
   ```

5. **Build and run:**

   ```bash
   cargo run --release
   ```

> Everything under `assets/extracted/`, `4x4data/`, `4x4e2/`, `4x4evo2-work/`
> and the other reference folders is **git-ignored**. Never commit extracted
> retail data.

### Requirements

- Rust (stable, 2021 edition) and Cargo.
- A GPU with OpenGL 3+ or Metal (macOS/Windows/Linux).
- Python 3 for the extraction tools (`pip install pillow` for art conversion).
- No network access is required to play single-player.

## Controls

| Action | Key |
|---|---|
| Throttle / brake | W / S or Up / Down |
| Steer | A / D or Left / Right |
| Handbrake | Space |
| Shift up / down | E / Q |
| Transfer case (2WD / 4WD-HI / 4WD-LO) | 4 |
| Rescue (back on the course) | H |
| Pause | P |
| Menus | Arrows, Enter, Esc, Tab |
| Save profile | F1 |
| Debug HUD | F3 |
| Track editor (from a race) | F12 |
| Editor: fly / look / faster | WASD + Space/Ctrl, right-mouse drag, Shift |
| Editor: save overlay | F5 |

## Building

```bash
cargo build --release      # binary at target/release/evox
cargo run --release        # build and launch
```

The workspace also contains the master server (`server/`) and leaderboard
(`leaderboard/`) crates. `cargo build` builds the client by default; use
`cargo build --workspace` for everything.

## Project layout

```
game/        client crate: simulation, rendering, UI, audio, net client, editor
server/      master server (lobby registry)
leaderboard/ signed time-trial service
tools/       POD extraction, Evo 2 import, art conversion, diagnostics
docs/        architecture, physics, formats, QA, modding, legal notes
assets/      local extraction target (git-ignored; bring your own data)
```

## Diagnostics

```bash
cargo test --workspace
cargo run -p evox -- --self-test 0
```

`--self-test 0` parses the parts catalog, vehicles, tracks and physics core;
exit code is the failure count. Tests that need retail data skip automatically
when `assets/extracted/` is absent, so a source-only checkout still builds and
tests clean.

## Legal

Code is MIT-licensed (see `LICENSE`). Game data, vehicles, tracks, audio and
trademarks remain the property of their respective owners (Terminal Reality,
GodGames and the licensed manufacturers). Evox is an independent, unofficial
project and is not affiliated with or endorsed by any of them. You are
responsible for owning a legal copy of the games you extract data from.

## Acknowledgements

Thanks to the 4x4 Evolution modding community and to the authors of the
open-source libraries this project builds on (`macroquad`, `glam`, `serde`,
`sha2` and friends).
