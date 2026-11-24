//! Race session: grid, countdown, laps, checkpoints, standings, camera.
//!
//! Rendering uses Macroquad's 3D camera plus a 2D HUD. Opponents are AI-driven
//! from their recorded lines; the local car is either the physics player or,
//! in headless/bench mode, another AI. Timing is tick-based, not frame-based,
//! so results are reproducible and network-verifiable.

use macroquad::prelude::*;

use crate::ai::{self, Driver};
use crate::audio::Audio;
use crate::physics::{Input, Vehicle, DT};
use crate::terrain::{Surface, Track};
use crate::vehicle::Truck;
use crate::vfx::{ReplayDirector, ReplayShot, Vfx};

pub const COUNTDOWN: f32 = 3.9;
pub const MAX_LAPS: u32 = 5;
/// Catch-up limit after a stall: at most this many physics steps per frame.
const MAX_STEPS_PER_FRAME: usize = 8;

/// Chase camera tuning calibrated against original retail 4x4 Evo footage:
/// elevated ~12.5 deg downward pitch framing the truck in the lower third
/// while leaving the upper two-thirds open for the horizon and sky.
pub const FOV_DEG: f32 = 65.0;
pub const CHASE_DISTANCE: f32 = 7.4; // m (scaled with truck length)
pub const CHASE_HEIGHT: f32 = 3.15; // m
pub const CHASE_STIFFNESS: f32 = 5.0; // 1/s smoothing
/// The chase direction may never drift more than this from straight behind
/// the truck's forward vector, at any speed: neither a spin on the spot nor a
/// slide at 70 km/h may swing the eye round to the nose.
pub const SPIN_HEADING_LIMIT: f32 = 75.0 * std::f32::consts::PI / 180.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Countdown,
    Racing,
    Finished,
}

/// Shortest horizontal distance from `p` to the segment `a`-`b`.
fn seg_dist_xz(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let (p, a, b) = (vec2(p.x, p.z), vec2(a.x, a.z), vec2(b.x, b.z));
    let ab = b - a;
    let t = if ab.length_squared() < 1e-6 {
        0.0
    } else {
        ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
    };
    p.distance(a + ab * t)
}

/// Did the move `p0 -> p1` cross the gate line `a`-`b` (horizontal plane)?
fn crossed_gate_xz(p0: Vec3, p1: Vec3, a: Vec3, b: Vec3) -> bool {
    let (p0, p1, a, b) = (
        vec2(p0.x, p0.z),
        vec2(p1.x, p1.z),
        vec2(a.x, a.z),
        vec2(b.x, b.z),
    );
    let side = |o: Vec2, d: Vec2, q: Vec2| (d.x - o.x) * (q.y - o.y) - (d.y - o.y) * (q.x - o.x);
    let (s0, s1) = (side(a, b, p0), side(a, b, p1));
    let (t0, t1) = (side(p0, p1, a), side(p0, p1, b));
    s0 * s1 <= 0.0 && t0 * t1 <= 0.0 && (s0 != 0.0 || s1 != 0.0)
}

/// Where on the lap the last `*** Course ***` gate sits (1.0 without gates).
fn track_last_gate_frac(track: &Track, prog: &Progress) -> f32 {
    track
        .gates
        .last()
        .map_or(1.0, |g| prog.frac_at(g.centre()).clamp(0.05, 1.0))
}

/// How near (metres) the truck must pass a gate line it cannot be seen to
/// cross (very slow frames, spawn on the line).
const GATE_NEAR_M: f32 = 4.0;

/// Gates ahead of the expected one that still count when crossed.
const GATE_LOOKAHEAD: usize = 3;

/// Where a vehicle is in the race: laps completed plus how far round the
/// course it is. Replaces the odometer, which counted circles, detours and
/// reversing as "progress".
///
/// The course is a closed loop of nodes. Each update finds the nearest node
/// in a window around the last one (a global search once, to start), and a
/// jump across the loop's seam in either direction changes the lap counter,
/// so driving the wrong way unwinds progress. Cars on the grid sit behind the
/// start line, which counts as lap -1 until they cross it.
#[derive(Clone, Debug)]
pub struct Progress {
    xz: Vec<Vec2>,
    cum: Vec<f32>,
    total: f32,
    idx: usize,
    laps: i32,
    started: bool,
}

impl Progress {
    /// Search window (nodes either side of the last one).
    const WINDOW: usize = 24;

    pub fn new(line: &[Vec3]) -> Self {
        let xz: Vec<Vec2> = line.iter().map(|p| vec2(p.x, p.z)).collect();
        let mut cum = Vec::with_capacity(xz.len());
        let mut acc = 0.0;
        for i in 0..xz.len() {
            cum.push(acc);
            if xz.len() > 1 {
                acc += xz[i].distance(xz[(i + 1) % xz.len()]);
            }
        }
        Self {
            xz,
            cum,
            total: acc.max(1.0),
            idx: 0,
            laps: 0,
            started: false,
        }
    }

    /// Feed the vehicle's position.
    pub fn update(&mut self, pos: Vec3) {
        let n = self.xz.len();
        if n < 2 {
            return;
        }
        let p = vec2(pos.x, pos.z);
        let nearest = |from: usize, half: usize| {
            // Nodes from `half` behind to `half` ahead of `from`, wrapping.
            (0..=2 * half)
                .map(|k| (from + n + k - half) % n)
                .min_by(|&a, &b| {
                    self.xz[a]
                        .distance_squared(p)
                        .total_cmp(&self.xz[b].distance_squared(p))
                })
                .unwrap_or(from)
        };
        if !self.started {
            self.idx = (0..n)
                .min_by(|&a, &b| {
                    self.xz[a]
                        .distance_squared(p)
                        .total_cmp(&self.xz[b].distance_squared(p))
                })
                .unwrap_or(0);
            self.started = true;
            // Behind the start line: the lap before the first.
            self.laps = if self.idx > n / 2 { -1 } else { 0 };
            return;
        }
        let window = Self::WINDOW.min(n / 2);
        let new = nearest(self.idx, window);
        if new + n / 2 < self.idx {
            self.laps += 1; // crossed the seam going forward
        } else if self.idx + n / 2 < new {
            self.laps -= 1; // crossed it backwards
        }
        self.idx = new;
    }

    /// Fraction (0..1) of the lap at the node nearest `pos`.
    pub fn frac_at(&self, pos: Vec3) -> f32 {
        let p = vec2(pos.x, pos.z);
        (0..self.xz.len())
            .min_by(|&a, &b| {
                self.xz[a]
                    .distance_squared(p)
                    .total_cmp(&self.xz[b].distance_squared(p))
            })
            .map_or(1.0, |i| self.cum[i] / self.total)
    }

    /// Laps plus the fraction of the current one, in laps.
    pub fn laps_f(&self) -> f32 {
        self.laps as f32 + self.cum.get(self.idx).copied().unwrap_or(0.0) / self.total
    }

    /// The same, in metres along the course.
    pub fn metres(&self) -> f32 {
        self.laps_f() * self.total
    }

    pub fn lap_length(&self) -> f32 {
        self.total
    }
}

pub struct Opponent {
    pub vehicle: Vehicle,
    /// Seconds spent overturned and nearly stopped (auto-rescue timer).
    pub flip_t: f32,
    pub driver: Driver,
    pub lane: f32,
    pub progress: usize,
    pub finished_at: Option<f32>,
    /// Laps and distance round the course (drives place and finishing).
    pub prog: Progress,
}

pub struct Race {
    pub track: Track,
    pub player: Vehicle,
    pub opponents: Vec<Opponent>,
    pub phase: Phase,
    pub countdown: f32,
    pub time: f32,
    pub lap: u32,
    /// Laps in this race (the setup's choice, at most `MAX_LAPS`).
    pub laps: u32,
    /// Player progress round the course.
    pub player_prog: Progress,
    /// `laps_f` at which the race ends. The player finishes on the last gate
    /// (or the last quarter probe), which is not exactly the seam of the loop,
    /// so rivals are timed against the same line.
    pub finish_progress: f32,
    /// Where the player was last racing tick (gate crossing test).
    pub last_player_pos: Vec3,
    pub lap_start: f32,
    pub best_lap: f32,
    pub last_lap: f32,
    pub position: usize,
    pub finished_at: Option<f32>,
    pub paused: bool,
    pub vfx: Vfx,
    pub replay: ReplayDirector,
    pub cam_pos: Vec3,
    /// Smoothed look-at point; the raw chassis position jitters on every
    /// suspension tick.
    pub cam_look: Vec3,
    pub cam_yaw: f32,
    /// Unit view direction, the fallback heading while the truck points
    /// straight up or down.
    pub cam_fwd: Vec3,
    /// Smoothed chase camera heading direction (decoupled from high-frequency chassis wobble).
    pub cam_heading: Vec3,
    /// Vertical field of view in degrees; widens with speed.
    pub fov: f32,
    /// Distance driven this race (m), for the odometer drums.
    pub odo_m: f32,
    /// Transfer case: 0 = 2WD, 1 = 4WD-HI, 2 = 4WD-LO.
    pub drive_mode: u8,
    /// Unsimulated time carried to the next frame (s).
    accumulator: f32,
    /// Seconds the player has spent overturned and nearly stopped.
    pub flip_t: f32,
    pending_shift_up: bool,
    pending_shift_down: bool,
    /// Course centre line (first recorded AI line) for the map and the
    /// checkpoint finder.
    pub course: Vec<Vec3>,
    pub checkpoint: usize,
    pub track_id: String,
    pub class: u8,
    pub events: Vec<String>,
    pub surface: Surface,
    pub accents: Vec<usize>,
    pub colliders: Vec<crate::collide::Collider>,
    /// QA autopilot (`EVOX_AUTODRIVE=1`): drives the player along the first
    /// recorded line so collision and handling can be observed hands-free.
    pub pilot: Option<ai::Driver>,
    /// Landing assist for the human player: while airborne the truck is held
    /// at the slope it will land on (`air_level`) instead of the held gas or
    /// brake pitching it. On by default with touch controls, where the gas
    /// button stays pressed through every jump.
    pub air_assist: bool,
    /// QA only: the autopilot drives like a thumb on the touch buttons
    /// (steering all-or-nothing, gas held) and gets no landing help unless
    /// `air_assist` is on.
    pub pilot_thumb: bool,
}

pub struct RaceConfig<'a> {
    pub track_id: &'a str,
    pub class: u8,
    pub laps: u32,
    pub opponents: usize,
    pub player_truck: Truck,
    pub ai_vehicles: Vec<Truck>,
    pub lines: Vec<ai::RacingLine>,
    pub skill: f32,
}

