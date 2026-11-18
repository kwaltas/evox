# AI drivers

Ground truth: the recorded retail laps under `data/data/AI/CLASS*/AI_*.TXT`
and the `.SIT` `*** Paths ***` splines parsed into `Track::paths`. A level with
no recording uses the longest paths; only `MATT` (no AI and no paths) gets the
generated circuit. The controller in `game/src/ai.rs` turns either source into
one `RacingLine` and produces the same `physics::Input` a human uses, so rivals
obey the rigid-body, suspension and tyre model unchanged.

## Path structure

`RacingLine::parse` reads the `.TXT` header (driver, class, point count) then
the `x, y, z` / speed / flag triples in feet. Points convert once to metres,
headings come from the next node (`forward = −Z`, matching the physics yaw),
and `compute_speed_caps` propagates a braking-distance limit backwards through
the loop. Query helpers are arc-length based, not node-index based:

| Helper | Purpose |
|---|---|
| `point_ahead(cursor, distance)` | Interpolated pure-pursuit aim point |
| `nearest_ahead(cursor, pos, window)` | Monotonic projection that cannot jump to a parallel switchback |
| `max_curvature(cursor, span, steps)` | Peak heading change per metre ahead |
| `speed_limit_ahead(cursor, distance)` | Slowest authored cap and where it occurs |

## Pure Pursuit steering

The target is one speed-scaled look-ahead down the line:

```
l_d = clamp(7 + 0.7·v − lead, 4, 24) m
α   = wrap(atan2(to.z, to.x) − heading_angle)
δ   = atan2(2·L·sin α, l_d),  L = 2.8 m
steer = clamp(δ / 0.55, −1, 1)
```

`lead` is the distance already travelled past the cursor node, so the aim point
stays a consistent distance ahead instead of stretching on sparse recordings.
`steer` is the same normalised rack input a player produces; `physics` applies
the rack rate limit, speed-sensitive falloff and tyre forces.

## Speed control

The target speed is the lower of

* the slowest authored cap within the look-ahead, and
* the curvature limit `sqrt(a_lat / κ)` with `a_lat = 10 m/s²`,

scaled by `skill`, the start-line `rubber` factor and `aggression`. Clear
straights get full throttle (`0.65 + 0.12·(cap − v)`, capped at 1). When the
current speed exceeds the cap, the required deceleration
`(v² − cap²) / (2d)` sets brake pressure above a 2.5 m/s² threshold and cuts
throttle proportionally.

## Rival awareness

`tick_with_rivals` receives every chassis position for the tick. A rival is
considered blocking when it lies in the forward cone (16 m long, ±3.2 m wide).
Inside a speed-dependent safe gap (`4.5 + 0.55·v`) the driver lifts and adds
brake proportional to the closing urgency. Cars in the next lane are ignored.
`tick` is the single-car wrapper used by the QA autopilot.

## Tests

`ai::tests` covers interpolation, curvature, the slowest-cap lookup, the
left/right pure-pursuit sign, full throttle on a clear straight, braking for a
rival in-lane, and ignoring a rival in the adjacent lane. The retail regression
is `cargo test --release -p evox drive_report -- --ignored --nocapture`, which
runs 60 s of autodrive on six levels and reports off-line, airborne, roll and
vertical-reversal metrics.
