//! Track data: `.LVL` header, terrain heightmap, `.CLR` texture-index map,
//! `.TEX` texture list and `.SIT` object placement.
//!
//! The original stores terrain as a 256x256 `u16` height grid (the
//! `<track>.RAW` next to the textures) where one unit is 1/32 ft. Cells are
//! 32 ft square, so a track spans 8192 ft (2.5 km). `.CLR` is a parallel grid
//! of indices into `.TEX` (the value range exactly matches `textureCount`),
//! `.SDW` a shadow mask. Collision is the bilinear height function itself;
//! geometry and props are generated from it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use macroquad::prelude::*;

use crate::vehicle::FT;

pub const GRID: usize = 256;

/// Terrain cell edge in the original world units (feet).
pub const CELL_FT: f32 = 32.0;
/// Terrain cell edge in metres.
pub const CELL_M: f32 = CELL_FT * FT;
/// Height samples are `1/32 ft` per unit.
pub const HEIGHT_M_PER_UNIT: f32 = FT / 32.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Surface {
    Asphalt,
    #[default]
    Dirt,
    Gravel,
    Mud,
    Sand,
    Grass,
    Water,
    Snow,
}

impl Surface {
    pub fn grip(self) -> f32 {
        match self {
            Surface::Asphalt => 1.12,
            Surface::Dirt => 0.92,
            Surface::Gravel => 0.84,
            Surface::Mud => 0.66,
            Surface::Sand => 0.62,
            Surface::Grass => 0.78,
            Surface::Water => 0.48,
            Surface::Snow => 0.55,
        }
    }

    pub fn rolling_drag(self) -> f32 {
        match self {
            Surface::Water => 6.0,
            Surface::Mud => 2.6,
            Surface::Sand => 2.0,
            Surface::Snow => 1.2,
            Surface::Gravel => 0.9,
            Surface::Grass => 0.7,
            _ => 0.2,
        }
    }

    pub fn splash(self) -> bool {
        matches!(self, Surface::Water | Surface::Mud)
    }

    pub fn dust(self) -> bool {
        matches!(self, Surface::Dirt | Surface::Gravel | Surface::Sand)
    }

    pub fn name(self) -> &'static str {
        match self {
            Surface::Asphalt => "asphalt",
            Surface::Dirt => "dirt",
            Surface::Gravel => "gravel",
            Surface::Mud => "mud",
            Surface::Sand => "sand",
            Surface::Grass => "grass",
            Surface::Water => "water",
            Surface::Snow => "snow",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TexRef {
    pub index: u16,
    pub blend: u8,
    pub file: String,
}

/// `.SIT` `!type,flags` value of the retail game's moving traffic (the taxi,
/// lorries and semis on THEHILL). Every such box also carries a non-zero
/// `bvel`; no other type does.
pub const PROP_KIND_TRAFFIC: u8 = 10;
/// `!type,flags` value of trees (262 of THEHILL's 596 boxes, 294 of ASPEN's).
pub const PROP_KIND_TREE: u8 = 12;

#[derive(Clone, Debug, Default)]
pub struct Prop {
    pub pos: Vec3,
    pub yaw: f32,
    pub model: String,
    /// First number of the box's `!type,flags` line.
    pub kind: u8,
    /// The box has a non-zero `bvel` (ft/s): the original drives it along the
    /// road. The port has no traffic yet.
    pub moving: bool,
}

impl Prop {
    /// Moving traffic the port cannot drive yet. Frozen at its start point it
    /// is a lorry parked across the racing line, so until traffic is ported
    /// it is neither drawn nor collided with (`Race::build` reports how many).
    pub fn is_unported_traffic(&self) -> bool {
        self.moving || self.kind == PROP_KIND_TRAFFIC
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: String,
    pub dir: PathBuf,
    pub heights: Vec<u16>,
    pub colors: Vec<u16>,
    pub shadows: Vec<u16>,
    pub scale: f32,
    /// Rigid lift applied to every height sample.
    pub height_offset: f32,
    pub water_height: f32,
    /// Water surface tint parsed from `@waterR,waterG,waterB,waterOpacity`.
    pub water_color: Color,
    /// Distance fog / horizon color derived from the level's sky texture.
    pub fog_color: Color,
    pub textures: Vec<TexRef>,
    pub detail_texture: Option<String>,
    pub sky_texture: Option<String>,
    pub light_dir: Vec3,
    pub props: Vec<Prop>,
    /// Vehicle grid from `*** Vehicles ***`: truck file, spawn point (metres)
    /// and heading. `MATT` (the retail test map) has no AI lines, so this is
    /// also the only start data available for it.
    pub spawns: Vec<Spawn>,
    /// `*** Course ***` checkpoint gates. Empty on the levels that only ship
    /// `*** Paths ***` splines (audit finding #3: the pentagon placeholder ran
    /// on 31 levels because gates were never parsed).
    pub gates: Vec<Gate>,
    /// `*** Paths ***` splines, in file order, as world-space points. Evo 2
    /// levels without a recorded `data/data/AI` lap carry their racing line
    /// here; the longest populated path becomes the course.
    pub paths: Vec<Vec<Vec3>>,
    /// Per-texture colour tint, normalised against the dominant ground
    /// material. Approximates the original multi-texture blend with a single
    /// bound albedo.
    pub tints: Vec<Color>,
    /// Mean ground material RGB per `.TEX` entry, used to tint wheel dust
    /// particles to the underlying sand/dirt/mud colour.
    pub ground_colors: Vec<Color>,
    /// Ground-texture atlas layout once `Assets::track_atlas` has packed the
    /// `.TEX` list; `None` falls back to the single tinted albedo.
    pub atlas: Option<Atlas>,
}

#[derive(Clone, Debug)]
pub struct Spawn {
    pub truck: String,
    pub pos: Vec3,
    pub heading: f32,
}

/// A `*** Course ***` gate: the `cstart`→`cend` line the car must cross, in
/// world metres. Retail race levels carry these instead of AI lines for lap
/// timing (`c1Count` of them, e.g. 14 on `DEVIL`, 22 on `OBSPARK1`).
#[derive(Clone, Debug)]
pub struct Gate {
    pub start: Vec3,
    pub end: Vec3,
    /// `ctype` (1 = drive-through).
    pub kind: u8,
    /// `cspeed` hint, km/h as stored.
    pub speed: f32,
    /// `cTrackWidth` in feet, converted to metres.
    pub width_m: f32,
}

impl Gate {
    /// Centre of the crossing line, which is what the lap logic and the
    /// waypoint beams use.
    pub fn centre(&self) -> Vec3 {
        (self.start + self.end) * 0.5
    }
}

/// Layout of the packed `.TEX` atlas: square tiles in rows of `cols`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Atlas {
    pub cols: usize,
    pub tile: usize,
    pub w: f32,
    pub h: f32,
}

/// Default sun when a level has none (offroad4x4 `render::SUN`).
pub const SUN: Vec3 = Vec3::new(0.42, 0.82, 0.38);

/// Lambert sun plus sky fill, ported from offroad4x4 `render::shade_sun`:
/// slopes facing away from the sun drop to ~30 %, flats under a high sun
/// reach full brightness, so relief reads at a distance.
pub fn shade_sun(c: Color, n: Vec3, sun: Vec3) -> Color {
    let l = n.normalize_or_zero().dot(sun.normalize_or_zero()).max(0.0);
    let sky = 0.5 + 0.5 * n.y.max(0.0);
    let k = 0.30 + 0.22 * sky + 0.55 * l;
    Color::new(
        (c.r * k).min(1.0),
        (c.g * k).min(1.0),
        (c.b * k).min(1.0),
        c.a,
    )
}

fn read_lvl(path: &Path) -> Result<HashMap<String, String>, String> {
    let text = std::fs::read(path).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&text);
    let mut map = HashMap::new();
    let mut lines = text.lines().map(|l| l.trim());
    while let Some(line) = lines.next() {
        if line.is_empty() || line.starts_with("//") || line.starts_with("signature:") {
            continue;
        }
        match line.chars().next() {
            Some('!') | Some('@') | Some('#') | Some('$') | Some('%') => {
                let val = lines.next().unwrap_or("").to_string();
                map.insert(line[1..].trim().to_string(), val);
            }
            _ => {}
        }
    }
    Ok(map)
}

struct DirCache {
    path: PathBuf,
    files: HashMap<String, (PathBuf, u64)>,
}

impl DirCache {
    fn scan(path: PathBuf) -> Self {
        let files = std::fs::read_dir(&path)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let path = e.path();
                        let name = path.file_name()?.to_str()?.to_ascii_uppercase();
                        let len = e.metadata().map(|m| m.len()).unwrap_or(0);
                        Some((name, (path, len)))
                    })
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        DirCache { path, files }
    }
}