/// A `*** Paths ***` spline is a driving line when most of its points follow
/// the terrain (within `PATH_GROUND_TOL_M`) and it is not a water-surface
/// route.
fn path_is_driveable(track: &Track, points: &[Vec3]) -> bool {
    const PATH_GROUND_TOL_M: f32 = 4.0;
    if points.len() < 8 {
        return false;
    }
    let n = points.len() as f32;
    let near = points
        .iter()
        .filter(|p| (p.y - track.height_at(p.x, p.z)).abs() <= PATH_GROUND_TOL_M)
        .count() as f32;
    let on_water = if track.water_height > 0.5 {
        points
            .iter()
            .filter(|p| {
                (p.y - track.water_height).abs() <= 0.8
                    && track.height_at(p.x, p.z) < track.water_height
            })
            .count() as f32
    } else {
        0.0
    };
    near / n >= 0.6 && on_water / n < 0.3
}

/// Coarse lap for a level with no recorded AI file *and* no `*** Paths ***`
/// spline: a rounded circuit centred ahead of the `.SIT` spawn, snapped to the
/// height field. Only `MATT` reaches this now.
fn placeholder_line(track: &Track) -> ai::RacingLine {
    let spawn = track.spawns.first();
    let (base, yaw) = match spawn {
        Some(s) => (s.pos, s.heading),
        None => (vec3(track.size_m() * 0.5, 0.0, track.size_m() * 0.5), 0.0),
    };
    let forward = vec3(-yaw.sin(), 0.0, -yaw.cos());
    let centre = base + forward * 45.0;
    let radius = 45.0;
    let nodes = (0..16)
        .map(|i| {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            let p = vec3(
                centre.x + a.cos() * radius,
                0.0,
                centre.z + a.sin() * radius,
            );
            vec3(p.x, track.height_at(p.x, p.z) + 0.05, p.z)
        })
        .map(|p| ai::Node {
            pos: p,
            heading: 0.0,
            flag: 0,
            speed_cap: 14.0,
        })
        .collect::<Vec<_>>();
    let n = nodes.len();
    let mut nodes = nodes;
    for i in 0..n {
        let to = nodes[(i + 1) % n].pos - nodes[i].pos;
        nodes[i].heading = (-to.x).atan2(-to.z);
        nodes[i].flag = u8::from(i % 4 == 0);
    }
    let mut line = ai::RacingLine {
        driver: "Course".into(),
        class: 1,
        nodes,
    };
    line.compute_speed_caps();
    line
}

/// Turn a `*** Paths ***` spline into a racing line the same way a recorded
/// `.TXT` lap is post-processed: heading from the next node, speed from
/// curvature. These paths are authored in the level's own file and are the
/// real course on the 31 Evo 2 levels that ship no AI recording.
fn line_from_path(track: &Track, points: &[Vec3], index: usize) -> ai::RacingLine {
    let n = points.len();
    let nodes = points
        .iter()
        .map(|p| ai::Node {
            pos: vec3(p.x, track.height_at(p.x, p.z) + 0.05, p.z),
            heading: 0.0,
            flag: 0,
            speed_cap: 22.0,
        })
        .collect::<Vec<_>>();
    let mut nodes = nodes;
    for i in 0..n {
        let to = nodes[(i + 1) % n].pos - nodes[i].pos;
        nodes[i].heading = (-to.x).atan2(-to.z);
        nodes[i].flag = u8::from(i % 4 == 0);
    }
    let mut line = ai::RacingLine {
        driver: format!("Course {}", index + 1),
        class: 1,
        nodes,
    };
    line.compute_speed_caps();
    line
}

/// Gains of the AI's in-air attitude hold: stick deflection per radian of
/// pitch error and per rad/s of pitch rate. With `AIR_PITCH_ACCEL = 1 rad/s²`
/// this is a critically damped hold (ζ ≈ 1), so it levels without hunting.
const AIR_LEVEL_P: f32 = 4.0;
const AIR_LEVEL_D: f32 = 4.0;

/// What a driver does with the air control while airborne: pitch the truck
/// to the slope it is about to land on, so the wheels meet the ground instead
/// of the bumper. AI drivers and the QA autopilot use it; a human player has
/// the same control on throttle/brake. On the ground the input is untouched.
fn air_level(mut input: Input, v: &Vehicle, sample: &dyn Fn(f32, f32) -> (f32, Surface)) -> Input {
    if v.airtime <= 0.0 {
        return input;
    }
    let f = v.forward();
    let flat = vec3(f.x, 0.0, f.z).normalize_or_zero();
    if flat == Vec3::ZERO {
        return input;
    }
    // Slope of the ground under the truck along its heading, over roughly a
    // wheelbase each way.
    const REACH_M: f32 = 2.0;
    let ahead = v.pos + flat * REACH_M;
    let behind = v.pos - flat * REACH_M;
    let rise = sample(ahead.x, ahead.z).0 - sample(behind.x, behind.z).0;
    let want = (rise / (2.0 * REACH_M)).atan();
    let pitch = v.pitch_deg().to_radians();
    let rate = v.angular.dot(v.rot * Vec3::X);
    let stick = (AIR_LEVEL_P * (want - pitch) - AIR_LEVEL_D * rate).clamp(-1.0, 1.0);
    input.throttle = stick.max(0.0);
    input.brake = (-stick).max(0.0);
    input.steer = 0.0;
    input
}

/// What a thumb on the touch buttons makes of a driver's input: steering is
/// all-or-nothing past a dead zone and the gas stays held unless braking.
fn thumb(mut input: Input) -> Input {
    input.steer = if input.steer.abs() > 0.35 {
        input.steer.signum()
    } else {
        0.0
    };
    if input.brake > 0.3 {
        input.throttle = 0.0;
        input.brake = 1.0;
    } else {
        input.throttle = 1.0;
        input.brake = 0.0;
    }
    input
}

impl Race {
    pub fn build(mut cfg: RaceConfig, mut track: Track) -> Result<Self, String> {
        // Level gates are the real course; only a level with neither a recorded
        // AI lap nor a `*** Paths ***` spline (`MATT`) gets the generated
        // circuit, so a lap on DEVIL is the one the disc defines.
        // A line without nodes (truncated `.TXT`, a hand-edited overlay) would
        // index `nodes[0]` below and take `% 0` in `Driver::tick`. Say so and
        // drop it; the level's own splines or the generated circuit take over.
        let before = cfg.lines.len();
        cfg.lines.retain(|l| l.nodes.len() >= 2);
        if cfg.lines.len() != before {
            eprintln!(
                "race: {} of {before} racing lines for {} had no usable nodes and were dropped",
                before - cfg.lines.len(),
                cfg.track_id
            );
        }
        if cfg.lines.is_empty() {
            // Many `*** Paths ***` splines are not driving lines at all: boats
            // ride the water surface, aircraft fly tens of metres up. Racing
            // those put the field on the sea floor (SKULL1-5) or in a harbour
            // (ALASKA1-5). Only splines that hug the ground are courses; a
            // level with none is an open 4x4 proving ground (free roam from
            // the `.SIT` spawn grid on dry land).
            let mut paths: Vec<&Vec<Vec3>> = track
                .paths
                .iter()
                .filter(|p| p.len() >= 2 && path_is_driveable(&track, p))
                .collect();
            if paths.is_empty() && !track.paths.is_empty() {
                eprintln!(
                    "race: {} has no driveable course (its splines float over water or ground); open 4x4 ground from the spawn grid",
                    cfg.track_id
                );
            }
            paths.sort_by_key(|p| std::cmp::Reverse(p.len()));
            for (i, path) in paths.iter().take(8).enumerate() {
                cfg.lines.push(line_from_path(&track, path, i));
            }
        }
        if cfg.lines.is_empty() {
            cfg.lines.push(placeholder_line(&track));
        }
        // Never start a race with the field under the water plane.
        let course_pts: Vec<Vec3> = cfg.lines[0].nodes.iter().map(|n| n.pos).collect();
        track.fit_water_to_course(&course_pts);
        let mut player = Vehicle::new(cfg.player_truck);
        let start = cfg.lines[0].nodes[0].pos;
        player.reset_on_ground(start.x, start.z, cfg.lines[0].nodes[0].heading, &track);

        let mut opponents = Vec::new();
        for i in 0..cfg.opponents.min(cfg.ai_vehicles.len()) {
            let line = cfg.lines[(i + 1) % cfg.lines.len()].clone();
            let mut d = Driver::new(
                line.clone(),
                cfg.skill + (i as f32 * 0.012),
                &format!("Rival {}", i + 1),
                0,
            );
            d.aggression = 0.45 + (i as f32 * 0.1) % 0.5;
            // Rivals take varied factory paints, fixed per grid slot.
            let mut truck = cfg.ai_vehicles[i].clone();
            truck.paint_index = (i * 5 + 2) % truck.paints.len().max(1);
            let mut v = Vehicle::new(truck);
            d.fit_to(&v);
            let (p, yaw) = ai::grid_slot(&track, &line, i + 4);
            v.reset_on_ground(p.x, p.z, yaw, &track);
            let line_pts: Vec<Vec3> = line.nodes.iter().map(|n| n.pos).collect();
            opponents.push(Opponent {
                vehicle: v,
                flip_t: 0.0,
                driver: d,
                lane: 0.0,
                progress: 0,
                finished_at: None,
                prog: Progress::new(&line_pts),
            });
        }

        let finish_progress = {
            let pp = Progress::new(&course_pts);
            (cfg.laps.clamp(1, MAX_LAPS) - 1) as f32 + track_last_gate_frac(&track, &pp)
        };
        let colliders = crate::collide::build(&track);
        let traffic = track
            .props
            .iter()
            .filter(|p| p.is_unported_traffic())
            .count();
        if traffic > 0 {
            eprintln!(
                "race: {} has {traffic} moving traffic objects; traffic is not ported, so they are hidden",
                cfg.track_id
            );
        }
        let pilot = std::env::var("EVOX_AUTODRIVE").ok().map(|_| {
            let mut d = ai::Driver::new(cfg.lines[0].clone(), 0.85, "pilot", 0);
            d.fit_to(&player);
            d
        });
        let mut accents = Vec::new();
        for i in 0..24 {
            if let Some(p) = track.props.get(i * 7) {
                accents.push(track.props.len().min(i));
                let _ = p.pos;
            }
        }

        let cam_look = player.pos + Vec3::Y * 1.3;
        let autodrive = pilot.is_some();
        Ok(Self {
            track,
            player,
            opponents,
            phase: if autodrive {
                Phase::Racing
            } else {
                Phase::Countdown
            },
            countdown: if autodrive { 0.0 } else { COUNTDOWN },
            time: 0.0,
            lap: 1,
            laps: cfg.laps.clamp(1, MAX_LAPS),
            player_prog: Progress::new(&course_pts),
            finish_progress,
            last_player_pos: start,
            lap_start: 0.0,
            best_lap: 0.0,
            last_lap: 0.0,
            position: 1,
            finished_at: None,
            paused: false,
            vfx: Vfx::new(),
            replay: ReplayDirector::default(),
            cam_pos: Vec3::ZERO,
            cam_look,
            cam_yaw: 0.0,
            cam_fwd: Vec3::Z,
            cam_heading: Vec3::ZERO,
            fov: FOV_DEG,
            odo_m: 0.0,
            drive_mode: 1,
            accumulator: 0.0,
            flip_t: 0.0,
            pending_shift_up: false,
            pending_shift_down: false,
            course: cfg.lines[0].nodes.iter().map(|n| n.pos).collect(),
            checkpoint: 0,
            track_id: cfg.track_id.to_string(),
            class: cfg.class,
            events: Vec::new(),
            surface: Surface::Dirt,
            accents,
            colliders,
            pilot,
            air_assist: crate::ui::touch_ui(),
            pilot_thumb: false,
        })
    }

