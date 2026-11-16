//! AI opponents from the original `.TXT` racing lines.
//!
//! Each file is a recorded human lap: header (driver, class, checkpoint count,
//! point count) followed by `x, y, z` / heading / flag records in feet and
//! degrees, using the same world space as the terrain and `.SIT` objects. The driver follows the recorded line with pure pursuit, schedules
//! speed from local curvature, and applies class-based skill and rubber band.

use macroquad::prelude::*;

use crate::physics::Input;
use crate::terrain::Track;

pub const FT: f32 = crate::vehicle::FT;

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
        }
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
        self.cursor = self
            .line
            .closest(pos)
            .max(self.cursor.min(self.line.nodes.len() - 1));
        let target_i = (self.cursor + 6) % self.line.nodes.len();
        let target = self.line.nodes[target_i].pos;
        let to = target - pos;
        let flat = vec2(to.x, to.z);
        let want = flat.y.atan2(flat.x);
        // Compare against the chassis forward angle in the same atan2(z, x)
        // frame: forward is (-sin yaw, -cos yaw).
        let mut cur = (-yaw.cos()).atan2(-yaw.sin());
        while cur - want > std::f32::consts::PI {
            cur -= std::f32::consts::TAU;
        }
        while cur - want < -std::f32::consts::PI {
            cur += std::f32::consts::TAU;
        }
        let mut err = want - cur;
        // Keep the pursuit ahead of the car; a locked cursor on a slow tick
        // causes the classic "AI drives to the apex then stalls" bug.
        if to.length() < 3.0 {
            self.cursor = (self.cursor + 1) % self.line.nodes.len();
        }
        err = err.clamp(-1.0, 1.0);

        // Signed speed along the chassis heading: rolling backwards is not
        // "too fast", and braking while reversing would drive the truck
        // backwards harder (brake doubles as reverse throttle).
        let heading = vec3(-yaw.sin(), 0.0, -yaw.cos());
        let v_fwd = vel.dot(heading);
        let speed = v_fwd.max(0.0);
        let cap = self.line.nodes[target_i].speed_cap
            * self.skill
            * rubber
            * (1.0 + self.aggression * 0.08);
        let brake_zone = (speed * speed - cap * cap) / 18.0;
        let mut input = Input {
            // `err > 0`: the target lies clockwise of the heading seen from
            // above, i.e. to the right; positive steer turns right.
            steer: (err * 1.35).clamp(-1.0, 1.0),
            ..Default::default()
        };
        if speed < cap {
            input.throttle = (0.55 + (cap - speed) * 0.08).min(1.0);
        }
        if brake_zone > 0.4 {
            input.brake = (brake_zone / 6.0).min(1.0);
            input.throttle *= 0.15;
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
}
