//! Terminal Reality `C3DModel` (SMF) reader.
//!
//! Text format: version, then per mesh a variable-length descriptor block
//! (LOD/name/opacity markers) ending in a `verts,?,faces,flags` header, one
//! texture record, then vertex records (`x,y,z,nx,ny,nz,u,v`) and `a,b,c`
//! triangle records. Retail truck models are authored in feet; coordinates are
//! converted to metres at parse time, matching `vehicle.rs`.

use std::collections::HashMap;

use macroquad::prelude::*;

use crate::vehicle::FT;

#[derive(Clone, Copy, Debug)]
pub struct LightNode {
    pub pos: Vec3,
    pub size: f32,
    pub rear: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SmfMesh {
    pub name: String,
    pub opaque: bool,
    pub lod_min: f32,
    pub texture: String,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    pub indices: Vec<u16>,
    pub radius: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Smf {
    pub meshes: Vec<SmfMesh>,
}

fn floats(line: &str) -> Vec<f32> {
    line.split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

/// `verts,?,faces,flags` — four bare integers. Vertex rows carry eight float
/// fields, so they never collide with this.
fn header(line: &str) -> Option<[usize; 4]> {
    let mut it = line.split(',').map(|s| s.trim());
    let mut out = [0usize; 4];
    for slot in out.iter_mut() {
        *slot = it.next()?.parse().ok()?;
    }
    if it.next().is_some() {
        return None;
    }
    Some(out)
}

impl Smf {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text
            .lines()
            .map(|l| l.trim_end_matches('\r').trim())
            .filter(|l| !l.is_empty());
        if lines.next() != Some("C3DModel") {
            return Err("not a C3DModel".into());
        }
        let _version = lines.next().ok_or("missing version")?;
        let mesh_count: usize = lines
            .next()
            .and_then(|l| l.parse().ok())
            .ok_or("bad mesh count")?;
        if mesh_count > 512 {
            return Err(format!("implausible mesh count {mesh_count}"));
        }
        let mut out = Smf::default();
        for m in 0..mesh_count {
            // Descriptor tokens (LOD distance, mesh name, opacity marker) vary
            // in number between releases; consume them until the count header.
            let mut desc = Vec::new();
            let hdr = loop {
                let line = lines
                    .next()
                    .ok_or_else(|| format!("mesh {m}: truncated before header"))?;
                if let Some(h) = header(&line) {
                    break h;
                }
                desc.push(line);
            };
            let nverts = hdr[0];
            let nfaces = hdr[2];
            if nverts > 200_000 || nfaces > 400_000 {
                return Err(format!("mesh {m}: implausible counts"));
            }
            let name = desc
                .iter()
                .find(|d| {
                    let u = d.to_ascii_uppercase();
                    !u.contains(',')
                        && !u.contains('.')
                        && !u.contains("OPAQUE")
                        && !u.contains("TRANS")
                        && u.parse::<f32>().is_err()
                })
                .map(|d| d.to_string())
                .unwrap_or_else(|| format!("mesh{m}"));
            let opaque = !desc
                .iter()
                .any(|d| d.to_ascii_uppercase().contains("TRANS"));
            let lod_min = desc
                .iter()
                .find_map(|d| {
                    let (a, _b) = d.split_once(',')?;
                    a.trim().parse::<f32>().ok()
                })
                .unwrap_or(0.0);

            // Exactly one texture record per mesh in the retail data. Consume
            // any extras defensively.
            let mut texture = String::new();
            loop {
                let peek = lines
                    .next()
                    .ok_or_else(|| format!("mesh {m}: missing texture record"))?;
                let up = peek.to_ascii_uppercase();
                if up.ends_with(".RAW") || up.ends_with(".TIF") {
                    // Evo 1 names `.RAW`, Evo 2 `.TIF`; both resolve by stem
                    // to the extracted RAW/ACT/OPA triplet.
                    texture = peek.rsplit(',').next().unwrap_or("").trim().to_string();
                    // A second consecutive texture record is rare but legal.
                    continue;
                }
                // Evo 2 material extras: a `v1` version tag before the texture
                // record and a quoted bump-map name after it.
                if peek.starts_with('"') || (up.starts_with('V') && !up.contains(',')) {
                    continue;
                }
                // Not a texture record: the first vertex. Handle it below.
                let v = floats(&peek);
                let mut mesh = SmfMesh {
                    name,
                    opaque,
                    lod_min,
                    texture,
                    ..Default::default()
                };
                if v.len() >= 8 {
                    mesh.positions.push(vec3(v[0], v[1], v[2]) * FT);
                    mesh.normals
                        .push(vec3(v[3], v[4], v[5]).normalize_or_zero());
                    mesh.uvs.push(vec2(v[6], v[7]));
                }
                for _ in 1..nverts {
                    let Some(line) = lines.next() else { break };
                    let v = floats(&line);
                    if v.len() < 8 {
                        continue;
                    }
                    mesh.positions.push(vec3(v[0], v[1], v[2]) * FT);
                    mesh.normals
                        .push(vec3(v[3], v[4], v[5]).normalize_or_zero());
                    mesh.uvs.push(vec2(v[6], v[7]));
                }
                for _ in 0..nfaces {
                    let Some(line) = lines.next() else { break };
                    let f = floats(&line);
                    if f.len() < 3 {
                        continue;
                    }
                    let a = f[0] as usize;
                    let b = f[1] as usize;
                    let c = f[2] as usize;
                    if a < mesh.positions.len()
                        && b < mesh.positions.len()
                        && c < mesh.positions.len()
                    {
                        mesh.indices
                            .extend_from_slice(&[a as u16, b as u16, c as u16]);
                    }
                }
                mesh.radius = mesh
                    .positions
                    .iter()
                    .map(|p| p.length())
                    .fold(0.0_f32, f32::max);
                out.meshes.push(mesh);
                break;
            }
        }
        Ok(out)
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for m in &self.meshes {
            for p in &m.positions {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
        }
        if lo.x > hi.x {
            (Vec3::ZERO, Vec3::ZERO)
        } else {
            (lo, hi)
        }
    }

    /// Flatten LOD-0 stock sub-meshes into one Macroquad mesh. Distances with
    /// `lod_min > 0` (lower-detail LODs) and un-equipped aftermarket bolt-ons
    /// are excluded, while pickup bed liners (`bedcovr`), dark interiors, and
    /// smoked cabin glass (`glassi`/`glasse`) are included with opaque body and
    /// dark interior backfaces so the vehicle never looks hollow or see-through.
    /// Light nodes for additive flares. Retail trucks carry `Light*` quads
    /// textured from the body sheet; the original draws a glow at each lamp
    /// instead of the flat red/white panel.
    pub fn light_nodes(&self) -> Vec<LightNode> {
        let mut out = Vec::new();
        for m in &self.meshes {
            if m.lod_min > 0.1
                || !m.name.to_ascii_lowercase().starts_with("light")
                || m.positions.is_empty()
            {
                continue;
            }
            let (mut lo, mut hi) = (m.positions[0], m.positions[0]);
            for p in &m.positions {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
            let centre = (lo + hi) * 0.5;
            let size = ((hi - lo).max_element() * 0.12).clamp(0.10, 0.34);
            out.push(LightNode {
                pos: centre,
                size,
                rear: centre.z < 0.0,
            });
        }
        out
    }

    pub fn merged_mesh(&self, texture: Option<Texture2D>, glass: Option<&[Vec<bool>]>) -> Mesh {
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut indices: Vec<u16> = Vec::new();
        let primary_tex = self
            .meshes
            .iter()
            .find(|m| !m.texture.is_empty())
            .map(|m| m.texture.to_ascii_uppercase())
            .unwrap_or_default();
        for (mesh_i, m) in self.meshes.iter().enumerate() {
            if m.lod_min > 0.1 {
                continue;
            }
            let low = m.name.to_ascii_lowercase();
            let is_addon = [
                "bushbar",
                "bushguard",
                "mudflps",
                "winch",
                "snorkel",
                "skid",
                "rfrack",
                "rollbr",
                "fog",
            ]
            .iter()
            .any(|k| low.starts_with(k));
            let is_glass = low.starts_with("glass");
            let is_light = low.starts_with("light");
            // Lamp quads become additive flares, not body geometry.
            if is_light {
                continue;
            }
            if is_addon || (!m.opaque && !is_glass) {
                continue;
            }
            // Sub-meshes normally share the body's palette texture, but bed
            // liners and glass may name their own; keep those regardless.
            if !primary_tex.is_empty()
                && !m.texture.is_empty()
                && !m.texture.eq_ignore_ascii_case(&primary_tex)
                && !is_glass
                && !low.starts_with("bedcovr")
            {
                continue;
            }
            if vertices.len() + m.positions.len() * 2 >= u16::MAX as usize {
                continue;
            }
            // A `glasse`/`glassi` shell also carries roof and pillar geometry,
            // so only vertices whose UV lands on a dark glass pixel get the
            // smoked tint (`alpha ≈ 0.86`); the rest keep the body texture
            // fully opaque. The bed liner is darkened slightly, nothing else.
            let glass_flags = glass.and_then(|g| g.get(mesh_i));
            let base = vertices.len() as u16;
            for (i, p) in m.positions.iter().enumerate() {
                let n = m.normals.get(i).copied().unwrap_or(Vec3::Y);
                let pane = glass_flags.and_then(|g| g.get(i)).copied().unwrap_or(false);
                let vcolor = if is_glass && pane {
                    // Smoked glazing: keep the tint but leave enough
                    // transmission that the cabin/sky reads through it.
                    if low.starts_with("glassi") {
                        [24, 30, 40, 170]
                    } else {
                        [48, 60, 78, 150]
                    }
                } else if low.starts_with("cockpit") || low.starts_with("undbody") {
                    [24, 26, 29, 255]
                } else if low.starts_with("bedcovr") {
                    [150, 152, 158, 255]
                } else {
                    [255, 255, 255, 255]
                };
                vertices.push(Vertex {
                    position: *p,
                    uv: m.uvs.get(i).copied().unwrap_or_default(),
                    color: vcolor,
                    normal: Vec4::new(n.x, n.y, n.z, 0.0),
                });
            }
            for idx in &m.indices {
                indices.push(base.saturating_add(*idx));
            }
            // Emit solid dark charcoal interior backfaces for the main shell,
            // pickup bed and cabin glass so openings or windows never expose
            // sky/terrain through the truck.
            if low.starts_with("body") || low.starts_with("bedcovr") {
                let back_base = vertices.len() as u16;
                // The interior shell must clear the outer skin by more than the
                // depth-buffer resolution at chase distance, otherwise it wins
                // the `LessOrEqual` tie and blacks out the roof and bed.
                let inset = if is_glass { 0.10 } else { 0.06 };
                for (i, p) in m.positions.iter().enumerate() {
                    let n = m.normals.get(i).copied().unwrap_or(Vec3::Y);
                    vertices.push(Vertex {
                        position: *p - n * inset,
                        uv: m.uvs.get(i).copied().unwrap_or_default(),
                        color: [12, 14, 18, 255],
                        normal: Vec4::new(-n.x, -n.y, -n.z, 0.0),
                    });
                }
                for c in m.indices.chunks_exact(3) {
                    indices.push(back_base.saturating_add(c[0]));
                    indices.push(back_base.saturating_add(c[2]));
                    indices.push(back_base.saturating_add(c[1]));
                }
            }
        }
        if vertices.is_empty() {
            if let Some(m) = self.meshes.first() {
                for (i, p) in m.positions.iter().enumerate() {
                    let n = m.normals.get(i).copied().unwrap_or(Vec3::Y);
                    vertices.push(Vertex {
                        position: *p,
                        uv: m.uvs.get(i).copied().unwrap_or_default(),
                        color: [255, 255, 255, 255],
                        normal: Vec4::new(n.x, n.y, n.z, 0.0),
                    });
                }
                indices.extend_from_slice(&m.indices);
            }
        }
        Mesh {
            vertices,
            indices,
            texture,
        }
    }

    /// Macroquad mesh for a single sub-mesh with a flat vertex tint.
    pub fn build_mesh(&self, mesh: &SmfMesh, tint: Color) -> Mesh {
        let color = [
            (tint.r * 255.0) as u8,
            (tint.g * 255.0) as u8,
            (tint.b * 255.0) as u8,
            (tint.a * 255.0) as u8,
        ];
        Mesh {
            vertices: mesh
                .positions
                .iter()
                .enumerate()
                .map(|(i, p)| Vertex {
                    position: *p,
                    uv: mesh.uvs.get(i).copied().unwrap_or_default(),
                    color,
                    normal: mesh
                        .normals
                        .get(i)
                        .map(|n| Vec4::new(n.x, n.y, n.z, 0.0))
                        .unwrap_or(Vec4::Y),
                })
                .collect(),
            indices: mesh.indices.clone(),
            texture: Some(Texture2D::empty()),
        }
    }
}

#[derive(Default)]
pub struct ModelCache {
    parsed: HashMap<String, Option<Smf>>,
}

impl ModelCache {
    pub fn get(&mut self, path: &str) -> Option<&Smf> {
        if !self.parsed.contains_key(path) {
            let parsed = std::fs::read_to_string(path)
                .ok()
                .and_then(|t| Smf::parse(&t).ok());
            self.parsed.insert(path.to_string(), parsed);
        }
        self.parsed.get(path)?.as_ref()
    }

    pub fn len(&self) -> usize {
        self.parsed.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMF: &str = "C3DModel\r\n\r\n4\r\n\r\n1\r\n\r\n0,50.000000\r\n\r\nBody\r\n\r\n1\r\n\r\n1\r\n\r\n3,1,1,0\r\n\r\n1,1,32,0,0,TEX.RAW\r\n\r\n0,0,0,0,0,1,0,0\r\n1,0,0,0,0,1,1,0\r\n0,1,0,0,0,1,0,1\r\n\r\n0,1,2\r\n";

    #[test]
    fn parses_triangle_and_converts_units() {
        let s = Smf::parse(SMF).unwrap();
        assert_eq!(s.meshes.len(), 1);
        let m = &s.meshes[0];
        assert_eq!(m.name, "Body");
        assert_eq!(m.indices, vec![0, 1, 2]);
        assert_eq!(m.texture, "TEX.RAW");
        assert!(m.opaque);
        // Truck models are authored in feet.
        assert!((m.positions[1].x - FT).abs() < 1e-6);
        assert!(m.radius > 0.0);
    }

    #[test]
    fn rejects_foreign_files() {
        assert!(Smf::parse("nope").is_err());
    }

    #[test]
    fn skips_out_of_range_faces() {
        let bad = SMF.replace("0,1,2", "0,1,9");
        let s = Smf::parse(&bad).unwrap();
        assert!(s.meshes[0].indices.is_empty());
    }

    #[test]
    fn light_meshes_become_flare_nodes_not_body_geometry() {
        let smf = "C3DModel\r\n4\r\n2\r\n0,50\r\n\
Body\r\n1\r\n1\r\n3,1,1,0\r\nv1\r\n\
1,1,64,0,1,T.TIF\r\n\"T_bump.TIF\"\r\n\
-1,0,0,0,1,0,0,0\r\n1,0,0,0,1,0,1,0\r\n0,1,-1,0,1,0,0.5,0.5\r\n0,1,2\r\n\
LightBR\r\n1\r\n1\r\n3,1,1,0\r\nv1\r\n\
1,1,32,0,1,T.TIF\r\n\"T_bump.TIF\"\r\n\
-2,0,-8,0,0,-1,0,0\r\n-1,0,-8,0,0,-1,1,0\r\n-1,1,-8,0,0,-1,1,1\r\n0,1,2\r\n";
        let s = Smf::parse(smf).unwrap();
        let nodes = s.light_nodes();
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].rear, "LightBR sits behind the rear axle");
        assert!((nodes[0].pos.z + 8.0 * FT).abs() < 0.01);
        assert!(nodes[0].size > 0.0 && nodes[0].size <= 0.34);

        let mesh = s.merged_mesh(None, None);
        // Body (3 verts) + its dark backfaces (3). The Light* quad must not
        // contribute three more vertices to the body mesh.
        assert_eq!(mesh.vertices.len(), 6);
    }

    #[test]
    fn transparent_descriptor_is_detected() {
        let t = SMF.replace("Body", "TRANSP");
        let s = Smf::parse(&t).unwrap();
        assert!(!s.meshes[0].opaque);
        let v = s.merged_mesh(None, None);
        assert_eq!(v.vertices.len(), 3);
        assert_eq!(v.indices.len(), 3);
    }

    #[test]
    fn retail_van_parses_as_feet() {
        let path = "../assets/extracted/model/vehicle/MODELS/BLAZERZR2.SMF";
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let s = Smf::parse(&text).unwrap();
        // Evo 2 retail truck.pod: BLAZERZR2 is 32 meshes, not the 35 of the
        // Evo 1 BLAZZR2 the old assertion was copied from.
        assert_eq!(s.meshes.len(), 32);
        let (lo, hi) = s.bounds();
        let len = hi.z - lo.z;
        // ~15 ft van body (4.61 m), converted to metres.
        assert!((4.0..6.0).contains(&len), "model length {len}");
        assert!(!s.meshes[0].texture.is_empty());
    }

    #[test]
    fn parses_evo2_material_records() {
        // Header and first records of Evo 2 `JEEPWRANG.SMF`.
        let text = "C3DModel\r\n4\r\n1\r\n0,50\r\nBody\r\n1\r\n1\r\n3,1,1,0\r\nv1\r\n\
1,1,64,0,1,JeepWrang.TIF\r\n\"JeepWrang_bump.TIF\"\r\n\
-2.25447,1.69068,5.19348,-0.316561,0.0924747,0.944054,0.878906,0.585938\r\n\
-2.04054,2.09229,5.19348,-0.134312,0.0715452,0.988353,0.98533,0.585938\r\n\
-2.20137,2.10483,5.17072,-0.316561,0.0924747,0.944054,0.9,0.6\r\n0,1,2\r\n";
        let s = Smf::parse(text).expect("evo2 smf parses");
        assert_eq!(s.meshes[0].texture, "JeepWrang.TIF");
        assert_eq!(s.meshes[0].positions.len(), 3);
    }

    #[test]
    fn merged_pickup_mesh_has_opaque_bed_dark_backfaces_and_tinted_glass() {
        let path = "../assets/extracted/model/vehicle/MODELS/TUNDRA.SMF";
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let s = Smf::parse(&text).unwrap();
        // Every `glass*` vertex is a pane for this test (no asset decoding).
        let panes: Vec<Vec<bool>> = s
            .meshes
            .iter()
            .map(|m| vec![m.name.starts_with("glass"); m.positions.len()])
            .collect();
        let mesh = s.merged_mesh(None, Some(&panes));
        assert!(
            mesh.vertices
                .iter()
                .any(|v| v.color == [150, 152, 158, 255]),
            "pickup bed liner (bedcovr) must be included, dark and opaque"
        );
        assert!(
            mesh.vertices.iter().any(|v| v.color == [12, 14, 18, 255]),
            "dark interior backfaces must be present and opaque"
        );
        assert!(
            mesh.vertices.iter().any(|v| v.color == [48, 60, 78, 150]),
            "exterior smoked glass tint must be included and translucent"
        );
        assert!(
            mesh.vertices.iter().all(|v| v.color[3] >= 150),
            "no vehicle vertex may be nearly invisible"
        );
        // Without a pane mask the glass shell keeps the body colour opaque, so
        // a roof that shares the `glasse` mesh is not blacked out.
        let plain = s.merged_mesh(None, None);
        assert!(plain.vertices.iter().all(|v| v.color[3] == 255));
    }
}
