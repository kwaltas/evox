//! AI opponents from the original `.TXT` racing lines.
//!
//! Each file is a recorded human lap: header (driver, class, checkpoint count,
//! point count) followed by `x, y, z` / heading / flag records in feet and
//! degrees, using the same world space as the terrain and `.SIT` objects. The
//! driver follows the recorded line with pure pursuit, schedules speed from
//! local curvature plus the authored caps, brakes for slower corners and lifts
//! for a rival blocking the lane, and applies class-based skill/rubber band.
//! See `docs/AI.md` for the geometry and tuning constants.

use macroquad::prelude::*;

use crate::physics::Input;
use crate::terrain::Track;

pub const FT: f32 = crate::vehicle::FT;

/// Pure-pursuit geometry. The controller aims at a point on the racing line
/// one look-ahead distance ahead and converts the bearing error into the
/// bicycle-model steering angle `δ = atan2(2·L·sin α, l_d)`. That angle is
/// turned into rack input with `physics::steer_lock_at`, the same
/// speed-sensitive lock the rack applies, so the AI never exceeds the
/// player's steering and does not under-steer at speed.
///
/// Defaults until `Driver::fit_to` reads the real truck: `physics::Config`
/// `steer_max` and a mid-size wheelbase.
const WHEELBASE_M: f32 = 2.8;
const MAX_STEER_RAD: f32 = 0.55;
const LOOKAHEAD_MIN_M: f32 = 7.0;
const LOOKAHEAD_MAX_M: f32 = 24.0;
/// Lateral acceleration the tyres can sustain before the curve-cap controller
/// starts lifting. Below the Pacejka peak on dirt (μ·g ≈ 9 m/s²).
const LATERAL_GRIP_MPS2: f32 = 10.0;
/// Service braking used for the corner-entry distance calculation.
const BRAKE_DECEL_MPS2: f32 = 6.5;
/// Rival-awareness cone ahead of the bumper.
const TRAFFIC_LOOK_M: f32 = 16.0;
const TRAFFIC_HALF_WIDTH_M: f32 = 3.2;
const TRAFFIC_MIN_GAP_M: f32 = 4.5;

fn wrap_pi(mut a: f32) -> f32 {
    const PI: f32 = std::f32::consts::PI;
    while a > PI {
        a -= std::f32::consts::TAU;
    }
    while a < -PI {
        a += std::f32::consts::TAU;
    }
    a
}

#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub pos: Vec3,
    pub heading: f32,
    pub flag: u8,
    pub speed_cap: f32,
}

#[derive(Clone, Debug)]
pub struct RacingLine {
    pub driver: String,
    pub class: u8,
    pub nodes: Vec<Node>,
}

impl RacingLine {
    pub fn parse(text: &str) -> Result<Self, String> {
        let all: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
        let mut driver = String::from("AI");
        let mut class = 1u8;
        let mut declared = 0usize;
        let mut i = 0;
        while i < all.len() {
            let lower = all[i].to_ascii_lowercase();
            if let Some(v) = all[i].split_once(':') {
                let val = v.1.trim();
                if lower.starts_with("driver name") {
                    driver = val.to_string();
                } else if lower.starts_with("class") {
                    class = val
                        .trim_start_matches("Class_")
                        .trim_start_matches("class_")
                        .parse()
                        .unwrap_or(1);
                } else if lower.starts_with("point count") {
                    declared = val.parse().unwrap_or(0);
                }
            } else if all[i]
                .chars()
                .next()
                .map(|c| c.is_ascii_digit() || c == '-' || c == '+')
                .unwrap_or(false)
            {
                break;
            }
            i += 1;
        }
        let mut nodes = Vec::new();
        while i + 2 < all.len() {
            let xyz: Vec<f32> = all[i]
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            if xyz.len() < 3 {
                i += 1;
                continue;
            }
            let kmh = all[i + 1].trim().parse::<f32>().unwrap_or(60.0);
            let flag = all[i + 2].trim().parse::<u8>().unwrap_or(0);
            nodes.push(Node {
                pos: vec3(xyz[0] * FT, xyz[1] * FT, xyz[2] * FT),
                heading: 0.0,
                flag,
                speed_cap: (kmh / 3.6).clamp(4.0, 45.0),
            });
            i += 3;
        }
        if declared > 0 && nodes.len() + 8 < declared {
            return Err(format!(
                "expected {declared} points, parsed {}",
                nodes.len()
            ));
        }
        if nodes.len() < 8 {
            return Err("racing line too short".into());
        }
        let n = nodes.len();
        for i in 0..n {
            let to = nodes[(i + 1) % n].pos - nodes[i].pos;
            // Physics yaw: forward is -Z.
            nodes[i].heading = (-to.x).atan2(-to.z);
        }
        let mut line = Self {
            driver,
            class,
            nodes,
        };
        line.compute_speed_caps();
        Ok(line)
    }