/// Case-insensitive asset lookup over a fixed search path.
///
/// Each directory is listed once: a level references thousands of texture
/// entries, and one `read_dir` per entry dominated load time.
struct Finder {
    dirs: Vec<DirCache>,
}

impl Finder {
    /// Search path, most specific first. The extractor sorted files by type,
    /// not by level: heightmaps and level textures landed under
    /// `texture/*/DATA`, `.TEX` under `unknown/*/DATA`, and the same-named
    /// *preview* art under `texture/*/ART` (a 64 KiB thumbnail that is not a
    /// 256×256 heightmap). The ART folders are last so a size check can
    /// reject them.
    fn new(root: &Path) -> Self {
        let mut dirs = vec![root.join("DATA"), root.join("WORLD")];
        if let Some(assets) = root.parent().and_then(|p| p.parent()) {
            for d in [
                "texture/texture/DATA",
                "texture/ui/DATA",
                "unknown/unknown/DATA",
                "unknown/ui/DATA",
                "texture/texture/ART",
                "texture/ui/ART",
                "evo2/texture/ART",
            ] {
                dirs.push(assets.join(d));
            }
        }
        let dirs = dirs.into_iter().map(DirCache::scan).collect();
        Self { dirs }
    }

    /// Prepend a directory to the search path (used for the per-track overlay,
    /// `assets/custom_tracks/<id>/`), so user edits win over the extraction.
    fn prepend(&mut self, dir: PathBuf) {
        self.dirs.insert(0, DirCache::scan(dir));
    }

    /// Find `name`. `want_len` (0 = any) rejects a file of the wrong size,
    /// which is what keeps `MATT.RAW`'s 64 KiB preview out of the heightmap
    /// slot.
    fn find(&self, name: &str, want_len: usize) -> Option<PathBuf> {
        let name_upper = name.to_ascii_uppercase();
        for dir in &self.dirs {
            if let Some((p, len)) = dir.files.get(&name_upper) {
                if want_len == 0 || *len as usize == want_len {
                    return Some(p.clone());
                }
            }
        }
        None
    }

    /// First match in an explicit directory list (used for texture tinting).
    fn find_in(&self, sub: &str, name: &str) -> Option<PathBuf> {
        let name_upper = name.to_ascii_uppercase();
        for dir in &self.dirs {
            if !dir.path.ends_with(sub) {
                continue;
            }
            if let Some((p, _)) = dir.files.get(&name_upper) {
                return Some(p.clone());
            }
        }
        None
    }
}

/// `raw`/`clr`/`tex`/`sky` names declared in the positional `.LVL` header.
/// Retail levels list heightmap `.raw`, `.clr`, `.tex`, then sky `.raw`
/// before prefixed keys (`!`, `@`, `#`, `$`, `%`); hand-made ones fall back to id.
fn declared_files(lvl: &Path) -> [String; 4] {
    let mut out: [String; 4] = Default::default();
    let Ok(bytes) = std::fs::read(lvl) else {
        return out;
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut seen_tex = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with(['!', '@', '#', '$', '%']) || l.starts_with("signature:") {
            break;
        }
        let low = l.to_ascii_lowercase();
        if low.ends_with(".raw") {
            if out[0].is_empty() && !seen_tex {
                out[0] = l.to_string();
            } else if seen_tex && out[3].is_empty() {
                out[3] = l.to_string();
                break;
            }
        } else if low.ends_with(".clr") && out[1].is_empty() {
            out[1] = l.to_string();
        } else if low.ends_with(".tex") && out[2].is_empty() {
            out[2] = l.to_string();
            seen_tex = true;
        }
    }
    out
}

fn default_fog_color(sky: Option<&str>) -> Color {
    let upper = sky.unwrap_or("").to_ascii_uppercase();
    if upper.contains("NITE") {
        Color::new(0.09, 0.12, 0.20, 1.0)
    } else if upper.contains("DUSK") {
        Color::new(0.62, 0.44, 0.36, 1.0)
    } else if upper.contains("CLOUD") || upper.contains("RAIN") {
        Color::new(0.55, 0.60, 0.66, 1.0)
    } else if upper.contains("CLEAR4") {
        Color::new(0.62, 0.74, 0.86, 1.0)
    } else {
        Color::new(0.56, 0.71, 0.86, 1.0)
    }
}

fn parse_water_color(lvl: &HashMap<String, String>) -> Color {
    lvl.get("waterR,waterG,waterB,waterOpacity")
        .and_then(|v| {
            let parts: Vec<f32> = v
                .split(',')
                .filter_map(|s| s.trim().parse::<f32>().ok())
                .collect();
            if parts.len() >= 4 {
                let (r, g, b) = (parts[0] / 255.0, parts[1] / 255.0, parts[2] / 255.0);
                let a = (parts[3] / 255.0).clamp(0.45, 0.90);
                // Neutral white (250..255) leaves the retail water texture untinted;
                // tint it to a natural mountain-stream blue-teal over the heightmap.
                if r > 0.94 && g > 0.94 && b > 0.94 {
                    Some(Color::new(0.24, 0.48, 0.64, a))
                } else {
                    Some(Color::new(
                        r.clamp(0.0, 1.0),
                        g.clamp(0.0, 1.0),
                        b.clamp(0.0, 1.0),
                        a,
                    ))
                }
            } else {
                None
            }
        })
        .unwrap_or(Color::new(0.20, 0.44, 0.60, 0.72))
}

fn read_tex(path: &Path) -> Vec<TexRef> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .skip(3)
        .filter_map(|l| {
            let mut p = l.trim().split(',');
            let index = p.next()?.trim().parse().ok()?;
            let blend = p.next()?.trim().parse().ok()?;
            let file = p.next()?.trim().to_string();
            Some(TexRef { index, blend, file })
        })
        .collect()
}

