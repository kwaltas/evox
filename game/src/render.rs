//! Vehicle and world model rendering from the retail `.SMF` library.
//!
//! `.SMF` is Terminal Reality's text mesh: one palette texture per file, feet
//! in, `C3DModel` descriptor first. `ModelCache` parses each file once,
//! flattens every sub-mesh into a single Macroquad `Mesh` with the bound
//! albedo, and keeps it until the process ends. Bodies and wheels are separate
//! cached meshes, so tire upgrades swap geometry without rebuilding the truck.
//!
//! Macroquad has no per-draw model matrix, so vertices are transformed into
//! world space each frame from a cloned mesh.

use std::collections::HashMap;
use std::path::Path;

use macroquad::material::{
    gl_use_default_material, gl_use_material, load_material, Material, MaterialParams,
};
use macroquad::prelude::*;
use miniquad::graphics::ShaderSource;

use crate::assets::Assets;
use crate::physics::Vehicle;
use crate::smf::Smf;
use crate::vehicle::FT;

pub const MODEL_DIR: &str = "assets/extracted/model/vehicle/MODELS";

/// Body used when a mod truck names a mesh that is not installed. It comes
/// from the retail library, so there is no primitive stand-in.
const SPARE_BODY: &str = "BLAZZR2.SMF";

/// Nominal radius of the `33X12` wheel meshes, in metres.
const WHEEL_RADIUS_M: f32 = 1.375 * FT;

struct PreparedPart {
    mesh: Mesh,
    /// Sub-mesh header carried `TRANSP`/`TRANSPI`/`TRANSPE`: draw with the
    /// alpha-cutout material instead of the opaque one.
    transparent: bool,
}

struct Prepared {
    mesh: Mesh,
    parts: Vec<PreparedPart>,
    lo: Vec3,
    hi: Vec3,
}

impl Prepared {
    /// Parse `file` from the model directories (Evo 1, then imported Evo 2)
    /// and bind its texture, recoloured with `paint` when given.
    fn load(
        assets: &mut Assets,
        file: &str,
        paint: Option<&crate::paint::Paint>,
        is_prop: bool,
    ) -> Option<Self> {
        let path = crate::vehicle::find_model(Path::new("."), file)
            .unwrap_or_else(|| Path::new(MODEL_DIR).join(file));
        let bytes = std::fs::read(&path).ok()?;
        // Retail files are ASCII, but mods may carry stray high bytes.
        let text = String::from_utf8_lossy(&bytes);
        let smf = Smf::parse(&text).ok()?;
        let primary_texture = smf
            .meshes
            .iter()
            .find(|m| !m.texture.is_empty())
            .and_then(|m| {
                if is_prop {
                    assets
                        .prop_texture(&m.texture)
                        .or_else(|| assets.model_texture(&m.texture))
                } else {
                    match paint {
                        Some(p) => assets.painted_texture(&m.texture, p),
                        None => assets.model_texture(&m.texture),
                    }
                }
            });
        let glass = (!is_prop).then(|| assets.glass_pixels(&smf));
        let mesh = smf.merged_mesh(primary_texture.clone(), glass.as_deref());
        let (lo, hi) = smf.bounds();

        // Build individual parts for scenery props:
        // Keeps all sub-meshes (including transparent ones like tree foliage, banners, fences)
        // and resolves each part's texture case-insensitively. Alpha comes from
        // the art's `.OPA`; only a `TRANSP*` part may also key index 0 out.
        let mut parts = Vec::new();
        for m in smf
            .meshes
            .iter()
            .filter(|m| m.lod_min <= 0.1 && !m.positions.is_empty() && !m.indices.is_empty())
        {
            let transparent = !m.opaque;
            let tex = if m.texture.is_empty() {
                primary_texture.clone()
            } else if is_prop {
                let t = if transparent {
                    assets.prop_texture_cutout(&m.texture)
                } else {
                    assets.prop_texture(&m.texture)
                };
                t.or_else(|| assets.model_texture(&m.texture))
            } else {
                assets.model_texture(&m.texture)
            };
            let mut vertices = Vec::with_capacity(m.positions.len());
            for (i, p) in m.positions.iter().enumerate() {
                let n = m.normals.get(i).copied().unwrap_or(Vec3::Y);
                vertices.push(Vertex {
                    position: *p,
                    uv: m.uvs.get(i).copied().unwrap_or_default(),
                    color: [255, 255, 255, 255],
                    normal: Vec4::new(n.x, n.y, n.z, 0.0),
                });
            }
            parts.push(PreparedPart {
                mesh: Mesh {
                    vertices,
                    indices: m.indices.clone(),
                    texture: tex,
                },
                transparent,
            });
        }
        if parts.is_empty() {
            parts.push(PreparedPart {
                mesh: clone_mesh(&mesh),
                transparent: false,
            });
        }

        Some(Self {
            mesh,
            parts,
            lo,
            hi,
        })
    }