    /// The recording already encodes cornering speeds, so start from those and
    /// only add a braking-distance lookahead so a car never enters a slower
    /// node too hot.
    pub(crate) fn compute_speed_caps(&mut self) {
        let n = self.nodes.len();
        let mut caps: Vec<f32> = self.nodes.iter().map(|n| n.speed_cap).collect();
        for i in (0..n).rev() {
            let next = caps[(i + 1) % n];
            let dist = (self.nodes[(i + 1) % n].pos - self.nodes[i].pos).length();
            let limit = (next * next + 2.0 * 9.0 * dist).sqrt();
            caps[i] = caps[i].min(limit);
        }
        for (n, cap) in self.nodes.iter_mut().zip(caps) {
            n.speed_cap = cap;
        }
    }

    pub fn closest(&self, p: Vec3) -> usize {
        let mut best = (f32::MAX, 0);
        for (i, n) in self.nodes.iter().enumerate() {
            let d = (n.pos - p).length_squared();
            if d < best.0 {
                best = (d, i);
            }
        }
        best.1
    }

    /// Nearest node searching only forward from `cursor`. A global nearest
    /// search can jump to a parallel switchback or the far side of a hairpin,
    /// which makes the controller cut the course; progress is monotonic.
    pub fn nearest_ahead(&self, cursor: usize, p: Vec3, window: usize) -> usize {
        let n = self.nodes.len();
        let mut best = (f32::MAX, cursor % n);
        for k in 0..window.min(n) {
            let i = (cursor + k) % n;
            let d = (self.nodes[i].pos - p).length_squared();
            if d < best.0 {
                best = (d, i);
            }
        }
        best.1
    }

    /// Length of the segment from node `i` to the next node.
    fn segment_len(&self, i: usize) -> f32 {
        let n = self.nodes.len();
        (self.nodes[(i + 1) % n].pos - self.nodes[i % n].pos).length()
    }

    /// Point on the line `distance` metres along the loop from `cursor`.
    /// This is the pure-pursuit target; it may fall between recorded nodes.
    pub fn point_ahead(&self, cursor: usize, distance: f32) -> Vec3 {
        let n = self.nodes.len();
        let mut i = cursor % n;
        let mut remaining = distance.max(0.0);
        for _ in 0..n {
            let next = (i + 1) % n;
            let seg = (self.nodes[next].pos - self.nodes[i].pos).length();
            if seg < 1e-3 {
                i = next;
                continue;
            }
            if remaining <= seg {
                let t = (remaining / seg).clamp(0.0, 1.0);
                return self.nodes[i].pos + (self.nodes[next].pos - self.nodes[i].pos) * t;
            }
            remaining -= seg;
            i = next;
        }
        self.nodes[i].pos
    }

    /// Peak curvature over the next `steps` windows of `span` nodes, as
    /// heading change per metre. Straight sections return ~0; a hairpin on a
    /// 30 m radius returns ~1/30.
    pub fn max_curvature(&self, cursor: usize, span: usize, steps: usize) -> f32 {
        let n = self.nodes.len();
        let span = span.clamp(1, n.saturating_sub(1).max(1));
        let mut peak = 0.0f32;
        for step in 0..steps {
            let i = (cursor + step) % n;
            let j = (i + span) % n;
            let mut arc = 0.0;
            for k in 0..span {
                arc += self.segment_len(i + k);
            }
            if arc < 1e-3 {
                continue;
            }
            let turn = wrap_pi(self.nodes[j].heading - self.nodes[i].heading).abs();
            peak = peak.max(turn / arc);
        }
        peak
    }