/// Mean colour of an indexed ART texture, used as a cheap material tint.
fn average_texture(finder: &Finder, file: &str) -> Option<[f32; 3]> {
    let stem = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())?
        .to_ascii_uppercase();
    let art = std::fs::read(finder.find_in("ART", &format!("{stem}.RAW"))?).ok()?;
    let act = std::fs::read(finder.find_in("ART", &format!("{stem}.ACT"))?).ok()?;
    if act.len() < 768 {
        return None;
    }
    let mut sum = [0f64; 3];
    for &i in &art {
        let p = i as usize * 3;
        sum[0] += act[p] as f64;
        sum[1] += act[p + 1] as f64;
        sum[2] += act[p + 2] as f64;
    }
    let n = art.len().max(1) as f64 * 255.0;
    Some([
        (sum[0] / n) as f32,
        (sum[1] / n) as f32,
        (sum[2] / n) as f32,
    ])
}

/// The CLR grid stores indices into the `.TEX` list. Return the most-used one.
pub fn dominant_texture(textures: &[TexRef], colors: &[u16]) -> usize {
    if textures.is_empty() {
        return 0;
    }
    let mut counts = vec![0usize; textures.len()];
    for &c in colors {
        if let Some(n) = counts.get_mut(c as usize) {
            *n += 1;
        }
    }
    counts
        .iter()
        .enumerate()
        .max_by_key(|(_, n)| **n)
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn build_material_colors(
    finder: &Finder,
    textures: &[TexRef],
    colors: &[u16],
) -> (Vec<Color>, Vec<Color>) {
    let dom = dominant_texture(textures, colors);
    // The lists repeat the same handful of stems hundreds of times; decode
    // each mean colour once.
    let mut cache: HashMap<String, Option<[f32; 3]>> = HashMap::new();
    let avgs: Vec<Option<[f32; 3]>> = textures
        .iter()
        .map(|t| {
            let key = t.file.to_ascii_uppercase();
            *cache
                .entry(key)
                .or_insert_with(|| average_texture(finder, &t.file))
        })
        .collect();
    let base = avgs.get(dom).and_then(|a| *a).unwrap_or([0.5, 0.5, 0.5]);
    let dust_fallback = [0.72f32, 0.62, 0.46];
    let tints = avgs
        .iter()
        .map(|a| {
            let a = a.unwrap_or(base);
            let f = |v: f32, b: f32| (v / b.max(0.03)).clamp(0.65, 1.35);
            Color::new(f(a[0], base[0]), f(a[1], base[1]), f(a[2], base[2]), 1.0)
        })
        .collect();
    let ground_colors = avgs
        .into_iter()
        .map(|a| {
            let rgb = a.unwrap_or(dust_fallback);
            Color::new(
                (rgb[0] * 1.12).clamp(0.28, 0.90),
                (rgb[1] * 1.12).clamp(0.28, 0.90),
                (rgb[2] * 1.12).clamp(0.28, 0.90),
                1.0,
            )
        })
        .collect();
    (tints, ground_colors)
}

impl Track {
    /// `root` is the shared track asset root; all tracks live side by side in
    /// `DATA/`, `LEVELS/` and `WORLD/`.
    pub fn load(id: &str, root: &Path) -> Result<Self, String> {
        let base = id.to_ascii_uppercase();
        let lvl_path = root.join("LEVELS").join(format!("{base}.LVL"));
        let lvl = read_lvl(&lvl_path).unwrap_or_default();
        // The level names its data files; the id is only a fallback for
        // hand-made tracks.
        let [raw_name, clr_name, tex_name, sky_name] = declared_files(&lvl_path);
        let mut finder = Finder::new(root);
        // User-edited overlay always wins over the retail extraction.
        finder.prepend(Path::new(crate::assets::CUSTOM_TRACKS_DIR).join(&base));
        let name_or = |declared: &str, ext: &str| {
            if declared.is_empty() {
                format!("{base}.{ext}")
            } else {
                declared.to_string()
            }
        };

        // A heightmap is exactly GRID² 16-bit samples. The same-named ART
        // previews are 64 KiB and must not be mistaken for one.
        let map = finder
            .find(&name_or(&raw_name, "RAW"), GRID * GRID * 2)
            .ok_or_else(|| format!("{base}: no {GRID}×{GRID} heightmap"))?;
        let raw = std::fs::read(&map).map_err(|e| format!("{}: {e}", map.display()))?;

        let heights: Vec<u16> = raw
            .chunks_exact(2)
            .take(GRID * GRID)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();

        let colors: Vec<u16> = finder
            .find(&name_or(&clr_name, "CLR"), 0)
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| {
                b.chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect()
            })
            .unwrap_or_default();
        let shadows: Vec<u16> = finder
            .find(&format!("{base}.SDW"), 0)
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| {
                b.chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect()
            })
            .unwrap_or_default();

        let water = lvl
            .get("waterHeight,waterTideHeight,waterTidePeriod")
            .and_then(|v| v.split(',').next()?.trim().parse::<f32>().ok())
            .unwrap_or(0.0);
        let light = lvl
            .get("lightSourceVector")
            .and_then(|v| {
                let mut it = v.split(',').map(|s| s.trim().parse::<f32>());
                Some(Vec3::new(
                    it.next()?.ok()?,
                    it.next()?.ok()?,
                    it.next()?.ok()?,
                ))
            })
            .unwrap_or(Vec3::new(0.7, -0.7, 0.0));

        // Object placements are absolute world coordinates in feet. Only the
        // `wPos` records are object anchors; the rest of the section is camera
        // and course data.
        let sit = finder.find(&format!("{base}.SIT"), 0);
        let props = sit
            .as_ref()
            .map(|p| parse_sit(p.clone()))
            .unwrap_or_default();
        let spawns = sit.as_ref().map(|p| parse_spawns(p)).unwrap_or_default();
        let (gates, paths) = sit.as_ref().map(|p| parse_course(p)).unwrap_or_default();

        let textures = finder
            .find(&name_or(&tex_name, "TEX"), 0)
            .map(|p| read_tex(&p))
            .unwrap_or_default();

        let sky_texture = lvl
            .get("skyTexture")
            .cloned()
            .or_else(|| (!sky_name.is_empty()).then_some(sky_name));
        let fog_color = default_fog_color(sky_texture.as_deref());
        let water_color = parse_water_color(&lvl);

        let (tints, ground_colors) = build_material_colors(&finder, &textures, &colors);

        Ok(Self {
            id: id.to_string(),
            dir: root.to_path_buf(),
            heights,
            colors,
            shadows,
            scale: CELL_M,
            height_offset: 0.0,
            // `!waterHeight` in retail `.LVL` is stored in half-feet (0.5 * FT),
            // verified across all 25 retail levels with AI lines (e.g. ASPEN 274 = 137 ft,
            // FM109 910 = 455 ft), whereas heightmaps use 1/32 ft units.
            water_height: water * 0.5 * FT,
            water_color,
            fog_color,
            tints,
            ground_colors,
            textures,
            detail_texture: lvl.get("detailTexture").cloned(),
            sky_texture,
            light_dir: light.normalize_or_zero(),
            props,
            spawns,
            gates,
            paths,
            atlas: None,
        })
    }

    pub fn size_m(&self) -> f32 {
        (GRID - 1) as f32 * self.scale
    }

    /// Bilinear height at world coordinates. Outside the grid the edge value
    /// is extended so cars cannot fall off the world.
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let gx = (x / self.scale).clamp(0.0, (GRID - 1) as f32);
        let gz = (z / self.scale).clamp(0.0, (GRID - 1) as f32);
        let x0 = gx.floor() as usize;
        let z0 = gz.floor() as usize;
        let x1 = (x0 + 1).min(GRID - 1);
        let z1 = (z0 + 1).min(GRID - 1);
        let fx = gx - x0 as f32;
        let fz = gz - z0 as f32;
        let h = |ix: usize, iz: usize| self.heights[iz * GRID + ix] as f32 * HEIGHT_M_PER_UNIT;
        let a = h(x0, z0) * (1.0 - fx) + h(x1, z0) * fx;
        let b = h(x0, z1) * (1.0 - fx) + h(x1, z1) * fx;
        a * (1.0 - fz) + b * fz + self.height_offset
    }

    /// World-space height query alias (`offroad4x4` compatibility).
    pub fn height_at_world(&self, x: f32, z: f32) -> f32 {
        self.height_at(x, z)
    }

    /// Exact rendered terrain triangle height at `(x, z)` matching the `[0, 3, 1]`
    /// and `[1, 3, 2]` triangle split in `build_mesh_tiled`.
    pub fn triangle_height_at(&self, x: f32, z: f32) -> f32 {
        let scale = self.scale.max(0.01);
        let max_cell = (GRID - 1) as f32;
        let gx = (x / scale).clamp(0.0, max_cell);
        let gz = (z / scale).clamp(0.0, max_cell);
        let ix0 = (gx.floor() as usize).min(GRID - 2);
        let iz0 = (gz.floor() as usize).min(GRID - 2);
        let fx = (gx - ix0 as f32).clamp(0.0, 1.0);
        let fz = (gz - iz0 as f32).clamp(0.0, 1.0);
        let corner = |ix: usize, iz: usize| self.height_at(ix as f32 * scale, iz as f32 * scale);
        let h00 = corner(ix0, iz0);
        let h10 = corner(ix0 + 1, iz0);
        let h01 = corner(ix0, iz0 + 1);
        let h11 = corner(ix0 + 1, iz0 + 1);
        if fx + fz <= 1.0 {
            h00 + fx * (h10 - h00) + fz * (h01 - h00)
        } else {
            h11 + (1.0 - fx) * (h01 - h11) + (1.0 - fz) * (h10 - h11)
        }
    }

    /// World-space terrain surface normal computed by central differences.
    pub fn normal_at_world(&self, x: f32, z: f32) -> Vec3 {
        let e = (self.scale * 0.25).max(0.5);
        let dx = self.height_at_world(x + e, z) - self.height_at_world(x - e, z);
        let dz = self.height_at_world(x, z + e) - self.height_at_world(x, z - e);
        let n = vec3(-dx, 2.0 * e, -dz);
        if n.length_squared() > 1e-6 {
            n.normalize()
        } else {
            Vec3::Y
        }
    }

    /// Mean RGB ground colour at `(x, z)` from the track's `.CLR` / `.TEX` map,
    /// falling back to a neutral warm sand/dirt colour when untextured.
    pub fn ground_color_at(&self, x: f32, z: f32) -> Color {
        let gx = (x / self.scale.max(0.01)).clamp(0.0, (GRID - 1) as f32) as usize;
        let gz = (z / self.scale.max(0.01)).clamp(0.0, (GRID - 1) as f32) as usize;
        let idx = self.colors.get(gz * GRID + gx).copied().unwrap_or(0) as usize;
        self.ground_colors
            .get(idx)
            .copied()
            .unwrap_or(Color::new(0.74, 0.63, 0.46, 1.0))
    }

    /// Surface classification from the original CLR material index.
    pub fn surface_at(&self, x: f32, z: f32) -> Surface {
        if self.water_height > 0.0 && self.height_at(x, z) < self.water_height {
            return Surface::Water;
        }
        let gx = (x / self.scale).clamp(0.0, (GRID - 1) as f32) as usize;
        let gz = (z / self.scale).clamp(0.0, (GRID - 1) as f32) as usize;
        let idx = self.colors.get(gz * GRID + gx).copied().unwrap_or(0) as usize;
        match idx % 8 {
            0 => Surface::Dirt,
            1 => Surface::Gravel,
            2 => Surface::Grass,
            3 => Surface::Mud,
            4 => Surface::Sand,
            5 => Surface::Asphalt,
            6 => Surface::Snow,
            _ => Surface::Dirt,
        }
    }

    pub fn sample(&self, x: f32, z: f32) -> (f32, Surface) {
        (self.height_at(x, z), self.surface_at(x, z))
    }

    /// Direction towards the sun. `.LVL` stores the direction light travels
    /// (pointing down), so flip it when it points below the horizon.
    pub fn sun_dir(&self) -> Vec3 {
        let l = self.light_dir;
        let s = if l.y < 0.0 { -l } else { l };
        if s.length_squared() < 1e-6 || s.y < 0.05 {
            SUN.normalize()
        } else {
            s.normalize()
        }
    }

    /// Height-field normal at grid vertex `(il, iz)` (central differences).
    pub fn normal_at(&self, il: usize, iz: usize) -> Vec3 {
        let h = |x: usize, z: usize| {
            self.heights[z.min(GRID - 1) * GRID + x.min(GRID - 1)] as f32 * HEIGHT_M_PER_UNIT
        };
        let (x0, x1) = (il.saturating_sub(1), (il + 1).min(GRID - 1));
        let (z0, z1) = (iz.saturating_sub(1), (iz + 1).min(GRID - 1));
        let dx = (h(x1, iz) - h(x0, iz)) / ((x1 - x0).max(1) as f32 * self.scale);
        let dz = (h(il, z1) - h(il, z0)) / ((z1 - z0).max(1) as f32 * self.scale);
        vec3(-dx, 1.0, -dz).normalize()
    }

    /// Colour for a grid cell, from the original palette index.
    pub fn cell_color(&self, il: usize, iz: usize) -> Color {
        let idx = self.colors.get(iz * GRID + il).copied().unwrap_or(0);
        let shade = 0.82
            + (self.shadows.get(iz * GRID + il).copied().unwrap_or(0) as f32 / 65535.0).min(1.0)
                * 0.18;
        if let Some(t) = self.tints.get(idx as usize) {
            return Color::new(t.r * shade, t.g * shade, t.b * shade, 1.0);
        }
        palette(idx as usize, shade)
    }

    /// Terrain mesh for a square of chunks. UVs tile the base texture.
    pub fn build_mesh(
        &self,
        cx0: usize,
        cz0: usize,
        chunk: usize,
        tex: Option<&Texture2D>,
    ) -> Option<Mesh> {
        let step = 1usize;
        // Inclusive end so adjacent chunks share their boundary vertices;
        // otherwise a one-cell gap of sky shows through as a blue grid.
        let x1 = (cx0 + chunk).min(GRID - 1);
        let z1 = (cz0 + chunk).min(GRID - 1);
        if cx0 >= x1 || cz0 >= z1 {
            return None;
        }
        let sun = self.sun_dir();
        if let (Some(atlas), Some(tex)) = (self.atlas, tex) {
            return Some(self.build_mesh_tiled(cx0, cz0, x1, z1, atlas, tex, sun));
        }
        let mut verts: Vec<Vertex> = Vec::new();
        let mut idx = Vec::new();
        let cols = (x1 - cx0).div_ceil(step) + 1;
        let rows = (z1 - cz0).div_ceil(step) + 1;
        for (ri, iz) in (cz0..=z1).step_by(step).enumerate() {
            for (ci, il) in (cx0..=x1).step_by(step).enumerate() {
                let x = il as f32 * self.scale;
                let z = iz as f32 * self.scale;
                let c = shade_sun(self.cell_color(il, iz), self.normal_at(il, iz), sun);
                verts.push(Vertex {
                    position: vec3(x, self.height_at(x, z), z),
                    uv: vec2(il as f32, iz as f32),
                    color: [
                        (c.r * 255.0) as u8,
                        (c.g * 255.0) as u8,
                        (c.b * 255.0) as u8,
                        255,
                    ],
                    normal: Vec4::new(0.0, 1.0, 0.0, 0.0),
                });
                if ci + 1 < cols && ri + 1 < rows {
                    let i0 = (ri * cols + ci) as u16;
                    let i1 = i0 + 1;
                    let i2 = ((ri + 1) * cols + ci) as u16;
                    let i3 = i2 + 1;
                    idx.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
                }
            }
        }
        Some(Mesh {
            vertices: verts,
            indices: idx,
            texture: tex.cloned(),
        })
    }

    /// Terrain of an original track, ported from offroad4x4
    /// `build_terrain_tiled`: every quad shows the ground texture its `.CLR`
    /// cell names (the original's per-cell multi-texture ground, rock faces
    /// included), lit per vertex by the level's sun.
    fn build_mesh_tiled(
        &self,
        cx0: usize,
        cz0: usize,
        x1: usize,
        z1: usize,
        atlas: Atlas,
        tex: &Texture2D,
        sun: Vec3,
    ) -> Mesh {
        let lit = |il: usize, iz: usize| {
            let ix = il.min(GRID - 1);
            let iz_c = iz.min(GRID - 1);
            let n = self.normal_at(ix, iz_c);
            let sdw = self
                .shadows
                .get(iz_c * GRID + ix)
                .map(|&v| 0.84 + (v as f32 / 65535.0).clamp(0.0, 1.0) * 0.16)
                .unwrap_or(1.0);
            let slope = (1.0 - n.y).clamp(0.0, 1.0);
            let rock = ((slope - 0.14) * 1.8).clamp(0.0, 0.35);
            let base = Color::new(
                (1.0 - rock * 0.10) * sdw,
                (1.0 - rock * 0.13) * sdw,
                (1.0 - rock * 0.16) * sdw,
                1.0,
            );
            let c = shade_sun(base, n, sun);
            [
                (c.r * 255.0) as u8,
                (c.g * 255.0) as u8,
                (c.b * 255.0) as u8,
                255,
            ]
        };
        let t = atlas.tile as f32;
        let ntex = self.textures.len().max(1);
        let mut verts: Vec<Vertex> = Vec::with_capacity((x1 - cx0) * (z1 - cz0) * 4);
        let mut idx: Vec<u16> = Vec::with_capacity((x1 - cx0) * (z1 - cz0) * 6);
        for iz in cz0..z1 {
            for il in cx0..x1 {
                let tile =
                    (self.colors.get(iz * GRID + il).copied().unwrap_or(0) as usize).min(ntex - 1);
                let (col, row) = ((tile % atlas.cols) as f32, (tile / atlas.cols) as f32);
                let base = verts.len() as u16;
                for (dx, dz) in [(0usize, 0usize), (1, 0), (1, 1), (0, 1)] {
                    let (vx, vz) = (il + dx, iz + dz);
                    let x = vx as f32 * self.scale;
                    let z = vz as f32 * self.scale;
                    let u = (col * t + 0.5 + dx as f32 * (t - 1.0)) / atlas.w;
                    let v = (row * t + 0.5 + dz as f32 * (t - 1.0)) / atlas.h;
                    verts.push(Vertex {
                        position: vec3(x, self.height_at(x, z), z),
                        uv: vec2(u, v),
                        color: lit(vx, vz),
                        normal: Vec4::new(0.0, 1.0, 0.0, 0.0),
                    });
                }
                idx.extend_from_slice(&[base, base + 3, base + 1, base + 1, base + 3, base + 2]);
            }
        }
        Mesh {
            vertices: verts,
            indices: idx,
            texture: Some(tex.clone()),
        }
    }

    pub fn prop_count(&self) -> usize {
        self.props.len()
    }
}

