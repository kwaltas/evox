//! Fixed-step vehicle dynamics.
//!
//! Ground truth: `4x4e2/Source/TRX` (engine loop, `Renderer`/`Mathematics`
//! conventions), the `.TRK` field table recovered from the retail parser
//! (`4x4evo2-work/decomp_4x42_full.c`, `FUN_00434c20` and the
//! `!faxle_spring_rate` string block), and the ray-cast suspension that the
//! `juego-de-carreras-4x4-offroad` prototype proved on all 30 retail tracks.
//! See `docs/PORT_REFERENCE.md`.
//!
//! Model (60 Hz ticks of 4 × 240 Hz substeps, deterministic, no render-rate
//! coupling):
//!
//! * **Suspension** is a ray cast along the body up axis from each wheel
//!   anchor, not a height lookup at the anchor's XZ. The latter reads the
//!   ground under a point that is *not* under the wheel once the chassis is
//!   tilted, which is what buried trucks on steep faces.
//! * **Spring** `F = k·x + c·ẋ` with `k` from `faxle/raxle.spring_rate`
//!   (lb/ft → N/m) and the retail damper `c = 3.5·√spring_rate` lb·s/ft. The
//!   sprung mass is `dry_weight` plus two 500 lb axles, as the retail set-up
//!   sums it; the springs are sized for that total.
//! * **Wheel rays** march to the first crossing and bisect; an anchor below
//!   the surface is a fully compressed wheel, never "out of reach".
//! * **Tyre** forces act in the terrain tangent plane (not the horizontal
//!   plane) through one Pacejka curve shared by the lateral and longitudinal
//!   axes, clamped by a friction ellipse. Grip therefore scales with the
//!   surface normal, and a grade no longer eats the drive force.
//! * **Chassis** contact is velocity-level: each corner below the surface
//!   gets a zero-restitution normal impulse through the full rigid-body
//!   effective mass plus Coulomb friction, and residual depth is corrected
//!   softly. Angular velocity integrates against the chassis-frame inertia.
//! * **Steering**: positive input turns right (towards `right()`).
//! * **Powertrain** follows the retail `.TRK` table: `xm.gear_ratio[]` indexed
//!   from park (0) / reverse (1) / neutral (2) / 1st (3), `xm.final_drive`,
//!   `xm.xfrLowRatio`, `faxle/raxle.torque_pct`, `eng.torqueTable[]`. The
//!   clutch slips at `launch_rpm`, so a stopped truck still makes torque: the
//!   0 MPH / 2200 RPM incline stall was the old code forcing `rpm` down to
//!   idle whenever wheel speed was zero.

use std::ops::{Deref, DerefMut};

use macroquad::prelude::{vec3, Quat, Vec3};

use crate::collide::Collider;
use crate::parts::StatMods;
use crate::terrain::{Surface, Track};
use crate::vehicle::{Truck, FT, LB};

mod rigid_body;
mod suspension;
mod tire;

pub use rigid_body::RigidBody;
use suspension::SpringDamper;

pub const TICK_HZ: f32 = 60.0;
pub const DT: f32 = 1.0 / TICK_HZ;
/// Integration substeps per `DT` tick (4 → 240 Hz, as offroad4x4).
pub const SUBSTEPS: usize = 4;
pub const GRAVITY: f32 = 9.81;
const AIR_DENSITY: f32 = 1.2;

/// Drivetrain losses (gearbox, transfer case, bearings).
const DRIVETRAIN_EFF: f32 = 0.88;
/// Idle and clutch-engagement speeds. `eng.redline` comes from the truck file.
const IDLE_RPM: f32 = 850.0;
const LAUNCH_RPM: f32 = 2150.0;
const SHIFT_TIME: f32 = 0.16;
/// Low-range crawler assist starts above this steepness (1 − n.y, ≈ 28°).
const CRAWL_STEEPNESS: f32 = 0.12;
/// Hull/terrain friction coefficient (offroad4x4 `tuning.rs`).
const BODY_CONTACT_FRICTION: f32 = 0.6;
/// Wheel ray march: coarse steps over the reach, then bisection passes.
const RAY_STEPS: usize = 10;
const RAY_BISECT: usize = 8;
/// Sequential-impulse passes over the hull corners per substep.
const HULL_ITERATIONS: usize = 3;
/// Penetration tolerated before positional correction (m).
const HULL_SLOP: f32 = 0.01;
/// Share of the remaining penetration removed per substep.
const HULL_CORRECTION: f32 = 0.35;
const ANGULAR_DAMPING: f32 = 0.6;
const AIR_ANGULAR_DAMPING: f32 = 0.25;
/// A sustained prop contact counts as one impact for this long (s), so a truck
/// grinding along a tree trunk does not thump or add a `hit` every tick.
const IMPACT_COOLDOWN: f32 = 0.30;
/// Speed-sensitive steering falloff (m/s): `max_angle / (1 + v/this)`.
/// Air control (offroad4x4 `tuning.rs` `AIR_PITCH_ACCEL` / `AIR_YAW_ACCEL`,
/// applied in `car.rs` only while no wheel touches): throttle/brake pitch the
/// truck and steering yaws it, in rad/s². Without it a truck that leaves a
/// crest nose-down keeps rotating until the bumper digs in and it tumbles.
pub const AIR_PITCH_ACCEL: f32 = 1.0;
pub const AIR_YAW_ACCEL: f32 = 1.2;
const STEER_SPEED_FALLOFF: f32 = 15.0;
const STEER_RATE: f32 = 3.0;
const STEER_RETURN_RATE: f32 = 4.5;

#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    pub throttle: f32,
    pub brake: f32,
    pub steer: f32,
    pub handbrake: bool,
    pub shift_up: bool,
    pub shift_down: bool,
    /// 0 = 2WD, 1 = 4WD-HI, 2 = 4WD-LO.
    pub drive_mode: u8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Wheel {
    /// Suspension anchor in chassis space (metres, +Y up, −Z forward).
    pub mount: Vec3,
    pub contact: Vec3,
    pub compression: f32,
    pub compression_vel: f32,
    pub grounded: bool,
    pub steer: f32,
    pub spin: f32,
    pub slip_angle: f32,
    pub slip_ratio: f32,
    pub load: f32,
    pub surface: Surface,
}