    /// Lowest authored speed cap within `max_distance` metres ahead, and the
    /// distance at which it occurs. Used for brake-point calculation.
    pub fn speed_limit_ahead(&self, cursor: usize, max_distance: f32) -> (f32, f32) {
        let n = self.nodes.len();
        let start = cursor % n;
        let mut i = start;
        let mut travelled = 0.0f32;
        let mut best = (self.nodes[start].speed_cap, 0.0f32);
        for _ in 0..n {
            let next = (i + 1) % n;
            let seg = self.segment_len(i);
            travelled += seg;
            if travelled > max_distance {
                break;
            }
            let cap = self.nodes[next].speed_cap;
            if cap < best.0 {
                best = (cap, travelled);
            }
            i = next;
        }
        best
    }
}

#[derive(Clone, Debug)]
pub struct Driver {
    pub line: RacingLine,
    pub cursor: usize,
    pub skill: f32,
    pub aggression: f32,
    pub stuck: f32,
    pub reverse: f32,
    pub finished: bool,
    pub name: String,
    /// Pure-pursuit wheelbase `L` (m).
    pub wheelbase_m: f32,
    /// Standstill front-wheel lock (rad) of the driven truck.
    pub steer_lock_rad: f32,
}

impl Driver {
    pub fn new(line: RacingLine, skill: f32, name: &str, start: usize) -> Self {
        Self {
            line,
            cursor: start % 623.max(1),
            skill: skill.clamp(0.55, 1.15),
            aggression: 0.6,
            stuck: 0.0,
            reverse: 0.0,
            finished: false,
            name: name.to_string(),
            wheelbase_m: WHEELBASE_M,
            steer_lock_rad: MAX_STEER_RAD,
        }
    }

    /// Use the driven truck's wheelbase and standstill steering lock for the
    /// pure-pursuit geometry.
    pub fn fit_to(&mut self, vehicle: &crate::physics::Vehicle) {
        self.wheelbase_m = vehicle.truck.wheelbase_m().max(1.5);
        self.steer_lock_rad = vehicle.steer_lock_rad();
    }

    pub fn tick(
        &mut self,
        pos: Vec3,
        vel: Vec3,
        yaw: f32,
        ahead: usize,
        dt: f32,
        rubber: f32,
    ) -> Input {
        self.tick_with_rivals(pos, vel, yaw, ahead, dt, rubber, &[])
    }