fn palette(idx: usize, shade: f32) -> Color {
    const BASE: [[f32; 3]; 8] = [
        [0.42, 0.32, 0.20], // dirt
        [0.52, 0.50, 0.46], // gravel
        [0.27, 0.45, 0.19], // grass
        [0.30, 0.24, 0.15], // mud
        [0.72, 0.63, 0.43], // sand
        [0.25, 0.25, 0.27], // asphalt
        [0.82, 0.85, 0.90], // snow
        [0.36, 0.40, 0.24],
    ];
    let [r, g, b] = BASE[idx % BASE.len()];
    // Tint toward white so the albedo texture keeps its own colour.
    let mix = |c: f32| (0.78 + c * 0.3) * shade;
    Color::new(mix(r), mix(g), mix(b), 1.0)
}

/// `.SIT` object placements. The `*** Boxes ***` section places props
/// (`wPos` in feet, `wOrient` Euler angles, `model` reference); the later
/// `*** Course ***` and vehicle sections are not scenery.
/// `*** Vehicles ***` grid: each block names a truck and carries its world
/// position (feet) and orientation vector.
fn parse_spawns(path: &Path) -> Vec<Spawn> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    let start = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case("*** Vehicles ***"));
    let Some(mut i) = start.map(|i| i + 2) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut truck = String::new();
    while i < lines.len() {
        let l = lines[i].trim();
        if l.starts_with("***") {
            break;
        }
        if l.eq_ignore_ascii_case("staticName") || l.eq_ignore_ascii_case("dynamicName") {
            truck = lines.get(i + 1).unwrap_or(&"").trim().to_string();
            i += 2;
            continue;
        }
        if l.eq_ignore_ascii_case("wPos") {
            let mut it = lines
                .get(i + 1)
                .unwrap_or(&"")
                .split(',')
                .map(|v| v.trim().parse::<f32>());
            let x = it.next().and_then(|v| v.ok()).unwrap_or(0.0);
            let y = it.next().and_then(|v| v.ok()).unwrap_or(0.0);
            let z = it.next().and_then(|v| v.ok()).unwrap_or(0.0);
            let pos = Vec3::new(x, y, z) * FT;
            out.push(Spawn {
                truck: String::new(),
                pos,
                heading: 0.0,
            });
            i += 2;
            continue;
        }
        if l.eq_ignore_ascii_case("wOrient") {
            let mut it = lines
                .get(i + 1)
                .unwrap_or(&"")
                .split(',')
                .map(|v| v.trim().parse::<f32>());
            let x = it.next().and_then(|v| v.ok()).unwrap_or(0.0);
            let z = it.next().and_then(|v| v.ok()).unwrap_or(-1.0);
            let heading = (-x).atan2(-z);
            if let Some(s) = out.last_mut() {
                s.heading = heading;
                s.truck = truck.clone();
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    out
}

/// `*** Course ***` gates and `*** Paths ***` splines from a `.SIT`.
///
/// Both sections are plain text; the gate list is `c1Count,course_direction`
/// followed by one `******* <index>` block per gate with `cstart`/`cend` world
/// lines (feet) plus a `cTrackWidth`, and the path list is `pathCount` followed
/// by point blocks. Everything is converted to metres here. Sections after
/// `*** Stadium ***`/`*** Backdrop ***` are ignored.
fn parse_course(path: &Path) -> (Vec<Gate>, Vec<Vec<Vec3>>) {
    #[derive(PartialEq)]
    enum Mode {
        None,
        Course,
        Paths,
    }
    let Ok(bytes) = std::fs::read(path) else {
        return (Vec::new(), Vec::new());
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .collect();
    let point = |i: usize| -> Option<Vec3> {
        let mut it = lines.get(i)?.split(',').map(|v| v.trim().parse::<f32>());
        let p = vec3(it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
        (p.is_finite()).then_some(p * FT)
    };

    let mut mode = Mode::None;
    let mut gates: Vec<Gate> = Vec::new();
    let mut paths: Vec<Vec<Vec3>> = Vec::new();
    let mut start: Option<Vec3> = None;
    let mut end: Option<Vec3> = None;
    let mut kind = 1u8;
    let mut speed = 0.0f32;
    let mut width_m = 0.0f32;
    let mut cur: Vec<Vec3> = Vec::new();
    // `c1Count` is authoritative: some levels (DEVIL) repeat the whole gate list
    // two more times in the same section, once per class.
    let mut declared = usize::MAX;
    // `cTrackWidth` is the last record of a gate block, so the gate is only
    // emitted once it is known (or when the next block starts).
    let flush = |start: &mut Option<Vec3>,
                 end: &mut Option<Vec3>,
                 gates: &mut Vec<Gate>,
                 kind: u8,
                 speed: f32,
                 width_m: f32| {
        if let (Some(a), Some(b)) = (start.take(), end.take()) {
            if (a - b).length_squared() > 0.25 {
                gates.push(Gate {
                    start: a,
                    end: b,
                    kind,
                    speed,
                    width_m,
                });
            }
        }
    };

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("*** Course") {
            mode = Mode::Course;
        } else if line.starts_with("#*** Paths") || line.starts_with("*** Paths") {
            flush(&mut start, &mut end, &mut gates, kind, speed, width_m);
            mode = Mode::Paths;
        } else if line.starts_with("*** ") {
            flush(&mut start, &mut end, &mut gates, kind, speed, width_m);
            if !cur.is_empty() {
                paths.push(std::mem::take(&mut cur));
            }
            mode = Mode::None;
        } else if mode == Mode::Course {
            if line == "c1Count,course_direction" {
                declared = lines
                    .get(i + 1)
                    .and_then(|v| v.split(',').next())
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(usize::MAX);
                i += 1;
            }
            match line {
                "cstart" => {
                    start = point(i + 1);
                    i += 1;
                }
                "cend" => {
                    end = point(i + 1);
                    i += 1;
                }
                "ctype,cspeed_type" => {
                    kind = lines
                        .get(i + 1)
                        .and_then(|v| v.split(',').next())
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(1);
                    i += 1;
                }
                "cdec_point,cspeed,lastentry" => {
                    speed = lines
                        .get(i + 1)
                        .and_then(|v| v.split(',').nth(1))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0.0);
                    i += 1;
                }
                "&cSpeedLimit,cTrackWidth" => {
                    width_m = lines
                        .get(i + 1)
                        .and_then(|v| v.split(',').nth(1))
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .unwrap_or(0.0)
                        * FT;
                    flush(&mut start, &mut end, &mut gates, kind, speed, width_m);
                    i += 1;
                }
                _ => {
                    if line.starts_with('*') {
                        flush(&mut start, &mut end, &mut gates, kind, speed, width_m);
                    }
                }
            }
        } else if mode == Mode::Paths {
            if line.starts_with("Path ") {
                if !cur.is_empty() {
                    paths.push(std::mem::take(&mut cur));
                }
            } else if line == "wPos" {
                if let Some(p) = point(i + 1) {
                    cur.push(p);
                }
                i += 1;
            }
        }
        i += 1;
    }
    if !cur.is_empty() {
        paths.push(cur);
    }
    gates.truncate(declared);
    paths.retain(|p| p.len() >= 8);
    (gates, paths)
}

/// `.SIT` revision 7 (4x4 Evo 2). `*** Boxes ***` here is a typed block list:
/// a `boxTypeList`, a `boxCount`, a numbered `Box name list`, and then
/// `// Box data list` with one block per object:
///
/// ```text
/// { 1 CCollide
///     ""    // name
///     "SR3STUMP.SMF"    // staticName
///     4.14567,1.01728,4    // size
///     4296.57,145.145,3908.04    // wPos
///     0.053881,6.25893,0    // wOrient
/// ```
///
/// Each field is a value line followed by its `// tag` comment, so the tags
/// identify fields without depending on their order. Positions are feet.
fn parse_sit_v7(text: &str) -> Vec<Prop> {
    /// Box types that place solid scenery. The rest (`CCheckpoint`,
    /// `CVehicle`, `CNonCollide*`, `CTriggerBox`, `CTreasure`,
    /// `CFlyingObject`, `CTrain`) are markers or pass-through objects.
    const SOLID: [&str; 7] = [
        "CBox",
        "CCollide",
        "CCollisionBox",
        "CGroundBox",
        "CLinkableBox",
        "CTree",
        "CTreeCollisionBox",
    ];

    let mut out: Vec<Prop> = Vec::new();
    let mut in_data = false;
    let mut kind = String::new();
    let mut pos: Option<Vec3> = None;
    let mut yaw = 0.0f32;
    let mut model = String::new();
    let mut mass = 0.0f32;

    for line in text.lines().map(|l| l.trim_end_matches('\r').trim()) {
        if !in_data {
            in_data = line == "// Box data list";
            continue;
        }
        if let Some(rest) = line.strip_prefix('{') {
            // `{ 3 CCheckpoint`: the ordinal is not the type.
            kind = rest.split_whitespace().nth(1).unwrap_or("").to_string();
            pos = None;
            yaw = 0.0;
            model.clear();
            mass = 0.0;
            continue;
        }
        if line.starts_with('}') {
            if SOLID.contains(&kind.as_str()) {
                if let (Some(p), false) = (pos, model.is_empty()) {
                    out.push(Prop {
                        pos: p,
                        yaw,
                        model: model.clone(),
                        moving: mass > 0.0,
                        ..Default::default()
                    });
                }
            }
            kind.clear();
            continue;
        }
        let Some((value, tag)) = line.split_once("//") else {
            continue;
        };
        let value = value.trim();
        match tag.trim() {
            "staticName" => {
                model = value
                    .trim_matches('"')
                    .trim_end_matches(".smf")
                    .trim_end_matches(".SMF")
                    .to_ascii_uppercase();
            }
            "wPos" => {
                let parts: Vec<f32> = value
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                if parts.len() >= 3 {
                    let p = vec3(parts[0] * FT, parts[1] * FT, parts[2] * FT);
                    if p.is_finite() && p.x.abs() < 9000.0 && p.z.abs() < 9000.0 {
                        pos = Some(p);
                    }
                }
            }
            "wOrient" => {
                let parts: Vec<f32> = value
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                if parts.len() >= 3 {
                    yaw = parts[2];
                }
            }
            "mass" => mass = value.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    out
}

/// Retail track name stored in a `.SIT` (`!Race Track Name` followed by the
/// display string). Used by the Quick Race list; file stems stay the ids.
pub fn sit_track_name(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines().map(str::trim);
    while let Some(line) = lines.next() {
        if line.eq_ignore_ascii_case("!Race Track Name") {
            return lines.find(|l| !l.is_empty()).map(str::to_string);
        }
    }
    None
}

pub(crate) fn parse_sit(path: PathBuf) -> Vec<Prop> {
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    if !text.starts_with("version") {
        return Vec::new();
    }
    if text.contains("// Box data list") {
        return parse_sit_v7(&text);
    }
    let mut out = Vec::new();
    let mut in_boxes = false;
    let mut pos: Option<Vec3> = None;
    let mut yaw = 0.0f32;
    let mut open = false;
    let mut lines = text.lines().map(|l| l.trim());
    while let Some(line) = lines.next() {
        if line.starts_with("*** Boxes") {
            in_boxes = true;
            continue;
        }
        if line.starts_with("*** Course") || line.starts_with("*** Vehicles") {
            in_boxes = false;
        }
        if !in_boxes {
            continue;
        }
        match line {
            "wPos" => {
                // A new box: later fields no longer belong to the last prop.
                open = false;
                let Some(vals) = lines.next() else { break };
                let parts: Vec<f32> = vals
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                if parts.len() >= 3 {
                    let p = vec3(parts[0] * FT, parts[1] * FT, parts[2] * FT);
                    if p.is_finite() && p.x.abs() < 9000.0 && p.z.abs() < 9000.0 {
                        pos = Some(p);
                    } else {
                        pos = None;
                    }
                }
            }
            "wOrient" => {
                let Some(vals) = lines.next() else { break };
                let parts: Vec<f32> = vals
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                if parts.len() >= 3 {
                    yaw = parts[2];
                }
            }
            "model" => {
                let Some(name) = lines.next() else { break };
                if let Some(p) = pos.take() {
                    let stem = name
                        .trim()
                        .trim_end_matches(".smf")
                        .trim_end_matches(".SMF")
                        .to_ascii_uppercase();
                    if !stem.is_empty() {
                        out.push(Prop {
                            pos: p,
                            yaw,
                            model: stem,
                            ..Default::default()
                        });
                        open = true;
                    }
                }
            }
            // `bvel` and `!type,flags` follow `model` inside the same box.
            "bvel" => {
                let Some(vals) = lines.next() else { break };
                let moving = vals
                    .split(',')
                    .filter_map(|s| s.trim().parse::<f32>().ok())
                    .any(|v| v != 0.0);
                if let (true, Some(prop)) = (open, out.last_mut()) {
                    prop.moving = moving;
                }
            }
            "!type,flags" => {
                let Some(vals) = lines.next() else { break };
                let kind = vals.split(',').next().and_then(|s| s.trim().parse().ok());
                if let (true, Some(prop), Some(kind)) = (open, out.last_mut(), kind) {
                    prop.kind = kind;
                }
            }
            _ => {}
        }
    }
    out
}

/// Cached terrain chunks. Rebuilding 33x33 vertex meshes every frame dominated
/// the frame budget, so meshes are kept until the track changes.
#[derive(Default)]
pub struct ChunkCache {
    track: String,
    meshes: HashMap<(usize, usize), Mesh>,
}

/// Chunks per axis in the 256-cell grid (256 / 32).
const CHUNK_AXIS: usize = 8;

impl ChunkCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue every chunk within `radius` of the player, nearest first and
    /// biased towards the direction of travel, then build at most `budget`
    /// missing meshes. Prefetching keeps generation ahead of the camera
    /// instead of building nine meshes in the frame a boundary is crossed.
    pub fn update(
        &mut self,
        track: &Track,
        cx: usize,
        cz: usize,
        chunk: usize,
        tex: Option<&Texture2D>,
        radius: i32,
        budget: usize,
    ) {
        if self.track != track.id {
            self.track = track.id.clone();
            self.meshes.clear();
        }
        // The chunk under the wheels is never dropped and is rebuilt ahead of
        // the budget, so a teleport or a bad frame cannot leave the player
        // floating over the void.
        if self.mesh(cx, cz).is_none() {
            if let Some(m) = track.build_mesh(cx * chunk, cz * chunk, chunk, tex) {
                self.meshes.insert((cx, cz), m);
            }
        }
        let mut wanted: Vec<((usize, usize), i32)> = Vec::new();
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let nx = cx as i32 + dx;
                let nz = cz as i32 + dz;
                if nx < 0 || nz < 0 || nx as usize >= CHUNK_AXIS || nz as usize >= CHUNK_AXIS {
                    continue;
                }
                let key = (nx as usize, nz as usize);
                if self.meshes.contains_key(&key) {
                    continue;
                }
                wanted.push((key, dx * dx + dz * dz));
            }
        }
        wanted.sort_by_key(|(_, d)| *d);
        for ((kx, kz), _) in wanted.into_iter().take(budget) {
            if let Some(mesh) = track.build_mesh(kx * chunk, kz * chunk, chunk, tex) {
                self.meshes.insert((kx, kz), mesh);
            }
        }
    }

    /// Chunk meshes are keyed by chunk index; the caller draws what exists.
    pub fn mesh(&self, cx: usize, cz: usize) -> Option<&Mesh> {
        self.meshes.get(&(cx, cz))
    }

    pub fn has(&self, cx: usize, cz: usize) -> bool {
        self.meshes.contains_key(&(cx, cz))
    }

    pub fn len(&self) -> usize {
        self.meshes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_track() -> Track {
        Track {
            id: "flat".into(),
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

    #[test]
    fn height_is_bilinear_and_bounded() {
        let mut t = flat_track();
        t.heights[10 * GRID + 10] = 10_000;
        assert!(t.height_at(10.0, 10.0) > 0.0);
        assert_eq!(t.height_at(-500.0, -500.0), 0.0);
        assert!(t.height_at(10_000.0, 10_000.0).is_finite());
    }

    #[test]
    fn water_overrides_surface() {
        let mut t = flat_track();
        t.water_height = 1.0;
        t.heights.iter_mut().for_each(|h| *h = 0);
        assert_eq!(t.surface_at(4.0, 4.0), Surface::Water);
    }

    /// Fording rivers: the tyre contact is the riverbed, not the water plane.
    /// A truck dropped over a flooded cell must settle on `height_at`, never
    /// "drive" on top of `water_height`.
    #[test]
    fn water_sample_is_the_riverbed_not_the_surface() {
        let mut t = flat_track();
        t.water_height = 5.0;
        t.heights.iter_mut().for_each(|h| *h = 0); // riverbed at y = 0
        let (ground_y, surface) = t.sample(40.0, 40.0);
        assert_eq!(surface, Surface::Water);
        assert_eq!(ground_y, 0.0, "tyre contact must be the riverbed");
        assert!(ground_y < t.water_height);
    }

    #[test]
    fn chunk_cache_builds_nearest_first_and_is_bounded() {
        let t = flat_track();
        let mut c = ChunkCache::new();
        // The centre chunk ignores the budget; the rest obey it.
        c.update(&t, 3, 3, 32, None, 2, 3);
        assert_eq!(c.len(), 4);
        assert!(c.mesh(3, 3).is_some(), "player chunk must build first");
        c.update(&t, 3, 3, 32, None, 2, 100);
        assert_eq!(c.len(), 25);
        // Edge chunks clamp to the 8x8 grid.
        c.update(&t, 0, 0, 32, None, 2, 100);
        assert!(c.mesh(0, 0).is_some());
        assert!(c.mesh(7, 7).is_none());
    }

    #[test]
    fn heightmap_matches_recorded_ai_elevation() {
        let root = if Path::new("assets/extracted").exists() {
            Path::new("assets/extracted/track/track")
        } else {
            Path::new("../assets/extracted/track/track")
        };
        if !root.exists() {
            return;
        }
        let t = Track::load("SHRTAIR", root).unwrap();
        assert!((t.scale - CELL_M).abs() < 1e-3);
        // First recorded point in AI_32SHRTAIR.TXT is (2842.16, 200.97,
        // 7019.94) feet; the heightmap must agree with its elevation.
        let (x, z) = (2842.16 * FT, 7019.94 * FT);
        let h = t.height_at(x, z);
        assert!((h - 200.97 * FT).abs() < 2.0, "height {h} vs AI");
        assert!(t.prop_count() > 10);
    }

    #[test]
    fn mesh_builds() {
        let t = flat_track();
        let m = t.build_mesh(0, 0, 32, None).unwrap();
        assert!(m.vertices.len() > 100);
        assert!(!m.indices.is_empty());
    }
}

#[cfg(test)]
mod real_track_tests {
    use super::*;

    fn root() -> &'static Path {
        if std::path::Path::new("assets/extracted").exists() {
            Path::new("assets/extracted/track/track")
        } else {
            Path::new("../assets/extracted/track/track")
        }
    }

    /// `*** Course ***` gates and `*** Paths ***` splines are the level's real
    /// course (audit finding #3). Parsed from the `.SIT` text, positions in feet
    /// converted once at load.
    #[test]
    fn sit_course_gates_and_paths_parse() {
        let mut sit = String::from(
            "version\r\nT.LVL\r\n*** Boxes ***\r\n*** Course ***\r\n\
c1Count,course_direction\r\n1,0\r\n***************** 1\r\nctype,cspeed_type\r\n1,0\r\n\
cstart\r\n0,0,0\r\ncend\r\n100,0,0\r\ncdec_point,cspeed,lastentry\r\n30,45,0\r\n\
&cSpeedLimit,cTrackWidth\r\n0,16\r\n#*** Paths ***\r\npathCount\r\n1\r\n\
Path 1 of 1\r\npointCount\r\n9\r\n",
        );
        for i in 0..9 {
            sit.push_str(&format!("Point {} of 9\r\nwPos\r\n{i},0,0\r\n", i + 1));
        }
        sit.push_str("*** Stadium ***\r\n");
        let dir = std::env::temp_dir().join("evox-course-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("T.SIT");
        std::fs::write(&path, sit).unwrap();
        let (gates, paths) = parse_course(&path);
        assert_eq!(gates.len(), 1);
        assert_eq!(gates[0].kind, 1);
        assert!((gates[0].width_m - 16.0 * FT).abs() < 1e-4);
        // Positions are feet: 100 ft = 100 * FT metres.
        assert!((gates[0].end.x - 100.0 * FT).abs() < 1e-4);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].len(), 9);
        assert!((paths[0][8].x - 8.0 * FT).abs() < 1e-4);
    }

    /// DEVIL ships 14 `*** Course ***` gates; ALASKA1 ships none and carries
    /// its course as `*** Paths ***` splines instead.
    #[test]
    fn retail_levels_expose_gates_or_paths() {
        require_assets!();
        let devil = parse_course(&root().join("WORLD/DEVIL.SIT"));
        // DEVIL repeats its 14-gate list once per class; `c1Count` trims it.
        assert_eq!(devil.0.len(), 14, "DEVIL gates {}", devil.0.len());
        let alaska = parse_course(&root().join("WORLD/ALASKA1.SIT"));
        assert!(alaska.0.is_empty(), "ALASKA1 has no gate list");
        assert!(
            alaska.1.iter().any(|p| p.len() > 20),
            "ALASKA1 must expose its path spline"
        );
    }

    /// Every shipped level must resolve a real heightmap through its `.LVL`,
    /// including the RTD-only ones and `MATT`, whose map sits in the UI data
    /// folder next to a same-named 64 KiB preview.
    #[test]
    fn every_retail_level_loads() {
        require_assets!();
        let mut ids: Vec<String> = std::fs::read_dir(root().join("LEVELS"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                e.path()
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            })
            .collect();
        ids.sort();
        assert!(ids.len() >= 16, "levels missing: {ids:?}");
        for id in &ids {
            let t = Track::load(id, root()).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(t.heights.len(), GRID * GRID, "{id}");
            let lo = t.heights.iter().min().copied().unwrap();
            let hi = t.heights.iter().max().copied().unwrap();
            assert!(hi - lo > 200, "{id} is flat ({lo}..{hi})");
            assert!(!t.textures.is_empty(), "{id} has no texture list");
        }
    }

    #[test]
    fn builds_chunk_from_retail_track() {
        require_assets!();
        let t = Track::load("ZONA", root()).unwrap();
        for (cx, cz) in [(0, 0), (6, 6), (160, 160), (224, 224)] {
            let m = t.build_mesh(cx, cz, 32, None);
            println!(
                "{cx},{cz} -> {:?}",
                m.as_ref().map(|m| (m.vertices.len(), m.indices.len()))
            );
            assert!(m.is_some());
        }
    }

    #[test]
    fn sit_race_track_name_is_localized() {
        require_assets!();
        let p = root().join("WORLD/BAJBEACH.SIT");
        assert_eq!(sit_track_name(&p).as_deref(), Some("Baja Beach"));
    }

    #[test]
    fn lvl_parses_sky_and_half_foot_water_height() {
        require_assets!();
        let hill = Track::load("THEHILL", root()).unwrap();
        assert_eq!(hill.sky_texture.as_deref(), Some("CLEAR1.RAW"));

        let aspen = Track::load("ASPEN", root()).unwrap();
        assert_eq!(aspen.sky_texture.as_deref(), Some("CLEAR4.RAW"));
        // ASPEN.LVL: !waterHeight = 274 half-feet = 137 ft = 41.7576 m.
        assert!((aspen.water_height - 274.0 * 0.5 * FT).abs() < 0.05);

        let alaska = Track::load("ALASKA1", root()).unwrap();
        assert_eq!(alaska.sky_texture.as_deref(), Some("cloudy2.raw"));
        // @waterR,G,B,A = 10,21,35,255
        assert!(alaska.water_color.r < 0.1 && alaska.water_color.b > alaska.water_color.r);
    }
}
