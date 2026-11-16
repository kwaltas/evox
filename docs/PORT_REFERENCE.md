# Port reference

Ground truth for everything evox claims to reproduce, where it lives on this
machine, and the rules for using it. Read this before touching
`game/src/physics.rs`, `audio.rs`, `terrain.rs`, `assets.rs` or the UI.

The retail game is 4x4 Evolution 1/2 (Terminal Reality / Gathering). evox is a
clean-room source port: geometry, formats, constants and behaviour come from
the decompile and the shipped asset set, never from guesswork.

## 1. Reference map

All reference trees are **read-only**. Never edit them, never "fix" them, never
copy a whole file in as a shortcut.

| Path | What it is | Use it for |
|---|---|---|
| `./4x4data/` | Retail install data: `assets/extracted/{track,model,texture,unknown,ui2,audio}` | Formats, track/vehicle/UI/sound art, values |
| `./4x4e2/` | 4x4 Evo 2 TRX engine source (C++) | Framework semantics: sounds, renderer, INI/`.TRK` stream reading, units |
| `./4x4e/` | 4x4 Evo 1 TRX engine source | Older revisions of the same engine; diff to date a behaviour |
| `./4x4re/` | Reverse-engineering notes for the same binaries | Struct layouts, call graphs, symbol names |
| `./craft-anything/` | Scratch workspace of the decompilation effort | Extra dumps and tooling |
| `<reference>/decomp_4x42_full.c` | Ghidra decompile of the retail executable, 238 200 lines | The only source for `.TRK` runtime layout and gameplay arithmetic |
| `<reference>/offroad4x4/app/src/` | Finished Rust prototype that drove all 30 retail tracks | Proven vehicle model, camera, audio, terrain usage |
| `<reference>/INTERFAZ_ORIGINAL.md` | Menu tree and layout notes taken from the retail UI | Screen composition, label wording |
| `.` | This port | — |

`<reference>` stands for a local checkout of the reference trees, kept outside
this repository.

### What is *not* in these trees

* `4x4e2/Source/TRX/Vehicles.Sounds.cxx` is a stub (the retail engine calls its
  platform sound layer directly). Engine audio comes from the prototype's
  `audio.rs` plus the bank's own formats, not from that file.
* `4x4e2`/`4x4e` contain no gameplay physics. `TRX` is the framework and the
  renderer back ends only. Vehicle dynamics come from the decompile and the
  prototype.
* The asset set ships **16 Evo 1 levels** (`track/track/LEVELS/*.LVL`, 15
  heightmaps in `texture/texture/DATA` plus `MATT.RAW` in `texture/ui/DATA`)
  **and 45 imported Evo 2-exclusive levels** brought in by
  `tools/evo2_tracks.py` (61 total; `ALASKA1-5`, `ELNORTE1-5`, `OBSPARK1-5`,
  `RIVER1-5`, `SKULL1-5`, `WASH1-5`, …). Every one carries a real 131 072-byte
  heightmap. The 16 Evo 1 `.RTD` files are all zeros and are *not* heightmaps.
  "All 30 tracks" describes the retail Evo 1 disc set; the extracted tree is 61
  levels because it also holds the Evo 2 exclusives.

## 2. Guardrails

1. **Consult ground truth first.** Before adding a formula, constant, unit
   conversion or file offset, find it in the table below or in the decompile.
   Cite the file and line in a comment.
2. **No synthetic formulas.** If the original has a curve, table or ratio, use
   it. Adding a hand-tuned `* 1.4 fudge` needs a comment saying what it
   replaces.
3. **No placeholder UI.** Screens use the extracted art (`ui2/bg_*`,
   `EDBTN*` strips, `CURSOR`, the `4X4FONT` sheet) and the original's layout.
   A flat coloured rectangle as a button is a bug.
4. **No silent fallbacks.** A missing heightmap must fail loudly; a missing
   texture list must not quietly become a grey plane. `MATT.RAW` exists twice
   (131 072-byte heightmap in `texture/ui/DATA`, 65 536-byte preview in
   `texture/ui/ART`) and size decides which is which.
5. **Units are converted once, at load.** Files are imperial (ft, lb); the
   simulation is SI. `vehicle::FT`, `FT_LB`, `LB` exist for exactly this.
6. **Fixed step, no render coupling.** Physics runs at `physics::DT`
   (1/60 s). Never read `get_frame_time()` inside the vehicle model.
7. **Add a test with the fix.** Every bug fixed in physics/audio/terrain gets a
   test that fails on the old code.
8. **Do not delete `/assets`** (git-ignored); it is the pipeline output.

## 3. Module ↔ source cross-reference