    /// Pure-pursuit steering plus curvature/traffic speed control. `rivals`
    /// are other chassis positions; a rival inside the forward cone makes the
    /// driver lift or brake instead of driving through it.
    pub fn tick_with_rivals(
        &mut self,
        pos: Vec3,
        vel: Vec3,
        yaw: f32,
        ahead: usize,
        dt: f32,
        rubber: f32,
        rivals: &[Vec3],
    ) -> Input {
        let node = self.line.nodes[self.cursor.min(self.line.nodes.len() - 1)].pos;
        if (node - pos).length() > 20.0 {
            // Teleport/rescue/crash recovery only; normal updates stay local
            // so a hairpin cannot capture the cursor onto the opposite lane.
            self.cursor = self.line.closest(pos);
        } else {
            self.cursor = self.line.nearest_ahead(self.cursor, pos, 24);
        }

        // Signed speed along the chassis heading: rolling backwards is not
        // "too fast", and braking while reversing would drive the truck
        // backwards harder (brake doubles as reverse throttle).
        let heading = vec3(-yaw.sin(), 0.0, -yaw.cos());
        let v_fwd = vel.dot(heading);
        let speed = v_fwd.max(0.0);

        // Pure pursuit: aim one speed-scaled look-ahead distance down the
        // recorded line. The target is interpolated between nodes so the
        // controller does not chatter when the line is sparse.
        let lead = (self.line.nodes[self.cursor].pos - pos).length();
        let lookahead = (LOOKAHEAD_MIN_M + 0.7 * speed - lead).clamp(4.0, LOOKAHEAD_MAX_M);
        let mut target = self.line.point_ahead(self.cursor, lookahead);
        // A cursor left behind after a spin or a teleport would point
        // backwards; walk it forward until the target is ahead of the bumper.
        for _ in 0..8 {
            let to = target - pos;
            if to.x * heading.x + to.z * heading.z >= 0.0 {
                break;
            }
            self.cursor = (self.cursor + 1) % self.line.nodes.len();
            target = self.line.point_ahead(self.cursor, lookahead);
        }
        let to = target - pos;
        let want = to.z.atan2(to.x);
        // Compare against the chassis forward angle in the same atan2(z, x)
        // frame: forward is (-sin yaw, -cos yaw).
        let cur = (-yaw.cos()).atan2(-yaw.sin());
        let alpha = wrap_pi(want - cur);
        // Bicycle-model pure pursuit: δ = atan2(2·L·sin α, l_d).
        let delta = (2.0 * self.wheelbase_m * alpha.sin()).atan2(lookahead.max(1.0));
        // Same speed-sensitive lock as the rack: at 20 m/s full input is only
        // ~0.24 rad, so dividing by the standstill lock under-steered ~2.3×.
        let lock = crate::physics::steer_lock_at(self.steer_lock_rad, v_fwd);
        let steer = (delta / lock).clamp(-1.0, 1.0);

        // Speed target: authored caps ahead, tightened by the sharpest
        // curvature the line is about to ask for.
        let scan = lookahead;
        let (path_cap, cap_distance) = self.line.speed_limit_ahead(self.cursor, scan);
        let curvature = self.line.max_curvature(self.cursor, 5, 3);
        let curve_cap = (LATERAL_GRIP_MPS2 / curvature.max(1e-4))
            .sqrt()
            .clamp(6.0, 45.0);
        let cap = path_cap.min(curve_cap) * self.skill * rubber * (1.0 + self.aggression * 0.08);
        let limit_distance = if curve_cap <= path_cap {
            lookahead.max(6.0)
        } else {
            cap_distance.max(3.0)
        };

        let mut input = Input {
            steer,
            ..Default::default()
        };
        if speed < cap {
            // Full throttle on the straights; taper only as the cap closes in.
            input.throttle = (0.65 + (cap - speed) * 0.12).min(1.0);
        }
        if speed > cap * 0.98 {
            let required = (speed * speed - cap * cap) / (2.0 * limit_distance);
            if required > 2.5 {
                input.brake = (required / BRAKE_DECEL_MPS2 * 0.8).clamp(0.12, 1.0);
                input.throttle *= (1.0 - input.brake).max(0.0);
            }
        }

        // Rival awareness: brake for a chassis filling the lane ahead.
        let safe_gap = TRAFFIC_MIN_GAP_M + speed * 0.55;
        let mut nearest = f32::MAX;
        for &r in rivals {
            let rel = r - pos;
            let forward = rel.x * heading.x + rel.z * heading.z;
            if forward <= 0.5 || forward > TRAFFIC_LOOK_M {
                continue;
            }
            let lateral = (rel - heading * forward).length();
            if lateral <= TRAFFIC_HALF_WIDTH_M {
                nearest = nearest.min(forward);
            }
        }
        if nearest < safe_gap {
            let urgency = ((safe_gap - nearest) / safe_gap.max(0.1)).clamp(0.0, 1.0);
            input.brake = input.brake.max(urgency * 0.85);
            input.throttle *= 1.0 - urgency;
        }

        input.drive_mode = match (ahead % 9) / 3 {
            0 => 2,
            1 => 1,
            _ => {
                if speed > 12.0 {
                    0
                } else {
                    1
                }
            }
        };

        // Held on the grid (`rubber == 0`) is not stuck.
        if rubber > 0.0 && vel.length() < 1.2 && self.reverse <= 0.0 {
            self.stuck += dt;
            if self.stuck > 1.5 {
                self.reverse = 1.1;
                self.stuck = 0.0;
            }
        } else {
            self.stuck = 0.0;
        }
        if self.reverse > 0.0 {
            self.reverse -= dt;
            input.throttle = 0.0;
            input.brake = 0.55;
            input.steer = -input.steer;
        }
        input
    }
}