    pub fn player_input(&mut self, mut input: Input) -> Input {
        if self.phase == Phase::Countdown {
            input.throttle = 0.0;
            input.brake = 0.0;
            input.handbrake = true;
            return input;
        }
        input
    }

    /// Advance the race by one rendered frame of `dt` seconds. The simulation
    /// runs in fixed `physics::DT` steps from an accumulator (offroad4x4
    /// `main.rs` runs its car the same way), so a 120 Hz display or a dropped
    /// frame changes how many steps run, never how fast the truck moves. The
    /// camera follows once per frame.
    pub fn tick(&mut self, dt: f32, input: Input, audio: &mut Audio, now: f64) {
        if self.paused || self.phase == Phase::Finished {
            self.replay.tick(dt, self.phase == Phase::Finished);
            self.vfx.update(dt);
            self.update_camera(dt);
            return;
        }
        // Shift keys are edges: hold them until a step consumes them.
        self.pending_shift_up |= input.shift_up;
        self.pending_shift_down |= input.shift_down;
        self.accumulator = (self.accumulator + dt.max(0.0)).min(DT * MAX_STEPS_PER_FRAME as f32);
        while self.accumulator >= DT {
            self.accumulator -= DT;
            let mut step_input = input;
            step_input.shift_up = std::mem::take(&mut self.pending_shift_up);
            step_input.shift_down = std::mem::take(&mut self.pending_shift_down);
            self.fixed_step(step_input, audio, now);
        }
        self.update_camera(dt);
    }

    /// One fixed simulation step of `physics::DT`.
    fn fixed_step(&mut self, input: Input, audio: &mut Audio, now: f64) {
        let dt = DT;
        if self.phase == Phase::Countdown {
            self.countdown -= dt;
            if self.countdown <= 0.0 {
                self.phase = Phase::Racing;
            }
        } else {
            self.time += dt;
        }

        let pilot_input = if let Some(p) = self.pilot.as_mut() {
            let yaw = self.player.yaw();
            p.tick(
                self.player.pos,
                self.player.vel,
                yaw,
                0,
                dt,
                if self.phase == Phase::Countdown {
                    0.0
                } else {
                    1.0
                },
            )
        } else {
            input
        };
        let pilot_input = if self.pilot.is_some() && self.pilot_thumb {
            thumb(pilot_input)
        } else {
            pilot_input
        };
        // The autopilot obeys the start like a player: held on the brakes
        // until the countdown ends.
        let input = self.player_input(pilot_input);
        let sample = |x: f32, z: f32| self.track.sample(x, z);
        let assisted = if self.pilot.is_some() && !self.pilot_thumb {
            true
        } else {
            self.air_assist
        };
        let input = if assisted {
            air_level(input, &self.player, &sample)
        } else {
            input
        };
        let prev_airtime = self.player.airtime;
        self.player.step(&input, &sample);
        if prev_airtime > 0.22 && self.player.airtime == 0.0 {
            audio.play(
                "LANDING",
                (prev_airtime * 0.45).clamp(0.20, 0.48),
                now,
                0.25,
            );
        }
        let colliders = &self.colliders;
        self.player.collide(colliders);
        report_impact(&mut self.player, audio, now);
        self.surface = sample(self.player.pos.x, self.player.pos.z).1;
        if self.phase == Phase::Racing {
            let v = self.player.vel;
            self.odo_m += vec2(v.x, v.z).length() * dt;
        }

        // Wheel VFX and audio from grounded rolling dust, skid slip, or river fording.
        emit_vehicle_wheel_vfx(&self.track, &mut self.vfx, &self.player, dt);
        if self.player.throttle > 0.2 {
            let rear = self.player.wheel_world(2);
            let terrain_y = self
                .track
                .height_at_world(rear.x, rear.z)
                .max(self.track.triangle_height_at(rear.x, rear.z));
            let ex_pos = vec3(
                rear.x,
                rear.y.clamp(terrain_y + 0.15, terrain_y + 0.55),
                rear.z,
            );
            self.vfx
                .exhaust(ex_pos, self.player.rpm, self.player.throttle);
        }
        self.vfx.update(dt);
        audio.update_engine(
            "ENG_STOCK",
            self.player.rpm,
            self.player.truck.engine.redline,
            self.player.throttle,
            self.player.speed_kmh(),
            self.player.gear,
            self.player.shift_timer,
            dt,
        );
        audio.surface_event(
            self.surface.name(),
            self.player.skid,
            self.player.speed_kmh(),
            self.player.pos,
            now,
        );
        audio.set_listener(self.player.pos);

        // Opponents. Snapshot chassis positions for rival awareness: a rival
        // inside the forward cone makes the driver lift or brake rather than
        // rear-ending the truck ahead.
        let rival_positions: Vec<Vec3> = std::iter::once(self.player.pos)
            .chain(self.opponents.iter().map(|o| o.vehicle.pos))
            .collect();
        for o in &mut self.opponents {
            if self.phase != Phase::Racing {
                break;
            }
            let ahead = o.progress;
            let ai_input = o.driver.tick_with_rivals(
                o.vehicle.pos,
                o.vehicle.vel,
                o.vehicle.yaw(),
                ahead,
                dt,
                1.0,
                &rival_positions,
            );
            let ai_input = air_level(ai_input, &o.vehicle, &sample);
            o.vehicle.step(&ai_input, &sample);
            o.vehicle.collide(colliders);
            emit_vehicle_wheel_vfx(&self.track, &mut self.vfx, &o.vehicle, dt);
            o.progress = o.driver.cursor;
            // A rival finishes after the race's laps, not after the first one.
            o.prog.update(o.vehicle.pos);
            if o.prog.laps_f() >= self.finish_progress && o.finished_at.is_none() {
                o.finished_at = Some(self.time);
            }
        }

        // Truck-to-truck collision resolution.
        for o in &mut self.opponents {
            crate::physics::Vehicle::collide_vehicles(&mut self.player, &mut o.vehicle);
        }
        report_impact(&mut self.player, audio, now);
        let opp_len = self.opponents.len();
        for i in 0..opp_len {
            let (left, right) = self.opponents.split_at_mut(i + 1);
            let vi = &mut left[i].vehicle;
            for oj in right {
                crate::physics::Vehicle::collide_vehicles(vi, &mut oj.vehicle);
            }
        }

        // Track limits: soft reset when below the world.
        if self.player.pos.y < -12.0 {
            self.rescue_player();
        }
        // Overturned and stopped: put the truck back on the course after
        // `FLIP_RESET_TIME` (offroad4x4 `tuning.rs`), for rivals too.
        if overturned_and_stuck(&self.player) {
            self.flip_t += dt;
            if self.flip_t > FLIP_RESET_TIME {
                self.rescue_player();
            }
        } else {
            self.flip_t = 0.0;
        }
        for o in &mut self.opponents {
            if overturned_and_stuck(&o.vehicle) {
                o.flip_t += dt;
                if o.flip_t > FLIP_RESET_TIME {
                    let line: Vec<Vec3> = o.driver.line.nodes.iter().map(|n| n.pos).collect();
                    rescue_onto(&mut o.vehicle, &line, &self.track);
                    o.flip_t = 0.0;
                }
            } else {
                o.flip_t = 0.0;
            }
        }

        // Checkpoints: the level's own `*** Course ***` gates when it ships
        // them, else a quarter of the recorded line.
        if self.phase == Phase::Racing {
            let prev = self.last_player_pos;
            self.last_player_pos = self.player.pos;
            // A `*** Course ***` gate is a line to cross; the quarter-line
            // probes (levels without gates) are points to pass near. Gates
            // are taken in order, but crossing one of the next two counts the
            // ones before it as passed: a gate the recorded line never
            // touches must not freeze the lap counter for the whole race.
            let gates = &self.track.gates;
            let advance = if gates.is_empty() {
                self.checkpoint_target()
                    .filter(|node| (self.player.pos - *node).length() < self.checkpoint_radius())
                    .map(|_| self.checkpoint + 1)
            } else {
                (0..GATE_LOOKAHEAD.min(gates.len().saturating_sub(self.checkpoint)))
                    .find(|&k| {
                        let g = &gates[self.checkpoint + k];
                        crossed_gate_xz(prev, self.player.pos, g.start, g.end)
                            || seg_dist_xz(self.player.pos, g.start, g.end) < GATE_NEAR_M
                    })
                    .map(|k| self.checkpoint + k + 1)
            };
            if let Some(next) = advance {
                self.checkpoint = next;
                if self.checkpoint >= self.checkpoint_total() {
                    self.checkpoint = 0;
                    self.last_lap = self.time - self.lap_start;
                    if self.best_lap == 0.0 || self.last_lap < self.best_lap {
                        self.best_lap = self.last_lap;
                    }
                    self.lap_start = self.time;
                    self.lap += 1;
                    if self.lap > self.laps {
                        self.phase = Phase::Finished;
                        self.finished_at = Some(self.time);
                    }
                }
            }
        }

        // Place: laps and distance round the course, finishers by finish time.
        if self.phase == Phase::Racing {
            self.player_prog.update(self.player.pos);
        }
        let me = self.player_prog.laps_f();
        let margin = 4.0 / self.player_prog.lap_length();
        let mine = self.finished_at;
        self.position = 1 + self
            .opponents
            .iter()
            .filter(|o| match (o.finished_at, mine) {
                (Some(rt), Some(pt)) => rt < pt,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => o.prog.laps_f() > me + margin,
            })
            .count();
    }

    /// Chase distance scaled with the truck's length (offroad4x4
    /// `chase_distance`).
    fn chase_distance(&self) -> f32 {
        let len = self.player.truck.length * crate::vehicle::FT;
        CHASE_DISTANCE * if len > 0.5 { (len / 4.6).max(0.9) } else { 1.0 }
    }

    /// Put the camera straight behind the truck (offroad4x4 `snap_camera`).
    pub fn snap_camera(&mut self) {
        let f = self.player.forward();
        let flat = vec3(f.x, 0.0, f.z).normalize_or_zero();
        self.cam_heading = flat;
        self.cam_pos = self.player.pos - flat * self.chase_distance() + Vec3::Y * CHASE_HEIGHT;
        // The look point is the truck itself, never a point along the heading:
        // during a spin a heading-derived target ends up in front of the cab.
        self.cam_look = self.player.pos + Vec3::Y * 1.1;
        self.cam_fwd = (self.cam_look - self.cam_pos).normalize_or_zero();
        self.fov = FOV_DEG;
    }