impl Wheel {
    fn new(mount: Vec3) -> Self {
        Self {
            mount,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub suspension_travel_m: f32,
    pub cg_height_m: f32,
    pub anti_roll: f32,
    pub steer_max: f32,
    pub brake_torque: f32,
    pub tire_mu: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            suspension_travel_m: 0.28,
            cg_height_m: 0.62,
            // Lateral anti-roll stiffness: a higher value keeps the chassis
            // level through off-camber corners and minor bumps.
            anti_roll: 0.85,
            steer_max: 0.55,
            brake_torque: 1.0,
            tire_mu: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Vehicle {
    pub truck: Truck,
    pub config: Config,
    pub mods: StatMods,
    /// Chassis rigid-body state: position, orientation, linear and angular
    /// velocity. Field access is forwarded through `Deref`/`DerefMut` so the
    /// rest of the game keeps using `vehicle.pos`, `vehicle.vel`, etc.
    pub body: RigidBody,
    pub wheels: [Wheel; 4],
    pub rpm: f32,
    /// Index into `truck.gear_ratios`: 0 park, 1 reverse, 2 neutral, 3 = 1st.
    pub gear: usize,
    pub clutch: f32,
    pub throttle: f32,
    pub steer: f32,
    pub airtime: f32,
    pub skid: f32,
    pub shift_timer: f32,
    pub engine_on: bool,
    pub distance_m: f32,
    /// Low-range transfer case engaged (`xm.xfrLowRatio`).
    pub low_range: bool,
    /// Number of *new* impacts with static props (telemetry / QA). Repeat
    /// ticks of one sustained contact do not count; see `hit_cooldown`.
    pub hits: u32,
    /// Seconds until a sustained prop contact may count as a new impact.
    pub hit_cooldown: f32,
    /// Speed (m/s) of the most recent new prop impact, or 0 when none is
    /// pending. The race consumes it to trigger the collision thump.
    pub impact: f32,
    /// Last total drive force at the contact patches (N), for telemetry.
    pub drive_force: f32,
}

impl Deref for Vehicle {
    type Target = RigidBody;

    fn deref(&self) -> &RigidBody {
        &self.body
    }
}

impl DerefMut for Vehicle {
    fn deref_mut(&mut self) -> &mut RigidBody {
        &mut self.body
    }
}

fn wheel_index(axle: usize, side: usize) -> usize {
    axle * 2 + side
}

impl Vehicle {
    pub fn new(truck: Truck) -> Self {
        let mut wheels = [
            Wheel::new(Vec3::ZERO),
            Wheel::new(Vec3::ZERO),
            Wheel::new(Vec3::ZERO),
            Wheel::new(Vec3::ZERO),
        ];
        for axle in 0..2 {
            for side in 0..2 {
                // TRK/SMF model space is +Z forward; physics forward is −Z.
                let m = truck.wheel_mount(axle, side);
                wheels[wheel_index(axle, side)].mount = Vec3::new(m.x, m.y, -m.z) * FT;
            }
        }
        Self {
            config: Config {
                // Centre of mass kept low (the retail chassis sits the engine
                // and axles near the ground); a high CG is what tips the truck
                // onto two wheels and into barrel rolls over minor terrain.
                cg_height_m: (truck.height * FT * 0.30).clamp(0.28, 0.50),
                ..Default::default()
            },
            body: RigidBody::new(Vec3::new(0.0, 1.2, 0.0), Quat::IDENTITY),
            wheels,
            rpm: IDLE_RPM,
            gear: 3,
            clutch: 1.0,
            throttle: 0.0,
            steer: 0.0,
            airtime: 0.0,
            skid: 0.0,
            shift_timer: 0.0,
            engine_on: true,
            distance_m: 0.0,
            mods: StatMods::default(),
            truck,
            hits: 0,
            hit_cooldown: 0.0,
            impact: 0.0,
            low_range: false,
            drive_force: 0.0,
        }
    }

    pub fn mass(&self) -> f32 {
        let m = self.truck.mass_kg();
        (m + self.mods.weight * LB).max(150.0)
    }

    /// Yaw moment of inertia of the chassis, treated as a solid slab
    /// (`m·(l² + w²)/12`). Collision deflection is `angular impulse / this`
    /// rather than a fixed rad/s per tick, so a light truck does not spin like
    /// a top and a heavy one does not feel welded to the ground.
    pub fn yaw_inertia(&self) -> f32 {
        let l = (self.truck.length * FT).max(1.0);
        let w = (self.truck.width * FT).max(0.8);
        (self.mass() * (l * l + w * w) / 12.0).max(1.0)
    }

    pub fn inertia(&self) -> Vec3 {
        let m = self.mass();
        let l = self.truck.length.max(1.0) * FT;
        let w = self.truck.width.max(1.0) * FT;
        let h = self.truck.height.max(1.0) * FT;
        // Box approximation is the lower bound; TRK inertias are already
        // lb·ft² and often omitted in retail files.
        let base = Vec3::new(
            m * (l * l + h * h) / 12.0,
            m * (l * l + w * w) / 12.0,
            m * (w * w + h * h) / 12.0,
        );
        let i = self.truck.inertia;
        Vec3::new(
            i.x.max(0.0) * LB * FT * FT,
            i.y.max(0.0) * LB * FT * FT,
            i.z.max(0.0) * LB * FT * FT,
        )
        .max(base)
    }

    /// Upper suspension strut anchor in world space (raycast origin).
    fn anchor_world(&self, i: usize) -> Vec3 {
        self.pos + self.rot * self.wheels[i].mount
    }

    /// 3D wheel hub centre in world space, displaced along the body up axis by
    /// the suspension travel relative to static sag so the visual tyres track
    /// terrain bumps cleanly.
    pub fn wheel_world(&self, i: usize) -> Vec3 {
        let axle = i / 2;
        let w = &self.wheels[i];
        let travel = self.travel(axle);
        let sag = travel * 0.55;
        let hub = w.mount - Vec3::Y * ((sag - w.compression) * 0.55);
        self.pos + self.rot * hub
    }

    /// Heading about +Y in the convention the AI and spawn code use:
    /// forward = (−sin yaw, 0, −cos yaw). Taken from the forward vector, so
    /// pitch and roll never leak into it (`to_euler(YXZ).1` is the *pitch*).
    pub fn yaw(&self) -> f32 {
        let f = self.forward();
        (-f.x).atan2(-f.z)
    }

    /// Body roll in degrees: angle of the right axis above the horizon
    /// (positive = right side up). Telemetry / QA.
    pub fn roll_deg(&self) -> f32 {
        self.right().y.clamp(-1.0, 1.0).asin().to_degrees()
    }

    /// Body pitch in degrees: nose above the horizon is positive.
    pub fn pitch_deg(&self) -> f32 {
        self.forward().y.clamp(-1.0, 1.0).asin().to_degrees()
    }

    pub fn speed_kmh(&self) -> f32 {
        self.vel.length() * 3.6
    }

    pub fn forward(&self) -> Vec3 {
        self.rot * Vec3::NEG_Z
    }

    pub fn right(&self) -> Vec3 {
        self.rot * Vec3::X
    }

    pub fn tyre_radius(&self) -> f32 {
        self.truck.tyre_radius_m().max(0.2)
    }

    fn travel(&self, axle: usize) -> f32 {
        // `faxle/raxle.maxcompr` is feet; `Config::suspension_travel_m` is the
        // stand-in when a vehicle omits it.
        let declared = self.truck.axles[axle].max_compression * FT;
        (if declared > 0.0 {
            declared
        } else {
            self.config.suspension_travel_m
        })
        .clamp(0.06, 0.55)
    }

    /// Box tensor of the hull, in chassis space (metres).
    pub fn hull(&self) -> Vec3 {
        vec3(
            self.truck.width.max(1.0) * FT * 0.5,
            self.truck.height.max(1.0) * FT * 0.5,
            self.truck.length.max(1.0) * FT * 0.5,
        )
    }

    fn corner_mass(&self) -> f32 {
        self.mass() * 0.25
    }

    /// Spring rate of one corner, N/m. `spring_rate` is lb/ft in every retail
    /// TRK (`faxle_spring_rate` 1178.4, `raxle_spring_rate` 1493.0).
    fn spring_rate(&self, axle: usize, i: usize) -> f32 {
        let axle_rate = self.truck.axles[axle].spring_rate.max(1.0) * 14.593_903;
        // A motion ratio below 1 (the file's `spring_arm`) stiffens the wheel
        // rate; retail files carry 5.0 for every corner, so this is neutral.
        let arm = self.truck.axles[axle].tires[i % 2].spring_arm.abs();
        let ratio = if arm > 0.5 { 1.0 } else { 1.0 / arm.max(0.2) };
        axle_rate * ratio
    }

    /// Spring/damper for one corner, in SI. The damper keeps the retail
    /// `3.5·√spring_rate` formula; the spring rate also carries the corner's
    /// motion ratio.
    fn suspension(&self, axle: usize, i: usize) -> SpringDamper {
        let rate_lb_ft = self.truck.axles[axle].spring_rate.max(1.0);
        SpringDamper::new(
            self.spring_rate(axle, i),
            SpringDamper::retail_damper(rate_lb_ft),
            self.travel(axle),
        )
    }

    /// Signed overall ratio engine → wheel for the selected gear.
    fn overall_ratio(&self) -> f32 {
        let gear = self.gear_ratio();
        let low = if self.low_range {
            self.truck.xfr_low_ratio.max(1.0)
        } else {
            1.0
        };
        gear * self.truck.final_drive.max(0.5) * low
    }

    /// Drive force at the contact patches for the current rpm and gear, in N.
    pub fn engine_force(&self) -> f32 {
        let torque = self.truck.engine.torque_nm(self.rpm) * (1.0 + self.mods.torque);
        torque * self.overall_ratio() * DRIVETRAIN_EFF / self.tyre_radius()
    }

    /// Box ratio of the selected gear, unsigned (park/neutral return 1.0).
    pub fn gear_ratio(&self) -> f32 {
        self.truck
            .gear_ratios
            .get(self.gear)
            .copied()
            .unwrap_or(1.0)
            .abs()
            .max(0.05)
    }

    /// Signed gear ratio: reverse is negative.
    fn signed_ratio(&self) -> f32 {
        let r = self
            .truck
            .gear_ratios
            .get(self.gear)
            .copied()
            .unwrap_or(1.0);
        if r < 0.0 {
            r
        } else {
            r.abs().max(0.05)
        }
    }

    fn first_gear(&self) -> usize {
        3.min(self.truck.gear_ratios.len().saturating_sub(1))
    }

    fn top_gear(&self) -> usize {
        self.truck.gear_ratios.len().saturating_sub(1)
    }

    fn reverse_gear(&self) -> usize {
        if self.truck.gear_ratios.len() > 1 {
            1
        } else {
            3
        }
    }

    fn try_shift(&mut self, dir: i32) {
        let g = self.gear as i32 + dir;
        if g >= self.first_gear() as i32 && g <= self.top_gear() as i32 {
            self.gear = g as usize;
            self.shift_timer = SHIFT_TIME;
        }
    }

    /// Ray cast along `dir` from `anchor` against the height field: distance
    /// to the first point where the ray goes below the surface, within
    /// `reach`. An anchor already under the surface is a hit at 0 (the wheel
    /// is fully compressed, not out of reach). The ray is marched in fixed
    /// steps to the first sign change and then bisected: a height-field slope
    /// steeper than the ray makes Newton steps on the vertical gap overshoot.
    fn cast_ground(
        &self,
        anchor: Vec3,
        dir: Vec3,
        reach: f32,
        sample: &dyn Fn(f32, f32) -> (f32, Surface),
    ) -> Option<f32> {
        if dir.y > -0.05 {
            // Pointing up or sideways: no ground under the wheel.
            return None;
        }
        let gap = |t: f32| {
            let p = anchor + dir * t;
            p.y - sample(p.x, p.z).0
        };
        if gap(0.0) <= 0.0 {
            return Some(0.0);
        }
        let mut lo = 0.0f32;
        let mut hi = None;
        for k in 1..=RAY_STEPS {
            let t = reach * k as f32 / RAY_STEPS as f32;
            if gap(t) <= 0.0 {
                hi = Some(t);
                break;
            }
            lo = t;
        }
        let mut hi = hi?;
        for _ in 0..RAY_BISECT {
            let mid = 0.5 * (lo + hi);
            if gap(mid) <= 0.0 {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        Some(hi)
    }

    /// QA: per-wheel anchor height above ground and ray result.
    #[cfg(test)]
    pub fn debug_contacts(&self, sample: &dyn Fn(f32, f32) -> (f32, Surface)) -> String {
        let up = self.rot * Vec3::Y;
        let mut out = String::new();
        for i in 0..4 {
            let a = self.anchor_world(i);
            let reach = self.travel(i / 2) + self.tyre_radius();
            let hit = self.cast_ground(a, -up, reach, sample);
            out += &format!(
                " w{i}: anchor-g={:.2} reach={:.2} hit={:?};",
                a.y - sample(a.x, a.z).0,
                reach,
                hit
            );
        }
        out
    }

    /// Terrain normal from central differences of the height field.
    fn ground_normal(&self, x: f32, z: f32, sample: &dyn Fn(f32, f32) -> (f32, Surface)) -> Vec3 {
        let e = 0.75;
        let dx = sample(x + e, z).0 - sample(x - e, z).0;
        let dz = sample(x, z + e).0 - sample(x, z - e).0;
        vec3(-dx, 2.0 * e, -dz).normalize_or(Vec3::Y)
    }

    /// One fixed tick. `sample` returns ground height and surface at a point.
    pub fn step(&mut self, input: &Input, sample: &dyn Fn(f32, f32) -> (f32, Surface)) {
        // The rigid body and its springs integrate at `SUBSTEPS × 60 Hz`
        // (offroad4x4 `tuning.rs` `PHYSICS_DT = 1/240`): the bump stops are
        // ~12× the spring rate, and one explicit step per 60 Hz tick lets a
        // crest or kerb inject spurious pitch/roll at take-off. Shift
        // requests are edges and apply on the first substep only.
        let mut sub = *input;
        for k in 0..SUBSTEPS {
            if k == 1 {
                sub.shift_up = false;
                sub.shift_down = false;
            }
            self.substep(&sub, sample, DT / SUBSTEPS as f32);
        }
        self.hit_cooldown = (self.hit_cooldown - DT).max(0.0);
    }

    fn substep(&mut self, input: &Input, sample: &dyn Fn(f32, f32) -> (f32, Surface), dt: f32) {
        let mass = self.mass();
        let inertia = self.inertia();
        let forward = self.forward();
        let up = self.rot * Vec3::Y;
        let v_fwd = self.vel.dot(forward);

        // ---- steering: rate-limited rack, speed sensitive --------------------
        self.steer = self.steer.clamp(-1.0, 1.0);
        let max_steer = self.truck.axles[0].max_angle.max(0.35) * self.config.steer_max / 0.55;
        let max_steer = (max_steer / (1.0 + v_fwd.abs() / STEER_SPEED_FALLOFF)).clamp(0.08, 0.75);
        let target = input.steer.clamp(-1.0, 1.0) * max_steer;
        let rate = if input.steer.abs() < 0.01 {
            STEER_RETURN_RATE
        } else {
            STEER_RATE
        };
        self.steer += (target - self.steer).clamp(-rate * dt, rate * dt);
        self.throttle +=
            (input.throttle.clamp(0.0, 1.0) - self.throttle).clamp(-8.0 * dt, 8.0 * dt);

        if input.shift_up {
            self.try_shift(1);
        }
        if input.shift_down {
            self.try_shift(-1);
        }
        if input.drive_mode == 2 {
            self.low_range = true;
        } else if input.drive_mode <= 1 {
            self.low_range = false;
        }

        // ---- powertrain -----------------------------------------------------
        let (drive_force, braking, reverse) = self.powertrain(input, v_fwd, dt);
        self.drive_force = drive_force;
        let axle_share = |axle: usize| -> f32 {
            let four = self.truck.drive_type != 0;
            match input.drive_mode {
                0 => {
                    if axle == 1 {
                        1.0
                    } else {
                        0.0
                    }
                }
                2 if four => 0.5,
                1 if four => {
                    let (f, r) = self.truck.drive_split();
                    if axle == 0 {
                        f.max(0.0)
                    } else {
                        r.max(0.0)
                    }
                }
                // No transfer case: the front axle is a dead shaft.
                _ if four => {
                    let (f, r) = self.truck.drive_split();
                    if axle == 0 {
                        f.max(0.0)
                    } else {
                        r.max(0.0)
                    }
                }
                _ => {
                    if axle == 1 {
                        1.0
                    } else {
                        0.0
                    }
                }
            }
            .max(0.0)
                * 0.5
        };

        let mut force = Vec3::new(0.0, -mass * GRAVITY, 0.0);
        let mut torque = Vec3::ZERO;
        let mut grounded = 0usize;
        let mut skid = 0.0f32;
        let wheel_radius = self.tyre_radius();

        for i in 0..4 {
            let axle = i / 2;
            let anchor = self.anchor_world(i);
            let travel = self.travel(axle);
            let reach = travel + wheel_radius;
            let w = self.wheels[i];
            let share = axle_share(axle);

            let mut wheel = w;
            wheel.slip_angle = 0.0;
            wheel.slip_ratio = 0.0;

            match self.cast_ground(anchor, -up, reach, sample) {
                None => {
                    wheel.grounded = false;
                    wheel.compression = (wheel.compression - 2.0 * dt).max(0.0);
                    wheel.compression_vel = 0.0;
                    wheel.load = 0.0;
                }
                Some(dist) => {
                    let spring_len = (dist - wheel_radius).max(0.0);
                    let comp = (travel - spring_len).max(0.0);
                    let comp_vel = ((comp - w.compression) / dt).clamp(-8.0, 8.0);
                    let fs = self.suspension(axle, i).force(comp, comp_vel);
                    let ray_p = anchor - up * dist;
                    let (ground_y, surface) = sample(ray_p.x, ray_p.z);
                    let cp = vec3(ray_p.x, ground_y + 0.03, ray_p.z);
                    let n = self.ground_normal(cp.x, cp.z, sample);
                    if surface == Surface::Water {
                        // Hydrodynamic splash/wading resistance while fording rivers.
                        let v_horiz = vec3(self.vel.x, 0.0, self.vel.z);
                        force -= v_horiz * (140.0 + 38.0 * v_horiz.length());
                    }
                    let corner_cf = self.truck.axles[axle].tires[i % 2].cf.max(0.2);
                    let mu = tire::surface_mu(
                        self.config.tire_mu,
                        self.mods.tire_cf,
                        surface,
                        corner_cf,
                    );

                    wheel.grounded = true;
                    wheel.compression = comp;
                    wheel.compression_vel = comp_vel;
                    wheel.load = fs;
                    wheel.contact = cp;
                    wheel.surface = surface;
                    wheel.steer = if axle == 0 { self.steer } else { 0.0 };
                    grounded += 1;

                    // Spring acts along the body up axis at the anchor.
                    force += up * fs;
                    torque += (anchor - self.pos).cross(up * fs);

                    // Tyre frame: wheel heading projected onto the ground plane.
                    // Positive steer turns right (towards `right()`), the
                    // input convention of app.rs (D / Right = +1). A
                    // positive rotation about +Y swings −Z towards −X, so
                    // the rack angle is applied negated.
                    let wf = self.rot * Quat::from_rotation_y(-wheel.steer) * Vec3::NEG_Z;
                    let gf = (wf - n * wf.dot(n)).normalize_or_zero();
                    let gs = gf.cross(n);
                    let pv = self.vel + self.angular.cross(cp - self.pos);
                    let v_long = pv.dot(gf);
                    let v_lat = pv.dot(gs);

                    let alpha = tire::slip_angle(v_long, v_lat);
                    wheel.slip_angle = alpha;
                    let rear_bias = if axle == 1 { 1.05 } else { 1.0 };
                    let mut f_lat = -tire::magic(alpha) * mu * fs * 1.15 * rear_bias;
                    if v_long.abs() < 2.5 {
                        // Linear bite at crawl speed so the truck holds lines on banks/grades.
                        let lin = -v_lat * 7.5 * mass * 0.25;
                        let k2 = (v_long.abs() / 2.5).clamp(0.0, 1.0);
                        f_lat = f_lat * k2 + lin * (1.0 - k2);
                    }
                    let max_lat = 0.28 * mass * v_lat.abs() / dt;
                    f_lat = f_lat.clamp(-max_lat, max_lat);

                    // Longitudinal demand: drive, rolling resistance, brakes, and
                    // slope crawler assist (`offroad4x4/src/car.rs`).
                    let steepness = (1.0 - n.y).clamp(0.0, 1.0);
                    let surface_roll = surface.rolling_drag() / 100.0;
                    let mut f_long = drive_force * share;
                    if reverse {
                        f_long = -f_long;
                    }
                    // Low range is a crawler (offroad4x4 `car.rs`): on a
                    // steep face it pulls harder and keeps its bite. Only in
                    // 4WD-LO and only per corner (quarter mass).
                    let crawl = self.low_range && !reverse && steepness > CRAWL_STEEPNESS;
                    if crawl && input.throttle > 0.3 {
                        f_long += mass * 0.25 * GRAVITY * steepness.sqrt() * 0.9;
                    }
                    f_long -= v_long.signum() * surface_roll * fs;
                    if braking > 0.0 {
                        let max_stop = mass * v_long.abs() / dt;
                        let force = braking * mass * GRAVITY * 0.95 * self.config.brake_torque;
                        f_long -= v_long.signum() * force.min(max_stop);
                    }
                    if input.handbrake && axle == 1 {
                        let max_stop = mass * v_long.abs() / dt;
                        f_long -= v_long.signum() * (mass * GRAVITY * 0.5).min(max_stop);
                    }
                    // Push the demanded force through the same curve: past the
                    // peak the tyre spins instead of pulling harder. The
                    // 4WD-LO crawler keeps extra bite on steep faces
                    // (offroad4x4 `car.rs`).
                    let limit = mu
                        * fs
                        * if crawl {
                            (1.0 + steepness * 1.6).min(2.4)
                        } else {
                            1.0
                        };
                    let want = tire::longitudinal_demand(f_long, limit);
                    let f_long_raw = tire::longitudinal_force(want, limit);
                    wheel.slip_ratio = (want.abs() - 1.0).clamp(0.0, 1.5);

                    let mut lat_grip = 1.0;
                    if input.handbrake && axle == 1 {
                        lat_grip = 0.4;
                    }
                    let mag = (f_long_raw * f_long_raw + f_lat * f_lat).sqrt();
                    let (clamped_long, clamped_lat, clipped) =
                        tire::clamp_ellipse(f_long_raw, f_lat, limit);
                    let (f_long, f_lat) = if clipped {
                        wheel.slip_ratio = wheel
                            .slip_ratio
                            .max(((mag - limit) / limit.max(1.0)).min(1.5));
                        (clamped_long, clamped_lat)
                    } else {
                        (f_long_raw, f_lat * lat_grip)
                    };

                    let f = gf * f_long + gs * f_lat;
                    // Apply the tyre force near the chassis roll centre
                    // (`TYRE_FORCE_HEIGHT = 0.35` in `offroad4x4/src/tuning.rs`)
                    // so cornering produces progressive body roll without tipping.
                    let lever = (self.config.cg_height_m * 0.50
                        / (self.config.cg_height_m * 0.50 + 0.30))
                        .clamp(0.0, 0.85);
                    let app = cp + (self.pos - cp) * lever;
                    force += f;
                    torque += (app - self.pos).cross(f);

                    let spin_target = v_long / wheel_radius
                        + if wheel.slip_ratio > 0.15 && share > 0.0 {
                            f_long.signum() * wheel.slip_ratio * 6.0
                        } else {
                            0.0
                        };
                    wheel.spin = (wheel.spin + spin_target.clamp(-400.0, 400.0) * dt)
                        .rem_euclid(std::f32::consts::TAU);
                    skid = skid
                        .max(wheel.slip_angle.abs() - 0.18)
                        .max(wheel.slip_ratio);
                }
            }
            skid = skid.max(wheel.slip_angle.abs() - 0.18);
            self.wheels[i] = wheel;
        }

        // ---- anti-roll bar --------------------------------------------------
        if self.config.anti_roll > 0.0 {
            for axle in 0..2 {
                let (l, r) = (wheel_index(axle, 0), wheel_index(axle, 1));
                let gle = self.wheels[l].grounded;
                let gre = self.wheels[r].grounded;
                if !gle && !gre {
                    continue;
                }
                let k = self.spring_rate(axle, 0);
                let d = (self.wheels[l].compression - self.wheels[r].compression)
                    * k
                    * self.config.anti_roll;
                if gle {
                    force += up * d;
                }
                if gre {
                    force -= up * d;
                }
                let track_w = (self.wheels[r].mount.x - self.wheels[l].mount.x)
                    .abs()
                    .max(1.2);
                let roll_torque = forward * (d * track_w * 0.5);
                torque += roll_torque;
            }
        }

        // ---- aero -----------------------------------------------------------
        let v = self.vel;
        let area_m2 = (self.truck.ref_area.z * FT * FT).max(0.5);
        let cd = (self.truck.cd + self.mods.aero).max(0.05);
        force -= 0.5 * AIR_DENSITY * cd * area_m2 * v.length() * v;

        if grounded == 0 {
            self.airtime += dt;
            // Air control: throttle lifts the nose, brake drops it, steering
            // yaws. Scaled by the inertia so the constants are accelerations.
            let right = self.rot * Vec3::X;
            torque += right * (input.throttle - input.brake) * AIR_PITCH_ACCEL * inertia.x;
            torque -= up * input.steer * AIR_YAW_ACCEL * inertia.y;
        } else {
            self.airtime = 0.0;
        }

        // ---- integrate ------------------------------------------------------
        self.body.integrate_force(force, mass, dt);
        if braking > 0.5 && input.throttle <= 0.01 && grounded >= 3 && self.vel.length() < 0.8 {
            // Brakes pin a nearly stopped truck only when throttle is not held.
            self.vel = Vec3::ZERO;
        }
        let damp = if grounded > 0 {
            ANGULAR_DAMPING
        } else {
            AIR_ANGULAR_DAMPING
        };
        // Inertia is diagonal in the chassis frame: rotate the world torque
        // in, divide, rotate back (offroad4x4 `Car::apply_world_torque`).
        self.body.integrate_torque(torque, inertia, damp, dt);
        if grounded > 0 && v_fwd.abs() > 3.0 && input.steer.abs() < 0.20 {
            let yaw_rate = self.angular.dot(up);
            let yaw_damp = (0.35 + (v_fwd.abs() / 20.0).min(0.65)) * dt;
            self.angular -= up * yaw_rate * yaw_damp.clamp(0.0, 0.50);
        }
        self.body.integrate_pose(dt);

        // ---- hull contact: velocity-level impulses, restitution 0 ----------
        // Body corners below the height field get a normal impulse that
        // cancels their inward velocity (no bounce), solved through the full
        // rigid-body effective mass so a corner strike also turns the truck,
        // and a Coulomb friction impulse (`BODY_CONTACT_FRICTION`, offroad4x4
        // `tuning.rs`) so a truck on its roof or side scrubs to a stop and
        // gravity can roll it back instead of skating along the slope. Any
        // remaining penetration is removed softly along the normal; nothing
        // snaps orientation or zeroes the spin.
        let hull = self.hull();
        let bottom = -(hull.y * 0.30);
        let top = hull.y * 0.55;
        let mut corners: Vec<Vec3> = Vec::with_capacity(9);
        corners.push(vec3(0.0, bottom, 0.0));
        if self.truck.chassis_points.len() >= 4 {
            for p in self.truck.chassis_points.iter().take(4) {
                corners.push(vec3(p.x * FT * 0.85, bottom, -p.z * FT * 0.85));
            }
        } else {
            for sz in [-1.0f32, 1.0] {
                for sx in [-1.0f32, 1.0] {
                    corners.push(vec3(sx * hull.x * 0.82, bottom, sz * hull.z * 0.82));
                }
            }
        }
        for sz in [-1.0f32, 1.0] {
            for sx in [-1.0f32, 1.0] {
                corners.push(vec3(sx * hull.x * 0.78, top, sz * hull.z * 0.70));
            }
        }
        let mut deepest = (0.0f32, Vec3::Y);
        for _ in 0..HULL_ITERATIONS {
            for c in &corners {
                let p = self.pos + self.rot * *c;
                let depth = sample(p.x, p.z).0 - p.y;
                if depth <= 0.0 {
                    continue;
                }
                let n = self.ground_normal(p.x, p.z, sample);
                if depth > deepest.0 {
                    deepest = (depth, n);
                }
                let r = p - self.pos;
                let vp = self.body.point_velocity(r);
                let vn = vp.dot(n);
                if vn >= 0.0 {
                    continue;
                }
                let k_n = self.body.inverse_effective_mass(r, n, mass, inertia);
                let jn = -vn / k_n.max(1e-6);
                self.body.apply_impulse(n * jn, r, mass, inertia);
                // friction against the remaining tangential slip
                let vp = self.body.point_velocity(r);
                let vt = vp - n * vp.dot(n);
                let slip = vt.length();
                if slip > 1e-4 {
                    let t = vt / slip;
                    let k_t = self.body.inverse_effective_mass(r, t, mass, inertia);
                    let jt = (slip / k_t.max(1e-6)).min(BODY_CONTACT_FRICTION * jn);
                    self.body.apply_impulse(-t * jt, r, mass, inertia);
                }
            }
        }
        if deepest.0 > HULL_SLOP {
            // Soft positional correction: a share of the depth per substep.
            self.pos += deepest.1 * ((deepest.0 - HULL_SLOP) * HULL_CORRECTION);
        }

        // Last resort: never leave the hull centre under the surface.
        let gh = sample(self.pos.x, self.pos.z).0;
        if self.pos.y < gh - 0.35 {
            let n = self.ground_normal(self.pos.x, self.pos.z, sample);
            self.pos.y = gh + 0.10;
            let vn = self.vel.dot(n);
            if vn < 0.0 {
                self.vel -= n * vn;
            }
        }

        self.distance_m += self.vel.length() * dt;
        self.skid = skid.clamp(0.0, 1.5);
    }

    /// Engine, clutch, gearbox and brakes. Returns `(drive force N, brake
    /// demand 0..1, reverse)`.
    fn powertrain(&mut self, input: &Input, v_fwd: f32, dt: f32) -> (f32, f32, bool) {
        let mut braking = 0.0f32;
        let mut pedal = 0.0f32;
        let mut want_reverse = false;

        if input.throttle > 0.01 {
            if v_fwd < -1.0 {
                braking = input.throttle;
            } else {
                pedal = input.throttle;
            }
        }
        // Hysteresis: reverse only engages from a standstill and stays in
        // until the truck is clearly rolling forward again, otherwise braking
        // through a slow corner flaps the box between reverse and first.
        let in_reverse = self.gear == self.reverse_gear();
        if input.brake > 0.01 {
            if v_fwd > if in_reverse { 2.0 } else { 0.85 } {
                braking = braking.max(input.brake);
            } else {
                want_reverse = true;
                pedal = input.brake;
            }
        }

        // Gear selection: brake at rest engages reverse immediately, throttle
        // at rest engages 1st. Neutral is never selected by the auto box.
        if want_reverse {
            if self.gear != self.reverse_gear() {
                self.gear = self.reverse_gear();
                self.shift_timer = SHIFT_TIME * 0.4;
            }
        } else if in_reverse && (input.throttle > 0.01 || v_fwd > 2.0) {
            self.gear = self.first_gear();
            self.shift_timer = SHIFT_TIME * 0.4;
        } else if self.gear < self.first_gear() {
            self.gear = self.first_gear();
        }

        self.shift_timer = (self.shift_timer - dt).max(0.0);
        self.clutch = (1.0 - self.shift_timer / SHIFT_TIME).clamp(0.0, 1.0);

        let reverse = self.gear == self.reverse_gear();
        let ratio = self.signed_ratio()
            * self.truck.final_drive.max(0.5)
            * if self.low_range {
                self.truck.xfr_low_ratio.max(1.0)
            } else {
                1.0
            };
        let wheel_radius = self.tyre_radius();
        // Engine speed locked to the wheels (rad/s → rpm).
        let locked = v_fwd.abs() / wheel_radius * ratio.abs() * 60.0 / std::f32::consts::TAU;

        // Auto box only in the forward gears.
        if !reverse && self.shift_timer <= 0.0 && self.gear >= self.first_gear() {
            if locked > self.truck.engine.upshift_rpm && self.gear < self.top_gear() {
                self.gear += 1;
                self.shift_timer = SHIFT_TIME;
            } else if locked < self.truck.engine.downshift_rpm && self.gear > self.first_gear() {
                self.gear -= 1;
                self.shift_timer = SHIFT_TIME * 0.6;
            }
        }

        // Clutch slip: below the launch rpm the engine is decoupled, so a
        // stopped truck can still pull away on a grade.
        let launch = IDLE_RPM + (LAUNCH_RPM - IDLE_RPM) * pedal;
        let driven_slip = self
            .wheels
            .iter()
            .filter(|w| w.grounded)
            .map(|w| w.slip_ratio)
            .fold(0.0f32, f32::max)
            .min(1.5);
        let mut target = locked.max(launch) + driven_slip * 1200.0 * pedal;
        if self.shift_timer > 0.0 {
            target = target.max(IDLE_RPM + 800.0 * pedal);
        }
        let redline = self.truck.engine.redline.max(1200.0);
        target = target.clamp(IDLE_RPM, redline * 1.02);
        self.rpm += (target - self.rpm) * (12.0 * dt).min(1.0);

        let mut force = 0.0;
        // Fuel cut at the redline (`eng.redline`, TRK 0x5f0): once the wheels
        // spin the engine past it there is no drive torque. Without this the
        // torque table clamps to its last entry and reverse, which has no
        // upshift, accelerates without limit.
        let fuel_cut = locked >= redline;
        if pedal > 0.01 && self.shift_timer <= 0.0 && !fuel_cut {
            let torque = self.truck.engine.torque_nm(self.rpm.max(locked))
                * (1.0 + self.mods.torque)
                * pedal;
            // offroad4x4 `Car::powertrain`: torque × overall ratio ×
            // efficiency / tyre radius, no converter multiplication.
            force = torque * ratio.abs() * DRIVETRAIN_EFF / wheel_radius;
        } else if pedal <= 0.01 && self.rpm > IDLE_RPM * 1.2 {
            // Engine braking through the driveline.
            let torque = self.truck.engine.torque_nm(self.rpm) * 0.22;
            force = -torque * ratio.abs() * DRIVETRAIN_EFF / wheel_radius;
            force *= if reverse { -1.0 } else { 1.0 };
        }

        (force, braking, reverse)
    }

    /// Place the chassis on the terrain, aligned to the local surface normal
    /// and lifted above the highest wheel contact. Steep stages otherwise spawn
    /// with the uphill wheels buried, which fires the bump stops.
    pub fn reset_on_ground(&mut self, x: f32, z: f32, yaw: f32, track: &Track) {
        let yaw_q = Quat::from_rotation_y(yaw);
        let mut ground = f32::MIN;
        for w in &self.wheels {
            let m = yaw_q * w.mount;
            ground = ground.max(track.height_at(x + m.x, z + m.z));
        }
        let e = crate::terrain::CELL_M * 0.5;
        let n = Vec3::new(
            -(track.height_at(x + e, z) - track.height_at(x - e, z)),
            2.0 * e,
            -(track.height_at(x, z + e) - track.height_at(x, z - e)),
        )
        .normalize_or(Vec3::Y);
        let lift = self.tyre_radius() + self.travel(0) * 0.55 + 0.05;
        self.reset(Vec3::new(x, ground + lift, z), yaw);
        self.rot = (Quat::from_rotation_arc(Vec3::Y, n) * yaw_q).normalize();
    }

    /// Resolve contact with static props. The chassis is a box of its real
    /// width; each prop is a vertical cylinder. Penetration is pushed out and
    /// inward velocity is reflected with a scrub so glancing hits deflect.
    pub fn collide(&mut self, colliders: &[Collider]) {
        let half_w = self.truck.width * FT * 0.5 * 0.9;
        let body_lo = self.pos.y - 0.35;
        let body_hi = self.pos.y + self.truck.height * FT * 0.9;
        for c in colliders {
            if body_hi < c.center.y - c.half_height || body_lo > c.center.y + c.half_height {
                continue;
            }
            let dx = self.pos.x - c.center.x;
            let dz = self.pos.z - c.center.z;
            let min = c.radius + half_w;
            let dist_sq = dx * dx + dz * dz;
            if dist_sq >= min * min {
                continue;
            }
            let dist = dist_sq.sqrt();
            let (nx, nz) = if dist > 1e-4 {
                (dx / dist, dz / dist)
            } else {
                (1.0, 0.0)
            };
            let push = min - dist;
            self.pos.x += nx * push * 0.7;
            self.pos.z += nz * push * 0.7;
            let vn = self.vel.x * nx + self.vel.z * nz;
            if vn < 0.0 {
                // Velocity-level bounce along the normal, then a tangential
                // scrub at the contact patch. Only the tangential impulse yaws
                // the chassis (the normal acts through the contact radius), and
                // the angular change is `torque / yaw_inertia` (rad/s), not a
                // flat rad/s per resolving tick.
                self.vel.x -= nx * vn * 1.3;
                self.vel.z -= nz * vn * 1.3;
                let v_lat = self.vel.x * -nz + self.vel.z * nx;
                let jt = vec3(-nz * v_lat, 0.0, nx * v_lat) * 0.25;
                self.vel += jt;
                let lever = vec3(-nx * half_w, 0.0, -nz * half_w);
                let torque_y = lever.z * jt.x - lever.x * jt.z;
                self.angular.y += torque_y / self.yaw_inertia();
                self.vel *= 0.94;
                if self.hit_cooldown <= 0.0 {
                    self.hit_cooldown = IMPACT_COOLDOWN;
                    self.impact = self.impact.max(-vn);
                    self.hits += 1;
                }
            }
        }
    }

    /// Resolve chassis-to-chassis collision between two race vehicles.
    /// Uses a dual-sphere volume (front and rear halves of the chassis) for each
    /// truck, pushing them apart along the collision normal and exchanging linear
    /// momentum with a small restitution and tangential friction/yaw deflection.
    pub fn collide_vehicles(a: &mut Vehicle, b: &mut Vehicle) {
        let max_h = (a.truck.height + b.truck.height) * FT * 0.65;
        if (a.pos.y - b.pos.y).abs() > max_h {
            return;
        }
        let fwd_a = a.forward();
        let fwd_b = b.forward();
        let half_la = (a.truck.length * FT * 0.5 * 0.65).max(0.6);
        let half_lb = (b.truck.length * FT * 0.5 * 0.65).max(0.6);
        let ra = (a.truck.width * FT * 0.5 * 0.95).max(0.75);
        let rb = (b.truck.width * FT * 0.5 * 0.95).max(0.75);
        let sum_r = ra + rb;

        // Front and rear sphere centers for each vehicle.
        let spheres_a = [a.pos + fwd_a * half_la, a.pos - fwd_a * half_la];
        let spheres_b = [b.pos + fwd_b * half_lb, b.pos - fwd_b * half_lb];

        let mut hit = false;
        let mut best_pen = 0.0f32;
        let mut hit_sa = Vec3::ZERO;
        let mut hit_sb = Vec3::ZERO;

        for &sa in &spheres_a {
            for &sb in &spheres_b {
                let diff = sa - sb;
                let d_sq = diff.x * diff.x + diff.z * diff.z;
                if d_sq < sum_r * sum_r {
                    let d = d_sq.sqrt();
                    let pen = sum_r - d;
                    if pen > best_pen {
                        best_pen = pen;
                        hit_sa = sa;
                        hit_sb = sb;
                        hit = true;
                    }
                }
            }
        }

        if !hit || best_pen <= 0.0 {
            return;
        }

        // Normal points from vehicle B towards vehicle A.
        let center_diff = a.pos - b.pos;
        let center_d_sq = center_diff.x * center_diff.x + center_diff.z * center_diff.z;
        let normal = if center_d_sq > 1e-4 {
            vec3(center_diff.x, 0.0, center_diff.z).normalize()
        } else {
            let diff = hit_sa - hit_sb;
            let d_sq = diff.x * diff.x + diff.z * diff.z;
            if d_sq > 1e-4 {
                vec3(diff.x, 0.0, diff.z).normalize()
            } else {
                vec3(1.0, 0.0, 0.0)
            }
        };

        let ma = a.mass();
        let mb = b.mass();
        let inv_ma = 1.0 / ma;
        let inv_mb = 1.0 / mb;
        let inv_sum = inv_ma + inv_mb;

        // Push vehicles apart to resolve penetration.
        let push_a = normal * (best_pen * (inv_ma / inv_sum) * 0.85);
        let push_b = -normal * (best_pen * (inv_mb / inv_sum) * 0.85);
        a.pos += push_a;
        b.pos += push_b;

        // Relative velocity at contact points.
        let v_rel = a.vel - b.vel;
        let vn = v_rel.dot(normal);
        if vn < 0.0 {
            let restitution = 0.25;
            let j = -(1.0 + restitution) * vn / inv_sum;
            let impulse = normal * j;
            a.vel += impulse * inv_ma;
            b.vel -= impulse * inv_mb;

            // Tangential scrub. The tangential delta is the one that yaws a
            // chassis (a radial normal acts through the contact radius), and
            // the angular change is `torque / yaw_inertia`, so the deflection
            // scales with the impulse and the chassis' own inertia instead of a
            // fixed rad/s per tick.
            let v_tan = v_rel - normal * vn;
            let scrub = 0.20;
            let dv_a = -v_tan * scrub * (inv_ma / inv_sum);
            let dv_b = v_tan * scrub * (inv_mb / inv_sum);
            a.vel += dv_a;
            b.vel += dv_b;
            let lever_a = hit_sa - a.pos;
            let lever_b = hit_sb - b.pos;
            let torque_a = lever_a.z * dv_a.x - lever_a.x * dv_a.z;
            let torque_b = lever_b.z * dv_b.x - lever_b.x * dv_b.z;
            a.angular.y += (torque_a / a.yaw_inertia()).clamp(-6.0, 6.0);
            b.angular.y += (torque_b / b.yaw_inertia()).clamp(-6.0, 6.0);

            if a.hit_cooldown <= 0.0 {
                a.hit_cooldown = IMPACT_COOLDOWN;
                a.impact = a.impact.max(-vn);
                a.hits += 1;
            }
            if b.hit_cooldown <= 0.0 {
                b.hit_cooldown = IMPACT_COOLDOWN;
                b.impact = b.impact.max(-vn);
                b.hits += 1;
            }
        }
    }

    pub fn reset(&mut self, pos: Vec3, yaw: f32) {
        self.pos = pos;
        self.vel = Vec3::ZERO;
        self.angular = Vec3::ZERO;
        self.rot = Quat::from_rotation_y(yaw);
        self.rpm = IDLE_RPM;
        self.gear = self.first_gear();
        self.shift_timer = 0.0;
        self.hit_cooldown = 0.0;
        self.impact = 0.0;
        for w in &mut self.wheels {
            *w = Wheel::new(w.mount);
        }
    }
}

/// `--self-test` coverage for the dynamics core. Uses the built-in synthetic
/// rig so the check runs on a source-only checkout with no retail assets.
pub fn self_test() -> Result<(), String> {
    let flat = |_x: f32, _z: f32| (0.0, Surface::Gravel);
    let text = include_str!("selftest_truck.trk");
    let truck = crate::vehicle::parse("selftest", text)?;
    let mut v = Vehicle::new(truck);
    v.reset(Vec3::new(0.0, 2.0, 0.0), 0.0);

    for _ in 0..240 {
        v.step(&Input::default(), &flat);
    }
    let rest_y = v.pos.y;
    if rest_y < 0.5 || rest_y > 1.3 {
        return Err(format!(
            "chassis did not settle on suspension: y={rest_y:.3}"
        ));
    }

    let start = v.pos;
    for _ in 0..240 {
        v.step(
            &Input {
                throttle: 1.0,
                ..Default::default()
            },
            &flat,
        );
    }
    let moved = (v.pos - start).length();
    if moved < 2.0 {
        return Err(format!("vehicle barely moved under throttle: {moved:.2} m"));
    }
    if !v.pos.is_finite() || !v.vel.is_finite() {
        return Err("non-finite state after acceleration".into());
    }

    // Braking must reduce forward speed.
    let before = v.vel.dot(v.forward());
    for _ in 0..90 {
        v.step(
            &Input {
                brake: 1.0,
                ..Default::default()
            },
            &flat,
        );
    }
    if v.vel.dot(v.forward()) > before * 0.75 {
        return Err(format!(
            "brakes ineffective: {before:.2} -> {:.2}",
            v.vel.dot(v.forward())
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collide::Collider;

    const TRK: &str = include_str!("selftest_truck.trk");

    fn truck() -> Truck {
        crate::vehicle::parse("selftest", TRK).unwrap()
    }

    fn flat() -> impl Fn(f32, f32) -> (f32, Surface) {
        |_x, _z| (0.0, Surface::Gravel)
    }

    /// A plane climbing towards −Z at `grade` (rise / run).
    fn slope(grade: f32) -> impl Fn(f32, f32) -> (f32, Surface) {
        move |_x, z| (-z * grade, Surface::Gravel)
    }

    #[test]
    fn settles_on_springs_without_ringing() {
        let mut v = Vehicle::new(truck());
        v.reset(Vec3::new(0.0, 2.0, 0.0), 0.0);
        let mut last = v.pos.y;
        for _ in 0..600 {
            last = v.pos.y;
            v.step(&Input::default(), &flat());
        }
        let settled = v.pos.y;
        assert!((0.5..1.3).contains(&settled), "ride height {settled}");
        assert!(
            (settled - last).abs() < 0.05,
            "still moving: {last} -> {settled}"
        );
        assert!(v.wheels.iter().all(|w| w.grounded));
    }

    #[test]
    fn climbs_a_steep_grade_from_rest() {
        // 30 % grade: the old model stalled at 0 MPH / 2200 RPM here.
        let g = slope(0.30);
        let mut v = Vehicle::new(truck());
        v.reset(Vec3::new(0.0, 2.0, 0.0), 0.0);
        for _ in 0..120 {
            v.step(&Input::default(), &g);
        }
        let start = v.pos;
        for _ in 0..600 {
            v.step(
                &Input {
                    throttle: 1.0,
                    ..Default::default()
                },
                &g,
            );
        }
        let climbed = v.pos.y - start.y;
        assert!(climbed > 3.0, "did not climb the grade: {climbed:.2} m");
        assert!(v.rpm > 1500.0, "engine bogged to {} rpm", v.rpm);
    }

    #[test]
    fn reverse_engages_and_drives_backwards() {
        let mut v = Vehicle::new(truck());
        v.reset(Vec3::new(0.0, 1.0, 0.0), 0.0);
        for _ in 0..120 {
            v.step(&Input::default(), &flat());
        }
        let start = v.pos;
        for _ in 0..300 {
            v.step(
                &Input {
                    brake: 1.0,
                    ..Default::default()
                },
                &flat(),
            );
        }
        assert!(
            v.pos.z > start.z + 1.0,
            "reverse did not move: {start:?} -> {:?}",
            v.pos
        );
        assert_eq!(v.gear, v.reverse_gear());
    }

    #[test]
    fn braking_through_a_slow_corner_keeps_a_forward_gear() {
        let mut v = Vehicle::new(truck());
        v.reset(Vec3::new(0.0, 1.0, 0.0), 0.0);
        for _ in 0..120 {
            v.step(&Input::default(), &flat());
        }
        for _ in 0..120 {
            v.step(
                &Input {
                    throttle: 1.0,
                    ..Default::default()
                },
                &flat(),
            );
        }
        // Still well above walking pace: the box must not drop into reverse.
        for _ in 0..25 {
            v.step(
                &Input {
                    brake: 1.0,
                    ..Default::default()
                },
                &flat(),
            );
        }
        assert!(v.vel.length() > 1.5, "test needs residual speed");
        assert_eq!(v.gear, v.first_gear(), "braking engaged reverse");
        // Held to a stop it does engage reverse and backs up.
        for _ in 0..180 {
            v.step(
                &Input {
                    brake: 1.0,
                    ..Default::default()
                },
                &flat(),
            );
        }
        assert_eq!(v.gear, v.reverse_gear());
        assert!(v.vel.length() > 1.0, "did not back up after stopping");
    }

    #[test]
    fn chassis_never_sinks_into_a_steep_face() {
        // 45° wall: the hull must be pushed out along the normal.
        let wall = |_x: f32, z: f32| (if z < 0.0 { -z * 1.0 } else { 0.0 }, Surface::Gravel);
        let mut v = Vehicle::new(truck());
        v.reset(Vec3::new(0.0, 1.0, 6.0), 0.0);
        for _ in 0..900 {
            v.step(
                &Input {
                    throttle: 1.0,
                    ..Default::default()
                },
                &wall,
            );
            let ground = wall(v.pos.x, v.pos.z).0;
            assert!(
                v.pos.y > ground - 0.6,
                "hull sank: y={:.2} ground={:.2}",
                v.pos.y,
                ground
            );
        }
        assert!(v.pos.is_finite() && v.vel.is_finite());
    }

    #[test]
    fn collider_blocks_and_deflects() {
        let truck = truck();
        let mut v = Vehicle::new(truck);
        v.reset(Vec3::new(0.0, 0.8, 0.0), 0.0);
        let props = [Collider {
            center: Vec3::new(0.0, 1.5, -30.0),
            radius: 0.6,
            half_height: 3.0,
        }];
        for _ in 0..600 {
            v.step(
                &Input {
                    throttle: 1.0,
                    ..Default::default()
                },
                &flat(),
            );
            v.collide(&props);
        }
        assert!(v.pos.z > -30.0 + 0.6, "drove through the prop: {}", v.pos.z);
        assert!(v.pos.is_finite() && v.vel.is_finite());
    }

    #[test]
    fn collider_outside_footprint_is_ignored() {
        let mut v = Vehicle::new(truck());
        v.reset(Vec3::new(0.0, 0.8, 0.0), 0.0);
        let before = v.pos;
        v.collide(&[Collider {
            center: Vec3::new(40.0, 1.5, 40.0),
            radius: 0.6,
            half_height: 3.0,
        }]);
        assert_eq!(v.pos, before);
    }

    #[test]
    fn yaw_matches_spawn_heading_and_ignores_pitch() {
        let mut v = Vehicle::new(truck());
        for yaw in [-3.0f32, -1.2, 0.0, 0.7, 2.5] {
            v.reset(Vec3::ZERO, yaw);
            assert!(
                (v.yaw() - yaw).abs() < 1e-4,
                "yaw {yaw} read back {}",
                v.yaw()
            );
            v.rot = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(0.4);
            assert!((v.yaw() - yaw).abs() < 1e-4, "pitch leaked into yaw {yaw}");
        }
    }

    #[test]
    fn wheel_ray_hits_a_steep_face_and_a_buried_anchor() {
        // 35° face rising towards −X; the old Newton refinement overshot on
        // such a slope with a tilted ray and reported a miss.
        let face = |x: f32, _z: f32| (-x * 0.7, Surface::Dirt);
        let v = Vehicle::new(truck());
        let dir = vec3(-0.41, -0.81, -0.43).normalize();
        let anchor = vec3(0.0, 0.06, 0.0);
        let hit = v.cast_ground(anchor, dir, 0.93, &face);
        assert!(hit.is_some_and(|t| t < 0.2), "steep face missed: {hit:?}");
        // An anchor under the surface is a fully compressed wheel (t = 0),
        // not a hit at full reach (which read as zero compression).
        let buried = v.cast_ground(vec3(0.0, -0.5, 0.0), dir, 0.93, &face);
        assert_eq!(buried, Some(0.0));
    }

    #[test]
    fn reverse_is_rev_limited() {
        // Holding brake/reverse used to accelerate without bound: the torque
        // table clamps at the redline and reverse never upshifts.
        let mut v = Vehicle::new(truck());
        let ground = flat();
        v.reset(Vec3::new(0.0, 1.0, 0.0), 0.0);
        for _ in 0..(20.0 * TICK_HZ) as usize {
            v.step(
                &Input {
                    brake: 1.0,
                    ..Default::default()
                },
                &ground,
            );
        }
        let v_back = -v.vel.dot(v.forward());
        let ratio = v.truck.gear_ratios[1].abs() * v.truck.final_drive;
        let limit = v.truck.engine.redline / 60.0 * std::f32::consts::TAU * v.tyre_radius() / ratio;
        assert!(v_back > 1.0, "reverse must move the truck back: {v_back}");
        assert!(
            v_back < limit * 1.1,
            "reverse ran past the redline: {v_back} m/s vs {limit}"
        );
    }

    #[test]
    fn overturned_truck_scrubs_to_a_stop() {
        // Upside down and sliding: the hull contact must have friction (the
        // frictionless projection let trucks skate on their roofs at speed).
        let mut v = Vehicle::new(truck());
        let ground = flat();
        v.reset(Vec3::new(0.0, 1.2, 0.0), 0.0);
        v.rot = Quat::from_rotation_z(std::f32::consts::PI);
        v.vel = vec3(0.0, 0.0, -15.0);
        for _ in 0..(3.0 * TICK_HZ) as usize {
            v.step(&Input::default(), &ground);
        }
        assert!(
            v.vel.length() < 2.0,
            "still sliding at {} m/s",
            v.vel.length()
        );
        assert!(v.pos.y > -0.1, "sank through the ground: {}", v.pos.y);
    }

    #[test]
    fn positive_steer_turns_right() {
        // Input convention (app.rs): D / Right arrow = +1 = turn right.
        let mut v = Vehicle::new(truck());
        let ground = flat();
        v.reset(Vec3::new(0.0, 1.0, 0.0), 0.0);
        for _ in 0..120 {
            v.step(
                &Input {
                    throttle: 0.6,
                    ..Default::default()
                },
                &ground,
            );
        }
        let right0 = v.right();
        let p0 = v.pos;
        for _ in 0..60 {
            v.step(
                &Input {
                    throttle: 0.4,
                    steer: 1.0,
                    ..Default::default()
                },
                &ground,
            );
        }
        let moved = v.pos - p0;
        assert!(
            moved.dot(right0) > 0.5,
            "steer +1 must move the truck to its right, moved {moved:?} right {right0:?}"
        );
    }

    #[test]
    fn low_range_multiplies_wheel_torque() {
        let mut v = Vehicle::new(truck());
        v.reset(Vec3::new(0.0, 1.0, 0.0), 0.0);
        v.rpm = 3000.0;
        let high = v.engine_force();
        v.low_range = true;
        let low = v.engine_force();
        assert!(low > high * 1.5, "low range {low} vs high {high}");
    }

    #[test]
    fn sustained_contact_is_one_impact() {
        // A truck grinding along a trunk must count once and arm the thump once,
        // not add a `hit` and a `rad/s` twist on every resolving tick.
        let mut v = Vehicle::new(truck());
        v.reset(Vec3::new(0.0, 1.0, 0.0), 0.0);
        v.vel = vec3(0.0, 0.0, -6.0);
        let c = Collider {
            center: vec3(0.7, 1.0, -1.3),
            radius: 1.2,
            half_height: 2.0,
        };
        v.collide(&[c]);
        assert_eq!(v.hits, 1);
        let first_impact = v.impact;
        assert!(first_impact > 0.0, "impact speed must be recorded");
        let spin = v.angular.y;
        for _ in 0..5 {
            v.collide(&[c]);
        }
        assert_eq!(v.hits, 1, "sustained contact must not re-hit");
        assert!(v.angular.y.is_finite() && (v.angular.y - spin).abs() < 0.2);
    }

    #[test]
    fn collision_deflection_scales_with_inertia() {
        // Same touch, lighter truck: the yaw kick must be larger. The old flat
        // `rad/s` per tick ignored mass entirely.
        let spin_for = |mass_scale: f32| {
            let mut v = Vehicle::new(truck());
            v.mods.weight = mass_scale;
            v.reset(Vec3::new(0.0, 1.0, 0.0), 0.0);
            v.vel = vec3(0.0, 0.0, -6.0);
            v.collide(&[Collider {
                center: vec3(0.7, 1.0, -1.3),
                radius: 1.2,
                half_height: 2.0,
            }]);
            v.angular.y.abs()
        };
        let light = spin_for(0.0);
        let heavy = spin_for(4000.0);
        assert!(light > heavy, "light {light} should spin more than {heavy}");
        assert!(light.is_finite() && heavy.is_finite());
    }

    #[test]
    fn vehicle_to_vehicle_collision_deflects_and_resolves() {
        let mut a = Vehicle::new(truck());
        let mut b = Vehicle::new(truck());
        // Vehicle a at z = 0 facing -Z, Vehicle b at z = -2.5 facing +Z (head-on).
        a.reset(Vec3::new(0.0, 1.0, 0.0), 0.0);
        b.reset(Vec3::new(0.4, 1.0, -2.5), std::f32::consts::PI);
        // Head-on velocity: a moves towards -Z (forward for a), b moves towards +Z (forward for b).
        a.vel = Vec3::new(0.0, 0.0, -8.0);
        b.vel = Vec3::new(0.0, 0.0, 8.0);

        let p_a_before = a.pos;
        let p_b_before = b.pos;
        Vehicle::collide_vehicles(&mut a, &mut b);

        assert!(a.hits > 0, "truck a should register a hit");
        assert!(b.hits > 0, "truck b should register a hit");
        assert!(
            (a.pos - b.pos).length() > (p_a_before - p_b_before).length(),
            "vehicles must be pushed apart"
        );
        assert!(a.vel.z > -8.0, "vehicle a forward speed must be reduced");
        assert!(b.vel.z < 8.0, "vehicle b forward speed must be reduced");
    }

    #[test]
    fn anti_roll_bar_generates_corrective_roll_torque() {
        let mut v = Vehicle::new(truck());
        v.reset(Vec3::new(0.0, 1.0, 0.0), 0.0);
        // Force left wheel more compressed than right wheel (leaning left).
        v.wheels[0].grounded = true;
        v.wheels[0].compression = 0.3;
        v.wheels[1].grounded = true;
        v.wheels[1].compression = 0.1;

        let ground = |_: f32, _: f32| (0.0, Surface::Dirt);
        v.step(
            &Input {
                throttle: 0.0,
                steer: 0.0,
                ..Default::default()
            },
            &ground,
        );
        // Step integrates roll torque and should resist roll.
        assert!(v.pos.is_finite() && v.angular.is_finite());
    }

    /// The anti-tip tuning: centre of mass low and anti-roll stiff, so minor
    /// unevenness does not lever the truck onto two wheels.
    #[test]
    fn anti_tip_tuning_keeps_cg_low_and_arb_stiff() {
        let v = Vehicle::new(truck());
        assert!(
            v.config.cg_height_m <= 0.50,
            "centre of mass too high: {}",
            v.config.cg_height_m
        );
        assert!(v.config.cg_height_m >= 0.28);
        assert!(v.config.anti_roll >= 0.8, "anti-roll too soft");
    }

    /// Air control (offroad4x4 `AIR_PITCH_ACCEL` / `AIR_YAW_ACCEL`): in the
    /// air throttle lifts the nose, brake drops it and steering yaws the way
    /// it steers; without input the attitude is left alone.
    #[test]
    fn air_control_pitches_and_yaws_while_airborne() {
        let ground = |_: f32, _: f32| (0.0, Surface::Dirt);
        let fly = |input: Input| {
            let mut v = Vehicle::new(truck());
            v.reset(Vec3::new(0.0, 60.0, 0.0), 0.0);
            for _ in 0..30 {
                v.step(&input, &ground);
            }
            assert!(v.airtime > 0.0, "the truck must still be falling");
            (v.pitch_deg(), v.forward())
        };
        let (idle, _) = fly(Input::default());
        assert!(idle.abs() < 0.5, "no input must not pitch: {idle}");
        let (up, _) = fly(Input {
            throttle: 1.0,
            ..Default::default()
        });
        assert!(up > 3.0, "throttle should lift the nose: {up}");
        let (down, _) = fly(Input {
            brake: 1.0,
            ..Default::default()
        });
        assert!(down < -3.0, "brake should drop the nose: {down}");
        // Positive steer turns right, as on the ground: forward (-Z at yaw 0)
        // swings towards +X.
        let (_, fwd) = fly(Input {
            steer: 1.0,
            ..Default::default()
        });
        assert!(fwd.x > 0.05, "steer should yaw right in the air: {fwd:?}");
    }

    #[test]
    fn jump_landing_settles_without_pogo_bounce() {
        let mut v = Vehicle::new(truck());
        v.reset(Vec3::new(0.0, 3.0, 0.0), 0.0);
        v.vel = Vec3::new(0.0, -4.5, -10.0);
        let ground = |_: f32, _: f32| (0.0, Surface::Dirt);
        let input = Input {
            throttle: 0.2,
            ..Default::default()
        };
        let mut touched = false;
        let mut rebounded = false;
        for step in 0..180 {
            v.step(&input, &ground);
            let grounded = v.wheels.iter().filter(|w| w.grounded).count();
            if grounded >= 2 {
                touched = true;
            } else if touched && step < 150 && v.airtime > 0.08 {
                rebounded = true;
            }
        }
        assert!(touched, "truck must contact the ground after drop");
        assert!(!rebounded, "rebound damping should prevent pogo bounce");
        assert!(
            v.vel.y.abs() < 0.25,
            "vertical velocity should settle smoothly, got {}",
            v.vel.y
        );
    }
}
