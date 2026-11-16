//! In-game track editor: a free-fly camera, a terrain raycast cursor, and the
//! overlay save that writes user edits to `assets/custom_tracks/` instead of
//! the extracted retail data.
//!
//! The editor never mutates the race or the retail `.POD`/`assets/extracted`
//! tree. It reads the loaded `Track`, moves an independent camera, and (on
//! demand) serialises prop placements into a `.SIT` overlay that the normal
//! asset resolution prefers.

use std::path::{Path, PathBuf};

use macroquad::prelude::*;

use crate::terrain::{Prop, Track};
use crate::vehicle::FT;

/// Vertical field of view of the editor camera.
const EDITOR_FOV: f32 = 65.0;
/// Normal fly speed (m/s); Shift multiplies it.
const FLY_SPEED: f32 = 24.0;

pub struct EditorState {
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// Terrain point under the mouse, refreshed every frame.
    pub cursor: Option<Vec3>,
    /// Props the user has dropped this session (saved with the track).
    pub placed: Vec<Prop>,
    pub fov: f32,
}

impl EditorState {
    /// Start the free camera at the chase camera's eye, looking at its target.
    pub fn from_chase(cam_pos: Vec3, cam_look: Vec3) -> Self {
        let dir = (cam_look - cam_pos).normalize_or_zero();
        let (yaw, pitch) = look_dir_to_yaw_pitch(dir);
        Self {
            pos: cam_pos,
            yaw,
            pitch,
            cursor: None,
            placed: Vec::new(),
            fov: EDITOR_FOV,
        }
    }

    /// Free-fly: WASD along the camera axes, Space/LCtrl up/down, Shift faster,
    /// and right-button mouse-drag to pan/pitch.
    pub fn update(&mut self, dt: f32) {
        let (fwd, right, _up) = self.basis();
        let fast = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
        let speed = FLY_SPEED * if fast { 4.0 } else { 1.0 } * dt;

        if is_key_down(KeyCode::W) {
            self.pos += fwd * speed;
        }
        if is_key_down(KeyCode::S) {
            self.pos -= fwd * speed;
        }
        if is_key_down(KeyCode::A) {
            self.pos -= right * speed;
        }
        if is_key_down(KeyCode::D) {
            self.pos += right * speed;
        }
        if is_key_down(KeyCode::Space) {
            self.pos.y += speed;
        }
        if is_key_down(KeyCode::LeftControl) {
            self.pos.y -= speed;
        }

        if is_mouse_button_down(MouseButton::Right) {
            let d = mouse_delta_position();
            self.yaw -= d.x * 0.003;
            self.pitch += d.y * 0.003;
            self.pitch = self.pitch.clamp(-1.5, 1.5);
        }
    }

    /// Refresh the terrain cursor from the current mouse position.
    pub fn update_cursor(&mut self, track: &Track) {
        let mouse = mouse_position();
        self.cursor = self.raycast(mouse, track);
    }

    pub fn camera(&self) -> Camera3D {
        let (fwd, _right, _up) = self.basis();
        Camera3D {
            position: self.pos,
            target: self.pos + fwd,
            up: Vec3::Y,
            fovy: self.fov.to_radians(),
            z_near: 0.12,
            ..Default::default()
        }
    }