| evox module | Ground truth |
|---|---|
| `game/src/vehicle.rs` | `decomp_4x42_full.c` `FUN_00434c20` (text → struct reader) and the writer at 202 100-202 420; `4x4e2/Source/TRX/IO.Streams.*`, `Assets.Basic.hxx` for key/stream rules |
| `game/src/physics.rs` | Offset table §4, prototype `app/src/car.rs` + `tuning.rs`, `4x4e2/Source/TRX/Mathematics.Basic.hxx` |
| `game/src/audio.rs` | Bank WAV formats in `assets/extracted/audio/sfx/SOUND`, prototype `app/src/audio.rs`, `4x4e2/Source/TRX/Sounds.Samples.*`, `Sounds.Controllers.Wave.*` |
| `game/src/terrain.rs` | `.LVL`/`.RAW`/`.CLR`/`.SDW`/`.TEX`/`.SIT` in `assets/extracted/track`, `4x4e2/Source/TRX/Renderer.*` for chunking |
| `game/src/assets.rs` | `4x4e2/Source/TRX/Assets.Basic.hxx`, `Renderers.Basic.Textures.hxx` for texture/alpha rules |
| `game/src/render.rs`, `smf.rs` | `4x4e2/Source/TRX/Renderers.Basic.Vertexes.hxx`, `Renderer.Module.*`, `*.SMF` in `assets/extracted/model` |
| `game/src/ui/*`, `classic.rs` | `INTERFAZ_ORIGINAL.md`, `ui2/*.png`, `4x4e2/Source/TRX/Editor.*` |
| `game/src/ai.rs`, `race.rs` | `4x4e2/Source/TRX/Objects.*`, AI line files under `assets/extracted/data/ai` |
| `game/src/net.rs`, `../server`, `../leaderboard` | No retail counterpart; the master/leaderboard protocol is ours (documented in `MULTIPLAYER.md`) |

## 4. `.TRK` layout (decompiled)

Text file, CRLF line endings, `key` line followed by a value line. The loader
`FUN_00434c20` parses it into a fixed struct; the writer at 202 100-202 420
dumps that struct back out, so it doubles as the field table.

The reader indexes the struct as a **`float[]`**, so a reader index `N`
corresponds to struct byte offset `N*4`. Both columns are given; the writer
offset is authoritative.

| Key | Reader word | Byte | Type | Unit |
|---|---|---|---|---|
| `sc[].pt` | 0x14 | 0x050 | float[3] | ft |
| `faxle.rtire.CF` | 0x3a | 0x0e8 | float | – |
| `faxle.rtire.spring_arm` | 0x43 | 0x10c | float | in |
| `faxle.ltire.CF` / `spring_arm` | 0x85 / 0x8e | 0x214 / 0x238 | float | – / in |
| `faxle.maxangle` | 0xb1 | 0x2c4 | float | rad |
| `faxle.maxcompr` | 0xb2 | 0x2c8 | float | ft |
| `faxle.spring_rate` | 0xb8 | 0x2e0 | float | lb/ft |
| `faxle.damping_cf` | 0xb9 | 0x2e4 | float | – |
| `faxle.damping_friction` | 0xba | 0x2e8 | float | – |
| `faxle.torque_pct` | 0xbf | 0x2fc | float | 0-1 |
| `faxle.axleBias` | 0xc2 | 0x308 | float | – |
| `raxle.rtire.CF` / `spring_arm` | 0xe8 / 0xf1 | 0x3a0 / 0x3c4 | float | – / in |
| `raxle.ltire.CF` / `spring_arm` | 0x133 / 0x13c | 0x4cc / 0x4f0 | float | – / in |
| `raxle.maxangle` | 0x15f | 0x57c | float | rad |
| `raxle.maxcompr` | 0x160 | 0x580 | float | ft |
| `raxle.spring_rate` | 0x166 | 0x598 | float | lb/ft |
| `raxle.damping_cf` / `damping_friction` | 0x167 / 0x168 | 0x59c / 0x5a0 | float | – |
| `raxle.torque_pct` | 0x16d | 0x5b4 | float | 0-1 |
| `eng.fuel_consum` / `friction_cf` | 0x176 / 0x177 | 0x5d8 / 0x5dc | float | – |
| `eng.redline` / `redlineTimer` | 0x17c / 0x17d | 0x5f0 / 0x5f4 | float | rpm / s |
| `eng.upshift_rpm` / `dnshift_rpm` | 0x17e / 0x17f | 0x5f8 / 0x5fc | float | rpm |
| `eng.maxHP` / `maxHPRPM` | 0x183 / 0x184 | 0x60c / 0x610 | float | hp / rpm |
| `eng.maxTorque` / `maxTorqueRPM` | 0x185 / 0x186 | 0x614 / 0x618 | float | lb·ft / rpm |
| `eng.torqueTableCount` | 0x187 | 0x61c | int | – |
| `eng.torqueTable[]` | 0x188… | 0x620… | float[] | lb·ft |
| `xm.final_drive` | 0x1ae | 0x6b8 | float | – |
| `xm.axle_ratio` | 0x1b0 | 0x6c0 | float | – |
| `xm.xfrLowRatio` | 0x1b1 | 0x6c4 | float | – |
| `xm.pct_loss` | 0x1b3 | 0x6cc | float | 0-1 |
| `xm.maxGear` | 0x1b5 | 0x6d4 | int | – |
| `xm.gear_ratio[]` | 0x1b6… | 0x6d8… | float[maxGear] | signed |
| `xm.gearInertia[]` | after ratios | — | float[maxGear] | lb·ft² |
| `dry_weight` | 0x481 | 0x1204 | float | lb |
| `ixx,iyy,izz` | 0x482 | 0x1208 | float[3] | lb·ft² |
| `refArea` | 0x485 | 0x1214 | float[3] | ft² |
| `max_brake_force_pct` | 0x48e | 0x1238 | float | 0-1 |
| `drive_type` | 0x48f | 0x123c | int | 0/1/2 |
| `steering_scaler` | 0x4a2 | 0x1288 | float | – |
| `numColors` | — | 0x2078 | int (≤ 16) | – |
| `colorList[]` | — | 0x207c… | 56-byte records | see §12 |

