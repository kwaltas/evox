# Architecture

## Boundaries

EvoX has three separable pieces:

1. **Asset intake** (`tools/`) — Python, offline, no runtime dependency.
2. **Client** (`game/`) — Rust/Macroquad, owns simulation, presentation, and
   the local save.
3. **Services** (`server/`, `leaderboard/`) — Rust, std-only, stateless apart
   from in-memory tables and an atomic JSON snapshot.

The client never links game assets. Every asset access goes through a path
produced by the intake step, so a bare checkout boots to an actionable error.

## Client layering

```
main → app (state machine)
        ├── ui/screens (pure projection of app state)
        ├── race (session: timing, standings, camera)
        │     ├── physics (fixed 60 Hz, deterministic)
        │     ├── terrain (analytic heightmap + streamed chunk meshes)
        │     ├── ai (recorded lines, pure pursuit)
        │     └── vfx (pooled particles, skid ring)
        ├── parts (catalog grammar, shop transactions)
        ├── vehicle / smf (original format readers)
        ├── audio (RPM-mapped engine loops, spatial one-shots)
        ├── save (checksummed profile, atomic write, mod watcher)
        └── net / master / leaderboard (UDP + HTTP clients)
```

Rules that keep it maintainable:

- `physics` has no rendering types and no I/O; it is unit tested.
- `parts` is pure logic. The shop screen only renders locks and calls
  `buy`/`equip`; it never re-derives a rule.
- Parsers return `Result` and never panic on hostile input. Bounds are checked
  before allocation; counts are sanity capped.
- `app` owns transitions. Screens receive `&mut App` and cannot jump directly
  to an arbitrary state without going through `go()`, which maintains the back
  stack.
- Terrain collision is the analytic height field, never the chunk mesh. Meshes
  are presentation only and may stream in a frame late without changing grip.

## Determinism

The simulation advances in fixed `1/60` steps. The renderer may run at any
rate; `get_frame_time()` only feeds interpolation-independent presentation
(camera easing, VFX fade). A recorded input stream plus a seed reproduces a
race, which is what the leaderboard digest and server validation assume.

## Units

Original data is feet, pounds, and ft-lb. Conversion happens once at parse
boundary (`vehicle::FT`, `LB`, `INCH_M`). Simulation and render space are
metric; nothing downstream sees feet.

## Asset classification

`tools/pod.py` separates payloads into class/group directories. The mapping is
data-driven (`EXT_CLASS`, `POD_GROUP`) and every file's MD5 lands in
`assets/manifest.json`, which the wizard and CI use as the integrity source.

## Network model

Host simulates; clients send inputs. Snapshots carry position, rotation,
velocity, suspension, RPM, gear, and a parts bitmask. Peers are validated
against acceleration and speed envelopes (`net::validate`), with strike
accounting. Remote cars render 120 ms in the past from a jitter buffer;
starved buffers dead-reckon for at most 250 ms.