    fn length(&self) -> f32 {
        (self.hi - self.lo).max_element()
    }
}

/// One parsed mesh per file name, with the failures remembered so a broken
/// reference is not retried every frame.
#[derive(Default)]
pub struct ModelCache {
    models: HashMap<String, Option<Prepared>>,
}

impl ModelCache {
    fn get(&mut self, assets: &mut Assets, key: &str, file: &str) -> Option<&Prepared> {
        self.get_entry(assets, key, file, None, false)
    }

    fn get_prop(&mut self, assets: &mut Assets, key: &str, file: &str) -> Option<&Prepared> {
        self.get_entry(assets, key, file, None, true)
    }

    fn get_painted(
        &mut self,
        assets: &mut Assets,
        key: &str,
        file: &str,
        paint: Option<&crate::paint::Paint>,
    ) -> Option<&Prepared> {
        self.get_entry(assets, key, file, paint, false)
    }

    fn get_entry(
        &mut self,
        assets: &mut Assets,
        key: &str,
        file: &str,
        paint: Option<&crate::paint::Paint>,
        is_prop: bool,
    ) -> Option<&Prepared> {
        if !self.models.contains_key(key) {
            let prepared = Prepared::load(assets, file, paint, is_prop);
            self.models.insert(key.to_string(), prepared);
        }
        self.models.get(key).and_then(|m| m.as_ref())
    }