    /// Forward / right / up for the current yaw and pitch. `yaw = 0, pitch = 0`
    /// looks along +Z; the chase camera uses the same axes as the race.
    fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let forward = vec3(sy * cp, sp, cy * cp);
        let right = vec3(cy, 0.0, -sy);
        let up = forward.cross(right).normalize_or(Vec3::Y);
        (forward, right, up)
    }

    /// Cast a ray from the camera through `mouse` (screen pixels) and return
    /// the first heightmap intersection, snapped to the ground. Marching plus
    /// a short bisection is exact enough for placing props and needs no GPU
    /// depth read-back.
    pub fn raycast(&self, mouse: (f32, f32), track: &Track) -> Option<Vec3> {
        self.raycast_screen(mouse, (screen_width(), screen_height()), track)
    }

    /// Pure form of [`Self::raycast`] with an explicit viewport, so the maths
    /// is testable without a GPU context.
    fn raycast_screen(
        &self,
        mouse: (f32, f32),
        (sw, sh): (f32, f32),
        track: &Track,
    ) -> Option<Vec3> {
        let (fwd, right, up) = self.basis();
        if sw <= 0.0 || sh <= 0.0 {
            return None;
        }
        let ndc_x = 2.0 * mouse.0 / sw - 1.0;
        let ndc_y = 1.0 - 2.0 * mouse.1 / sh;
        let tan_h = (self.fov * 0.5).to_radians().tan();
        let aspect = sw / sh;
        let dir = (fwd + right * (ndc_x * aspect * tan_h) + up * (ndc_y * tan_h)).normalize();

        const MAX_DIST: f32 = 4000.0;
        const STEPS: usize = 256;
        let step = MAX_DIST / STEPS as f32;
        let origin = self.pos;
        let mut t = 0.0;
        let mut prev_above = origin.y >= track.height_at_world(origin.x, origin.z);
        for _ in 0..STEPS {
            t += step;
            let p = origin + dir * t;
            let above = p.y >= track.height_at_world(p.x, p.z);
            if above != prev_above {
                let (mut lo, mut hi) = (t - step, t);
                for _ in 0..8 {
                    let mid = (lo + hi) * 0.5;
                    let q = origin + dir * mid;
                    if q.y >= track.height_at_world(q.x, q.z) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let q = origin + dir * hi;
                return Some(vec3(q.x, track.height_at_world(q.x, q.z), q.z));
            }
            prev_above = above;
        }
        None
    }

    /// A wireframe ring and stem at the terrain cursor, so the user can see
    /// exactly where the brush points. Drawn inside the active 3D pass.
    pub fn draw_cursor(&self, track: &Track, color: Color) {
        let Some(c) = self.cursor else { return };
        let y = track.height_at_world(c.x, c.z) + 0.12;
        let r = 1.4;
        const SEGS: usize = 32;
        let mut prev = vec3(c.x + r, y, c.z);
        for i in 1..=SEGS {
            let a = i as f32 / SEGS as f32 * std::f32::consts::TAU;
            let p = vec3(c.x + a.cos() * r, y, c.z + a.sin() * r);
            draw_line_3d(prev, p, color);
            prev = p;
        }
        draw_line_3d(vec3(c.x, y - 0.4, c.z), vec3(c.x, y + 0.7, c.z), color);
    }
}

fn look_dir_to_yaw_pitch(d: Vec3) -> (f32, f32) {
    let pitch = d.y.clamp(-1.0, 1.0).asin();
    let yaw = d.z.atan2(d.x);
    (yaw, pitch)
}

/// Serialise prop placements into the `.SIT` `*** Boxes ***` subset the
/// terrain loader reads back (`parse_sit`). Positions are written in feet, as
/// in the retail files. The vehicles and course sections are preserved so a
/// saved overlay still starts a race.
pub fn serialize_sit(track: &Track, extra: &[Prop]) -> String {
    let mut props = track.props.clone();
    props.extend_from_slice(extra);

    let mut s = String::from("version\n1\n");

    if !track.spawns.is_empty() {
        s.push_str("*** Vehicles ***\n");
        for spawn in &track.spawns {
            let sx = -spawn.heading.sin();
            let sz = -spawn.heading.cos();
            s.push_str("Vehicle\n");
            s.push_str("staticName\n");
            s.push_str(&format!("{}\n", spawn.truck));
            s.push_str("wPos\n");
            s.push_str(&format!(
                "{:.3},{:.3},{:.3}\n",
                spawn.pos.x / FT,
                spawn.pos.y / FT,
                spawn.pos.z / FT
            ));
            s.push_str("wOrient\n");
            s.push_str(&format!("0,0,{:.6}\n", sx.atan2(sz)));
        }
    }

    s.push_str("*** Boxes ***\n");
    s.push_str("// boxCount\n");
    s.push_str(&format!("{}\n", props.len()));
    for (i, p) in props.iter().enumerate() {
        s.push_str(&format!("{} CCollide\n", i + 1));
        s.push_str("wPos\n");
        s.push_str(&format!(
            "{:.3},{:.3},{:.3}\n",
            p.pos.x / FT,
            p.pos.y / FT,
            p.pos.z / FT
        ));
        s.push_str("wOrient\n");
        s.push_str(&format!("0,0,{:.6}\n", p.yaw));
        s.push_str("model\n");
        s.push_str(&format!("{}.SMF\n", p.model));
    }

    if !track.gates.is_empty() {
        s.push_str("*** Course ***\n");
        s.push_str("c1Count,course_direction\n");
        s.push_str(&format!("{},0\n", track.gates.len()));
        for (i, g) in track.gates.iter().enumerate() {
            s.push_str(&format!(
                "********************************* {}\n",
                i * 2 + 1
            ));
            s.push_str("ctype,cspeed_type\n1,0\n");
            s.push_str("cstart\n");
            s.push_str(&format!(
                "{:.3},{:.3},{:.3}\n",
                g.start.x / FT,
                g.start.y / FT,
                g.start.z / FT
            ));
            s.push_str("cend\n");
            s.push_str(&format!(
                "{:.3},{:.3},{:.3}\n",
                g.end.x / FT,
                g.end.y / FT,
                g.end.z / FT
            ));
            s.push_str("cdec_point,cspeed,lastentry\n30.000000,0.000000,0\n");
            s.push_str("&cSpeedLimit,cTrackWidth\n");
            s.push_str(&format!("0.000000,{:.6}\n", g.width_m / FT));
        }
    }

    s
}