/// Pick a racing line for a track, preferring the given class band.
pub fn line_for<'a>(lines: &'a [RacingLine], _track: &str, class: u8) -> Option<&'a RacingLine> {
    let mut candidates: Vec<&RacingLine> = lines.iter().collect();
    candidates.sort_by_key(|l| (l.class as i32 - class as i32).abs());
    candidates.into_iter().find(|_| true)
}

pub fn grid_slot(track: &Track, line: &RacingLine, index: usize) -> (Vec3, f32) {
    let node = line.nodes[index.min(line.nodes.len() - 1)];
    let offset = (index as f32 - 3.5) * 2.6;
    let right = vec3(node.heading.cos(), 0.0, -node.heading.sin());
    let p = node.pos + right * offset;
    (
        vec3(p.x, track.height_at(p.x, p.z) + 0.6, p.z),
        node.heading,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> RacingLine {
        let mut text =
            String::from("File Version: 12\nDriver Name: Test\nClass: Class_2\nPoint Count: 20\n");
        for i in 0..20 {
            let a = i as f32 / 20.0 * std::f32::consts::TAU;
            let x = 300.0 + a.cos() * 120.0;
            let z = 300.0 + a.sin() * 120.0;
            text.push_str(&format!(
                "{x:.2}, 10.0, {z:.2}\n{}\n0\n",
                (a.to_degrees() + 90.0) % 360.0
            ));
        }
        RacingLine::parse(&text).unwrap()
    }

    #[test]
    fn parses_nodes_and_caps() {
        let l = line();
        assert_eq!(l.nodes.len(), 20);
        assert_eq!(l.class, 2);
        assert!(l.nodes.iter().all(|n| n.speed_cap > 0.0));
        // Feet conversion to metres.
        assert!((l.nodes[0].pos.x - 420.0 * FT).abs() < 1e-3);
    }

    #[test]
    fn driver_steers_toward_line() {
        let l = line();
        let start = l.nodes[0].pos;
        let mut d = Driver::new(l, 0.9, "bot", 0);
        let input = d.tick(start, Vec3::new(0.0, 0.0, 0.0), 0.0, 0, 1.0 / 60.0, 1.0);
        assert!(input.steer.abs() <= 1.0);
        assert!(input.throttle > 0.0);
    }

    #[test]
    fn short_line_rejected() {
        assert!(RacingLine::parse("Point Count: 3\n1,2,3\n0\n0\n").is_err());
    }

    fn straight_line() -> RacingLine {
        let nodes = (0..20)
            .map(|i| Node {
                pos: vec3(0.0, 0.0, -i as f32 * 10.0),
                heading: 0.0,
                flag: 0,
                speed_cap: 30.0,
            })
            .collect();
        RacingLine {
            driver: "Line".into(),
            class: 1,
            nodes,
        }
    }

    #[test]
    fn lookahead_interpolates_between_nodes() {
        let l = straight_line();
        let p = l.point_ahead(0, 15.0);
        assert!((p.z + 15.0).abs() < 1e-4, "target {p:?}");
        // Beyond one segment it wraps onto the next node's segment.
        let p = l.point_ahead(0, 25.0);
        assert!((p.z + 25.0).abs() < 1e-4, "target {p:?}");
    }

    #[test]
    fn curvature_is_flat_on_a_straight_and_positive_in_a_corner() {
        let straight = straight_line();
        assert!(straight.max_curvature(0, 3, 4) < 1e-5);
        let mut corner = straight_line();
        for (i, n) in corner.nodes.iter_mut().enumerate() {
            n.heading = i as f32 * 0.18;
        }
        assert!(corner.max_curvature(0, 3, 4) > 0.005);
    }

    #[test]
    fn speed_limit_reports_the_slowest_node_ahead() {
        let mut l = straight_line();
        l.nodes[3].speed_cap = 9.0;
        let (cap, dist) = l.speed_limit_ahead(0, 40.0);
        assert!((cap - 9.0).abs() < 1e-6);
        assert!((dist - 30.0).abs() < 1e-3, "dist {dist}");
    }

    #[test]
    fn pure_pursuit_steers_toward_the_line() {
        let l = straight_line();
        let mut d = Driver::new(l, 0.9, "bot", 0);
        // Off to the right of a straight: the target is left, so steer left
        // (negative) to converge.
        let right = d.tick(
            vec3(1.0, 0.0, 0.0),
            vec3(0.0, 0.0, -8.0),
            0.0,
            0,
            1.0 / 60.0,
            1.0,
        );
        assert!(right.steer < -0.02, "steer {}", right.steer);
        let mut d = Driver::new(straight_line(), 0.9, "bot", 0);
        let left = d.tick(
            vec3(-1.0, 0.0, 0.0),
            vec3(0.0, 0.0, -8.0),
            0.0,
            0,
            1.0 / 60.0,
            1.0,
        );
        assert!(left.steer > 0.02, "steer {}", left.steer);
    }

    /// The rack shrinks full lock with speed (`physics::steer_lock_at`). The
    /// AI must ask for the pure-pursuit wheel angle in those terms; dividing
    /// by the standstill lock under-steered ~2.3× at 20 m/s and sent trucks
    /// wide into corners (THEHILL off line, DEVIL rolls in `drive_report`).
    #[test]
    fn pure_pursuit_rack_input_matches_the_speed_sensitive_lock() {
        let v = 20.0;
        let pos = vec3(1.5, 0.0, 0.0);
        let mut d = Driver::new(straight_line(), 0.9, "bot", 0);
        let input = d.tick(pos, vec3(0.0, 0.0, -v), 0.0, 0, 1.0 / 60.0, 1.0);

        let lead = (d.line.nodes[d.cursor].pos - pos).length();
        let l_d = (LOOKAHEAD_MIN_M + 0.7 * v - lead).clamp(4.0, LOOKAHEAD_MAX_M);
        let target = d.line.point_ahead(d.cursor, l_d);
        let to = target - pos;
        let alpha = wrap_pi(to.z.atan2(to.x) - (-1.0f32).atan2(0.0));
        let delta = (2.0 * WHEELBASE_M * alpha.sin()).atan2(l_d);
        let wheel = input.steer * crate::physics::steer_lock_at(MAX_STEER_RAD, v);
        assert!(
            (wheel - delta).abs() < 1e-3,
            "wheel angle {wheel} rad, pure pursuit wants {delta} rad"
        );
        assert!(
            input.steer.abs() > delta.abs() / MAX_STEER_RAD * 1.5,
            "rack input {} ignores the speed falloff",
            input.steer
        );
    }

    #[test]
    fn full_throttle_on_a_clear_straight() {
        let mut d = Driver::new(straight_line(), 0.9, "bot", 0);
        let input = d.tick(
            vec3(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, -5.0),
            0.0,
            0,
            1.0 / 60.0,
            1.0,
        );
        assert!(input.throttle > 0.99, "throttle {}", input.throttle);
        assert_eq!(input.brake, 0.0);
    }

    #[test]
    fn brakes_for_a_rival_in_the_lane() {
        let mut d = Driver::new(straight_line(), 0.9, "bot", 0);
        let pos = vec3(0.0, 0.0, 0.0);
        let vel = vec3(0.0, 0.0, -15.0);
        let clear = d.tick(pos, vel, 0.0, 0, 1.0 / 60.0, 1.0);
        let mut d = Driver::new(straight_line(), 0.9, "bot", 0);
        let blocked =
            d.tick_with_rivals(pos, vel, 0.0, 0, 1.0 / 60.0, 1.0, &[vec3(0.0, 0.0, -6.0)]);
        assert!(blocked.brake > clear.brake, "brake {}", blocked.brake);
        assert!(
            blocked.throttle < clear.throttle,
            "throttle {} vs {}",
            blocked.throttle,
            clear.throttle
        );
        // A rival in the next lane must not trigger the brakes.
        let mut d = Driver::new(straight_line(), 0.9, "bot", 0);
        let beside = d.tick_with_rivals(pos, vel, 0.0, 0, 1.0 / 60.0, 1.0, &[vec3(8.0, 0.0, -6.0)]);
        assert_eq!(beside.brake, clear.brake);
    }
}