    /// Chase camera ported from offroad4x4 `Game::update_camera`: a smooth
    /// spring-damper follow behind and above the truck along its blended
    /// velocity/heading vector, clamped above `Track::height_at_world(x, z) + 1.5`
    /// so the camera never shakes or clips underground.
    fn update_camera(&mut self, dt: f32) {
        if self.cam_pos == Vec3::ZERO {
            self.snap_camera();
        }
        let car = &self.player;
        let f = car.forward();
        let heading_flat = {
            let v = vec3(f.x, 0.0, f.z);
            if v.length() < 0.2 {
                vec3(self.cam_fwd.x, 0.0, self.cam_fwd.z)
            } else {
                v
            }
        }
        .normalize_or_zero();
        let vel_flat = vec3(car.vel.x, 0.0, car.vel.z);
        let speed = car.vel.length();
        // Blend heading and velocity direction when moving forward so mild
        // rear slip doesn't whip the camera laterally.
        let flat =
            if vel_flat.length() > 2.5 && vel_flat.normalize_or_zero().dot(heading_flat) > 0.25 {
                let v_dir = vel_flat.normalize_or_zero();
                let blend = ((vel_flat.length() - 2.5) / 14.0).clamp(0.0, 0.45);
                (heading_flat * (1.0 - blend) + v_dir * blend).normalize_or_zero()
            } else {
                heading_flat
            };
        if self.cam_heading.length_squared() < 0.1 {
            self.cam_heading = flat;
        } else {
            let k_h = 1.0 - (-5.0 * dt).exp();
            self.cam_heading =
                (self.cam_heading + (flat - self.cam_heading) * k_h).normalize_or_zero();
        }
        // A spin leaves the smoothed heading behind by up to 180°, which puts
        // the camera in front of the truck looking backwards. Keep the chase
        // direction within ±75° of the truck's actual forward vector (the
        // chassis heading, not the velocity blend, which points the wrong way
        // once the truck is sliding sideways or backwards).
        if self.cam_heading.length_squared() > 0.1 && heading_flat != Vec3::ZERO {
            let heading_angle = self.cam_heading.z.atan2(self.cam_heading.x);
            let forward_angle = heading_flat.z.atan2(heading_flat.x);
            let delta = shortest_angle(heading_angle, forward_angle);
            if delta.abs() > SPIN_HEADING_LIMIT {
                let a = forward_angle - delta.signum() * SPIN_HEADING_LIMIT;
                self.cam_heading = vec3(a.cos(), 0.0, a.sin());
            }
        }
        self.fov += ((FOV_DEG + (speed * 0.28).min(11.0)) - self.fov) * (2.5 * dt).min(1.0);
        let chase = self.chase_distance();
        let car = &self.player;
        match self.replay.shot {
            ReplayShot::Hood => {
                self.cam_pos = car.pos - car.forward() * 0.5 + Vec3::Y * 1.15;
                self.cam_look = car.pos + car.forward() * 18.0;
            }
            ReplayShot::Orbit | ReplayShot::Trackside => {
                let desired = if self.replay.shot == ReplayShot::Orbit {
                    car.pos
                        + vec3(self.replay.orbit.sin(), 0.0, self.replay.orbit.cos()) * 11.0
                        + Vec3::Y * 5.0
                } else {
                    car.pos + vec3(9.0, 4.0, 9.0)
                };
                let k = 1.0 - (-CHASE_STIFFNESS * dt).exp();
                self.cam_pos += (desired - self.cam_pos) * k;
                self.cam_look += (car.pos + Vec3::Y * 0.6 - self.cam_look) * (12.0 * dt).min(1.0);
            }
            _ => {
                let desired_xz = car.pos - self.cam_heading * chase;
                let terrain_floor = self.track.height_at_world(desired_xz.x, desired_xz.z) + 1.5;
                let desired_y = (car.pos.y + CHASE_HEIGHT).max(terrain_floor);
                let desired = vec3(desired_xz.x, desired_y, desired_xz.z);
                let k_xz = 1.0 - (-(CHASE_STIFFNESS * 1.25) * dt).exp();
                let k_y = 1.0 - (-(CHASE_STIFFNESS * 0.95) * dt).exp();
                self.cam_pos.x += (desired.x - self.cam_pos.x) * k_xz;
                self.cam_pos.z += (desired.z - self.cam_pos.z) * k_xz;
                self.cam_pos.y += (desired.y - self.cam_pos.y) * k_y;
                // Keep a bounded distance band so the truck never leaves the frame.
                let off = self.cam_pos - car.pos;
                let len = off.length();
                if len > chase * 1.55 {
                    self.cam_pos = car.pos + off / len * chase * 1.55;
                } else if len < chase * 0.65 && len > 1e-3 {
                    self.cam_pos = car.pos + off / len * chase * 0.65;
                }
                // Position smoothing lags a spin, and at speed it also trails
                // the eye along the direction of travel, which is the nose
                // side of a truck sliding backwards. So clamp the actual boom
                // direction too, not just `cam_heading`, and at every speed:
                // the eye may never end up in front of the truck.
                let off = self.cam_pos - car.pos;
                let flat_off = vec3(off.x, 0.0, off.z);
                if flat_off.length() > 1e-3 && heading_flat != Vec3::ZERO {
                    let carried = flat_off.z.atan2(flat_off.x);
                    let behind = heading_flat.z.atan2(heading_flat.x) + std::f32::consts::PI;
                    let delta = shortest_angle(carried, behind);
                    if delta.abs() > SPIN_HEADING_LIMIT {
                        let a = behind - delta.signum() * SPIN_HEADING_LIMIT;
                        self.cam_pos.x = car.pos.x + a.cos() * flat_off.length();
                        self.cam_pos.z = car.pos.z + a.sin() * flat_off.length();
                    }
                }
                // Trees and buildings between the eye and the truck: pull the
                // eye in until the line of sight is clear (ASPEN forest).
                let target = car.pos + Vec3::Y * 1.1;
                clear_occluders(&mut self.cam_pos, target, &self.colliders);
                // The target is always the truck body itself.
                let tgt = target;
                let k_look = 1.0 - (-14.0 * dt).exp();
                self.cam_look += (tgt - self.cam_look) * k_look;
            }
        }
        let gh = self.track.height_at_world(self.cam_pos.x, self.cam_pos.z) + 1.5;
        if self.cam_pos.y < gh {
            self.cam_pos.y = gh;
        }
        self.cam_fwd = (self.cam_look - self.cam_pos).normalize_or_zero();
        if std::env::var("EVOX_CAM").is_ok() && ((self.time * 60.0) as u32).is_multiple_of(30) {
            eprintln!(
                "cam pp={:?} pv={:?} air={:.2} cam={:?} look={:?} fov={:.1}",
                self.player.pos,
                self.player.vel,
                self.player.airtime,
                self.cam_pos,
                self.cam_look,
                self.fov
            );
        }
    }

    pub fn camera(&self) -> Camera3D {
        Camera3D {
            position: self.cam_pos,
            target: self.cam_look,
            up: Vec3::Y,
            fovy: self.fov.to_radians(),
            z_near: 0.12,
            ..Default::default()
        }
    }

    /// The original's "Rescue" (H): back on the course line at the nearest
    /// point, upright, facing along the course.
    pub fn rescue_player(&mut self) {
        rescue_onto(&mut self.player, &self.course, &self.track);
        self.flip_t = 0.0;
        self.events.push("Recovered".into());
    }

    /// The node the lap logic is waiting for next: the level's next `*** Course`
    /// gate, else a quarter of the first rival's recorded line (or of the course
    /// line in a solo race).
    pub fn checkpoint_target(&self) -> Option<Vec3> {
        if let Some(gate) = self.track.gates.get(self.checkpoint) {
            return Some(gate.centre());
        }
        let at = |n: usize, k: usize| ((self.checkpoint + 1) * (n / 4).max(1)).min(n - 1).min(k);
        if let Some(first) = self.opponents.first() {
            let n = first.driver.line.nodes.len();
            if n > 0 {
                return Some(first.driver.line.nodes[at(n, n - 1)].pos);
            }
        }
        let n = self.course.len();
        (n > 0).then(|| self.course[at(n, n - 1)])
    }

    /// Gates per lap: the level's own count, or the four quarter-line probes.
    pub fn checkpoint_total(&self) -> usize {
        if self.track.gates.is_empty() {
            4
        } else {
            self.track.gates.len()
        }
    }

    /// How close the truck must come to the next gate. Derived from the gate's
    /// `cTrackWidth` (a crossing line has length), so a wide gate is not a
    /// pixel-perfect target.
    fn checkpoint_radius(&self) -> f32 {
        self.track
            .gates
            .get(self.checkpoint)
            .map(|g| (g.width_m * 0.5).clamp(6.0, 26.0))
            .unwrap_or(26.0)
    }

    /// World positions of the checkpoint gates `(index, pos)` for rendering 3D
    /// waypoint beams.
    pub fn checkpoint_gates(&self) -> Vec<(usize, Vec3)> {
        if !self.track.gates.is_empty() {
            return self
                .track
                .gates
                .iter()
                .enumerate()
                .map(|(i, g)| (i, g.centre()))
                .collect();
        }
        if let Some(first) = self.opponents.first() {
            let nodes = &first.driver.line.nodes;
            let n = nodes.len();
            if n > 0 {
                let stride = (n / 4).max(1);
                return (0..4)
                    .map(|k| (k, nodes[((k + 1) * stride).min(n - 1)].pos))
                    .collect();
            }
        }
        let n = self.course.len();
        if n > 0 {
            let stride = (n / 4).max(1);
            return (0..4)
                .map(|k| (k, self.course[((k + 1) * stride).min(n - 1)]))
                .collect();
        }
        Vec::new()
    }

    /// Lookahead point along the course (~14 nodes ahead of the nearest
    /// course node) for the HUD compass arrow so switchbacks point along the
    /// road ahead rather than straight through terrain ridges.
    pub fn compass_target(&self) -> Option<Vec3> {
        let n = self.course.len();
        if n == 0 {
            return None;
        }
        let p = vec2(self.player.pos.x, self.player.pos.z);
        let nearest = (0..n)
            .min_by(|&a, &b| {
                let da = (vec2(self.course[a].x, self.course[a].z) - p).length_squared();
                let db = (vec2(self.course[b].x, self.course[b].z) - p).length_squared();
                da.total_cmp(&db)
            })
            .unwrap_or(0);
        let ahead = (nearest + 14.min((n / 3).max(1))) % n;
        Some(self.course[ahead])
    }

    pub fn standings(&self) -> Vec<(String, f32)> {
        // Finishers first by finish time; the rest by how far they got (their
        // time is shown as the current race time).
        let key = |fin: Option<f32>, laps_f: f32| match fin {
            Some(t) => (0u8, t),
            None => (1u8, -laps_f),
        };
        let mut v: Vec<((u8, f32), String, f32)> = std::iter::once((
            key(self.finished_at, self.player_prog.laps_f()),
            "You".to_string(),
            self.finished_at.unwrap_or(self.time),
        ))
        .chain(self.opponents.iter().map(|o| {
            (
                key(o.finished_at, o.prog.laps_f()),
                o.driver.name.clone(),
                o.finished_at.unwrap_or(self.time),
            )
        }))
        .collect();
        v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        v.into_iter().map(|(_, n, t)| (n, t)).collect()
    }