/// Write a `.SIT` overlay for `track` (its props plus `extra`) into
/// `assets/custom_tracks/<ID>/`. The retail tree is never touched.
pub fn save_track_overlay(track: &Track, extra: &[Prop]) -> std::io::Result<PathBuf> {
    let dir = Path::new(crate::assets::CUSTOM_TRACKS_DIR).join(track.id.to_ascii_uppercase());
    std::fs::create_dir_all(&dir)?;
    let sit = serialize_sit(track, extra);
    let path = dir.join(format!("{}.SIT", track.id.to_ascii_uppercase()));
    std::fs::write(&path, sit)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{Gate, GRID};

    fn track() -> Track {
        Track {
            id: "TEST".into(),
            dir: PathBuf::new(),
            heights: vec![0; GRID * GRID],
            colors: vec![0; GRID * GRID],
            shadows: vec![0; GRID * GRID],
            scale: 1.0,
            height_offset: 0.0,
            water_height: 0.0,
            water_color: Color::new(0.2, 0.44, 0.6, 0.72),
            fog_color: Color::new(0.56, 0.71, 0.86, 1.0),
            textures: vec![],
            detail_texture: None,
            sky_texture: None,
            light_dir: Vec3::Y,
            props: vec![Prop {
                pos: vec3(10.0, 0.0, 20.0),
                yaw: 1.5,
                model: "ROCK1".into(),
                ..Default::default()
            }],
            spawns: vec![],
            gates: vec![Gate {
                start: vec3(0.0, 0.0, 0.0),
                end: vec3(5.0, 0.0, 0.0),
                kind: 1,
                speed: 0.0,
                width_m: 8.0,
            }],
            paths: vec![],
            tints: vec![],
            ground_colors: vec![],
            atlas: None,
        }
    }

    /// The overlay `.SIT` round-trips: positions are stored in feet, so a
    /// 10 m prop comes back as ~32.8 ft in the file.
    #[test]
    fn sit_overlay_writes_feet_and_round_trips() {
        let t = track();
        let text = serialize_sit(&t, &[]);
        assert!(text.contains("*** Boxes ***"));
        assert!(text.contains("ROCK1.SMF"));
        assert!(text.contains("32.808"), "10 m prop in feet: {text}");
        // The loader must read it back at the same world position.
        let dir = std::env::temp_dir().join("evox-editor-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("TEST.SIT");
        std::fs::write(&path, &text).unwrap();
        let props = crate::terrain::parse_sit(path);
        assert_eq!(props.len(), 1);
        assert!((props[0].pos.x - 10.0).abs() < 0.05);
        assert!((props[0].pos.z - 20.0).abs() < 0.05);
    }

    /// A ray straight down from an elevated camera must land on the terrain.
    #[test]
    fn raycast_hits_the_heightmap() {
        let t = track();
        // Pitch straight down, 100 m up.
        let mut ed = EditorState {
            pos: vec3(50.0, 100.0, 50.0),
            yaw: 0.0,
            pitch: -std::f32::consts::FRAC_PI_2 + 0.01,
            cursor: None,
            placed: vec![],
            fov: EDITOR_FOV,
        };
        let _ = &mut ed;
        let hit = ed.raycast_screen((400.0, 300.0), (800.0, 600.0), &t);
        // The exact centre ray with pitch near −90° lands near (50, 50).
        let h = hit.expect("centre ray must hit");
        assert!((h.x - 50.0).abs() < 5.0 && (h.z - 50.0).abs() < 5.0);
        assert!(h.y.abs() < 0.01);
    }
}
