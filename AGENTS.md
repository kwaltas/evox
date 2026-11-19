# evox — agent instructions

Clean-room source port of 4x4 Evolution 1/2 in Rust (macroquad). The retail
binaries and data are on this machine and are the specification.

## Read first

* `docs/PORT_REFERENCE.md` — reference paths, `.TRK`/terrain/audio formats,
  physics model, guardrails, runbook.
* `docs/ARCHITECTURE.md`, `docs/PHYSICS.md`, `docs/QA.md`, `docs/MODDING.md`.
* `tools/evo2_tracks.py` — imports the Evo 2 levels (60 on this `evo2-only`
  intake: the 15 Evo 1 tracks Evo 2 re-ships plus 45 exclusives) and reports the `.SIT` `*** Course ***` gates and
  `*** Paths ***` splines each level carries; both are parsed at runtime by
  `terrain::parse_course`.

## Non-negotiables

1. **Ground truth before code.** Find the constant, offset, curve or file name
   in the reference trees or the decompile, and cite it in a comment. No
   invented physics, no hand-tuned fudge factors without saying what they
   replace.
2. **Reference trees are read-only.** `./4x4data/`, `./4x4e2/`, `./4x4e/`,
   `./4x4re/`, `./craft-anything/`, `./4x4evo2-work/`.
3. **No placeholder UI.** Use the extracted art (`assets/ui2`, `EDBTN*`,
   `CURSOR`, `4X4FONT`) via `ui::Ui` and `classic::menu_frame`. A flat rect as a
   button is a bug.
4. **No silent fallbacks.** Missing heightmap, texture list, model or sound
   must fail visibly. `MATT.RAW` exists as both a 131 072-byte heightmap and a
   65 536-byte preview — check the size.
5. **Be honest about coverage.** The `evo2-only` extraction has 60 levels (15
   Evo 1 tracks re-shipped by Evo 2 plus 45 Evo 2 exclusives) and 150 vehicle
   definitions, all levels with real heightmaps. The Evo 1 test map `MATT` is
   not part of this intake.
   `.RTD` files are all zeros, not heightmaps.
6. **Imperial in, SI simulated.** Convert once at load with `vehicle::FT`,
   `FT_LB`, `LB`.
7. **Fixed 60 Hz step** (`physics::DT`); no `get_frame_time()` inside the
   vehicle model.
8. **Every fix gets a test** that fails on the old behaviour, and
   `docs/PORT_REFERENCE.md` is updated when a format detail is discovered.
9. **Never sign off on UI, camera, or physics changes using unit tests alone;
   always run `EVOX_AUTODRIVE=1` + `EVOX_SHOT` and visually inspect the
   rendered frames and telemetry.**

## Before you call it done

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release
```

All four must pass. For physics, AI or camera changes also run the headless
retail drive and compare against the table in `docs/PORT_REFERENCE.md` §11:

```bash
cargo test --release -p evox drive_report -- --ignored --nocapture
```

Unit tests on flat or synthetic ground passed while the live game had the
heading read from the pitch angle, inverted steering and trucks skating on
their roofs — see §11 for the list.

## Code style

* Follow the surrounding module: doc comments explain *why* with a reference,
  tests live in a `#[cfg(test)] mod tests` at the bottom.
* Units in names (`_m`, `_lb`, `_nm`), no bare magic numbers.
* Keep public API used by `app.rs` stable unless you update every caller.
* Rust 2021, `cargo fmt` clean.