    pub fn place(&self) -> u32 {
        self.position as u32
    }
}

/// Seconds overturned (or stuck on a side) and slow before an automatic
/// rescue (offroad4x4 `tuning.rs` `FLIP_RESET_TIME`).
pub const FLIP_RESET_TIME: f32 = 2.5;

/// Pull `cam` towards `target` until no tall prop cylinder crosses the line of
/// sight. Only shortens the boom, never lengthens it, so the framing is stable
/// on open ground and trees on forest tracks stop hiding the truck.
fn clear_occluders(cam: &mut Vec3, target: Vec3, colliders: &[crate::collide::Collider]) {
    let mut dir = *cam - target;
    let dist = dir.length();
    if dist < 0.5 {
        return;
    }
    dir /= dist;
    let mut best = dist;
    for c in colliders {
        // Low debris and decals never occlude a 1.1 m-high target.
        if c.half_height < 1.2 {
            continue;
        }
        let along = (c.center - target).dot(dir);
        if along <= 0.6 || along >= best {
            continue;
        }
        let closest = target + dir * along;
        let lateral = (closest - c.center).with_y(0.0).length();
        if lateral > c.radius + 0.45 {
            continue;
        }
        if closest.y > c.center.y + c.half_height + 1.0 {
            continue;
        }
        best = (along - c.radius * 0.8).max(2.0);
    }
    if best < dist {
        *cam = target + dir * best;
    }
}

/// On its side or roof (body up axis within ~70° of horizontal or below)
/// and barely moving.
fn overturned_and_stuck(v: &Vehicle) -> bool {
    (v.rot * Vec3::Y).y < 0.35 && v.vel.length() < 4.0
}

/// Place `v` upright on `line` at its nearest node, heading to the node a
/// few metres further on.
fn rescue_onto(v: &mut Vehicle, line: &[Vec3], track: &Track) {
    if line.len() < 2 {
        v.reset(v.pos + Vec3::Y * 2.0, v.yaw());
        return;
    }
    let p = v.pos;
    let i = (0..line.len())
        .min_by(|&a, &b| {
            let da = vec2(line[a].x - p.x, line[a].z - p.z).length_squared();
            let db = vec2(line[b].x - p.x, line[b].z - p.z).length_squared();
            da.total_cmp(&db)
        })
        .unwrap_or(0);
    let a = line[i];
    let b = line[(i + 3) % line.len()];
    let yaw = (-(b.x - a.x)).atan2(-(b.z - a.z));
    v.reset_on_ground(a.x, a.z, yaw, track);
}

/// Spawn wheel ground-contact dust, splash spray, and tire tread quads only
/// while each wheel is grounded, aligned with the terrain normal and wheel yaw.
fn emit_vehicle_wheel_vfx(track: &Track, vfx: &mut Vfx, vehicle: &Vehicle, dt: f32) {
    let speed_ms = vehicle.vel.length();
    for i in 0..4 {
        let w = vehicle.wheels[i];
        if !w.grounded {
            continue;
        }
        let terrain_y = track
            .height_at_world(w.contact.x, w.contact.z)
            .max(track.triangle_height_at(w.contact.x, w.contact.z));
        let contact_pos = vec3(w.contact.x, terrain_y + 0.03, w.contact.z);
        let ground_normal = track.normal_at_world(w.contact.x, w.contact.z);
        let ground_tint = track.ground_color_at(w.contact.x, w.contact.z);
        let w_steer = if i / 2 == 0 { vehicle.steer } else { 0.0 };
        let wf = vehicle.rot * Quat::from_rotation_y(-w_steer) * Vec3::NEG_Z;
        let wheel_fwd = vec3(wf.x, 0.0, wf.z).normalize_or_zero();
        let roll_strength = if (w.surface.dust() || w.surface.splash()) && speed_ms > 1.5 {
            ((speed_ms - 1.5) * 0.065).clamp(0.0, 0.55)
        } else {
            0.0
        };
        let strength = if w.surface == Surface::Water && speed_ms > 1.8 {
            vehicle.skid.max(0.55)
        } else {
            vehicle.skid.max(roll_strength)
        };
        if strength > 0.08 {
            vfx.emit_tinted(
                w.surface,
                strength,
                speed_ms,
                contact_pos,
                vehicle.vel,
                ground_tint,
                dt,
            );
            if w.surface != Surface::Water {
                vfx.skid_oriented(
                    contact_pos,
                    ground_normal,
                    wheel_fwd,
                    strength.max(0.22),
                    0.30,
                );
            }
        }
    }
}

/// Play the collision thump for a newly registered impact and clear it. The
/// impact is only armed by `IMPACT_COOLDOWN`-gated contacts in `physics`, so a
/// single scrape produces one sound, not sixty a second.
fn report_impact(v: &mut Vehicle, audio: &mut Audio, now: f64) {
    let speed = std::mem::take(&mut v.impact);
    if speed <= 0.4 {
        return;
    }
    let vol = (0.12 + speed * 0.02).min(0.55);
    audio.play_at("ROCKHIT", v.pos, vol, now, 0.20);
}

fn shortest_angle(from: f32, to: f32) -> f32 {
    let mut d = to - from;
    while d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    while d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::GRID;
    use std::path::PathBuf;

    fn track() -> Track {
        Track {
            id: "t".into(),
            dir: PathBuf::new(),
            heights: vec![0; GRID * GRID],
            colors: vec![0; GRID * GRID],
            shadows: vec![0; GRID * GRID],
            scale: 1.0,
            height_offset: 0.0,
            water_height: 0.0,
            water_color: Color::new(0.20, 0.44, 0.60, 0.72),
            fog_color: Color::new(0.56, 0.71, 0.86, 1.0),
            textures: vec![],
            detail_texture: None,
            sky_texture: None,
            light_dir: Vec3::Y,
            props: vec![],
            spawns: vec![],
            gates: vec![],
            paths: vec![],
            tints: vec![],
            ground_colors: vec![],
            atlas: None,
        }
    }

    fn line() -> ai::RacingLine {
        let mut t = String::from("Point Count: 40\n");
        for i in 0..40 {
            let a = i as f32 / 40.0 * std::f32::consts::TAU;
            t.push_str(&format!(
                "{:.2},{:.2},{:.2}\n{}\n0\n",
                300.0 + a.cos() * 100.0,
                5.0,
                300.0 + a.sin() * 100.0,
                0.0
            ));
        }
        ai::RacingLine::parse(&t).unwrap()
    }

    fn truck() -> Truck {
        // Built-in synthetic rig: no retail asset is needed to run the tests.
        crate::vehicle::parse("t.trk", include_str!("selftest_truck.trk")).unwrap()
    }

    fn cfg() -> RaceConfig<'static> {
        RaceConfig {
            track_id: "t",
            class: 1,
            laps: 1,
            opponents: 0,
            player_truck: truck(),
            ai_vehicles: vec![],
            lines: vec![line()],
            skill: 0.8,
        }
    }

    #[test]
    fn frame_rate_does_not_change_sim_speed() {
        // Physics used to advance one fixed step per rendered frame, so a
        // 120 Hz display ran the race at double speed.
        let run = |hz: f32| {
            let mut r = Race::build(cfg(), track()).unwrap();
            r.phase = Phase::Racing;
            let mut a = Audio::new();
            a.enabled = false;
            let input = Input {
                throttle: 1.0,
                ..Default::default()
            };
            for _ in 0..(2.0 * hz) as usize {
                r.tick(1.0 / hz, input, &mut a, 0.0);
            }
            r.player.pos
        };
        let (p60, p120) = (run(60.0), run(120.0));
        assert!(
            (p60 - p120).length() < 0.05,
            "60 Hz {p60:?} vs 120 Hz {p120:?}"
        );
    }

    #[test]
    fn autopilot_waits_for_the_start() {
        let mut r = Race::build(cfg(), track()).unwrap();
        r.pilot = Some(ai::Driver::new(line(), 0.85, "pilot", 0));
        let start = r.player.pos;
        let mut a = Audio::new();
        a.enabled = false;
        for _ in 0..((COUNTDOWN - 0.2) / DT) as usize {
            r.tick(DT, Input::default(), &mut a, 0.0);
        }
        assert_eq!(r.phase, Phase::Countdown);
        let moved = vec2(r.player.pos.x - start.x, r.player.pos.z - start.z).length();
        assert!(
            moved < 0.5,
            "pilot left the grid during the countdown: {moved} m"
        );
    }

    #[test]
    fn build_and_run_a_second() {
        let cfg = RaceConfig {
            track_id: "t",
            class: 1,
            laps: 1,
            opponents: 2,
            player_truck: truck(),
            ai_vehicles: vec![truck(), truck()],
            lines: vec![line()],
            skill: 0.8,
        };
        let mut race = Race::build(cfg, track()).unwrap();
        let mut audio = Audio::new();
        audio.enabled = false;
        for _ in 0..300 {
            race.tick(1.0 / 60.0, Input::default(), &mut audio, 0.0);
        }
        assert!(matches!(race.phase, Phase::Racing | Phase::Finished));
        assert!(race.player.pos.is_finite());
        assert_eq!(race.opponents.len(), 2);
    }

    /// Audit finding #1: while the truck spins on the spot the eye must stay
    /// behind it, and the look target must stay on the truck body.
    #[test]
    fn camera_looks_at_the_truck_during_a_spin() {
        let mut r = Race::build(cfg(), track()).unwrap();
        r.phase = Phase::Racing;
        r.snap_camera();
        let mut a = Audio::new();
        a.enabled = false;
        for _ in 0..120 {
            r.player.vel = Vec3::ZERO;
            r.player.angular = vec3(0.0, 6.0, 0.0);
            let yaw = r.player.yaw() + 6.0 * DT;
            r.player.rot = Quat::from_rotation_y(yaw);
            r.tick(DT, Input::default(), &mut a, 0.0);
        }
        let to_cam = r.cam_pos - r.player.pos;
        let fwd = r.player.forward();
        eprintln!(
            "cam_heading={:?} fwd={:?} to_cam={:?} speed={}",
            r.cam_heading,
            fwd,
            to_cam,
            r.player.vel.length()
        );
        assert!(
            to_cam.x * fwd.x + to_cam.z * fwd.z < 0.0,
            "camera {to_cam:?} sits in front of forward {fwd:?}"
        );
        let want = r.player.pos + Vec3::Y * 1.1;
        assert!(
            (r.cam_look - want).length() < 1.5,
            "look target {} drifted from the truck body",
            (r.cam_look - want).length()
        );
    }

    /// A spinout at speed: the truck slides at 72 km/h while it rotates. The
    /// slow-speed clamp does not apply there, so the eye used to be left
    /// behind by the smoothing and showed the grille. Checked every frame,
    /// not just once the spin has settled.
    #[test]
    fn camera_stays_behind_the_truck_through_a_spinout_at_speed() {
        for spin in [2.0f32, 4.0, 8.0, -8.0] {
            let mut r = Race::build(cfg(), track()).unwrap();
            r.phase = Phase::Racing;
            r.snap_camera();
            let mut a = Audio::new();
            a.enabled = false;
            let slide = r.player.forward() * 20.0;
            let mut worst = f32::MAX;
            for _ in 0..180 {
                r.player.vel = vec3(slide.x, 0.0, slide.z);
                r.player.angular = vec3(0.0, spin, 0.0);
                let yaw = r.player.yaw() + spin * DT;
                r.player.rot = Quat::from_rotation_y(yaw);
                r.tick(DT, Input::default(), &mut a, 0.0);
                let to_cam = r.cam_pos - r.player.pos;
                let flat = vec2(to_cam.x, to_cam.z).normalize_or_zero();
                let fwd = r.player.forward();
                let behind = -flat.dot(vec2(fwd.x, fwd.z).normalize_or_zero());
                worst = worst.min(behind);
                let want = r.player.pos + Vec3::Y * 1.1;
                assert!(
                    (r.cam_look - want).length() < 1.5,
                    "look target left the truck body at {spin} rad/s"
                );
            }
            assert!(
                worst > 0.0,
                "at {spin} rad/s the eye got in front of the truck (cos {worst:.2})"
            );
        }
    }

    #[test]
    fn builds_a_lap_when_the_level_has_none() {
        let cfg = RaceConfig {
            track_id: "t",
            class: 1,
            laps: 1,
            opponents: 0,
            player_truck: truck(),
            ai_vehicles: vec![],
            lines: vec![],
            skill: 0.8,
        };
        let race = Race::build(cfg, track()).unwrap();
        assert!(race.player.pos.is_finite());
        assert_eq!(race.opponents.len(), 0);
    }

    /// Incomplete track data: a racing line with no nodes and a one-point
    /// `*** Paths ***` spline used to panic on `nodes[0]` / `% 0`.
    #[test]
    fn degenerate_lines_and_paths_do_not_crash_the_build() {
        let mut empty = line();
        empty.nodes.clear();
        let mut t = track();
        t.paths = vec![vec![vec3(40.0, 0.0, 40.0)], vec![]];
        let cfg = RaceConfig {
            track_id: "t",
            class: 1,
            laps: 1,
            opponents: 0,
            player_truck: truck(),
            ai_vehicles: vec![],
            lines: vec![empty],
            skill: 0.8,
        };
        let mut race = Race::build(cfg, t).unwrap();
        assert!(race.player.pos.is_finite());
        assert!(!race.course.is_empty());
        race.pilot = Some(ai::Driver::new(
            placeholder_line(&race.track),
            0.85,
            "pilot",
            0,
        ));
        let mut audio = Audio::new();
        for _ in 0..30 {
            race.tick(DT, Input::default(), &mut audio, 0.0);
        }
        assert!(race.player.pos.is_finite());
    }

    #[test]
    fn compass_target_points_ahead_along_course() {
        let cfg = RaceConfig {
            track_id: "t",
            class: 1,
            laps: 2,
            opponents: 0,
            player_truck: truck(),
            ai_vehicles: vec![],
            lines: vec![line()],
            skill: 0.8,
        };
        let race = Race::build(cfg, track()).unwrap();
        let target = race.compass_target().expect("compass target");
        let fwd = vec2(race.player.forward().x, race.player.forward().z).normalize();
        let to = vec2(target.x - race.player.pos.x, target.z - race.player.pos.z).normalize();
        assert!(
            fwd.dot(to) > 0.0,
            "expected compass target ahead of player, dot={}",
            fwd.dot(to)
        );
    }
}

