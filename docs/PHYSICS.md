# Physics reference

Ground truth: the `.TRK` field table in `docs/PORT_REFERENCE.md` §4 (recovered
from `decomp_4x42_full.c`) and the ray-cast vehicle in the offroad4x4
prototype, which drove all 30 retail tracks. Units at rest are imperial and are
converted once at load; the simulation is SI in fixed 60 Hz ticks of four
240 Hz substeps (offroad4x4 `PHYSICS_DT = 1/240`), driven from a frame-time
accumulator in `Race::tick`, so the display rate never changes sim speed.

## Suspension

| Field | Unit | Meaning |
|---|---|---|
| `<axle>.spring_rate` | lb/ft | Wheel rate; converted ×14.5939 → N/m |
| `<axle>.maxcompr` | ft | Travel before the bump stop |
| `<axle>.maxangle` | rad | Axle articulation limit |
| `<axle>.ltire/rtire.static_bpos` | ft | Anchor, chassis space |
| `<axle>.ltire/rtire.spring_arm` | in | Motion-ratio arm |
| `<axle>.ltire/rtire.CF` | – | Per-corner grip multiplier |

A ray is cast **along the body up axis** from each anchor. A height lookup at
the anchor's XZ reads the ground under a point that is not under the wheel once
the chassis tilts, which is what used to bury trucks on steep faces.

The ray is marched in ten steps to the first crossing and bisected. An anchor
already below the surface is a fully compressed wheel (distance 0); the old
Newton refinement overshot on faces steeper than the ray and reported a miss
there, and reported "zero compression" for buried anchors, which let trucks
settle on their hull on steep ground.

Spring `F = k·x + c·ẋ`; damper `c = 3.5·√spring_rate` lb·s/ft (×14.5939 →
N·s/m), the retail formula: `decomp_4x42_full.c` 199 044-199 051 and
200 586-200 592, constant `DAT_0065d482`/`DAT_0065d8a2` = 3.5f read from
`4x42.exe`. (It replaced an invented `ζ = 0.45` damping ratio.) Bump stops add
`10k` past 92 % of travel. Anti-roll torque comes from the left/right
compression delta and `Config::anti_roll`.

Air control (offroad4x4 `tuning.rs`): with no wheel on the ground, throttle
minus brake pitches the truck at `AIR_PITCH_ACCEL` (1.0 rad/s², throttle =
nose up) and steering yaws it at `AIR_YAW_ACCEL` (1.2 rad/s²). AI drivers and
the QA autopilot use it to hold the slope they are about to land on
(`race::air_level`).

## Tire model

Single Pacejka curve, shared by the lateral and longitudinal axes:

```
magic(x) = sin(atan(C·B·x − E·(B·x − atan(B·x))))
```

`B = 8.6`, `C = 1.62`, `E = 0.92`; longitudinal result scaled by 0.92. Forces
act in the **terrain tangent plane**, so grip scales with the surface normal and
a grade no longer eats the drive force. Slip angle is
`atan(v_lat / max(|v_long|, 0.8))` with a linear bite below 2 m/s so a truck can
crawl and park. Longitudinal demand is pushed through the same curve, so past
the peak the tyre spins instead of pulling harder; a friction ellipse at
`μ·Fz` clamps both.

`μ = config.tire_mu · (1 + parts.tire_cf) · surface.grip · corner.CF`.
Surface grip: asphalt 1.12, dirt 0.92, gravel 0.84, grass 0.78, mud 0.66,
sand 0.62, snow 0.55, water 0.48.

## Chassis

Nine body points: centre and bottom ring (from `sc[].pt` X/Z when the file has
scrape points, else AABB corners) plus a roof ring for roll-overs. Contact is
velocity-level: each point below the surface gets a zero-restitution normal
impulse through the full rigid-body effective mass
(`1/m + n·((I⁻¹(r×n))×r)`), then a Coulomb friction impulse capped at
`0.6·jn` (`BODY_CONTACT_FRICTION`, offroad4x4). Three passes per substep;
residual depth above 1 cm is removed at 35 % per substep along the normal.
Nothing snaps orientation or zeroes spin, so a truck on its side scrubs to a
stop and gravity rolls it back. The previous frictionless projection let
trucks skate on their roofs at 60 km/h for whole laps.

Angular velocity integrates against the chassis-frame inertia
(`ω += R·(Rᵀτ ⊘ I)·dt`); dividing a world torque by body-axis inertias mixed
the axes on any tilted truck.

Overturned (body up within ~70° of horizontal) and below 4 m/s for
`FLIP_RESET_TIME = 2.5 s` → rescue onto the course line, upright. `H` does the
same on demand (the original's "Rescue").

## Drivetrain

`drive_type` and the in-race transfer mode pick the split: 2WD (rear only),
4WD-HI (axle `torque_pct`), 4WD-LO (`xfrLowRatio` and the split). Auto box
upshifts at `eng.upshift_rpm`, downshifts at `eng.dnshift_rpm`, 160 ms clutch
cut. Reverse is index 1 of `gear_ratio[]` (0 = park, 1 = reverse, 2 = neutral,
3 = 1st), and the table is read by index, not by ordinal. Brake below
0.85 m/s selects reverse; a fuel cut at `eng.redline` (wheel-locked rpm) stops
drive torque, without which reverse, which never upshifts, accelerated
without limit. Drive force is `torque · ratio · 0.88 / r` (no converter
multiplication); the 4WD-LO crawler adds `¼m·g·√steepness·0.9` per corner
above 0.12 steepness and raises the traction limit, as in offroad4x4.

Steering: positive input turns right (towards `right()`); the rack angle is
applied as a rotation of `−steer` about +Y.

The clutch slips at `LAUNCH_RPM = 2150`, so a stopped truck still produces
torque: the old model pinned rpm to idle whenever wheel speed was zero, which
is exactly the 0 MPH / 2200 RPM incline stall. Torque comes from
`eng.torqueTable[]` normalised and scaled by `eng.maxTorque`; the curve shape is
authoritative.

## Mass and inertia

`mass = (dry_weight + 2 × 500 lb) × 0.4536 + parts.weight × 0.4536`, floored
at 150 kg. The retail set-up writes 500 lb into each axle (`+0x2f0`, `+0x5a8`
= `0x43fa0000`, decompile 198 990-198 991) and sums them with `dry_weight`
(154 597); the spring rates are sized for that total (≈70 % static sag).
Inertia is the TRK tensor (`ixx,iyy,izz`) converted, floored at a box
approximation of the hull.

## Aero

`F = ½ · 1.2 · (CD + parts.aero) · refArea.x · refArea.z · |v| · v`, CD floored
at 0.05. `CL` and `CY` are parsed and exposed for future lift/side modelling.

## Tuning loop

1. Record a reference run with `tools/telemetry/bridge.py` in the VM.
2. Record the same input in EvoX with `EVOX_TELEMETRY=run.jsonl`.
3. `python3 tools/telemetry/delta.py ref.jsonl run.jsonl` — RMSE and bias per
   channel, with a raise/lower hint.
4. Adjust the TRK (or the surface table), rebuild, repeat.

`--self-test 0` asserts the car settles on its springs and that brakes shed at
least 25 % of speed in three seconds, which catches sign and unit regressions.
The unit tests in `physics.rs` add the three cases that used to fail: a 30 %
grade from rest, reverse from rest, and no hull penetration into a 45° face.