`gear_ratio[]` is **indexed by gear number, not by "1st"**: index 0 = park,
1 = reverse (negative), 2 = neutral, 3 = 1st, and so on. `maxGear` counts from
park. Reading it as "0 = 1st" is the classic port bug.

Retail values to sanity-check any edit: `faxle.spring_rate ≈ 1178.4`,
`raxle.spring_rate ≈ 1493.0`, `maxcompr 1.25`, `torque_pct 0.4/0.6`,
`final_drive 4.3`, `dry_weight 3675`, `static_bpos ±2.565, -0.5, +5.41/-4.27`.
`damping_cf`, `damping_friction` and the `sc[].pt` Y datum exist in the binary
but not in the text files, so they are currently unused (see §5).

## 5. Physics model

* **Suspension is a ray cast along the body up axis** from each wheel anchor,
  not a height lookup at the anchor's XZ. A tilted chassis over a steep face
  otherwise reads the ground under the wrong point and buries the truck.
* Spring `F = k·x + c·ẋ`, `k` from `spring_rate` (lb/ft→N/m), damper
  `c = 3.5·√spring_rate` lb·s/ft — the retail formula (decompile
  199 044-199 051 / 200 586-200 592, constant 3.5f at `DAT_0065d482`).
* Sprung mass is `dry_weight + 2 × 500 lb` axles (decompile 198 990-198 991,
  154 597). Integration: 60 Hz ticks of 4 × 240 Hz substeps.
* Wheel rays are marched and bisected; a buried anchor is full compression.
* Tyre forces act in the terrain **tangent plane**, one Pacejka curve shared by
  both axes (`B 8.6`, `C 1.62`, `E 0.92`), friction-ellipse clamp. No
  traction multiplier hacks.
* Chassis contact: zero-restitution normal impulses through the rigid-body
  effective mass plus Coulomb friction (0.6), soft positional correction;
  `sc[].pt` X/Z supplies the footprint when present (Y datum unresolved).
  Angular integration uses the chassis-frame inertia.
* Powertrain: clutch slips at `LAUNCH_RPM = 2150`, so a stopped truck still
  makes torque. That is the fix for the 0 MPH / 2200 RPM incline stall, where
  the old code forced rpm to idle with zero wheel speed.
* Reverse is gear index 1; `xfrLowRatio` multiplies wheel torque in low range.
  Fuel cut at `eng.redline` (reverse used to accelerate without limit).
* Positive steer input turns right. Heading is `Vehicle::yaw()` from the
  forward vector — `to_euler(YXZ).1` is the **pitch**.

## 6. Terrain, level and texture formats

* `.LVL` (text): line 1 comment, version lines, then **three file names** in
  order `raw`, `clr`, `tex`, then `sky RAW` (line 7, e.g. `CLEAR1.RAW`,
  `cloudy2.raw`), ambience WAV, then prefixed keys `! @ # $ %`
  (`!waterHeight…` in **half-feet** `0.5 * FT`, `@waterR,waterG,waterB,waterOpacity`,
  `$lightSourceVector`, `%vegTexture`, `#detailTexture`).
* `CLOUDY2.RAW` sky indices (`230..=245`) are shifted relative to `CLOUDY2.ACT`
  non-zero palette entries (`192..=207`); `palette_to_rgba_keyed` remaps
  all-black palette lookups from `[min_idx..]` onto the first non-zero palette entry.
* `.RAW` heightmap: exactly 131 072 bytes = 256×256 `u16` LE, 1/32 ft per unit,
  cells 32 ft apart (8192 ft across). `.RTD` is 131 072 bytes of zeros.
* `.CLR`: 256×256 `u16`, index into the `.TEX` list. `.SDW`: shadow mask.
* `.TEX` (text): `Version`, `1`, `textureCount,shadowTextureCount`, then
  `<index>,<blend>,<file>.RAW`.
* ART `*.RAW` + `*.ACT`: 8-bit indexed 256×256 art with a 768-byte palette;
  the same stem in `texture/ui/ART` is a 256×256 **preview**, not terrain.