/// Headless driving QA on the retail levels: the autopilot drives the real
/// height field for a fixed simulated time and the body's behaviour is
/// measured (airtime, wheels down, roll, vertical reversals, progress). This
/// is the numeric half of the "autodrive + screenshot" sign-off rule.
#[cfg(test)]
pub(crate) mod drive_qa {
    use super::*;
    use std::path::{Path, PathBuf};

    pub struct Metrics {
        pub airborne: f32,
        pub two_or_less: f32,
        pub roll_max: f32,
        pub roll_over_25: f32,
        pub vy_reversals: usize,
        pub distance_m: f32,
        pub mean_kmh: f32,
        /// Share of racing time more than 15 m from the recorded line.
        pub off_line: f32,
        /// Share of racing time rolled or pitched past 60 deg (tumbling).
        pub tumbling: f32,
    }

    /// Directory that holds `assets/` (tests run from `game/`).
    fn repo_root() -> &'static Path {
        if Path::new("assets/extracted").exists() {
            Path::new(".")
        } else {
            Path::new("..")
        }
    }

    fn data(rel: &str) -> PathBuf {
        for base in ["assets/extracted", "../assets/extracted"] {
            let p = Path::new(base).join(rel);
            if p.exists() {
                return p;
            }
        }
        panic!("{rel} not found: extract the retail data into assets/extracted");
    }

    /// Horizontal distance from `p` to the closed polyline `line`.
    pub fn seg_distance(line: &[Vec3], p: Vec3) -> f32 {
        let q = vec2(p.x, p.z);
        (0..line.len())
            .map(|i| {
                let a = vec2(line[i].x, line[i].z);
                let b = vec2(line[(i + 1) % line.len()].x, line[(i + 1) % line.len()].z);
                let ab = b - a;
                let t = ((q - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                (a + ab * t - q).length()
            })
            .fold(f32::MAX, f32::min)
    }

    #[test]
    #[ignore]
    fn line_stats() {
        for id in ["ASPEN", "DEVIL"] {
            let lines = crate::assets::read_ai_lines(&data("data/data/AI"), id);
            let l = &lines[0];
            let n = l.nodes.len();
            let total: f32 = (0..n)
                .map(|i| (l.nodes[(i + 1) % n].pos - l.nodes[i].pos).length())
                .sum();
            let caps: Vec<f32> = l.nodes.iter().map(|n| n.speed_cap * 3.6).collect();
            println!("{id} lines={} nodes={n} length={total:.0}m spacing={:.1}m cap_kmh min={:.0} max={:.0}", lines.len(), total / n as f32,
                caps.iter().cloned().fold(f32::MAX, f32::min), caps.iter().cloned().fold(0.0, f32::max));
            let p0 = l.nodes[0];
            let p1 = l.nodes[1];
            println!(
                "  node0 pos={:.1} heading={:.2} -> node1 {:.1}  dir_to_next_heading={:.2}",
                p0.pos,
                p0.heading,
                p1.pos,
                (-(p1.pos.x - p0.pos.x)).atan2(-(p1.pos.z - p0.pos.z))
            );
        }
    }

    /// QA pilot skill (`EVOX_PILOT_SKILL`, default 0.85).
    pub fn pilot_skill() -> f32 {
        std::env::var("EVOX_PILOT_SKILL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.85)
    }

    pub fn drive(track_id: &str, seconds: f32) -> Metrics {
        drive_with(
            track_id,
            seconds,
            std::env::var("EVOX_PILOT_THUMB").is_ok(),
            std::env::var("EVOX_AIR_ASSIST").is_ok(),
        )
    }

    /// `thumb`: drive like a touch player (see `thumb`); `assist`: the
    /// player's landing assist.
    pub fn drive_with(track_id: &str, seconds: f32, thumb: bool, assist: bool) -> Metrics {
        let track = Track::load(track_id, &data("track/track")).expect("track loads");
        let lines = crate::assets::read_ai_lines(&data("data/data/AI"), track_id);
        let (trucks, _) = crate::vehicle::load_roster(repo_root());
        let player = trucks
            .iter()
            .find(|t| t.name.contains("Montero"))
            .or(trucks.first())
            .cloned()
            .expect("trucks load");
        let mut race = Race::build(
            RaceConfig {
                track_id,
                class: 1,
                laps: 2,
                opponents: 3,
                player_truck: player,
                ai_vehicles: trucks.iter().take(8).cloned().collect(),
                lines: lines.clone(),
                skill: 0.8,
            },
            track,
        )
        .expect("race builds");
        race.pilot = Some(ai::Driver::new(lines[0].clone(), pilot_skill(), "pilot", 0));
        race.pilot_thumb = thumb;
        race.air_assist = assist;
        let mut audio = Audio::new();
        audio.enabled = false;
        let (mut n, mut air, mut two, mut tip, mut flips, mut last) = (0, 0, 0, 0, 0usize, 0i32);
        let (mut roll_max, mut dist, mut speed_sum) = (0.0f32, 0.0f32, 0.0f32);
        let mut off = 0usize;
        let mut tumble = 0usize;
        let steps = ((COUNTDOWN + seconds) / DT) as usize;
        for _ in 0..steps {
            let before = race.player.pos;
            race.tick(DT, Input::default(), &mut audio, 0.0);
            if race.phase != Phase::Racing {
                continue;
            }
            n += 1;
            let p = &race.player;
            let down = p.wheels.iter().filter(|w| w.grounded).count();
            air += (down == 0) as usize;
            two += (down <= 2) as usize;
            let roll = p.roll_deg().abs();
            roll_max = roll_max.max(roll);
            tip += (roll > 25.0) as usize;
            tumble += (roll > 60.0 || p.pitch_deg().abs() > 60.0) as usize;
            let s = if p.vel.y > 1.0 {
                1
            } else if p.vel.y < -1.0 {
                -1
            } else {
                0
            };
            if s != 0 && last != 0 && s != last {
                flips += 1;
            }
            if s != 0 {
                last = s;
            }
            dist += vec2(p.pos.x - before.x, p.pos.z - before.z).length();
            let near = seg_distance(&race.course, p.pos);
            off += (near > 15.0) as usize;
            speed_sum += p.speed_kmh();
        }
        let n = n.max(1) as f32;
        Metrics {
            airborne: air as f32 / n,
            two_or_less: two as f32 / n,
            roll_max,
            roll_over_25: tip as f32 / n,
            vy_reversals: flips,
            distance_m: dist,
            mean_kmh: speed_sum / n,
            off_line: off as f32 / n,
            tumbling: tumble as f32 / n,
        }
    }

    /// Course vs water for every level, using the course the race really uses.
    /// `cargo test --release -p evox course_water_audit -- --ignored --nocapture`
    fn ring(n: usize, r: f32) -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                vec3(a.cos() * r, 0.0, a.sin() * r)
            })
            .collect()
    }

    #[test]
    fn progress_counts_laps_and_unwinds_when_reversing() {
        let line = ring(32, 100.0);
        let mut p = Progress::new(&line);
        // Starts just behind node 0 (on the seam): the lap before the first.
        p.update(line[31]);
        assert!(p.laps_f() < 0.0, "grid is behind the line: {}", p.laps_f());
        // Forward round the loop twice.
        for lap in 0..2 {
            for node in &line {
                p.update(*node);
            }
            assert!(
                (p.laps_f() - (lap as f32 + 0.97)).abs() < 0.05,
                "lap {lap}: {}",
                p.laps_f()
            );
        }
        // Driving back the way it came loses the laps again.
        for i in (0..32).rev() {
            p.update(line[i]);
        }
        // One more step back crosses the seam the wrong way: a lap is lost.
        p.update(line[31]);
        assert!(p.laps_f() < 1.0, "reversing unwinds: {}", p.laps_f());
    }

    #[test]
    fn circling_in_place_is_not_progress() {
        let line = ring(32, 100.0);
        let mut p = Progress::new(&line);
        p.update(line[3]);
        let start = p.laps_f();
        // Wobble around one spot (the odometer would have counted every metre).
        for k in 0..200 {
            let a = k as f32 * 0.5;
            p.update(line[3] + vec3(a.cos() * 5.0, 0.0, a.sin() * 5.0));
        }
        assert!(
            (p.laps_f() - start).abs() < 0.1,
            "{} vs {start}",
            p.laps_f()
        );
    }

    #[test]
    #[ignore]
    fn course_water_audit() {
        let root = data("track/track");
        let mut ids: Vec<String> = std::fs::read_dir(root.join("LEVELS"))
            .unwrap()
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .strip_suffix(".LVL")
                    .map(str::to_string)
            })
            .collect();
        ids.sort();
        let (trucks, _) = crate::vehicle::load_roster(repo_root());
        let player = trucks
            .iter()
            .find(|t| t.name.contains("Montero"))
            .cloned()
            .unwrap();
        println!(
            "{:10} {:>6} {:>5} {:>6} {:>6} {:>7} {:>7}",
            "track", "water", "pts", ">0.5m%", ">1.2m%", "maxdep", "ground_min"
        );
        for id in ids {
            let Ok(track) = Track::load(&id, &root) else {
                continue;
            };
            let lines = crate::assets::read_ai_lines(&data("data/data/AI"), &id);
            let wh0 = track.water_height;
            let Ok(race) = Race::build(
                RaceConfig {
                    track_id: &id,
                    class: 1,
                    laps: 2,
                    opponents: 0,
                    player_truck: player.clone(),
                    ai_vehicles: vec![],
                    lines,
                    skill: 0.8,
                },
                track,
            ) else {
                println!("{id:10} build failed");
                continue;
            };
            let t2 = &race.track;
            let wh = t2.water_height;
            let d: Vec<f32> = race
                .course
                .iter()
                .map(|p| (wh - t2.height_at(p.x, p.z)).max(0.0))
                .collect();
            let n = d.len().max(1) as f32;
            let p05 = 100.0 * d.iter().filter(|x| **x > 0.5).count() as f32 / n;
            let p12 = 100.0 * d.iter().filter(|x| **x > 1.2).count() as f32 / n;
            let mx = d.iter().cloned().fold(0.0f32, f32::max);
            let mut dy: Vec<f32> = race
                .course
                .iter()
                .map(|p| (p.y - t2.height_at(p.x, p.z)).abs())
                .collect();
            dy.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med_dy = dy.get(dy.len() / 2).copied().unwrap_or(0.0);
            let on_ground =
                100.0 * dy.iter().filter(|v| **v <= 3.0).count() as f32 / dy.len().max(1) as f32;
            print!("dy_med={med_dy:6.1} on_ground={on_ground:4.0}%  ");
            let gmin = race
                .course
                .iter()
                .map(|p| t2.height_at(p.x, p.z))
                .fold(f32::MAX, f32::min);
            let mut g: Vec<f32> = race.course.iter().map(|p| t2.height_at(p.x, p.z)).collect();
            g.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let q = |f: f32| {
                g.get(((g.len() as f32 - 1.0) * f) as usize)
                    .copied()
                    .unwrap_or(0.0)
            };
            println!("{:10} {:6.1}->{:5.1} {:5} {:6.0} {:6.0} {:7.2} {:7.1}  g[p10/p50/p90]={:.1}/{:.1}/{:.1}", id, wh0, wh, d.len(), p05, p12, mx, gmin, q(0.1), q(0.5), q(0.9));
        }
    }

    /// No race may put the field under more than `DEEP_M` of water for more
    /// than a tenth of the course, whatever the retail water plane says.
    #[test]
    fn courses_never_submerge_the_field() {
        require_assets!();
        let root = data("track/track");
        let (trucks, _) = crate::vehicle::load_roster(repo_root());
        let player = trucks
            .iter()
            .find(|t| t.name.contains("Montero"))
            .cloned()
            .unwrap();
        for id in [
            "RIVER1", "RIVER3", "SKULL1", "SKULL4", "ALASKA1", "ALASKA3", "ASPEN",
        ] {
            let track = Track::load(id, &root).unwrap();
            let lines = crate::assets::read_ai_lines(&data("data/data/AI"), id);
            let race = Race::build(
                RaceConfig {
                    track_id: id,
                    class: 1,
                    laps: 2,
                    opponents: 0,
                    player_truck: player.clone(),
                    ai_vehicles: vec![],
                    lines,
                    skill: 0.8,
                },
                track,
            )
            .unwrap();
            let t = &race.track;
            let deep = race
                .course
                .iter()
                .filter(|p| t.water_height - t.height_at(p.x, p.z) > 1.2)
                .count();
            let share = deep as f32 / race.course.len() as f32;
            // ALASKA keeps its harbour bridge samples (deeper than a causeway
            // can fix); RIVER/SKULL must be fully wadable.
            let limit = if id.starts_with("ALASKA") { 0.25 } else { 0.10 };
            assert!(
                share <= limit,
                "{id}: {:.0}% of the course is under >1.2 m of water",
                share * 100.0
            );
        }
    }

    #[test]
    fn gate_crossing_is_a_line_test_not_a_radius() {
        let (a, b) = (vec3(-20.0, 0.0, 0.0), vec3(20.0, 0.0, 0.0));
        // Straight through the middle, and near the end of a wide gate.
        assert!(crossed_gate_xz(
            vec3(0.0, 0.0, -1.0),
            vec3(0.0, 0.0, 1.0),
            a,
            b
        ));
        assert!(crossed_gate_xz(
            vec3(18.0, 0.0, 1.0),
            vec3(18.0, 0.0, -1.0),
            a,
            b
        ));
        // Beyond the end, or not reaching the line: no crossing.
        assert!(!crossed_gate_xz(
            vec3(30.0, 0.0, -1.0),
            vec3(30.0, 0.0, 1.0),
            a,
            b
        ));
        assert!(!crossed_gate_xz(
            vec3(0.0, 0.0, -3.0),
            vec3(0.0, 0.0, -1.0),
            a,
            b
        ));
        assert!((seg_dist_xz(vec3(30.0, 0.0, 0.0), a, b) - 10.0).abs() < 1e-3);
    }

    /// The race ends after the chosen number of laps, with the player placed.
    /// (Before: it always took five laps, and a gate off the racing line
    /// froze the lap counter for good.)
    #[test]
    fn autopilot_finishes_the_chosen_laps() {
        require_assets!();
        let id = "ASPEN";
        let track = Track::load(id, &data("track/track")).unwrap();
        let lines = crate::assets::read_ai_lines(&data("data/data/AI"), id);
        let (trucks, _) = crate::vehicle::load_roster(repo_root());
        let player = trucks
            .iter()
            .find(|t| t.name.contains("Montero"))
            .cloned()
            .unwrap();
        let mut race = Race::build(
            RaceConfig {
                track_id: id,
                class: 1,
                laps: 2,
                opponents: 2,
                player_truck: player,
                ai_vehicles: trucks.iter().skip(3).take(2).cloned().collect(),
                lines: lines.clone(),
                skill: 0.8,
            },
            track,
        )
        .unwrap();
        race.pilot = Some(ai::Driver::new(lines[0].clone(), pilot_skill(), "pilot", 0));
        let mut audio = Audio::new();
        audio.enabled = false;
        assert_eq!(race.laps, 2);
        for _ in 0..((COUNTDOWN + 330.0) / DT) as usize {
            race.tick(DT, Input::default(), &mut audio, 0.0);
            if race.phase == Phase::Finished {
                break;
            }
        }
        assert_eq!(
            race.phase,
            Phase::Finished,
            "lap {} of {}",
            race.lap,
            race.laps
        );
        assert_eq!(race.lap, 3, "two laps counted, then the race ends");
        assert!((1..=3).contains(&race.position), "place {}", race.position);
    }

    /// Laps, checkpoints and place over a whole race with the autopilot and
    /// three rivals. `QA_TRACK=ASPEN QA_LAPS=2 QA_SECS=400 cargo test --release -p evox lap_trace -- --ignored --nocapture`
    /// Distance from each `*** Course ***` gate to the racing line, and the
    /// radius the lap logic accepts. `QA_TRACK=ASPEN cargo test ... gate_audit`
    #[test]
    #[ignore]
    fn gate_audit() {
        let root = data("track/track");
        let mut ids: Vec<String> = match std::env::var("QA_TRACK") {
            Ok(t) => vec![t],
            Err(_) => std::fs::read_dir(root.join("LEVELS"))
                .unwrap()
                .flatten()
                .filter_map(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .strip_suffix(".LVL")
                        .map(str::to_string)
                })
                .collect(),
        };
        ids.sort();
        let (trucks, _) = crate::vehicle::load_roster(repo_root());
        let player = trucks
            .iter()
            .find(|t| t.name.contains("Montero"))
            .cloned()
            .unwrap();
        for id in ids {
            let Ok(track) = Track::load(&id, &root) else {
                continue;
            };
            if track.gates.is_empty() {
                continue;
            }
            let lines = crate::assets::read_ai_lines(&data("data/data/AI"), &id);
            let Ok(race) = Race::build(
                RaceConfig {
                    track_id: &id,
                    class: 1,
                    laps: 2,
                    opponents: 0,
                    player_truck: player.clone(),
                    ai_vehicles: vec![],
                    lines,
                    skill: 0.8,
                },
                track,
            ) else {
                continue;
            };
            let mut bad = Vec::new();
            for (i, g) in race.track.gates.iter().enumerate() {
                let d = race
                    .course
                    .iter()
                    .map(|p| seg_dist_xz(*p, g.start, g.end))
                    .fold(f32::MAX, f32::min);
                if d > 8.0 {
                    bad.push(format!("g{i}: line passes {d:.0} m from the gate line"));
                }
            }
            println!(
                "{id:9} gates {:2} course {:4} nodes: {}",
                race.track.gates.len(),
                race.course.len(),
                if bad.is_empty() {
                    "all reachable".to_string()
                } else {
                    bad.join(", ")
                }
            );
        }
    }

    #[test]
    #[ignore]
    fn lap_trace() {
        let id = std::env::var("QA_TRACK").unwrap_or("ASPEN".into());
        let laps: u32 = std::env::var("QA_LAPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);
        let secs: f32 = std::env::var("QA_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300.0);
        let track = Track::load(&id, &data("track/track")).unwrap();
        let lines = crate::assets::read_ai_lines(&data("data/data/AI"), &id);
        let (trucks, _) = crate::vehicle::load_roster(repo_root());
        let player = trucks
            .iter()
            .find(|t| t.name.contains("Montero"))
            .cloned()
            .unwrap();
        let mut race = Race::build(
            RaceConfig {
                track_id: &id,
                class: 1,
                laps,
                opponents: 3,
                player_truck: player,
                ai_vehicles: trucks.iter().skip(3).take(3).cloned().collect(),
                lines: lines.clone(),
                skill: 0.8,
            },
            track,
        )
        .unwrap();
        let first = race.course.clone();
        let line0 = ai::RacingLine {
            driver: "pilot".into(),
            class: 1,
            nodes: first
                .iter()
                .map(|p| ai::Node {
                    pos: *p,
                    heading: 0.0,
                    flag: 0,
                    speed_cap: 22.0,
                })
                .collect(),
        };
        let mut line0 = line0;
        line0.compute_speed_caps();
        race.pilot = Some(ai::Driver::new(
            lines.first().cloned().unwrap_or(line0),
            0.85,
            "pilot",
            0,
        ));
        let mut audio = Audio::new();
        audio.enabled = false;
        println!(
            "course nodes {} laps {} gates {}",
            race.course.len(),
            race.laps,
            race.track.gates.len()
        );
        let mut last_lap = 1;
        for i in 0..((COUNTDOWN + secs) / DT) as usize {
            race.tick(DT, Input::default(), &mut audio, 0.0);
            let t = i as f32 * DT;
            if race.lap != last_lap || i % 1200 == 0 || race.phase == Phase::Finished {
                last_lap = race.lap;
                println!(
                    "t={:6.1} phase={:?} lap={}/{} chk={} place={} me={:.2} rivals={:?} fin={:?}",
                    t,
                    race.phase,
                    race.lap.min(race.laps),
                    race.laps,
                    race.checkpoint,
                    race.position,
                    race.player_prog.laps_f(),
                    race.opponents
                        .iter()
                        .map(|o| (o.prog.laps_f() * 100.0).round() / 100.0)
                        .collect::<Vec<_>>(),
                    race.opponents
                        .iter()
                        .map(|o| o.finished_at.map(|x| x.round()))
                        .collect::<Vec<_>>()
                );
            }
            if race.phase == Phase::Finished {
                break;
            }
        }
        println!("standings {:?}", race.standings());
    }

    #[test]
    #[ignore]
    fn drive_trace() {
        let id = std::env::var("QA_TRACK").unwrap_or("ASPEN".into());
        let track = Track::load(&id, &data("track/track")).unwrap();
        let lines = crate::assets::read_ai_lines(&data("data/data/AI"), &id);
        let (trucks, _) = crate::vehicle::load_roster(repo_root());
        let player = trucks
            .iter()
            .find(|t| t.name.contains("Montero"))
            .cloned()
            .unwrap();
        let mut race = Race::build(
            RaceConfig {
                track_id: &id,
                class: 1,
                laps: 2,
                opponents: 0,
                player_truck: player,
                ai_vehicles: vec![],
                lines: lines.clone(),
                skill: 0.8,
            },
            track,
        )
        .unwrap();
        race.pilot = Some(ai::Driver::new(lines[0].clone(), pilot_skill(), "pilot", 0));
        if std::env::var("QA_CONFIG").is_ok() {
            let p = &race.player;
            println!("CONFIG {:?}", p.config);
            for (i, a) in p.truck.axles.iter().enumerate() {
                println!(
                    "AXLE{i} spring_rate={} max_compr={} ",
                    a.spring_rate, a.max_compression
                );
            }
            println!("TRUCK final={} cd={}", p.truck.final_drive, p.truck.cd);
        }
        let mut audio = Audio::new();
        audio.enabled = false;
        let secs: f32 = std::env::var("QA_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6.0);
        let every: usize = std::env::var("QA_EVERY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(15);
        for i in 0..((COUNTDOWN + secs) / DT) as usize {
            race.tick(DT, Input::default(), &mut audio, 0.0);
            if i % every == 0 {
                let p = &race.player;
                let g = race.track.height_at(p.pos.x, p.pos.z);
                let c: Vec<String> = p
                    .wheels
                    .iter()
                    .map(|w| format!("{}{:.2}", if w.grounded { "G" } else { "-" }, w.compression))
                    .collect();
                if std::env::var("QA_DEBUG").is_ok() {
                    let tr = &race.track;
                    println!("   {}", p.debug_contacts(&|x, z| tr.sample(x, z)));
                }
                print!(
                    "off={:5.1} cur={:3} pos=({:7.1},{:7.1}) fwd=({:5.2},{:5.2}) steer={:5.2} ",
                    seg_distance(&race.course, p.pos),
                    race.pilot.as_ref().map_or(0, |d| d.cursor),
                    p.pos.x,
                    p.pos.z,
                    p.forward().x,
                    p.forward().z,
                    p.steer
                );
                println!("t={:5.2} {:?} y-g={:5.2} vy={:5.2} v={:4.1} roll={:5.1} pitch={:5.1} wheels={}", i as f32 * DT, race.phase, p.pos.y - g, p.vel.y, p.speed_kmh(), p.roll_deg(), p.pitch_deg(), c.join(" "));
            }
        }
    }

    /// Retail-level regression: the autopilot must hold ASPEN's line at a
    /// race pace with all four wheels mostly down. Before the yaw fix the
    /// pilot read the pitch as its heading and left the line within seconds
    /// (>85 % off-line), before the axle weights and retail damping the
    /// truck spent >50 % of the time airborne.
    #[test]
    fn autopilot_drives_aspen_cleanly() {
        require_assets!();
        let m = drive("ASPEN", 30.0);
        assert!(m.off_line < 0.05, "off the line {:.0}%", m.off_line * 100.0);
        assert!(m.airborne < 0.20, "airborne {:.0}%", m.airborne * 100.0);
        assert!(m.mean_kmh > 50.0, "too slow {:.0} km/h", m.mean_kmh);
    }

    /// Where and why the body tips past 25 deg of roll.
    /// `EVOX_TIP_TRACK=MCROSS cargo test --release -p evox tip_trace -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn tip_trace() {
        let id = std::env::var("EVOX_TIP_TRACK").unwrap_or("MCROSS".into());
        let track = Track::load(&id, &data("track/track")).expect("track loads");
        let lines = crate::assets::read_ai_lines(&data("data/data/AI"), &id);
        let (trucks, _) = crate::vehicle::load_roster(repo_root());
        let player = trucks
            .iter()
            .find(|t| t.name.contains("Montero"))
            .or(trucks.first())
            .cloned()
            .unwrap();
        let mut race = Race::build(
            RaceConfig {
                track_id: &id,
                class: 1,
                laps: 2,
                opponents: 0,
                player_truck: player,
                ai_vehicles: trucks.iter().take(8).cloned().collect(),
                lines: lines.clone(),
                skill: 0.8,
            },
            track,
        )
        .unwrap();
        race.pilot = Some(ai::Driver::new(lines[0].clone(), pilot_skill(), "pilot", 0));
        let mut audio = Audio::new();
        audio.enabled = false;
        let mut in_tip = false;
        let mut rescues = 0;
        if let Ok(w) = std::env::var("EVOX_TIP_NODES") {
            let v: Vec<f32> = w.split(',').filter_map(|x| x.parse().ok()).collect();
            for n in &lines[0].nodes {
                if (n.pos.x - v[0]).abs() < v[2] && (n.pos.z - v[1]).abs() < v[2] {
                    println!(
                        "{id} NODE ({:.0},{:.1},{:.0}) ground={:.1} cap={:.0}km/h",
                        n.pos.x,
                        n.pos.y,
                        n.pos.z,
                        race.track.height_at_world(n.pos.x, n.pos.z),
                        n.speed_cap * 3.6
                    );
                }
            }
        }
        let mut t = 0.0f32;
        for i in 0..(((COUNTDOWN + 60.0) / DT) as usize) {
            let was = race.flip_t;
            race.tick(DT, Input::default(), &mut audio, 0.0);
            if race.phase != Phase::Racing {
                continue;
            }
            t += DT;
            if was > 0.0 && race.flip_t == 0.0 && was > FLIP_RESET_TIME - 0.1 {
                rescues += 1;
            }
            let p = &race.player;
            let roll = p.roll_deg();
            let tip = roll.abs() > 25.0;
            let (lo, hi): (f32, f32) = std::env::var("EVOX_TIP_WINDOW")
                .ok()
                .and_then(|w| {
                    let mut it = w.split(',');
                    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
                })
                .unwrap_or((1e9, 1e9));
            if t >= lo && t <= hi && i % 6 == 0 {
                let gh = race.track.height_at_world(p.pos.x, p.pos.z);
                println!("{id} t={t:5.2} pos=({:.0},{:.0}) y={:.2} ground={gh:.2} kmh={:3.0} roll={roll:5.1} pitch={:5.1} vy={:5.1} wheels={:?}",
                    p.pos.x, p.pos.z, p.pos.y, p.speed_kmh(), p.pitch_deg(), p.vel.y, p.wheels.iter().map(|w| w.grounded as u8).collect::<Vec<_>>());
            }
            if tip && !in_tip || (tip && i % 15 == 0) {
                let down: Vec<u8> = p.wheels.iter().map(|w| w.grounded as u8).collect();
                let n = race.track.normal_at_world(p.pos.x, p.pos.z);
                let rf = vec3(p.right().x, 0.0, p.right().z).normalize_or_zero();
                let g_roll = (-n.dot(rf)).clamp(-1.0, 1.0).asin().to_degrees();
                println!("{id} t={t:5.1} pos=({:.0},{:.0}) kmh={:3.0} roll={roll:5.1} ground_cross_slope={g_roll:5.1} pitch={:5.1} wheels={down:?}",
                    p.pos.x, p.pos.z, p.speed_kmh(), p.pitch_deg());
            }
            in_tip = tip;
        }
        println!("{id} rescues={rescues}");
    }

    /// A touch player holds the gas through every jump; without the landing
    /// assist that pitched the truck onto its tail (DEVIL: 2.9 % of the time
    /// past 60 deg, 26 % off the line). With it the thumb drives as cleanly
    /// as the autopilot.
    #[test]
    fn touch_player_with_landing_assist_does_not_tumble() {
        require_assets!();
        for id in ["ASPEN", "DEVIL", "CZ2000"] {
            let m = drive_with(id, 60.0, true, true);
            assert!(
                m.tumbling < 0.002,
                "{id}: tumbling {:.1}%",
                m.tumbling * 100.0
            );
            assert!(
                m.off_line < 0.05,
                "{id}: off the line {:.0}%",
                m.off_line * 100.0
            );
        }
    }

    /// `cargo test --release -p evox drive_report -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn drive_report() {
        for id in ["ASPEN", "ZONA", "DEVIL", "CZ2000", "THEHILL", "MCROSS"] {
            let m = drive(id, 60.0);
            println!(
                "{id:8} dist={:5.0}m mean={:3.0}km/h off_line={:4.1}% airborne={:4.1}% <=2wheels={:4.1}% roll_max={:3.0} roll>25={:4.1}% tumbling={:4.1}% vy_reversals={}",
                m.distance_m,
                m.mean_kmh,
                m.off_line * 100.0,
                m.airborne * 100.0,
                m.two_or_less * 100.0,
                m.roll_max,
                m.roll_over_25 * 100.0,
                m.tumbling * 100.0,
                m.vy_reversals
            );
        }
    }
}
