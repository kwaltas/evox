# QA protocol

Run before every phase sign-off. Exit codes are the failure count.

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p evox -- --self-test 1
python3 tools/pod.py --self-test
python3 tools/verify_manifest.py assets/extracted assets/manifest.json
```

Camera: position, yaw and look-at are exponentially damped (frame-rate
independent), the look target is always the truck body (`pos + 1.1 m`), the
chase direction is clamped to ±75° of forward below 9 m/s so a spin cannot put
the eye in front of the cab, and the eye is pulled past occluding prop
cylinders then clamped to `height_at + 1.5`. `EVOX_CAM=1` exposes the samples
so a regression is visible as `cam.y < g`.

Audio: engine note is pre-rendered loops at the `ENGINE_RPMS` points, all
playing, cross-faded by rpm and summed to unity; buffers are produced at
44 100 Hz so quad-snd's nearest-neighbour resampler never runs. Music is
decoded lazily with an LRU of 2 tracks. `EVOX_MUSIC=1` logs track switches.

Presentation checks: launch with no arguments to watch the boot cards
(Enter skips), and start a race to inspect the corner gauges. Both use the
extracted `4X4FONT`/`NEEDLE`/`ODOM` sheets; text must never fall back to the
system font once assets are loaded.

Hands-on checks use `cargo run -p evox -- --race TRACK`. Set
`EVOX_AUTODRIVE=1` to let the recorded AI line drive the player,
`EVOX_CAM=1` to log chassis/camera state, and `EVOX_MESHDBG=1` to log the
chunk cache and FPS, while observing collision, streaming and landing
behaviour.

## What each gate covers

| Gate | Catches |
|---|---|
| unit tests | parser bounds, expression algebra, save tamper, physics settle/brake, prop colliders, collision impulse scaling, netcode round-trip, TTL sweep |
| `--self-test` | whole-catalog parse, 150 vehicle definitions, 60 heightmaps, collider set, audio/AI/course presence, packaging |
| chunk cache test | nearest-first build order, per-frame budget, grid clamping |
| `pod.py --self-test` | both POD revisions, compressed and plain, truncation, traversal |
| manifest verify | truncated extraction, disk rot |

## Known issues (open)

1. **Props use one inscribed cylinder each**, sized from the model bounds;
   thin sign boards are therefore a little oversized and bridges/deck props
   are solid. Props draw one model per `.SIT` box without instancing, so dense
   tracks can cost a few hundred draw calls.
2. **60 levels are in this extraction** (`evo2-only` intake, Evo 2 pods
   only): the 15 Evo 1 tracks that Evo 2 re-ships and 45 Evo 2 exclusives
   (`tools/evo2_tracks.py`). Every one ships a 256×256 heightmap in
   `texture/texture/DATA`, so none of them is flat. `.RTD` is all zeros
   and is no longer used as a heightmap.
3. **`MATT` (Evo 1 test map, absent from the `evo2-only` intake) has no lap
   data.** If an Evo 1 tree supplies it, it has no AI recording and
   its 20 `.SIT` path slots hold at most 3 points, so the race starts on a
   generated circuit from its `.SIT` spawn. Its grid is a straight line in the
   file, so rival placement there is approximate. Every other level uses its
   own `*** Course ***` gates or `*** Paths ***` splines.
4. **Master URL is compiled in** (`127.0.0.1:8790`); no settings field yet.
5. **No separate headless crate.** `Race` builds `audio::Audio` with the mixer
   disabled; a true `--headless` binary is pending.
6. **No VR.** Symmetric perspective only; OpenXR needs per-eye passes.
7. **Leaderboard trusts the digest plus range checks**, so a valid payload can
   be replayed with a fresh install id.
8. **No demo recording.** The replay camera cycles angles after the flag but
   frames are not stored for later playback.
9. **Terrain binds the dominant `.TEX` albedo** with per-material vertex tints;
   blend layers are parsed but not composited, and MGRASS1 is a cutout mask,
   not a ground albedo.
10. **Chunk builds are budgeted by speed (2-8 per frame)** and prefetched six
   chunks ahead of travel; the chunk under the player ignores the budget and
   is never evicted, so the "brown void" only reappears on a track whose
   heightmap is empty.
11. **Shop tabs use the generic ICONA–ICOND art**, not per-category icons.
12. **Buttons fall back to `BUTTON3`** because the `EDBTN*` frames ship as
    `.PBM`, which the ART loader does not read yet.
13. **A mod truck whose mesh is not installed borrows the Blazer body** for
    the session; nothing is drawn with placeholder primitives, and the file is
    listed in `Renderer::missing`.

## Regression habits

- Every parser test includes one hostile case (truncated, wrong magic,
  out-of-range index, unterminated group).
- Physics assertions are qualitative (settles, moves, brakes) so tuning does
  not require golden numbers.
- Netcode asserts caps, not timings, so CI is not flaky on a loaded runner.
- Tests that need the git-ignored `assets/extracted` tree start with
  `require_assets!()`, which skips (not fails) when the tree is not extracted.
  CI runs source-only.

## Determinism smoke test

Two runs with identical input streams must match within 1 cm after 600 ticks.
Pending input recording; the current harness checks finiteness and settling.

## Sign-off sheet

| Phase | Gate | Result |
|---|---|---|
| 1 | pod self-test + 32,052-file extraction, 0 skipped | pass |
| 2 | physics settle/brake/throttle | pass |
| 4B | 90-part catalog, 26 prerequisite expressions, conflict tests | pass |
| 5A/B | 151 vehicles, 61 levels, 978 SMF payloads | pass |
| 7A | snapshot round-trip, interpolation cap, teleport rejection | pass |
| 9B | strikes and envelope validation | pass |
| 11A | wizard MD5 verify, traversal-safe names | pass |