    pub fn len(&self) -> usize {
        self.models.values().filter(|m| m.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

/// Reusable vertex/index/order scratch so drawing a model never allocates a
/// fresh `Mesh` per part per frame (audit finding #5).
#[derive(Default)]
struct Scratch {
    vertices: Vec<Vertex>,
    indices: Vec<u16>,
    order: Vec<([u16; 3], f32)>,
}

/// A world material plus the remembered failure state, so a shader that the
/// back end refuses is not recompiled on every draw.
#[derive(Default)]
struct MaterialSlot(Option<Option<Material>>);

impl MaterialSlot {
    fn get(&mut self) -> Option<&Material> {
        self.0.as_ref().and_then(|m| m.as_ref())
    }
}

#[derive(Default)]
pub struct Renderer {
    cache: ModelCache,
    /// Files that failed to load, surfaced in the self-test and the debug HUD.
    pub missing: Vec<String>,
    spare: Option<String>,
    pub cam_pos: Vec3,
    pub fog_color: Color,
    pub fog_start: f32,
    pub fog_end: f32,
    scratch: Scratch,
    opaque_material: MaterialSlot,
    cutout_material: MaterialSlot,
    sky_material: MaterialSlot,
    water_material: MaterialSlot,
}

impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of meshes currently resident.
    pub fn loaded(&self) -> usize {
        self.cache.len()
    }

    /// Cache key of a truck body, loading it (with `paint`) on first use.
    fn body_key(
        &mut self,
        assets: &mut Assets,
        base: &str,
        paint: Option<&crate::paint::Paint>,
    ) -> Option<String> {
        let base = base.trim().to_ascii_uppercase();
        if !base.is_empty() {
            let key = match paint {
                Some(p) => format!("BODY#{base}#{}", p.key()),
                None => format!("BODY#{base}"),
            };
            let file = format!("{base}.SMF");
            if self.cache.get_painted(assets, &key, &file, paint).is_some() {
                return Some(key);
            }
            if !self.missing.contains(&file) {
                self.missing.push(file);
            }
        }
        // Mod truck without the mesh installed: borrow an intact retail body
        // for this frame instead of materialising a placeholder.
        if self.spare.is_none() {
            let key = format!("BODY#{SPARE_BODY}");
            let ok = self.cache.get(assets, &key, SPARE_BODY).is_some();
            self.spare = ok.then(|| key);
        }
        self.spare.clone()
    }

    /// Bind the world fog / alpha-cutout shader using `self.cam_pos`, `self.fog_color`,
    /// `self.fog_start`, and `self.fog_end` (when `fog_end > fog_start`).
    ///
    /// Uses separate `Material` instances for opaque/painted meshes (`cutout = false`,
    /// where `tex.a` carries the vehicle `.OPA` paint mask rather than transparency)
    /// and alpha-cutout scenery props (`cutout = true`).
    pub fn bind_world_material(&mut self, cutout: bool) -> bool {
        let cam = self.cam_pos;
        let fog = self.fog_color;
        let (fs, fe) = (self.fog_start, self.fog_end);
        let slot = if cutout {
            &mut self.cutout_material
        } else {
            &mut self.opaque_material
        };
        let Some(m) = get_or_create_world_material(slot, true, miniquad::Comparison::LessOrEqual)
        else {
            return false;
        };
        m.set_uniform("FogColor", Vec4::new(fog.r, fog.g, fog.b, fog.a));
        m.set_uniform("FogCam", Vec4::new(cam.x, cam.y, cam.z, 1.0));
        m.set_uniform(
            "FogParams",
            Vec4::new(fs, fe, if cutout { 1.0 } else { 0.0 }, 0.0),
        );
        gl_use_material(m);
        true
    }

    pub fn unbind_world_material(&self) {
        gl_use_default_material();
    }

    /// Bind the depth-tested translucent material with `alpha_blend = (Zero, One)`
    /// so translucent 3D geometry (blob shadows, water, waypoint beams) preserves
    /// an opaque framebuffer alpha channel.
    pub fn bind_translucent_material(&mut self) -> bool {
        let cam = self.cam_pos;
        let fc = self.fog_color;
        let Some(m) = get_or_create_world_material(
            &mut self.water_material,
            true,
            miniquad::Comparison::LessOrEqual,
        ) else {
            return false;
        };
        m.set_uniform("FogColor", Vec4::new(fc.r, fc.g, fc.b, 1.0));
        m.set_uniform("FogCam", Vec4::new(cam.x, cam.y, cam.z, 1.0));
        m.set_uniform("FogParams", Vec4::new(1e5, 0.0, 0.0, 0.0));
        gl_use_material(m);
        true
    }

    /// Project a soft radial elliptical drop shadow onto the terrain triangles
    /// beneath `v` so vehicles read as firmly planted on hills and after jumps.
    pub fn draw_blob_shadow(&mut self, track: &crate::terrain::Track, v: &Vehicle) {
        let hull = v.hull();
        let hw = (hull.x * 0.56).max(0.92);
        let hl = (hull.z * 0.54).max(1.90);
        let f_raw = v.forward();
        let fwd = vec3(f_raw.x, 0.0, f_raw.z).normalize_or(Vec3::NEG_Z);
        let r_raw = v.rot * Vec3::X;
        let right = vec3(r_raw.x, 0.0, r_raw.z).normalize_or(Vec3::X);
        let g_center = track.triangle_height_at(v.pos.x, v.pos.z);
        let clearance = (v.pos.y - g_center - v.tyre_radius()).max(0.0);
        let air_atten = (1.0 - clearance * 0.22).clamp(0.15, 1.0);
        let center_alpha = (0.58 * air_atten * 255.0).clamp(0.0, 255.0) as u8;
        let inner_alpha = (0.36 * air_atten * 255.0).clamp(0.0, 255.0) as u8;
        if center_alpha < 4 {
            return;
        }

        const SEGS: usize = 16;
        let mut vertices = Vec::with_capacity(1 + SEGS * 2);
        vertices.push(Vertex {
            position: vec3(v.pos.x, g_center + 0.05, v.pos.z),
            uv: Vec2::ZERO,
            color: [0, 0, 0, center_alpha],
            normal: Vec4::new(0.0, 1.0, 0.0, 0.0),
        });
        for (ring_scale, ring_alpha) in [(0.62f32, inner_alpha), (1.00f32, 0u8)] {
            for i in 0..SEGS {
                let ang = (i as f32 / SEGS as f32) * std::f32::consts::TAU;
                let (s, c) = ang.sin_cos();
                let wp = v.pos + right * (hw * ring_scale * c) + fwd * (hl * ring_scale * s);
                let y = track.triangle_height_at(wp.x, wp.z) + 0.05;
                vertices.push(Vertex {
                    position: vec3(wp.x, y, wp.z),
                    uv: Vec2::ZERO,
                    color: [0, 0, 0, ring_alpha],
                    normal: Vec4::new(0.0, 1.0, 0.0, 0.0),
                });
            }
        }

        let mut indices = Vec::with_capacity(SEGS * 9);
        for i in 0..SEGS {
            let i0 = 1 + i as u16;
            let i1 = 1 + ((i + 1) % SEGS) as u16;
            let o0 = (1 + SEGS + i) as u16;
            let o1 = (1 + SEGS + (i + 1) % SEGS) as u16;
            indices.extend_from_slice(&[0, i0, i1]);
            indices.extend_from_slice(&[i0, o0, i1, i1, o0, o1]);
        }

        let used = self.bind_translucent_material();
        let mesh = Mesh {
            vertices,
            indices,
            texture: None,
        };
        draw_mesh(&mesh);
        if used {
            gl_use_default_material();
        }
    }

    /// Draw a 360-degree sky cylinder/dome centered around `cam_pos` using the
    /// `.LVL` sky texture wrapped across the horizon without modifying depth.
    pub fn draw_sky_dome(&mut self, cam_pos: Vec3, sky_tex: Option<&Texture2D>, fog_color: Color) {
        const SEGS: usize = 24;
        let radius = 320.0;
        let y_bot = cam_pos.y - 110.0;
        let y_hor = cam_pos.y + 8.0;
        let y_top = cam_pos.y + 240.0;
        let fog_rgba = [
            (fog_color.r * 255.0) as u8,
            (fog_color.g * 255.0) as u8,
            (fog_color.b * 255.0) as u8,
            255,
        ];

        let mut band_verts = Vec::with_capacity((SEGS + 1) * 3);
        let mut band_idx = Vec::with_capacity(SEGS * 12);
        for i in 0..=SEGS {
            let frac = i as f32 / SEGS as f32;
            let ang = frac * std::f32::consts::TAU;
            let (sx, sz) = (
                cam_pos.x + ang.cos() * radius,
                cam_pos.z + ang.sin() * radius,
            );
            let u = frac * 4.0;
            band_verts.push(Vertex {
                position: vec3(sx, y_bot, sz),
                uv: vec2(u, 0.96),
                color: fog_rgba,
                normal: Vec4::new(0.0, 1.0, 0.0, 0.0),
            });
            band_verts.push(Vertex {
                position: vec3(sx, y_hor, sz),
                uv: vec2(u, 0.94),
                color: [255, 255, 255, 255],
                normal: Vec4::new(0.0, 1.0, 0.0, 0.0),
            });
            band_verts.push(Vertex {
                position: vec3(sx, y_top, sz),
                uv: vec2(u, 0.04),
                color: [255, 255, 255, 255],
                normal: Vec4::new(0.0, 1.0, 0.0, 0.0),
            });
            if i < SEGS {
                let a = (i * 3) as u16;
                let b = ((i + 1) * 3) as u16;
                // Lower horizon skirt blended with fog_color + upper textured sky wall.
                band_idx.extend_from_slice(&[a, b, a + 1, b, b + 1, a + 1]);
                band_idx.extend_from_slice(&[a + 1, b + 1, a + 2, b + 1, b + 2, a + 2]);
            }
        }
        // Zenith cap disc matching top row UV.
        let zenith_idx = band_verts.len() as u16;
        band_verts.push(Vertex {
            position: vec3(cam_pos.x, y_top + 40.0, cam_pos.z),
            uv: vec2(0.5, 0.02),
            color: [255, 255, 255, 255],
            normal: Vec4::new(0.0, -1.0, 0.0, 0.0),
        });
        for i in 0..SEGS {
            let t0 = (i * 3 + 2) as u16;
            let t1 = ((i + 1) * 3 + 2) as u16;
            band_idx.extend_from_slice(&[t0, t1, zenith_idx]);
        }

        if let Some(m) = get_or_create_world_material(
            &mut self.sky_material,
            false,
            miniquad::Comparison::Always,
        ) {
            m.set_uniform(
                "FogColor",
                Vec4::new(fog_color.r, fog_color.g, fog_color.b, 1.0),
            );
            m.set_uniform("FogCam", Vec4::new(cam_pos.x, cam_pos.y, cam_pos.z, 1.0));
            m.set_uniform("FogParams", Vec4::new(1e5, 0.0, 0.0, 0.0));
            gl_use_material(m);
        }
        let mesh = Mesh {
            vertices: band_verts,
            indices: band_idx,
            texture: sky_tex.cloned(),
        };
        draw_mesh(&mesh);
        gl_use_default_material();
    }

    /// Render the translucent rippling water surface over flooded valleys of
    /// `track`, dissolving into `track.fog_color` at distance.
    pub fn draw_water(&mut self, track: &crate::terrain::Track, cam_pos: Vec3, time: f32) {
        if track.water_height <= 0.5 {
            return;
        }
        const HALF_CELLS: i32 = 22;
        let step = track.scale.max(1.0);
        let cx = (cam_pos.x / step).floor() * step;
        let cz = (cam_pos.z / step).floor() * step;
        let wc = track.water_color;
        let fc = track.fog_color;
        let cols = (HALF_CELLS * 2 + 1) as usize;
        let mut cell_h = vec![0.0f32; cols * cols];
        let mut verts: Vec<Vertex> = Vec::with_capacity(cols * cols);
        let mut indices: Vec<u16> = Vec::with_capacity((cols - 1) * (cols - 1) * 6);

        for gz in 0..cols {
            for gx in 0..cols {
                let x = cx + (gx as i32 - HALF_CELLS) as f32 * step;
                let z = cz + (gz as i32 - HALF_CELLS) as f32 * step;
                cell_h[gz * cols + gx] = track.height_at_world(x, z);
                let wave = (x * 0.14 + time * 1.6).sin() * (z * 0.14 + time * 1.1).cos() * 0.05;
                let shimmer = ((x * 0.09 + z * 0.11 + time * 2.1).sin() * 0.05).max(-0.04);
                let dist = vec2(x - cam_pos.x, z - cam_pos.z).length();
                let fog_f = if self.fog_end > self.fog_start {
                    ((dist - self.fog_start) / (self.fog_end - self.fog_start)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let r = ((wc.r + shimmer).clamp(0.0, 1.0)) * (1.0 - fog_f) + fc.r * fog_f;
                let g = ((wc.g + shimmer).clamp(0.0, 1.0)) * (1.0 - fog_f) + fc.g * fog_f;
                let b = ((wc.b + shimmer * 1.2).clamp(0.0, 1.0)) * (1.0 - fog_f) + fc.b * fog_f;
                let a = wc.a * (1.0 - fog_f * 0.35);
                verts.push(Vertex {
                    position: vec3(x, track.water_height + 0.05 + wave, z),
                    uv: Vec2::ZERO,
                    color: [
                        (r * 255.0) as u8,
                        (g * 255.0) as u8,
                        (b * 255.0) as u8,
                        (a * 255.0) as u8,
                    ],
                    normal: Vec4::new(0.0, 1.0, 0.0, 0.0),
                });
            }
        }
        let cutoff = track.water_height + 0.25;
        for gz in 0..(cols - 1) {
            for gx in 0..(cols - 1) {
                let i0 = gz * cols + gx;
                let i1 = i0 + 1;
                let i2 = (gz + 1) * cols + gx;
                let i3 = i2 + 1;
                if cell_h[i0] > cutoff
                    && cell_h[i1] > cutoff
                    && cell_h[i2] > cutoff
                    && cell_h[i3] > cutoff
                {
                    continue;
                }
                indices.extend_from_slice(&[
                    i0 as u16, i2 as u16, i1 as u16, i1 as u16, i2 as u16, i3 as u16,
                ]);
            }
        }
        if indices.is_empty() {
            return;
        }
        if let Some(m) = get_or_create_world_material(
            &mut self.water_material,
            true,
            miniquad::Comparison::LessOrEqual,
        ) {
            m.set_uniform("FogColor", Vec4::new(fc.r, fc.g, fc.b, 1.0));
            m.set_uniform("FogCam", Vec4::new(cam_pos.x, cam_pos.y, cam_pos.z, 1.0));
            m.set_uniform("FogParams", Vec4::new(1e5, 0.0, 0.0, 0.0));
            gl_use_material(m);
        }
        let mesh = Mesh {
            vertices: verts,
            indices,
            texture: None,
        };
        draw_mesh(&mesh);
        gl_use_default_material();
    }

    /// Draw a physics vehicle (body + four wheels) using the retail assets,
    /// depth-sorted back-to-front relative to `self.cam_pos`.
    pub fn draw_vehicle(&mut self, assets: &mut Assets, v: &Vehicle) {
        let cam = self.cam_pos;
        let base = v.truck.model_base.clone();
        let basis = (v.rot * Quat::from_rotation_y(std::f32::consts::PI)).normalize();
        let scale = (v.truck.tyre_radius_m() / WHEEL_RADIUS_M).clamp(0.7, 1.6);
        let body_dist = (v.pos - cam).length_squared();

        let mut wheel_order: Vec<(usize, f32)> = (0..4)
            .map(|i| (i, (v.wheel_world(i) - cam).length_squared()))
            .collect();
        wheel_order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let use_fog = self.fog_end > self.fog_start && self.bind_world_material(false);

        // 1. Far wheels (behind the chassis relative to camera).
        for &(i, d) in &wheel_order {
            if d < body_dist {
                continue;
            }
            self.draw_one_wheel(assets, v, i, basis, scale, cam);
        }
        // 2. Chassis body (triangles painter-sorted back-to-front).
        if let Some(key) = self.body_key(assets, &base, v.truck.paint()) {
            if let Some(model) = model_mesh(&self.cache, &key) {
                draw_scratch(
                    &mut self.scratch,
                    &model.mesh,
                    v.pos,
                    basis,
                    1.0,
                    WHITE,
                    cam,
                );
            }
        }
        // 3. Near wheels (in front of the chassis relative to camera).
        for &(i, d) in &wheel_order {
            if d >= body_dist {
                continue;
            }
            self.draw_one_wheel(assets, v, i, basis, scale, cam);
        }
        if use_fog {
            self.unbind_world_material();
        }
    }

    fn draw_one_wheel(
        &mut self,
        assets: &mut Assets,
        v: &Vehicle,
        i: usize,
        basis: Quat,
        scale: f32,
        cam: Vec3,
    ) {
        let Some((key, own)) = self.wheel_key(assets, &v.truck, i % 2 == 1) else {
            return;
        };
        let scale = if own { 1.0 } else { scale };
        let steer = if i / 2 == 0 { v.steer } else { 0.0 };
        let wheel_basis = basis
            * Quat::from_rotation_y(-steer)
            * Quat::from_axis_angle(Vec3::X, v.wheels[i].spin);
        let wpos = v.wheel_world(i);
        if let Some(w) = model_mesh(&self.cache, &key) {
            draw_scratch(
                &mut self.scratch,
                &w.mesh,
                wpos,
                wheel_basis,
                scale,
                WHITE,
                cam,
            );
        }
    }

    /// Cache key of the truck's own tyre (`tireModelBaseName` at full detail),
    /// drawn at scale 1 because the physics radius is measured from the same
    /// mesh. A truck without one falls back to the generic `33X12` tyre scaled
    /// to its radius, and the file is listed in `missing`. The second value is
    /// whether the mesh is the truck's own.
    fn wheel_key(
        &mut self,
        assets: &mut Assets,
        truck: &crate::vehicle::Truck,
        left: bool,
    ) -> Option<(String, bool)> {
        let own = truck.tire_model_file(left);
        let key = format!("WHEEL#{own}");
        if self.cache.get(assets, &key, &own).is_some() {
            return Some((key, true));
        }
        if !self.missing.contains(&own) {
            self.missing.push(own);
        }
        let file = if left {
            "33X12_12L.SMF"
        } else {
            "33X12_12R.SMF"
        };
        let key = format!("WHEEL#{file}");
        self.cache
            .get(assets, &key, file)
            .is_some()
            .then_some((key, false))
    }

    /// Draw a model at an arbitrary pose (scenery props). Opaque parts use the
    /// standard depth-writing material, `TRANSP*` parts the alpha-cutout one.
    pub fn draw_model(&mut self, assets: &mut Assets, base: &str, pos: Vec3, yaw: f32, scale: f32) {
        let base = base.trim().to_ascii_uppercase();
        if base.is_empty() {
            return;
        }
        let key = format!("PROP#{base}");
        let file = format!("{base}.SMF");
        let cam = self.cam_pos;
        if self.cache.get_prop(assets, &key, &file).is_none() {
            return;
        }
        let basis = Quat::from_rotation_y(yaw) * Quat::from_rotation_y(std::f32::consts::PI);
        for transparent in [false, true] {
            let Some(model) = model_mesh(&self.cache, &key) else {
                return;
            };
            if !model.parts.iter().any(|p| p.transparent == transparent) {
                continue;
            }
            let used = self.bind_world_material(transparent);
            let Some(model) = model_mesh(&self.cache, &key) else {
                return;
            };
            for part in model.parts.iter().filter(|p| p.transparent == transparent) {
                draw_scratch(&mut self.scratch, &part.mesh, pos, basis, scale, WHITE, cam);
            }
            if used {
                self.unbind_world_material();
            }
        }
    }

    /// Draw a complete truck (body + four wheels at static ride height) for the
    /// Garage and Quick Race 3D turntables, depth-sorted relative to `eye`.
    pub fn draw_truck_preview(
        &mut self,
        assets: &mut Assets,
        truck: &crate::vehicle::Truck,
        pos: Vec3,
        yaw: f32,
        eye: Vec3,
    ) {
        self.cam_pos = eye;
        self.fog_end = 0.0;
        let yaw_q = Quat::from_rotation_y(yaw);
        let basis = (yaw_q * Quat::from_rotation_y(std::f32::consts::PI)).normalize();
        let r_m = truck.tyre_radius_m();
        let mount_y = truck.wheel_mount(0, 0).y * FT;
        let body_pos = pos + Vec3::Y * (r_m - mount_y).max(r_m);
        let scale = (r_m / WHEEL_RADIUS_M).clamp(0.7, 1.6);

        let mut wheels: Vec<(usize, usize, Vec3, f32)> = Vec::with_capacity(4);
        for axle in 0..2 {
            for side in 0..2 {
                let m = truck.wheel_mount(axle, side);
                let w_local = Vec3::new(m.x * FT, 0.0, -m.z * FT);
                let w_pos = pos + Vec3::Y * r_m + yaw_q * w_local;
                wheels.push((axle, side, w_pos, (w_pos - eye).length_squared()));
            }
        }
        wheels.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
        let body_dist = (body_pos - eye).length_squared();

        for &(_axle, side, w_pos, d) in &wheels {
            if d < body_dist {
                continue;
            }
            if let Some((key, own)) = self.wheel_key(assets, truck, side == 1) {
                let s = if own { 1.0 } else { scale };
                if let Some(w) = model_mesh(&self.cache, &key) {
                    draw_scratch(&mut self.scratch, &w.mesh, w_pos, basis, s, WHITE, eye);
                }
            }
        }
        if let Some(key) = self.body_key(assets, &truck.model_base, truck.paint()) {
            if let Some(model) = model_mesh(&self.cache, &key) {
                draw_scratch(
                    &mut self.scratch,
                    &model.mesh,
                    body_pos,
                    basis,
                    1.0,
                    WHITE,
                    eye,
                );
            }
        }
        for &(_axle, side, w_pos, d) in &wheels {
            if d >= body_dist {
                continue;
            }
            if let Some((key, own)) = self.wheel_key(assets, truck, side == 1) {
                let s = if own { 1.0 } else { scale };
                if let Some(w) = model_mesh(&self.cache, &key) {
                    draw_scratch(&mut self.scratch, &w.mesh, w_pos, basis, s, WHITE, eye);
                }
            }
        }
    }

    pub fn model_length(&mut self, assets: &mut Assets, base: &str) -> Option<f32> {
        let key = self.body_key(assets, base, None)?;
        model_mesh(&self.cache, &key).map(|m| m.length())
    }
}

/// Transform `mesh` into `scratch` and draw it in world space, painter-sorted
/// back-to-front from `cam`. Reuses the renderer's vertex/index buffers, so a
/// cached model part is never cloned or reallocated per frame.
fn draw_scratch(
    scratch: &mut Scratch,
    mesh: &Mesh,
    pos: Vec3,
    basis: Quat,
    scale: f32,
    tint: Color,
    cam: Vec3,
) -> bool {
    if mesh.indices.len() < 3 {
        return false;
    }
    let sun = crate::terrain::SUN;
    scratch.vertices.clear();
    scratch.indices.clear();
    scratch.vertices.reserve(mesh.vertices.len());
    for v in &mesh.vertices {
        let n = basis * vec3(v.normal.x, v.normal.y, v.normal.z);
        let lit = crate::terrain::shade_sun(tint, n, sun);
        scratch.vertices.push(Vertex {
            position: pos + basis * (v.position * scale),
            uv: v.uv,
            color: [
                (v.color[0] as f32 * lit.r).clamp(0.0, 255.0) as u8,
                (v.color[1] as f32 * lit.g).clamp(0.0, 255.0) as u8,
                (v.color[2] as f32 * lit.b).clamp(0.0, 255.0) as u8,
                v.color[3],
            ],
            normal: Vec4::new(n.x, n.y, n.z, 0.0),
        });
    }
    scratch.indices.reserve(mesh.indices.len());
    if cam != Vec3::ZERO && mesh.indices.len() >= 6 {
        scratch.order.clear();
        for c in mesh.indices.chunks_exact(3) {
            let centre = (scratch.vertices[c[0] as usize].position
                + scratch.vertices[c[1] as usize].position
                + scratch.vertices[c[2] as usize].position)
                * (1.0 / 3.0);
            scratch
                .order
                .push(([c[0], c[1], c[2]], (centre - cam).length_squared()));
        }
        scratch
            .order
            .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (tri, _) in &scratch.order {
            scratch.indices.extend_from_slice(tri);
        }
    } else {
        scratch.indices.extend_from_slice(&mesh.indices);
    }

    let mut out = Mesh {
        vertices: std::mem::take(&mut scratch.vertices),
        indices: std::mem::take(&mut scratch.indices),
        texture: mesh.texture.clone(),
    };
    draw_mesh(&out);
    out.vertices.clear();
    out.indices.clear();
    scratch.vertices = std::mem::take(&mut out.vertices);
    scratch.indices = std::mem::take(&mut out.indices);
    true
}

/// Cached model by key, borrowing only the cache so `&mut self.scratch` stays
/// available for the draw call.
fn model_mesh<'a>(cache: &'a ModelCache, key: &str) -> Option<&'a Prepared> {
    cache.models.get(key).and_then(|m| m.as_ref())
}

fn clone_mesh(m: &Mesh) -> Mesh {
    Mesh {
        vertices: m.vertices.clone(),
        indices: m.indices.clone(),
        texture: m.texture.clone(),
    }
}

fn get_or_create_world_material(
    mat: &mut MaterialSlot,
    depth_write: bool,
    depth_test: miniquad::Comparison,
) -> Option<&Material> {
    if mat.0.is_none() {
        let ctx = unsafe { macroquad::window::get_internal_gl().quad_context };
        let shader = match ctx.info().backend {
            miniquad::Backend::OpenGl => ShaderSource::Glsl {
                vertex: r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
attribute vec4 normal;

varying lowp vec2 uv;
varying lowp vec4 color;
varying mediump float v_dist;

uniform mat4 Model;
uniform mat4 Projection;
uniform vec4 FogCam;

void main() {
    gl_Position = Projection * Model * vec4(position, 1.0);
    vec4 wp = Model * vec4(position, 1.0);
    v_dist = length(wp.xyz - FogCam.xyz);
    color = color0 / 255.0;
    uv = texcoord;
}"#,
                fragment: r#"#version 100
varying lowp vec4 color;
varying lowp vec2 uv;
varying mediump float v_dist;

uniform sampler2D Texture;
uniform mediump vec4 FogColor;
uniform mediump vec4 FogParams;

void main() {
    lowp vec4 tex = texture2D(Texture, uv);
    if (FogParams.z > 0.5 && tex.a < 0.5) {
        discard;
    }
    mediump float fog_f = (FogParams.y > FogParams.x)
        ? clamp((v_dist - FogParams.x) / (FogParams.y - FogParams.x), 0.0, 1.0)
        : 0.0;
    lowp vec3 c = mix(color.rgb * tex.rgb, FogColor.rgb, fog_f);
    lowp float out_a = (FogParams.z > 0.5) ? (color.a * tex.a) : color.a;
    gl_FragColor = vec4(c, out_a);
}"#,
            },
            miniquad::Backend::Metal => ShaderSource::Msl {
                program: r#"
#include <metal_stdlib>
using namespace metal;

struct Vertex
{
    float3 position    [[attribute(0)]];
    float2 texcoord    [[attribute(1)]];
    float4 color0      [[attribute(2)]];
};

struct RasterizerData
{
    float4 position [[position]];
    float4 color [[user(locn0)]];
    float2 uv [[user(locn1)]];
    float dist [[user(locn2)]];
};

struct Uniforms
{
    float4x4 Model;
    float4x4 Projection;
    float4 _Time;
    float4 FogColor;
    float4 FogCam;
    float4 FogParams;
};

vertex RasterizerData vertexShader(Vertex v [[stage_in]],
                                   constant Uniforms& uniforms [[buffer(0)]])
{
    RasterizerData out;

    float4 wp = uniforms.Model * float4(v.position, 1);
    out.position = uniforms.Projection * wp;
    out.color = v.color0 / 255.0;
    out.uv = v.texcoord;
    out.dist = length(wp.xyz - uniforms.FogCam.xyz);

    return out;
}

fragment float4 fragmentShader(RasterizerData in [[stage_in]],
                               constant Uniforms& uniforms [[buffer(0)]],
                               texture2d<float> tex [[texture(0)]],
                               sampler texSmplr [[sampler(0)]])
{
    float4 t = tex.sample(texSmplr, in.uv);
    if (uniforms.FogParams.z > 0.5 && t.a < 0.5) {
        discard_fragment();
    }
    float fog_f = (uniforms.FogParams.y > uniforms.FogParams.x)
        ? clamp((in.dist - uniforms.FogParams.x) / (uniforms.FogParams.y - uniforms.FogParams.x), 0.0, 1.0)
        : 0.0;
    float4 base_col = in.color * t;
    float3 rgb = mix(base_col.rgb, uniforms.FogColor.rgb, fog_f);
    float out_a = (uniforms.FogParams.z > 0.5) ? base_col.a : in.color.a;
    return float4(rgb, out_a);
}
"#,
            },
        };
        let params = MaterialParams {
            pipeline_params: miniquad::PipelineParams {
                depth_write,
                depth_test,
                color_blend: Some(miniquad::BlendState::new(
                    miniquad::Equation::Add,
                    miniquad::BlendFactor::Value(miniquad::BlendValue::SourceAlpha),
                    miniquad::BlendFactor::OneMinusValue(miniquad::BlendValue::SourceAlpha),
                )),
                alpha_blend: Some(miniquad::BlendState::new(
                    miniquad::Equation::Add,
                    miniquad::BlendFactor::Zero,
                    miniquad::BlendFactor::One,
                )),
                ..Default::default()
            },
            uniforms: vec![
                UniformDesc::new("FogColor", UniformType::Float4),
                UniformDesc::new("FogCam", UniformType::Float4),
                UniformDesc::new("FogParams", UniformType::Float4),
            ],
            ..Default::default()
        };
        *mat = MaterialSlot(Some(load_material(shader, params).ok()));
    }
    mat.get()
}

/// Parse a body while the loading screen is still up.
pub fn warm_cache(assets: &mut Assets, base: &str) {
    let mut r = Renderer::new();
    let _ = r.model_length(assets, base);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retail_body_and_wheel_models_load() {
        let Ok(_) = std::fs::metadata(Path::new(MODEL_DIR).join("BLAZZR2.SMF")) else {
            return;
        };
        let text = std::fs::read_to_string(Path::new(MODEL_DIR).join("BLAZZR2.SMF")).unwrap();
        let smf = Smf::parse(&text).unwrap();
        let (lo, hi) = smf.bounds();
        assert!(hi.z - lo.z > 4.0, "expected metre-scale geometry");
        assert!(hi.y - lo.y > 1.2);
        let wheel = std::fs::read_to_string(Path::new(MODEL_DIR).join("33X12_12L.SMF")).unwrap();
        let w = Smf::parse(&wheel).unwrap();
        let (wlo, whi) = w.bounds();
        assert!((whi.y - wlo.y - WHEEL_RADIUS_M * 2.0).abs() < 0.01);
    }
}