* `.SIT`: sections `*** Vehicles ***` (the grid: `staticName`, `wPos`,
  `wOrient`, absolute feet) then `*** Boxes ***` (props), `*** Course ***`,
  `*** Paths ***`, `*** Stadium ***`, `*** Backdrop ***`.
  `*** Vehicles ***` and `*** Boxes ***` give the grid and the props.
  Each box carries, after `model`: `mass` (slugs), `bvel` (ft/s) and
  `!type,flags`. Observed types (THEHILL, DEVIL, ZONA, ASPEN): `0` markers and
  model-less boxes; `1` poles and turn signs (always `mass > 0`); `2` and `13`
  movable objects with mass (hay, planks, logs, the parked taxi); `7` static
  structures (buildings, bridges, arches, rocks, DEVIL's aircraft); `10`
  moving traffic (the only type with a non-zero `bvel`); `12` trees; `14` the
  `CHEST3` pickup. `terrain::Prop` keeps `kind` and `moving`. Traffic and
  knock-over physics are **not ported**: type 10 boxes are hidden and have no
  collider (frozen at their start point they block the racing line), and
  types 1/2/13 are still immovable.
* Prop colliders (`collide.rs`): a tree (type 12) is one 0.4 m trunk at the
  placement, as in offroad4x4 `car.rs`. Every other prop is cut 0.6–1.8 m
  above the terrain and covered with cylinders, so an arch is two posts and a
  wall is solid along its base. The old single cylinder sized from the whole
  bounding box (capped at 2.2 m) turned canopies, sign arms and start banners
  into invisible walls on the racing line.
  `*** Course ***` is `c1Count,course_direction` followed by one `******* <n>`
  block per gate (`ctype,cspeed_type`, `cstart`, `cend`, `cdec_point,cspeed,
  lastentry`, `&cSpeedLimit,cTrackWidth`), all in feet; it is the real lap
  geometry. `*** Paths ***` is `pathCount` and per-path point blocks with
  `wPos`; half the levels carry their racing line there instead. Both are read
  by `terrain::parse_course` into `Track::gates` / `Track::paths`.
* Course coverage in the extraction: 35 of 61 levels ship `*** Course ***`
  gates (DEVIL 14, OBSPARK* 22, YUKON 56, …). The other 26 (`ALASKA*`,
  `ELNORTE*`, `RIVER*`, `SKULL*`, `WASH*`) ship paths only, and `MATT` ships
  neither, which is the single level that still needs the generated circuit.
* Files are scattered by type, not by level: heightmaps and level textures in
  `texture/{texture,ui}/DATA`, `.TEX` in `unknown/{unknown,ui}/DATA`, art in
  `texture/*/ART`. `terrain::Finder` lists each directory once and matches
  names case-insensitively, preferring the level's own declared names.

## 7. Audio

* Bank: 321 WAVs in `assets/extracted/audio/sfx/SOUND`; 299 are 16-bit mono
  22 050 Hz, 10 are 8-bit mono (`EVO1..10` music), 7 are 44 100 Hz 16-bit, one
  is 44 100 stereo (`UI-01`), and `SKID-D1N.WAV` is format tag 32 (Yamaha
  ADPCM, 4-bit) with a bogus `blockAlign` of 1.
* Yamaha ADPCM decodes through the 89-slot step-size table (`YAMAHA_STEP`) and
  the ±(2^n) index shifts, as MAME's `ymz280b` and FFmpeg's `adpcm_yamaha` do:
  the nibble is a signed delta against the current step, and the nibble also
  moves a 0..=88 index. The retail `SKID-D1N.WAV` decodes to a 26 915 peak
  (0.82 full scale, no clipped samples) at 22 050 Hz, so it plays at its true
  pitch. `TRX/Sounds.Samples.cxx` rejects tag 32 outside WinCE because the
  platform decodes it in hardware.
* macroquad hands buffers to quad-snd, which resamples everything to 44 100
  stereo with a documented nearest-neighbour resampler and sums voices with no
  headroom. So: **produce 44 100 Hz ourselves** (windowed-sinc) and keep the
  cross-faded voice sum at unity.
* Engine: loops cut at `ENGINE_RPMS` from `ENG_STOCK.WAV` (what offroad4x4's
  `tools/evo2_sfx.py` converts to `engine_run.wav`) pitched from
  `REF_RPM = 3100`, `ENG_IDLE3.WAV` at the bottom point, tail cross-faded into
  the head, DC removed, RMS levelled. Idle plus one active run step at a time
  (±75 rpm hysteresis, ~35 ms gain slew). **Do not use `ENG_STOCK2.WAV`**: it
  is a saturated buzz (crest 1.07, ~80 % of samples at full scale).
* TRX pitches one engine voice continuously through the DirectSound playback
  rate (`Sounds.Effects.cxx` `SelectSoundEffectFrequency`); macroquad has no
  rate control, so stepped loops are the substitute. `Vehicles.Sounds.cxx`
  is a stub in both TRX trees — there is no engine state machine to port.
* Mix headroom (`audio::tests::mix_report`): worst-case simultaneous peak of
  music + engine + loudest one-shot is 0.91 at default gains.
* Music is decoded on a worker thread with an LRU of 2 tracks. Decoding all 18
  at boot cost seconds and hundreds of MB.

### Levels without an AI lap

`data/data/AI` only covers the Evo 1 levels. Every Evo 2 level instead carries
its own course in the `.SIT`: either `*** Course ***` gate lines (the lap the
disc defines) or a `*** Paths ***` spline. `Race::build` uses the recorded AI
file first, then the longest eight path splines (`line_from_path`), so the
generated circuit is only reached by `MATT`, which ships neither — it has a
`.SIT` grid and a texture, and its 20 path slots hold at most 3 points. The
circuit is walked from that grid (start point and heading are the level's own;
only the corners are generated) and planted on the height field. Do not reuse
the trick on a level with real course data.

Lap checkpoints follow `Track::gates` when present, with a capture radius of
half the gate's `cTrackWidth` (6-26 m). Without gates the code falls back to
four quarter-line probes.

## 8. UI

`classic.rs` holds the original's frame: full-width `button3` strips, `cursor`
art beside the highlight, orange `MENU_SEL` text, blue `BAR` header/footer,
`profile` and `BACK` plate bottom left, clock bottom right. `ui2("<screen>")`
returns the authentic background photo (`bg_main`, `bg_qr`, `bg_career`,
`bg_parts`, `bg_options`, `bg_results`, …). Every screen calls
`classic::menu_frame` and keeps to that vocabulary.

## 9. Runbook

```bash
cd /path/to/evox
cargo test --workspace                     # includes retail-data tests
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release
./target/release/evox                      # needs the window
```

Screenshot hooks (QA): `EVOX_SHOT=/tmp/x.png EVOX_SHOT_T=3 ./target/debug/evox
--screen quickrace`, `--race ZONA`, `EVOX_AUTODRIVE=1` for a self-driving
capture, `EVOX_SHOT` then exits by itself. `--screen` selects a *state*
(`quickrace`, `shop`, `main`, …); the window size is `--resolution WxH`.

Data-dependent tests start with `require_assets!()` and skip when
`assets/extracted` is absent, so a source-only CI checkout is green.

> **Mandatory Verification Rule:** Never sign off on UI, camera, or physics
> changes using unit tests alone; always run `EVOX_AUTODRIVE=1` + `EVOX_SHOT`
> and visually inspect the rendered frames and telemetry.

## 10. Ground-Truth Diff Findings (`offroad4x4` & `4x4e/Source/TRX` vs `evox`)

Direct comparison between the working prototype
(<reference>/offroad4x4/app/src/),
the TRX source (`./4x4e/Source/TRX/`), and `evox/game/src/`:

1. **Chase Camera (`offroad4x4/src/main.rs` vs `evox/game/src/race.rs`)**:
   - `offroad4x4` decouples horizontal (`k_xz`) and vertical (`k_y`) spring
     rates behind a blended heading + velocity vector (`vel_flat`), keeps a
     bounded distance band (`0.65× .. 1.55× chase_distance`), and clamps the
     camera floor at `Track::height_at_world(cam.x, cam.z) + 1.5` m so steep
     canyon walls never clip the near plane.
2. **Terrain & 3D Model Shading (`offroad4x4/src/render.rs` vs `evox/game/src/terrain.rs` & `render.rs`)**:
   - `offroad4x4` modulates per-cell `.CLR` atlas tiles with `.SDW` shadow
     masks (`0.84 + 0.16 * sdw`), slope rock contrast, and per-vertex Lambert
     sun + sky hemisphere fill (`shade_sun`).
   - In `evox/game/src/render.rs`, `draw_mesh_in_place` rotated vertex normals
     into world space each frame without applying `shade_sun` to `v.color`
     (leaving SMF bodies and props flat-lit), and `wheel_world` returned the
     upper suspension mount without suspension displacement. Applying
     `shade_sun(tint, n, SUN)` per transformed vertex and displacing wheel hubs
     by suspension compression restores 3D relief and wheel articulation.
3. **Engine Audio State Machine (`4x4e/Source/TRX/Vehicles.Sounds.cxx`, `Sounds.Samples.cxx` & `evox/game/src/audio.rs`)**:
   - In `quad-snd`, crossfading two pitch-shifted copies of the same harmonic
     `ENG_STOCK2.WAV` loop across wide 1000-RPM spans caused continuous
     dual-voice phase combing and robotic chorusing.
   - Replacing wide dual-voice crossfades with the TRX two-source architecture
     (`ENG_IDLE3.WAV` idle recording + 12 fine-grained band-limited pitch steps
     of `ENG_STOCK2.WAV` selected with `±75 RPM` hysteresis and 35 ms gain
     slewing) ensures that idle-to-load blends two uncorrelated recordings while
     only a single `ENG_STOCK2.WAV` pitch step is active at steady RPM.
4. **Suspension, Slope Traction & Impulse Contact** — *superseded by §11.*
   The frictionless `vel -= n * vn` projection described here let trucks skate
   on their sides; the 1.55× converter multiplication and all-mode crawler
   force are not in offroad4x4 and were removed (§11).
5. **UI & Menu Fidelity (`offroad4x4/src/hud.rs` & `main.rs` vs `evox/game/src/ui/screens.rs`)**:
   - Quick Race now pairs the left-hand `button3` track list with non-overlapping
     Track Preview and 3D Truck Turntable (`draw_truck_preview` with all four
     wheels) cards above the interactive `< Vehicle >` cycler and session
     controls.
   - Garage uses `classic::menu_frame` with the full 4-wheeled 3D truck
     turntable, stat bars, and shop/test-drive actions.

## 11. Live-QA findings (2026-11-15)

Every item below was invisible to `cargo test` and found by driving the
retail levels with `EVOX_AUTODRIVE=1`, `EVOX_TELEMETRY` and `EVOX_SHOT`, then
reproduced headless with `race::drive_qa`. Each has a regression test.

| # | Symptom in live play | Root cause | Fix (ground truth) | Test |
|---|---|---|---|---|
| 1 | Pilot and AI rivals leave the line within seconds, rivals flip | Heading read as `rot.to_euler(YXZ).1`, which is **pitch** | `Vehicle::yaw()` from the forward vector | `yaw_matches_spawn_heading_and_ignores_pitch`, `autopilot_drives_aspen_cleanly` |
| 2 | D / Right steers left | `+steer` rotated −Z towards −X; AI compensated with `-err` | Rack applied as `−steer`; AI uses `+err` | `positive_steer_turns_right` |
| 3 | Truck backs off the grid and reaches 85 km/h in reverse | AI "stuck" timer ran during the countdown; brake = reverse; no rev limit | Pilot obeys countdown; AI uses signed speed; fuel cut at redline | `autopilot_waits_for_the_start`, `reverse_is_rev_limited` |
| 4 | Sim speed depends on display rate | One physics step per rendered frame | Accumulator in `Race::tick` (offroad4x4 `main.rs`) | `frame_rate_does_not_change_sim_speed` |
| 5 | Trucks skate on their side/roof at speed | Frictionless hull projection, spin zeroed | Rigid-body impulses + Coulomb friction 0.6 | `overturned_truck_scrubs_to_a_stop` |
| 6 | Truck rests on its hull with "no wheel contact" on steep faces | Ray Newton step overshoots; buried anchor read as zero compression | March + bisect; buried = full compression | `wheel_ray_hits_a_steep_face_and_a_buried_anchor` |
| 7 | Light, bouncy, ~50 % airborne | Missing 2 × 500 lb axles; invented ζ damping; 60 Hz explicit | Retail mass and `3.5·√k` damping; 240 Hz substeps | `parses_values_and_lists`, drive report |
| 8 | Engine tone harsh / static-like | `ENG_STOCK2.WAV` is a clipped square wave | `ENG_STOCK.WAV` as in offroad4x4 | `engine_run_recording_is_not_saturated` |
| 9 | Grey square in title bars | `LCD_EVO` not extracted → generated placeholder | `classic::BAR` + `ui2` logo, `ui2` backgrounds | screenshots |
| 10 | Overlapping list text (shop, dealership), career rows under the footer | `Ui::row` drew title and note blindly; no scroll | `Ui::fit` truncation; scrolling career list | `fit_cuts_with_dots_and_keeps_short_text`, screenshots |
| 11 | Quick Race preview shows the EVO 2 logo | Preview art never loaded; silent `LOADING1` fallback | `Assets::track_preview` (`texture/ui/ART/<LEVEL>.RAW`); missing file is reported on the card | `every_level_has_a_preview_picture` |

Headless 60 s autopilot report (`cargo test --release -p evox drive_report --
--ignored --nocapture`), Montero Ltd, after the fixes:

| Level | Mean | Off line | Airborne | ≤ 2 wheels | Max roll |
|---|---|---|---|---|---|
| ASPEN | 80 km/h | 0 % | 6 % | 10 % | 39° |
| ZONA | 62 km/h | 5 % | 13 % | 22 % | 73° |
| DEVIL | 69 km/h | 0 % | 19 % | 28 % | 83° |
| CZ2000 | 86 km/h | 0 % | 6 % | 11 % | 24° |
| THEHILL | 97 km/h | 9 % | 1 % | 2 % | 27° |
| MCROSS | 83 km/h | 0 % | 9 % | 18 % | 33° |

Re-run 2026-11-16 after porting air control (`AIR_PITCH_ACCEL`,
`AIR_YAW_ACCEL` from offroad4x4 `tuning.rs`; AI drivers hold the landing
slope with it) and the prop-collider rewrite. A sweep of `anti_roll` (0, 0.5,
0.85, 1.5) and of the CG factor (0.20, 0.30, 0.45) moved none of these roll
figures consistently: the tumbles came from nose-first landings and from
striking oversized prop cylinders, not from the roll stiffness. ZONA and
DEVIL still reach 73° / 83° on crest landings that arrive with roll rate;
there is no roll control in the air.

Live game, 60 s autodrive telemetry, previous commit → this one (frames with
|roll| > 60°): DEVIL 93 → 40, THEHILL 34 → 0, ZONA 22 → 0, others 0 → 0.
DEVIL's mean speed fell 60 → 47 km/h: the line passes within 1.4 m of the
centre of `AG3ROCK3`, which is now solid out to its real edge.

Before: 67-91 % airborne and 86-97 % off the line on the same levels.
DEVIL and MCROSS still have crest launches at 100 km/h+; the AI line caps
(48-159 km/h) come from the retail recordings.

QA commands:

```bash
cargo test --release -p evox drive_report -- --ignored --nocapture   # 6 levels × 60 s
QA_TRACK=DEVIL QA_SECS=20 QA_EVERY=30 cargo test --release -p evox drive_trace -- --ignored --nocapture
cargo test --release -p evox mix_report -- --ignored --nocapture      # audio headroom
EVOX_TELEMETRY=t.jsonl EVOX_AUTODRIVE=1 EVOX_SHOT=s.png EVOX_SHOT_T=34 ./target/release/evox --race ASPEN
```

Telemetry lines carry `x y z vy roll pitch grounded air ground_y cam_y
cam_ground phase` in addition to speed/rpm/slip/suspension/gear.

### Peer audit (2026-11-16)

Read-only audit plus live `DEVIL` video. Same rule as above: every row has a
regression test, none of them rely on `cargo test` alone.

| # | Symptom | Root cause | Fix | Test |
|---|---|---|---|---|
| 1 | Camera ends up in front of the truck during a spin; trees hide it on ASPEN | `cam_heading` drove the look target too | Look target is `truck.pos + 1.1 m`; heading clamped to ±75° of forward below 9 m/s; eye pulled past tall prop cylinders | `camera_looks_at_the_truck_during_a_spin` |
| 2 | Tire marks and dust in the sky | Mark corners used the bilinear field, which is above the drawn triangles on crests | Quads snapped to `triangle_height_at + 0.03`, normal-aligned, drawn before `set_default_camera()` | `skids_follow_a_slope_exactly`, `skids_and_dust_meshes_hug_terrain_not_sky` |
| 3 | 31 levels ran a pentagon placeholder | Evo 2 gates/waypoints live in the `.SIT` and were never parsed | `terrain::parse_course` reads `*** Course ***` / `*** Paths ***`; placeholder only for `MATT` | `sit_course_gates_and_paths_parse`, `retail_levels_expose_gates_or_paths` |
| 4 | Collisions added a flat `±2 rad/s` per resolving tick; no sound | Raw per-tick angular gift, no contact gate | Yaw change is `angular impulse / yaw_inertia`; `IMPACT_COOLDOWN` gates `hits` and the `ROCKHIT` thump | `sustained_contact_is_one_impact`, `collision_deflection_scales_with_inertia` |
| 5 | Hundreds of heap allocations per frame | `clone_mesh` per part per frame; `transparent` ignored | One reusable scratch buffer; opaque and cutout parts drawn in separate passes; failed `load_material` cached | `ModelCache` load paths, HUD `missing` |
| 6 | Palette index 0 punched holes in beds and trim | Index 0 was a global key colour | Only `.OPA` or a `TRANSP*` part keys a pixel; bed liner dark charcoal, glass smoked with a dark backface | `index_zero_is_opaque_without_an_explicit_mask` |
| 7 | `START RACE` clipped on short windows | Panel height was `sh * 0.69`, independent of content | Panel height derived from the row stack and the button | screenshots (1280×720, 800×600) |
| 8 | `SKID-D1N.WAV` clipped; `--screen WxH` shadowed `--screen <state>` | Approximated ADPCM step, one flag for two meanings | 89-slot Yamaha step table; CLI flag renamed `--resolution WxH` | `skid_wav_decodes_without_clipping`, `--help` |

## 12. Vehicle roster, factory paint and UI art (audit 2026-11-15)

### Roster

| Source | `.TRK` files | Unique names | Notes |
|---|---|---|---|
| Evo 1 (`assets/extracted/model/vehicle/TRUCK`) | 151 | 108 | Every body, LOD body (`<base>0.SMF`), tyre and texture resolves |
| Evo 2-only (`assets/extracted/evo2/TRUCK`) | 41 | +25 | All Jeeps, ARB Monster Truck, Rally Editions, Dodge Ram, Infiniti QX4, Frontier SC |
| **Total** | **192** | **133** | Evo 2's `MPRT2D` renames Evo 1's "Pajero Rally 2 Door" to "Montero" (new body); the Evo 1 file is kept |

**There is no Jeep in the Evo 1 data.** `4x4.exe` lists "Jeep" only among
manufacturer names. The Wrangler Sport (`JWS4/42/43`), the yellow
**ARB Wrangler Rally Edition** (`AJWS`, body `ARBJEEP`, fixed livery,
`numColors 0`), Grand Cherokee and Liberty are Evo 2 trucks.

`python3 tools/evo2_trucks.py` imports them from the unpacked Evo 2
`TRUCK.POD` (`./4x4evo2-work/pods/TRUCK`). It copies the
Evo 2-only `.TRK` files, the 110 models they reference, and converts 109
textures. Evo 1 wins on any file-name collision (109 `.TRK` files, all
models/textures it already ships); the run writes `evo2/evo2_trucks.json`. The
roster loads Evo 1 first so saved garage indices stay valid
(`vehicle::load_roster`, `ROSTER`, `MODEL_DIRS`).

### Formats

* Evo 2 `.TRK` is `version 7` (Evo 1: 6): same key/value layout, adds
  `teamRequirement`, six lights instead of seven.
* Evo 2 `.SMF` adds a `v1` tag before each texture record, names textures
  `.TIF`, and follows them with a quoted bump-map line (`"X_bump.TIF"`).
  `smf.rs` accepts both.
* Evo 2 truck textures are uncompressed palette TIFFs with **two samples per
  pixel** (index, mask) that Pillow cannot open; Evo 1 ships the same data as
  `<stem>.RAW` + `.ACT` + `.OPA`. The import writes the Evo 1 triplet. The
  `.OPA` of a truck texture is the **paint mask**, not just opacity.
* Tyres: `<tireModelBaseName>{08,12,16}{L,R}.SMF` are LODs of one tyre
  (38/170/211 vertices, identical radius). evox draws `16` at scale 1 and
  measures the physics radius from it (offroad4x4 measures `<tire>08L` the
  same way); the old generic `33X12_12` tyre is only a reported fallback.

### `colorList[]` schema (factory paint)

Text: `numColors` then `h,s,v,r,g,b,name` per line (`name` from file version 6,
`NULL` = unnamed). Struct: 0x38-byte records at `+0x207c`: six floats and a
32-byte name (reader `decomp_4x42_full.c` 201 526-201 583). The writer at
202 416 prints the floats in a decompiler-scrambled argument order; the
reader's order is authoritative.

| Field | Meaning | Used by |
|---|---|---|
| 0 | hue shift, degrees | `loadTruckTextures`, ÷ 360 (`DAT_0065b5c0`) |
| 1 | saturation shift, % | ÷ 100 (`DAT_0065b5c8`) |
| 2 | value (brightness) shift, % | ÷ 100 |
| 3-5 | RGB swatch, 0-255 | menus only |
| 6 | colour name | menus |

`colorAdjustTexture` (`core/colorvar.cpp`, `FUN_004273d0` 17 454-17 900):

1. Palette → HSV (`FUN_004269a0`: V = max, S = Δ/max, H in turns, −1 for
   greys).
2. Reference hue = hue of the palette index used most under mask > 250.
3. Per entry: `H = clamp(H, ref ± 0.05) + dh` wrapped to 0..1
   (`DAT_0062ff01/05/09` = 0.05/−0.05/−1.0); `S = clamp(S + ds)`,
   `V = clamp(V + dv)`; back to RGB (`FUN_004267d0`).
4. Per pixel: mask 255 → adjusted, ≤ 128 → original, between → linear blend by
   `(mask − 128)/127`; alpha = mask.

The selected index lives at `+0x2074` (`loadTruckTextures - Invalid color
index!`); evox stores it as `Truck::paint_index` (Quick Race "Paint" row, AI
rivals `(slot·5 + 2) mod n`). Implementation: `game/src/paint.rs`. Check:
**535 / 541** chromatic paints across both games land within 25° of their own
swatch hue (`factory_paints_match_their_swatches`).

### UI art

* Track previews: `texture/ui/ART/<LEVEL>.RAW` (256×256) for every level,
  loaded by `Assets::track_preview`; a missing file is printed on the card.
* `EDBTN*` are the track editor's 20×20 RLE `.PBM` toolbar icons
  (`PBM\x1a`, w, h, …). They were preloaded as menu panels, never resolved,
  and the placeholder cache made every button and panel grey. Panels and
  buttons now use the original `ui2/button3` strip (Evo 1 `BUTTON3` without
  the Evo 2 art); `Assets::tex` no longer caches placeholders under real
  names, so `tex_now` stays `None` for missing art and the miss is listed in
  `assets.missing`.

QA hooks: `EVOX_TRUCK=<TRK stem>` and `EVOX_PAINT=<index>` select the truck and
paint for screenshots, e.g.
`EVOX_TRUCK=AJWS EVOX_SHOT=s.png EVOX_SHOT_T=3 ./target/release/evox --screen quickrace`.

