//! Particles, skid marks, and the replay camera director.
//!
//! Particles are pooled and terrain-aware: mud, dust, water, and exhaust have
//! distinct sizes, lifetimes, and gravity. Skid marks are a fixed ring of
//! world-space quads that fade, so long races cannot grow memory.

use std::collections::VecDeque;

use macroquad::prelude::*;

use crate::terrain::Surface;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Dust,
    Mud,
    Water,
    Smoke,
}

#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub pos: Vec3,
    pub vel: Vec3,
    pub life: f32,
    pub max_life: f32,
    pub size: f32,
    pub kind: Kind,
    pub spin: f32,
    pub tint: Color,
}

#[derive(Clone, Copy, Debug)]
pub struct Skid {
    pub pos: Vec3,
    pub normal: Vec3,
    pub forward: Vec3,
    pub age: f32,
    pub strength: f32,
    pub width: f32,
}

pub struct Vfx {
    particles: Vec<Particle>,
    skids: VecDeque<Skid>,
    pub max_particles: usize,
    pub max_skids: usize,
    rng: u32,
}

impl Default for Vfx {
    fn default() -> Self {
        Self::new()
    }
}

impl Vfx {
    pub fn new() -> Self {
        Self {
            particles: Vec::with_capacity(512),
            skids: VecDeque::with_capacity(1024),
            max_particles: 512,
            max_skids: 1024,
            rng: 0x9e37_79b9,
        }
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng & 0xffff) as f32 / 65535.0
    }

    pub fn emit(&mut self, surface: Surface, slip: f32, speed: f32, pos: Vec3, vel: Vec3, dt: f32) {
        self.emit_tinted(
            surface,
            slip,
            speed,
            pos,
            vel,
            Color::new(0.74, 0.64, 0.48, 1.0),
            dt,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn emit_tinted(
        &mut self,
        surface: Surface,
        slip: f32,
        speed: f32,
        pos: Vec3,
        vel: Vec3,
        tint: Color,
        dt: f32,
    ) {
        if self.particles.len() >= self.max_particles {
            return;
        }
        let rate = if surface.dust() {
            42.0
        } else if surface.splash() {
            45.0
        } else {
            10.0
        };
        let count = ((slip * speed * rate * dt) as usize).min(4);
        for _ in 0..count {
            if self.particles.len() >= self.max_particles {
                break;
            }
            let jitter = vec3(
                (self.rand() - 0.5) * 0.35,
                0.04 + self.rand() * 0.08,
                (self.rand() - 0.5) * 0.35,
            );
            let kind = if surface == Surface::Water {
                Kind::Water
            } else if surface == Surface::Mud {
                Kind::Mud
            } else if surface.dust() {
                Kind::Dust
            } else {
                Kind::Smoke
            };
            let (life, size) = match kind {
                Kind::Dust | Kind::Mud => (0.85, 0.28),
                Kind::Water => (0.75, 0.34),
                Kind::Smoke => (0.55, 0.08),
            };
            let r1 = self.rand() - 0.5;
            let r2 = self.rand();
            let r3 = self.rand() - 0.5;
            let sz = size * (0.8 + self.rand() * 0.6);
            let spin = self.rand() * std::f32::consts::TAU;
            let spray_vel = match kind {
                Kind::Water => vel * 0.30 + vec3(r1 * 2.2, 1.4 + r2 * 1.6, r3 * 2.2),
                Kind::Dust | Kind::Mud => vel * 0.15 + vec3(r1 * 1.25, 0.28 + r2 * 0.55, r3 * 1.25),
                Kind::Smoke => vel * 0.20 + vec3(r1 * 0.8, 0.3 + r2 * 0.4, r3 * 0.8),
            };
            self.particles.push(Particle {
                pos: pos + jitter,
                vel: spray_vel,
                life,
                max_life: life,
                size: sz,
                kind,
                spin,
                tint,
            });
        }
    }

    pub fn exhaust(&mut self, pos: Vec3, rpm: f32, throttle: f32) {
        if self.particles.len() >= self.max_particles || throttle < 0.15 {
            return;
        }
        if self.rand() > (rpm / 12000.0).clamp(0.02, 0.22) {
            return;
        }
        let r1 = self.rand() - 0.5;
        let r2 = self.rand();
        let r3 = self.rand() - 0.5;
        self.particles.push(Particle {
            pos,
            vel: vec3(r1 * 0.6, 0.25 + r2 * 0.35, r3 * 0.6),
            life: 0.45,
            max_life: 0.45,
            size: 0.08,
            kind: Kind::Smoke,
            spin: 0.0,
            tint: Color::new(0.35, 0.35, 0.36, 1.0),
        });
    }

    pub fn skid(&mut self, pos: Vec3, normal: Vec3, strength: f32, width: f32) {
        self.skid_oriented(pos, normal, Vec3::NEG_Z, strength, width);
    }

    pub fn skid_oriented(
        &mut self,
        pos: Vec3,
        normal: Vec3,
        forward: Vec3,
        strength: f32,
        width: f32,
    ) {
        if strength < 0.08 {
            return;
        }
        if self
            .skids
            .iter()
            .rev()
            .take(8)
            .any(|s| (s.pos - pos).length_squared() < 0.18 * 0.18)
        {
            return;
        }
        if self.skids.len() >= self.max_skids {
            self.skids.pop_front();
        }
        self.skids.push_back(Skid {
            pos,
            normal,
            forward,
            age: 0.0,
            strength,
            width,
        });
    }

    pub fn update(&mut self, dt: f32) {
        for p in &mut self.particles {
            p.life -= dt;
            p.vel.y -= 4.5 * dt;
            p.vel *= 1.0 - 1.8 * dt;
            p.pos += p.vel * dt;
            p.spin += dt;
        }
        self.particles.retain(|p| p.life > 0.0);
        for s in &mut self.skids {
            s.age += dt;
        }
        while self.skids.front().map(|s| s.age > 26.0) == Some(true) {
            self.skids.pop_front();
        }
    }

    /// Build a batched 3D world-space quad mesh for all active tire tread marks,
    /// projected onto `track`'s terrain tangent plane and lifted 3 cm above the
    /// rendered terrain triangles.
    pub fn build_skids_mesh(&self, track: &crate::terrain::Track) -> Mesh {
        let mut vertices = Vec::with_capacity(self.skids.len() * 4);
        let mut indices = Vec::with_capacity(self.skids.len() * 6);
        for s in &self.skids {
            let n = track.normal_at_world(s.pos.x, s.pos.z);
            let mut fwd = (s.forward - n * s.forward.dot(n)).normalize_or_zero();
            if fwd.length_squared() < 1e-6 {
                fwd = n.cross(Vec3::X).normalize_or(Vec3::NEG_Z);
            }
            let right = fwd.cross(n).normalize_or(Vec3::X);
            let hw = s.width.max(0.24) * 0.5;
            let hl = 0.28;
            let corners = [
                s.pos - right * hw - fwd * hl,
                s.pos + right * hw - fwd * hl,
                s.pos + right * hw + fwd * hl,
                s.pos - right * hw + fwd * hl,
            ];
            let alpha = (((1.0 - s.age / 26.0).clamp(0.0, 1.0)
                * (0.35 + 0.45 * s.strength).clamp(0.0, 0.72))
                * 255.0)
                .clamp(0.0, 255.0) as u8;
            if alpha < 3 {
                continue;
            }
            let base = vertices.len() as u16;
            for c in corners {
                // The rendered triangle surface, not the bilinear field: the
                // quad corners are up to 0.28 m away from the contact point and
                // must not sink into or hover over the drawn ground.
                let ground_y = track.triangle_height_at(c.x, c.z);
                vertices.push(Vertex {
                    position: vec3(c.x, ground_y + 0.03, c.z),
                    uv: Vec2::ZERO,
                    color: [38, 32, 24, alpha],
                    normal: Vec4::new(n.x, n.y, n.z, 0.0),
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        Mesh {
            vertices,
            indices,
            texture: None,
        }
    }

    /// Render tire marks in the active 3D world camera pass.
    pub fn draw_skids_3d(&self, track: &crate::terrain::Track) {
        if self.skids.is_empty() {
            return;
        }
        let mesh = self.build_skids_mesh(track);
        if !mesh.indices.is_empty() {
            draw_mesh(&mesh);
        }
    }

    /// Build a batched 3D world-space camera-facing billboard mesh for all
    /// active dust, mud, water, and exhaust puffs, clamped close to the ground.
    pub fn build_particles_mesh(&self, cam: &Camera3D, track: &crate::terrain::Track) -> Mesh {
        let cam_fwd = (cam.target - cam.position).normalize_or(Vec3::NEG_Z);
        let cam_right = cam_fwd.cross(Vec3::Y).normalize_or(Vec3::X);
        let cam_up = cam_right.cross(cam_fwd).normalize_or(Vec3::Y);
        let mut order: Vec<(usize, f32)> = self
            .particles
            .iter()
            .enumerate()
            .map(|(i, p)| (i, (p.pos - cam.position).length_squared()))
            .collect();
        order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut vertices = Vec::with_capacity(self.particles.len() * 7);
        let mut indices = Vec::with_capacity(self.particles.len() * 18);

        for (idx, _) in order {
            let p = &self.particles[idx];
            let t = (p.life / p.max_life).clamp(0.0, 1.0);
            let fade = (((1.0 - t) * 5.0 + 0.30).min(t * 1.4)).clamp(0.0, 1.0);
            let r = (p.size * (1.75 - 0.75 * t)).clamp(0.10, 0.75);
            let terrain_y = track.triangle_height_at(p.pos.x, p.pos.z);
            let center = vec3(
                p.pos.x,
                p.pos.y.clamp(terrain_y + r * 0.45 + 0.03, terrain_y + 1.35),
                p.pos.z,
            );
            let color = match p.kind {
                Kind::Dust => Color::new(p.tint.r, p.tint.g, p.tint.b, fade * 0.46),
                Kind::Mud => Color::new(
                    p.tint.r * 0.45,
                    p.tint.g * 0.38,
                    p.tint.b * 0.28,
                    fade * 0.68,
                ),
                Kind::Water => Color::new(0.90, 0.96, 1.0, fade * 0.72),
                Kind::Smoke => Color::new(0.38, 0.38, 0.40, fade * 0.26),
            };
            let cr = (color.r * 255.0).clamp(0.0, 255.0) as u8;
            let cg = (color.g * 255.0).clamp(0.0, 255.0) as u8;
            let cb = (color.b * 255.0).clamp(0.0, 255.0) as u8;
            let ca = (color.a * 255.0).clamp(0.0, 255.0) as u8;
            let edge_a = ((color.a * 0.25) * 255.0).clamp(0.0, 255.0) as u8;
            if ca < 3 {
                continue;
            }
            let base = vertices.len() as u16;
            let n = -cam_fwd;
            let n4 = Vec4::new(n.x, n.y, n.z, 0.0);
            vertices.push(Vertex {
                position: center,
                uv: Vec2::ZERO,
                color: [cr, cg, cb, ca],
                normal: n4,
            });
            for k in 0..6u16 {
                let ang = p.spin + k as f32 * (std::f32::consts::TAU / 6.0);
                let (s, c) = ang.sin_cos();
                let pos = center + (cam_right * c + cam_up * s) * r;
                vertices.push(Vertex {
                    position: pos,
                    uv: Vec2::ZERO,
                    color: [cr, cg, cb, edge_a],
                    normal: n4,
                });
                let next = 1 + ((k + 1) % 6);
                indices.extend_from_slice(&[base, base + 1 + k, base + next]);
            }
        }
        Mesh {
            vertices,
            indices,
            texture: None,
        }
    }

    /// Render 3D dust/spray/smoke billboards in the active 3D camera pass.
    pub fn draw_particles_3d(&self, cam: &Camera3D, track: &crate::terrain::Track) {
        if self.particles.is_empty() {
            return;
        }
        let mesh = self.build_particles_mesh(cam, track);
        if !mesh.indices.is_empty() {
            draw_mesh(&mesh);
        }
    }

    pub fn clear(&mut self) {
        self.particles.clear();
        self.skids.clear();
    }

    pub fn counts(&self) -> (usize, usize) {
        (self.particles.len(), self.skids.len())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayShot {
    Chase,
    Hood,
    Orbit,
    Trackside,
    Finish,
}

/// Cycles angles after the flag, mimicking the original completion replay.
pub struct ReplayDirector {
    pub shot: ReplayShot,
    pub timer: f32,
    pub orbit: f32,
}

impl Default for ReplayDirector {
    fn default() -> Self {
        Self {
            shot: ReplayShot::Chase,
            timer: 0.0,
            orbit: 0.0,
        }
    }
}

impl ReplayDirector {
    pub fn tick(&mut self, dt: f32, finished: bool) {
        self.timer += dt;
        self.orbit += dt * 0.35;
        if finished && self.timer > 3.2 {
            self.timer = 0.0;
            self.shot = match self.shot {
                ReplayShot::Chase => ReplayShot::Orbit,
                ReplayShot::Orbit => ReplayShot::Trackside,
                ReplayShot::Trackside => ReplayShot::Hood,
                ReplayShot::Hood => ReplayShot::Finish,
                ReplayShot::Finish => ReplayShot::Chase,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flat, empty track used as a base for the terrain-hugging tests.
    fn slope_track() -> crate::terrain::Track {
        use crate::terrain::{Track, GRID};
        Track {
            id: "test".into(),
            dir: std::path::PathBuf::new(),
            heights: vec![1600; GRID * GRID],
            colors: vec![0; GRID * GRID],
            shadows: vec![0; GRID * GRID],
            scale: 9.7536,
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
            ground_colors: vec![Color::new(0.82, 0.72, 0.54, 1.0)],
            atlas: None,
        }
    }

    #[test]
    fn emission_is_capped_and_expires() {
        let mut v = Vfx::new();
        v.max_particles = 20;
        for _ in 0..100 {
            v.emit(Surface::Mud, 1.0, 20.0, Vec3::ZERO, Vec3::ZERO, 0.1);
        }
        assert!(v.counts().0 <= 20);
        v.update(3.0);
        assert_eq!(v.counts().0, 0);
    }

    #[test]
    fn skids_are_a_ring() {
        let mut v = Vfx::new();
        v.max_skids = 5;
        for i in 0..12 {
            v.skid(vec3(i as f32, 0.0, 0.0), Vec3::Y, 1.0, 0.2);
        }
        assert_eq!(v.counts().1, 5);
        let last = v.skids.back().unwrap();
        assert_eq!(last.pos.x, 11.0);
    }

    #[test]
    fn replay_cycles_shots() {
        let mut r = ReplayDirector::default();
        for _ in 0..400 {
            r.tick(1.0 / 60.0, true);
        }
        assert_ne!(r.shot, ReplayShot::Chase);
    }

    /// Sloped ground (not the flat case above): every corner of every tread
    /// quad must sit on the *rendered* triangle surface plus 3 cm, so marks can
    /// never hover in the air on a hillside.
    #[test]
    fn skids_follow_a_slope_exactly() {
        use crate::terrain::{Track, GRID};
        let mut track = Track {
            id: "slope".into(),
            dir: std::path::PathBuf::new(),
            // 1/32 ft per unit → one grid step rises ~2.9 m over 9.75 m.
            heights: (0..GRID * GRID).map(|i| (i % GRID) as u16 * 30).collect(),
            colors: vec![0; GRID * GRID],
            shadows: vec![0; GRID * GRID],
            scale: 9.7536,
            ..slope_track()
        };
        track.height_offset = 0.0;
        let (x, z) = (120.0f32, 140.0f32);
        let contact_y = track.height_at_world(x, z) + 0.03;
        let mut v = Vfx::new();
        v.skid_oriented(
            vec3(x, contact_y, z),
            track.normal_at_world(x, z),
            vec3(0.0, 0.0, -1.0),
            0.6,
            0.30,
        );
        let mesh = v.build_skids_mesh(&track);
        assert_eq!(mesh.vertices.len(), 4);
        for vert in &mesh.vertices {
            let want = track.triangle_height_at(vert.position.x, vert.position.z) + 0.03;
            assert!(
                (vert.position.y - want).abs() < 1e-3,
                "corner {} off the drawn ground (want {want})",
                vert.position.y
            );
        }
    }

    #[test]
    fn skids_and_dust_meshes_hug_terrain_not_sky() {
        let track = slope_track();
        let ground_y = track.height_at_world(120.0, 120.0);
        let mut v = Vfx::new();
        v.skid_oriented(
            vec3(120.0, ground_y + 0.03, 120.0),
            track.normal_at_world(120.0, 120.0),
            vec3(0.0, 0.0, -1.0),
            0.5,
            0.30,
        );
        // Simulate even a mistakenly high particle Y to verify clamping keeps it near ground.
        v.emit_tinted(
            Surface::Sand,
            0.8,
            25.0,
            vec3(120.0, ground_y + 50.0, 120.0),
            vec3(0.0, 0.0, -20.0),
            track.ground_color_at(120.0, 120.0),
            0.1,
        );
        v.update(0.1);
        let sm = v.build_skids_mesh(&track);
        assert_eq!(sm.vertices.len(), 4);
        for vert in &sm.vertices {
            assert!((vert.position.y - (ground_y + 0.03)).abs() < 0.05);
        }
        let cam = Camera3D {
            position: vec3(120.0, ground_y + 3.0, 128.0),
            target: vec3(120.0, ground_y + 1.0, 120.0),
            up: Vec3::Y,
            ..Default::default()
        };
        let pm = v.build_particles_mesh(&cam, &track);
        assert!(!pm.vertices.is_empty());
        for vert in &pm.vertices {
            assert!(
                vert.position.y >= ground_y - 0.8 && vert.position.y <= ground_y + 2.5,
                "particle vertex y={} away from ground_y={ground_y}",
                vert.position.y
            );
        }
    }
}
