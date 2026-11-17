//! Extracted asset access: paletted ART → RGBA textures, fonts, audio paths.
//!
//! The original ART is 8-bit indexed (`.RAW` + `.ACT` palette + optional
//! `.OPA` alpha). Textures are decoded once into RGBA and cached by stem.
//! Missing art falls back to a generated placeholder so a bare checkout still
//! runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use macroquad::prelude::*;
use macroquad::text::{load_ttf_font, Font};

use crate::terrain::Track;

pub const ART_DIR: &str = "assets/extracted/texture/ui/ART";
pub const SOUND_DIR: &str = "assets/extracted/audio/sfx/SOUND";
/// User-edited tracks live here, never in `assets/extracted`. The asset
/// resolution checks this first so the retail tree stays read-only.
pub const CUSTOM_TRACKS_DIR: &str = "assets/custom_tracks";
/// Palette artwork shared by vehicle, prop and track textures.
pub const TEX_ART_DIR: &str = "assets/extracted/texture/texture/ART";
pub const TEX_PALETTE_DIR: &str = "assets/extracted/texture/palette/ART";
/// The extractor split ground art by level: most levels' bitmaps are in
/// `texture/texture/ART`, but the `MATT` textures and the UI share
/// `texture/ui/ART`. Level art must check both.
const ALT_ART_DIR: &str = "assets/extracted/texture/ui/ART";

/// Evo 2 truck art converted by `tools/evo2_trucks.py` (same RAW/ACT/OPA
/// triplet as Evo 1, searched after it).
pub const EVO2_TEX_ART_DIR: &str = "assets/extracted/evo2/texture/ART";
pub const EVO2_PALETTE_DIR: &str = "assets/extracted/evo2/palette/ART";
pub const EVO2_OPACITY_DIR: &str = "assets/extracted/evo2/opacity/ART";
pub const TEX_OPACITY_DIR: &str = "assets/extracted/texture/opacity/ART";

use std::sync::OnceLock;

static ART_DIR_CACHE: OnceLock<HashMap<&'static str, HashMap<String, PathBuf>>> = OnceLock::new();

/// Read `stem.ext` from the overlay tree `assets/custom_tracks/*/`. The
/// overlay is organised as one directory per track, so a modified texture for
/// any level is found without knowing which level asked for it.
fn custom_art_bytes(stem: &str, ext: &str) -> Option<Vec<u8>> {
    let name_upper = format!("{stem}.{ext}").to_ascii_uppercase();
    let rd = std::fs::read_dir(CUSTOM_TRACKS_DIR).ok()?;
    for track in rd.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()) {
        let mut p = track.path().join(&name_upper);
        if !p.exists() {
            p = track.path().join(name_upper.to_ascii_lowercase());
        }
        if p.exists() {
            if let Ok(b) = std::fs::read(p) {
                return Some(b);
            }
        }
    }
    None
}

fn get_art_dir_cache() -> &'static HashMap<&'static str, HashMap<String, PathBuf>> {
    ART_DIR_CACHE.get_or_init(|| {
        let all_dirs = [
            TEX_ART_DIR,
            ALT_ART_DIR,
            TEX_PALETTE_DIR,
            EVO2_PALETTE_DIR,
            TEX_OPACITY_DIR,
            EVO2_OPACITY_DIR,
            EVO2_TEX_ART_DIR,
        ];
        let mut cache = HashMap::new();
        for dir in all_dirs {
            let mut file_map = HashMap::new();
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let path = e.path();
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        file_map.insert(name.to_ascii_uppercase(), path);
                    }
                }
            }
            cache.insert(dir, file_map);
        }
        cache
    })
}

/// Read `stem.ext` from the art directories (Evo 1 ground/vehicle art, the
/// UI art, then the imported Evo 2 truck art). User-edited copies under
/// `assets/custom_tracks/*/` always win over the retail extraction.
fn art_bytes(stem: &str, ext: &str) -> Option<Vec<u8>> {
    if let Some(b) = custom_art_bytes(stem, ext) {
        return Some(b);
    }
    let name_upper = format!("{stem}.{ext}").to_ascii_uppercase();
    let dirs: &[&str] = match ext {
        "ACT" => &[TEX_ART_DIR, ALT_ART_DIR, TEX_PALETTE_DIR, EVO2_PALETTE_DIR],
        "OPA" => &[TEX_OPACITY_DIR, EVO2_OPACITY_DIR, ALT_ART_DIR, TEX_ART_DIR],
        _ => &[TEX_ART_DIR, ALT_ART_DIR, EVO2_TEX_ART_DIR],
    };
    let cache = get_art_dir_cache();
    for &dir in dirs {
        if let Some(map) = cache.get(dir) {
            if let Some(path) = map.get(&name_upper) {
                if let Ok(b) = std::fs::read(path) {
                    return Some(b);
                }
            }
        }
    }
    None
}

pub struct Assets {
    cache: HashMap<String, Texture2D>,
    /// Decoded vehicle textures for per-vertex glass classification.
    rgba_cache: HashMap<String, (u32, u32, Vec<u8>)>,
    sky_horizons: HashMap<String, Color>,
    pub font: Font,
    pub trucks_dir: PathBuf,
    pub tracks_dir: PathBuf,
    pub ai_dir: PathBuf,
    pub parts_txt: PathBuf,
    pub parts_loc: PathBuf,
    pub sky: Option<Texture2D>,
    pub missing: Vec<String>,
}

fn dims(len: usize) -> Option<(u32, u32)> {
    let side = (len as f64).sqrt() as usize;
    if side * side == len {
        return Some((side as u32, side as u32));
    }
    match len {
        307_200 => Some((640, 480)),
        76_096 => Some((232, 328)),
        57_024 => Some((264, 216)),
        38_704 => Some((248, 156)),
        29_040 => Some((220, 132)),
        1_536 => Some((32, 48)),
        _ => None,
    }
}

/// Decode indexed pixels through a 256-entry VGA palette.
///
/// Palette index 0 is **not** transparent here. The retail art uses index 0 as
/// solid black as often as a key colour (pickup bed liners, bumper rubber,
/// window frames), so keying it blindly punches holes through solid meshes.
/// Only an explicit `.OPA` mask or a `TRANSP*` sub-mesh flag may cut pixels out
/// (`prop_texture_cutout`).
fn palette_to_rgba(pixels: &[u8], act: &[u8], opa: Option<&[u8]>, size: (u32, u32)) -> Vec<u8> {
    palette_to_rgba_keyed(pixels, act, opa, size, false)
}

/// Decode indexed pixels through a 256-entry VGA palette, optionally keying
/// index 0 out when no `.OPA` mask exists.
/// Handles sky/art textures such as `CLOUDY2.RAW` whose byte indices are shifted
/// above zero while all non-zero `.ACT` entries sit at a lower contiguous block.
fn palette_to_rgba_keyed(
    pixels: &[u8],
    act: &[u8],
    opa: Option<&[u8]>,
    (w, h): (u32, u32),
    key_zero: bool,
) -> Vec<u8> {
    let has_nonzero_pixel = pixels.iter().any(|&p| {
        let c = p as usize * 3;
        c + 2 < act.len() && (act[c] | act[c + 1] | act[c + 2]) != 0
    });
    let remap_offset: Option<(u8, usize)> = if !has_nonzero_pixel && !pixels.is_empty() {
        let first_nz = (0..(act.len() / 3).min(256))
            .find(|&i| (act[i * 3] | act[i * 3 + 1] | act[i * 3 + 2]) != 0);
        let min_px = pixels.iter().copied().min();
        match (min_px, first_nz) {
            (Some(lo), Some(dst)) => Some((lo, dst)),
            _ => None,
        }
    } else {
        None
    };

    let mut out = vec![0u8; pixels.len() * 4];
    for (i, p) in pixels.iter().enumerate() {
        let idx = match remap_offset {
            Some((lo, dst)) => (dst + (*p - lo) as usize).min((act.len() / 3).saturating_sub(1)),
            None => (*p as usize).min((act.len() / 3).saturating_sub(1)),
        };
        let c = idx * 3;
        out[i * 4] = act[c];
        out[i * 4 + 1] = act[c + 1];
        out[i * 4 + 2] = act[c + 2];
        out[i * 4 + 3] = match opa.and_then(|o| o.get(i).copied()) {
            Some(a) => a,
            None if key_zero && *p == 0 => 0,
            None => 255,
        };
    }
    let _ = (w, h);
    out
}

/// Average RGB across the lower horizon band (bottom eighth of the sky texture).
fn horizon_color_from_rgba(rgba: &[u8], w: u32, h: u32) -> Option<Color> {
    if w == 0 || h == 0 || rgba.len() < (w * h * 4) as usize {
        return None;
    }
    let y0 = (h * 7) / 8;
    let mut r = 0u64;
    let mut g = 0u64;
    let mut b = 0u64;
    let mut n = 0u64;
    for y in y0..h {
        for x in 0..w {
            let idx = ((y * w + x) * 4) as usize;
            r += rgba[idx] as u64;
            g += rgba[idx + 1] as u64;
            b += rgba[idx + 2] as u64;
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    let col = Color::new(
        (r as f32 / (n as f32 * 255.0)).clamp(0.0, 1.0),
        (g as f32 / (n as f32 * 255.0)).clamp(0.0, 1.0),
        (b as f32 / (n as f32 * 255.0)).clamp(0.0, 1.0),
        1.0,
    );
    (col.r + col.g + col.b > 0.04).then_some(col)
}

fn placeholder(name: &str) -> Texture2D {
    let mut img = Image::gen_image_color(8, 8, Color::new(0.35, 0.35, 0.4, 1.0));
    let seed = name
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    for y in 0..8 {
        for x in 0..8 {
            let v = ((seed >> ((x + y) % 24)) & 0x3) as f32 / 6.0;
            img.set_pixel(x, y, Color::new(0.3 + v, 0.32 + v, 0.4 + v, 1.0));
        }
    }
    Texture2D::from_image(&img)
}

impl Assets {
    pub async fn load() -> Self {
        let mut a = Self {
            cache: HashMap::new(),
            rgba_cache: HashMap::new(),
            sky_horizons: HashMap::new(),
            font: load_ttf_font("assets/vehicles/ui.ttf")
                .await
                .unwrap_or_default(),
            trucks_dir: PathBuf::from("assets/extracted/model/vehicle/TRUCK"),
            tracks_dir: PathBuf::from("assets/extracted/track/track"),
            ai_dir: PathBuf::from("assets/extracted/data/data/AI"),
            parts_txt: PathBuf::from("assets/extracted/data/ui/PARTS/PARTS.TXT"),
            parts_loc: PathBuf::from("assets/extracted/data/ui/PARTS/PARTS.LOC"),
            sky: None,
            missing: Vec::new(),
        };
        // (The `EDBTN*` names are the track editor's 20x20 RLE `.PBM` toolbar
        // icons, not menu art; they are no longer preloaded.)
        a.preload(&[
            "GARAGE1",
            "SHOWROOM",
            "CURSOR",
            "CKBOX",
            "LCD_EVO",
            "NAVUP",
            "NAVDOWN",
            "NAVLEFT",
            "NAVRIGHT",
            "NAVUD",
            "NAVLR",
            "ARROW",
            "ARROWG",
            "ARROWR",
            "STRTGRN",
            "STRTRED",
            "BUTTON3",
            "TRUCK0SH",
            "TRUCK1SH",
            "TRUCK2SH",
            "TRUCK3SH",
            "TRUCK4SH",
            "TRUCK5SH",
            "TRUCK6SH",
            "TRUCK7SH",
            "4X4FONT",
            "EVOLOGO4",
            "TRILOGO",
            "FLAG2",
            "FLAG3",
            "NEEDLE",
            "ODOM",
            "TICK",
            "PROFILE_FILE_ICON_1",
            "PROFILE_FILE_ICON_2",
            "PROFILE_FILE_ICON_3",
            "VMU_GARAGE",
            "VMU_PROFILE",
            "VMU_REPLAY",
            "VMU_TRACK",
            "VMU_MASTER",
            "VMU_HOTLAP",
            "MATT",
            "MGRASS1",
            "DEFSKY",
            "DET7",
            "LOGO256",
            "LOGO480",
            "LOADING1",
            "LOADING2",
            "LOADING3",
            "ICONA",
            "ICONB",
            "ICONC",
            "ICOND",
        ])
        .await;
        a.sky = a.tex_opt("DEFSKY").await;
        let _ = a.tex_opaque("MGRASS1").await;
        a
    }

    async fn preload(&mut self, names: &[&str]) {
        for n in names {
            let _ = self.tex(n).await;
        }
    }

    pub async fn tex_opt(&mut self, name: &str) -> Option<Texture2D> {
        if let Some(t) = self.cache.get(name) {
            return Some(t.clone());
        }
        let base = Path::new(ART_DIR).join(name);
        let Ok(raw) = load_file(&format!("{}.RAW", base.display())).await else {
            return None;
        };
        let Ok(act) = load_file(&format!("{}.ACT", base.display())).await else {
            return None;
        };
        if act.len() < 768 {
            return None;
        }
        let Some(size) = dims(raw.len()) else {
            return None;
        };
        let opa = load_file(&format!("{}.OPA", base.display())).await.ok();
        let rgba = palette_to_rgba(&raw, &act, opa.as_deref(), size);
        let tex = Texture2D::from_rgba8(size.0 as u16, size.1 as u16, &rgba);
        tex.set_filter(FilterMode::Linear);
        self.cache.insert(name.to_string(), tex.clone());
        Some(tex)
    }

    /// Opaque decode from an explicit file stem + ART directory (track art).
    pub fn tex_opaque_fs(&mut self, key: &str, raw: &Path) -> bool {
        if self.cache.contains_key(key) {
            return true;
        }
        let Ok(pixels) = std::fs::read(raw) else {
            return false;
        };
        let Ok(act) = std::fs::read(raw.with_extension("ACT")) else {
            return false;
        };
        let Some(size) = dims(pixels.len()) else {
            return false;
        };
        if act.len() < 768 {
            return false;
        }
        let rgba = palette_to_rgba(&pixels, &act, None, size);
        let tex = Texture2D::from_rgba8(size.0 as u16, size.1 as u16, &rgba);
        tex.set_filter(FilterMode::Linear);
        self.cache.insert(key.to_string(), tex);
        true
    }

    /// Same ART but with the alpha mask ignored, for surfaces such as ground
    /// detail where the OPA is a cutout mask rather than transparency.
    pub async fn tex_opaque(&mut self, name: &str) -> Option<Texture2D> {
        let key = format!("{name}#opaque");
        if let Some(t) = self.cache.get(&key) {
            return Some(t.clone());
        }
        let base = Path::new(ART_DIR).join(name);
        let raw = load_file(&format!("{}.RAW", base.display())).await.ok()?;
        let act = load_file(&format!("{}.ACT", base.display())).await.ok()?;
        let size = dims(raw.len())?;
        if act.len() < 768 {
            return None;
        }
        let rgba = palette_to_rgba(&raw, &act, None, size);
        let tex = Texture2D::from_rgba8(size.0 as u16, size.1 as u16, &rgba);
        tex.set_filter(FilterMode::Linear);
        self.cache.insert(key, tex.clone());
        Some(tex)
    }

    pub async fn tex(&mut self, name: &str) -> Texture2D {
        match self.tex_opt(name).await {
            Some(t) => t,
            None => {
                if !self.missing.iter().any(|m| m == name) {
                    self.missing.push(name.to_string());
                }
                // Missing art is recorded (self-test / debug HUD) and never
                // cached under the real name: `tex_now` stays `None`, so
                // callers take their authentic fallback instead of drawing a
                // generated square.
                placeholder(name)
            }
        }
    }

    pub fn tex_now(&self, name: &str) -> Option<&Texture2D> {
        self.cache.get(name)
    }

    /// Decode a game texture (`.RAW` + palette `.ACT` + optional `.OPA`
    /// opacity) from the shared art directories. Synchronous because these are
    /// small and already on disk.
    fn load_art(&mut self, key: &str, stem: &str, alpha: bool) -> Option<Texture2D> {
        let stem = Path::new(stem)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(stem)
            .to_ascii_uppercase();
        let pixels = art_bytes(&stem, "RAW")?;
        let act = art_bytes(&stem, "ACT")?;
        if act.len() < 768 {
            return None;
        }
        let size = dims(pixels.len())?;
        let opa = if alpha { art_bytes(&stem, "OPA") } else { None };
        let rgba = palette_to_rgba(&pixels, &act, opa.as_deref(), size);
        let tex = Texture2D::from_rgba8(size.0 as u16, size.1 as u16, &rgba);
        tex.set_filter(FilterMode::Linear);
        self.cache.insert(key.to_string(), tex.clone());
        Some(tex)
    }

    /// Load the interface font atlas (`4X4FONT`) with its opacity mask and
    /// measure every glyph from the mask. Returns the texture plus advance
    /// widths for codes 32..127.
    pub fn load_font(&mut self) -> Option<Texture2D> {
        self.load_ui_art("FONT", "4X4FONT", true);
        self.cache.get("FONT").cloned()
    }

    /// Interface art (`texture/ui/ART`), whose palette and opacity masks sit
    /// beside the pixels rather than in the shared texture libraries.
    fn load_ui_art(&mut self, key: &str, stem: &str, alpha: bool) {
        let base = Path::new(ART_DIR).join(stem);
        let Ok(pixels) = std::fs::read(base.with_extension("RAW")) else {
            return;
        };
        let Ok(act) = std::fs::read(base.with_extension("ACT")) else {
            return;
        };
        if act.len() < 768 {
            return;
        }
        let Some(size) = dims(pixels.len()) else {
            return;
        };
        let opa = alpha
            .then(|| std::fs::read(base.with_extension("OPA")).ok())
            .flatten();
        let rgba = palette_to_rgba(&pixels, &act, opa.as_deref(), size);
        let tex = Texture2D::from_rgba8(size.0 as u16, size.1 as u16, &rgba);
        tex.set_filter(FilterMode::Linear);
        self.cache.insert(key.to_string(), tex);
    }

    /// Track preview picture (`texture/ui/ART/<TRACK>.RAW`, 256×256): the
    /// same stem as the level, loaded once into `PREVIEW#<TRACK>`. `MATT`'s
    /// 64 KiB file here is the preview, not its heightmap.
    ///
    /// When a level has no 64 KiB preview the retail UI substitutes
    /// `NOTRACK.RAW`: `FUN_005463b0` in retail `4x4.exe` builds
    /// `art\<level>.raw`, loads it, and keeps the name only if the loaded
    /// image's size field is `0x10000` (256×256); otherwise it passes
    /// `notrack.raw` to the texture setter. Evo 2 ships per-track art only
    /// for the 15 Evo 1 tracks, so the other 45 show this placeholder.
    pub fn track_preview(&mut self, track: &str) -> Option<Texture2D> {
        let stem = track.to_ascii_uppercase();
        let key = format!("PREVIEW#{stem}");
        if !self.cache.contains_key(&key) {
            self.load_ui_art(&key, &stem, false);
        }
        if let Some(tex) = self.cache.get(&key).cloned() {
            return Some(tex);
        }
        const PLACEHOLDER: &str = "PREVIEW#NOTRACK";
        if !self.cache.contains_key(PLACEHOLDER) {
            self.load_ui_art(PLACEHOLDER, "NOTRACK", false);
        }
        eprintln!("track preview {stem}.RAW missing; using retail NOTRACK.RAW");
        self.cache.get(PLACEHOLDER).cloned()
    }

    /// True when `track` has its own 64 KiB preview, i.e. [`Self::track_preview`]
    /// is not the retail `NOTRACK.RAW` placeholder. The UI uses this to keep
    /// the missing file visible instead of silently showing the placeholder.
    pub fn has_own_track_preview(&self, track: &str) -> bool {
        let stem = track.to_ascii_uppercase();
        std::fs::metadata(Path::new(ART_DIR).join(format!("{stem}.RAW")))
            .map(|m| m.len() == 65_536)
            .unwrap_or(false)
    }

    /// The gauge needle is a colour-keyed sprite: the art ships without an
    /// opacity mask, so key the background ramp sampled around the blade.
    pub fn load_needle(&mut self) -> Option<Texture2D> {
        let pixels = std::fs::read(Path::new(ART_DIR).join("NEEDLE.RAW")).ok()?;
        let act = std::fs::read(Path::new(ART_DIR).join("NEEDLE.ACT")).ok()?;
        if act.len() < 768 {
            return None;
        }
        let size = dims(pixels.len())?;
        let (w, h) = size;
        let mut bg = Vec::new();
        let mut sample = |x: u32, y: u32| {
            let i = pixels[(y * w + x) as usize] as usize;
            let c = [act[i * 3], act[i * 3 + 1], act[i * 3 + 2]];
            if !bg.contains(&c) {
                bg.push(c);
            }
        };
        for x in 0..w {
            for y in 0..4 {
                sample(x, y);
            }
        }
        for y in 0..h * 5 / 8 {
            for x in 0..12 {
                sample(x, y);
                sample(w - 1 - x, y);
            }
        }
        let mut rgba = Vec::with_capacity(pixels.len() * 4);
        for &i in &pixels {
            let i = i as usize;
            let (r, g, b) = (act[i * 3], act[i * 3 + 1], act[i * 3 + 2]);
            let dist = bg
                .iter()
                .map(|c| {
                    (c[0] as i32 - r as i32).abs()
                        + (c[1] as i32 - g as i32).abs()
                        + (c[2] as i32 - b as i32).abs()
                })
                .min()
                .unwrap_or(255);
            let a = ((dist - 6) * 14).clamp(0, 255) as u8;
            rgba.extend_from_slice(&[r, g, b, a]);
        }
        let tex = Texture2D::from_rgba8(w as u16, h as u16, &rgba);
        tex.set_filter(FilterMode::Linear);
        self.cache.insert("NEEDLE#KEY".into(), tex.clone());
        Some(tex)
    }

    /// Per-vertex flag: does this UV land on a dark *glass* pixel of the mesh's
    /// own texture? Retail shells named `glasse`/`glassi` carry the window
    /// panes and the body-coloured pillars/roof in one mesh, so tinting the
    /// whole mesh dark turns the cab roof black. Sampling the texture keeps the
    /// roof body colour and only the panes smoked.
    pub fn glass_pixels(&mut self, smf: &crate::smf::Smf) -> Vec<Vec<bool>> {
        smf.meshes
            .iter()
            .map(|m| {
                let Some((w, h, rgba)) = self.model_rgba(&m.texture) else {
                    return vec![false; m.positions.len()];
                };
                m.uvs
                    .iter()
                    .map(|uv| {
                        let x = ((uv.x.rem_euclid(1.0)) * w as f32) as u32;
                        let y = ((uv.y.rem_euclid(1.0)) * h as f32) as u32;
                        let o = ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize;
                        let l =
                            (rgba[o] as u32 * 3 + rgba[o + 1] as u32 * 6 + rgba[o + 2] as u32) / 10;
                        l < 44
                    })
                    .collect()
            })
            .collect()
    }

    /// Decoded RGBA of a vehicle texture on the CPU (for per-vertex sampling).
    fn model_rgba(&mut self, stem: &str) -> Option<(u32, u32, Vec<u8>)> {
        let name = Path::new(stem)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(stem)
            .to_ascii_uppercase();
        let key = format!("VEHRAW#{name}");
        if let Some((w, h, px)) = self.rgba_cache.get(&key) {
            return Some((*w, *h, px.clone()));
        }
        let pixels = art_bytes(&name, "RAW")?;
        let act = art_bytes(&name, "ACT")?;
        if act.len() < 768 {
            return None;
        }
        let (w, h) = dims(pixels.len())?;
        let rgba = palette_to_rgba(&pixels, &act, None, (w, h));
        self.rgba_cache.insert(key, (w, h, rgba.clone()));
        Some((w, h, rgba))
    }

    /// Texture referenced by a vehicle or wheel SMF mesh (e.g. `BLAZZR2.RAW`).
    /// Decoded 100 % opaque (`use_opa = false`) because on vehicles `.OPA` is
    /// the paint-shift mask (`paint.rs`), never a transparency cutout. Bed
    /// liners, bumpers and window frames that sit on palette index 0 therefore
    /// stay solid.
    pub fn model_texture(&mut self, stem: &str) -> Option<Texture2D> {
        let name = Path::new(stem)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(stem)
            .to_ascii_uppercase();
        let key = format!("VEH#{name}");
        if let Some(t) = self.cache.get(&key) {
            return Some(t.clone());
        }
        self.load_art(&key, &name, false)
    }

    /// Texture referenced by a scenery prop SMF mesh (e.g. `KM3SEMI.RAW` or `KM3CHK.RAW`).
    ///
    /// Alpha comes from the companion `.OPA` when the art ships one; without a
    /// mask every pixel stays opaque, including palette index 0. Props whose
    /// foliage or banner is drawn with a `TRANSP*` sub-mesh use
    /// [`Self::prop_texture_cutout`] instead, which additionally keys index 0.
    pub fn prop_texture(&mut self, stem: &str) -> Option<Texture2D> {
        let (name, key) = self.prop_key(stem, "");
        if let Some(t) = self.cache.get(&key) {
            return Some(t.clone());
        }
        let pixels = art_bytes(&name, "RAW")?;
        let act = art_bytes(&name, "ACT")?;
        if act.len() < 768 {
            return None;
        }
        let size = dims(pixels.len())?;
        let opa = art_bytes(&name, "OPA");
        let rgba = palette_to_rgba(&pixels, &act, opa.as_deref(), size);
        let tex = Texture2D::from_rgba8(size.0 as u16, size.1 as u16, &rgba);
        tex.set_filter(FilterMode::Linear);
        self.cache.insert(key, tex.clone());
        Some(tex)
    }

    /// Same as [`Self::prop_texture`] but for a sub-mesh the `.SMF` header flags
    /// `TRANSP` / `TRANSPI` / `TRANSPE`: pixels of palette index 0 are cut out
    /// when the art has no `.OPA` mask of its own.
    pub fn prop_texture_cutout(&mut self, stem: &str) -> Option<Texture2D> {
        let (name, key) = self.prop_key(stem, "#cut");
        if let Some(t) = self.cache.get(&key) {
            return Some(t.clone());
        }
        let pixels = art_bytes(&name, "RAW")?;
        let act = art_bytes(&name, "ACT")?;
        if act.len() < 768 {
            return None;
        }
        let size = dims(pixels.len())?;
        let opa = art_bytes(&name, "OPA");
        let key_zero = opa.is_none();
        let rgba = palette_to_rgba_keyed(&pixels, &act, opa.as_deref(), size, key_zero);
        let tex = Texture2D::from_rgba8(size.0 as u16, size.1 as u16, &rgba);
        tex.set_filter(FilterMode::Linear);
        self.cache.insert(key, tex.clone());
        Some(tex)
    }

    /// Normalised texture stem and cache key for a prop texture variant.
    fn prop_key(&self, stem: &str, suffix: &str) -> (String, String) {
        let name = Path::new(stem)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(stem)
            .to_ascii_uppercase();
        let key = format!("PROP#{name}{suffix}");
        (name, key)
    }

    /// A truck body texture with a factory paint applied (`paint.rs`, the
    /// retail `colorAdjustTexture`). The second channel (`.OPA`) is both the
    /// paint mask and the alpha, as in the original.
    pub fn painted_texture(
        &mut self,
        stem: &str,
        paint: &crate::paint::Paint,
    ) -> Option<Texture2D> {
        let name = Path::new(stem)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(stem)
            .to_ascii_uppercase();
        let key = format!("VEH#{name}#{}", paint.key());
        if let Some(t) = self.cache.get(&key) {
            return Some(t.clone());
        }
        let pixels = art_bytes(&name, "RAW")?;
        let act = art_bytes(&name, "ACT")?;
        if act.len() < 768 {
            return None;
        }
        let (w, h) = dims(pixels.len())?;
        let mask = art_bytes(&name, "OPA").unwrap_or_else(|| vec![255; pixels.len()]);
        let rgba = crate::paint::recolor(&pixels, &act, &mask, paint);
        let tex = Texture2D::from_rgba8(w as u16, h as u16, &rgba);
        tex.set_filter(FilterMode::Linear);
        self.cache.insert(key, tex.clone());
        Some(tex)
    }

    /// Bind the dominant terrain albedo (`TRK#BASE`) for this track. The
    /// `.CLR` grid indexes the `.TEX` list, so the most-used index is the
    /// ground material the level is built around.
    pub fn track_albedo(&mut self, track: &Track) -> bool {
        let dom = crate::terrain::dominant_texture(&track.textures, &track.colors);
        if let Some(t) = track.textures.get(dom) {
            if self.load_art("TRK#BASE", &t.file, false).is_some() {
                return true;
            }
        }
        for t in &track.textures {
            if self.load_art("TRK#BASE", &t.file, false).is_some() {
                return true;
            }
        }
        false
    }

    /// Pack every ground texture of the track's `.TEX` list into one atlas
    /// (`TRK#ATLAS#<id>`) so each terrain cell can show its own material, as
    /// the original renders it. Missing textures become the mean colour of
    /// the ones present.
    pub fn track_atlas(&mut self, track: &mut Track) -> bool {
        const TILE: usize = 64;
        let n = track.textures.len();
        if n == 0 {
            return false;
        }
        let key = format!("TRK#ATLAS#{}", track.id);
        let cols = (n as f32).sqrt().ceil() as usize;
        let rows = n.div_ceil(cols);
        let (w, h) = (cols * TILE, rows * TILE);
        if self.cache.contains_key(&key) {
            track.atlas = Some(crate::terrain::Atlas {
                cols,
                tile: TILE,
                w: w as f32,
                h: h as f32,
            });
            return true;
        }
        let mut rgba = vec![0u8; w * h * 4];
        let mut filled = vec![false; n];
        let mut sum = [0u64; 3];
        let mut count = 0u64;
        for (i, t) in track.textures.iter().enumerate() {
            let stem = Path::new(&t.file)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&t.file)
                .to_ascii_uppercase();
            let (Some(px), Some(act)) = (art_bytes(&stem, "RAW"), art_bytes(&stem, "ACT")) else {
                continue;
            };
            let Some((sw, sh)) = dims(px.len()) else {
                continue;
            };
            if act.len() < 768 || sw == 0 || sh == 0 {
                continue;
            }
            let (ox, oy) = ((i % cols) * TILE, (i / cols) * TILE);
            for y in 0..TILE {
                for x in 0..TILE {
                    let sx = x * sw as usize / TILE;
                    let sy = y * sh as usize / TILE;
                    let c = px[sy * sw as usize + sx] as usize * 3;
                    let o = ((oy + y) * w + ox + x) * 4;
                    rgba[o..o + 4].copy_from_slice(&[act[c], act[c + 1], act[c + 2], 255]);
                    sum[0] += act[c] as u64;
                    sum[1] += act[c + 1] as u64;
                    sum[2] += act[c + 2] as u64;
                    count += 1;
                }
            }
            filled[i] = true;
        }
        if count == 0 {
            return false;
        }
        let mean = [
            (sum[0] / count) as u8,
            (sum[1] / count) as u8,
            (sum[2] / count) as u8,
            255,
        ];
        for (i, ok) in filled.iter().enumerate() {
            if *ok {
                continue;
            }
            let (ox, oy) = ((i % cols) * TILE, (i / cols) * TILE);
            for y in 0..TILE {
                for x in 0..TILE {
                    let o = ((oy + y) * w + ox + x) * 4;
                    rgba[o..o + 4].copy_from_slice(&mean);
                }
            }
        }
        let tex = Texture2D::from_rgba8(w as u16, h as u16, &rgba);
        tex.set_filter(FilterMode::Linear);
        self.cache.insert(key, tex);
        track.atlas = Some(crate::terrain::Atlas {
            cols,
            tile: TILE,
            w: w as f32,
            h: h as f32,
        });
        true
    }

    /// Terrain texture to bind for `track`: its atlas when packed, else the
    /// single base albedo.
    pub fn terrain_texture(&self, track: &Track) -> Option<Texture2D> {
        if track.atlas.is_some() {
            if let Some(t) = self.cache.get(&format!("TRK#ATLAS#{}", track.id)) {
                return Some(t.clone());
            }
        }
        self.cache.get("TRK#BASE").cloned()
    }

    /// Load `track`'s `.LVL` sky texture (e.g. `CLEAR1.RAW`, `cloudy2.raw`),
    /// falling back to `DEFSKY`, and refine `track.fog_color` from the lower
    /// horizon band so distant terrain dissolves cleanly into the sky.
    pub fn track_sky(&mut self, track: &mut Track) -> Option<Texture2D> {
        let stem = track
            .sky_texture
            .as_deref()
            .and_then(|s| Path::new(s).file_stem()?.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("DEFSKY")
            .to_ascii_uppercase();
        let key = format!("SKY#{stem}");
        if let Some(tex) = self.cache.get(&key) {
            if let Some(&horizon) = self.sky_horizons.get(&stem) {
                track.fog_color = horizon;
            }
            return Some(tex.clone());
        }
        let load_stem = |name: &str| -> Option<(u32, u32, Vec<u8>)> {
            let pixels = art_bytes(name, "RAW")?;
            let act = art_bytes(name, "ACT")?;
            if act.len() < 768 {
                return None;
            }
            let size = dims(pixels.len())?;
            let rgba = palette_to_rgba(&pixels, &act, None, size);
            Some((size.0, size.1, rgba))
        };
        let Some((w, h, rgba)) = load_stem(&stem).or_else(|| load_stem("DEFSKY")) else {
            return self.sky.clone();
        };
        if let Some(horizon) = horizon_color_from_rgba(&rgba, w, h) {
            track.fog_color = horizon;
            self.sky_horizons.insert(stem, horizon);
        }
        let tex = Texture2D::from_rgba8(w as u16, h as u16, &rgba);
        tex.set_filter(FilterMode::Linear);
        self.cache.insert(key, tex.clone());
        Some(tex)
    }

    /// Every level the install ships, keyed by the `.LVL` id. The level file
    /// is authoritative: it names the heightmap, colour and texture list, so a
    /// stray `.RTD` cannot invent a track.
    pub fn list_tracks(&self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.tracks_dir.join("LEVELS")) {
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.extension().map(|x| x.eq_ignore_ascii_case("lvl")) != Some(true) {
                    continue;
                }
                if let Some(n) = p.file_stem().and_then(|n| n.to_str()) {
                    out.push((n.to_string(), self.tracks_dir.clone()));
                }
            }
        }
        // Overlay tracks: a directory `assets/custom_tracks/<NAME>/` with its
        // own `<NAME>.LVL` overrides (or adds) a level.
        if let Ok(rd) = std::fs::read_dir(CUSTOM_TRACKS_DIR) {
            for e in rd.filter_map(|e| e.ok()) {
                let dir = e.path();
                if !dir.is_dir() {
                    continue;
                }
                let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if dir.join(format!("{name}.LVL")).exists() {
                    let mut seen = false;
                    for (n, _) in out.iter_mut() {
                        if n.eq_ignore_ascii_case(name) {
                            seen = true;
                            break;
                        }
                    }
                    if !seen {
                        out.push((name.to_string(), self.tracks_dir.clone()));
                    }
                }
            }
        }
        out.sort_by(|a: &(String, PathBuf), b| a.0.cmp(&b.0));
        out
    }

    /// Retail display name from `WORLD/<ID>.SIT` (`!Race Track Name`),
    /// e.g. `BAJBEACH` -> `Baja Beach`.
    pub fn track_display_name(&self, id: &str) -> Option<String> {
        let path = self
            .tracks_dir
            .join("WORLD")
            .join(format!("{}.SIT", id.to_ascii_uppercase()));
        crate::terrain::sit_track_name(&path)
    }

    pub fn load_track(&self, id: &str) -> Result<Track, String> {
        if !self
            .list_tracks()
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(id))
        {
            return Err(format!("unknown track {id}"));
        }
        Track::load(id, &self.tracks_dir)
    }

    pub fn ai_lines(&self, track: &str) -> Vec<crate::ai::RacingLine> {
        read_ai_lines(&self.ai_dir, track)
    }
}

/// Recorded AI lines for `track` under the `data/data/AI/<class>` tree,
/// sorted by class. Falls back to the numeric-suffix-stripped stem
/// (e.g. `OBSPARK1` -> `OBSPARK`) when no direct track match exists.
pub fn read_ai_lines(ai_dir: &Path, track: &str) -> Vec<crate::ai::RacingLine> {
    let scan = |needle: &str| -> Vec<crate::ai::RacingLine> {
        let mut out = Vec::new();
        let needle_upper = needle.to_ascii_uppercase();
        if let Ok(rd) = std::fs::read_dir(ai_dir) {
            for class in rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
            {
                let Ok(files) = std::fs::read_dir(&class) else {
                    continue;
                };
                for f in files.filter_map(|e| e.ok()).map(|e| e.path()) {
                    let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.to_ascii_uppercase().contains(&needle_upper) {
                        continue;
                    }
                    if let Ok(text) = std::fs::read_to_string(&f) {
                        if let Ok(l) = crate::ai::RacingLine::parse(&text) {
                            out.push(l);
                        }
                    }
                }
            }
        }
        out
    };
    let mut out = scan(track);
    if out.is_empty() {
        let base = track.trim_end_matches(|c: char| c.is_ascii_digit());
        if base.len() >= 3 && !base.eq_ignore_ascii_case(track) {
            out = scan(base);
        }
    }
    out.sort_by_key(|l| l.class);
    out
}

#[cfg(test)]
mod tests {
    /// Retail shows `texture/ui/ART/<LEVEL>.RAW` for the selected level and
    /// substitutes `NOTRACK.RAW` when that 64 KiB load fails: `FUN_005463b0`
    /// in retail `4x4.exe` keeps the level name only when the loaded image's
    /// size field is `0x10000`, else it passes `notrack.raw`. Evo 2 ships
    /// per-track art only for the 15 Evo 1 tracks; the 45 Evo 2-exclusive
    /// levels legitimately use the placeholder.
    #[test]
    fn every_level_has_a_preview_or_the_retail_placeholder() {
        require_assets!();
        let root = ["assets/extracted", "../assets/extracted"]
            .into_iter()
            .map(Path::new)
            .find(|p| p.exists())
            .expect("extracted assets");
        let art = root.join("texture/ui/ART");
        let placeholder = art.join("NOTRACK.RAW");
        assert_eq!(
            std::fs::metadata(&placeholder)
                .map(|m| m.len())
                .unwrap_or(0),
            65_536,
            "retail NOTRACK.RAW placeholder missing or not 256x256"
        );

        let mut own = 0usize;
        let mut missing = std::collections::BTreeSet::new();
        for e in std::fs::read_dir(root.join("track/track/LEVELS"))
            .unwrap()
            .flatten()
        {
            let stem = e
                .path()
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .to_ascii_uppercase();
            let raw = art.join(format!("{stem}.RAW"));
            let len = std::fs::metadata(&raw).map(|m| m.len()).unwrap_or(0);
            if len == 0 {
                missing.insert(stem);
            } else {
                assert_eq!(
                    len,
                    65_536,
                    "{stem}: preview {} is not 256x256",
                    raw.display()
                );
                own += 1;
            }
        }

        assert_eq!(own, 15, "Evo 1 tracks with their own retail preview");
        let expected: std::collections::BTreeSet<&str> = [
            "ALASKA1", "ALASKA2", "ALASKA3", "ALASKA4", "ALASKA5", "AUTUMN", "BAJBEACH", "CASTLE",
            "COSTRICA", "DESERT", "DVALLEY", "EGYPT", "ELNORTE1", "ELNORTE2", "ELNORTE3",
            "ELNORTE4", "ELNORTE5", "MEDI", "OBSPARK1", "OBSPARK2", "OBSPARK3", "OBSPARK4",
            "OBSPARK5", "PEAK", "QUARRY", "RIVER1", "RIVER2", "RIVER3", "RIVER4", "RIVER5",
            "SKULL1", "SKULL2", "SKULL3", "SKULL4", "SKULL5", "THAI", "TIBET", "TRIBAJA",
            "VULTURE", "WASH1", "WASH2", "WASH3", "WASH4", "WASH5", "YUKON",
        ]
        .into_iter()
        .collect();
        let got: std::collections::BTreeSet<&str> = missing.iter().map(String::as_str).collect();
        assert_eq!(got, expected, "levels without their own preview art");
    }

    use super::*;

    #[test]
    fn dimensions_inferred() {
        assert_eq!(dims(65536), Some((256, 256)));
        assert_eq!(dims(307200), Some((640, 480)));
        assert_eq!(dims(8), None);
        assert_eq!(dims(1024), Some((32, 32)));
    }

    #[test]
    fn palette_decode_sets_alpha() {
        let act: Vec<u8> = (0..768).map(|i| (i % 256) as u8).collect();
        let opa = vec![128u8; 2];
        let out = palette_to_rgba(&[1, 2], &act, Some(&opa), (2, 1));
        assert_eq!(&out[0..4], &[3, 4, 5, 128]);
        assert_eq!(out[7], 128);
    }

    /// Audit finding #8: palette index 0 used to be a key colour everywhere,
    /// which punched holes through pickup beds, bumper rubber and window frames
    /// that legitimately use solid black. Only a `.OPA` mask or a `TRANSP*`
    /// sub-mesh may cut a pixel out now.
    #[test]
    fn index_zero_is_opaque_without_an_explicit_mask() {
        let mut act = vec![0u8; 768];
        act[3..6].copy_from_slice(&[10, 20, 30]);
        let pixels = [0u8, 1];

        let solid = palette_to_rgba(&pixels, &act, None, (2, 1));
        assert_eq!(
            &solid[0..3],
            &[0, 0, 0],
            "index 0 is black, not a colour key"
        );
        assert_eq!(solid[3], 255, "index 0 must stay opaque");
        assert_eq!(solid[7], 255);

        // A cutout prop sub-mesh without art of its own may still key index 0.
        let cut = palette_to_rgba_keyed(&pixels, &act, None, (2, 1), true);
        assert_eq!(cut[3], 0);
        assert_eq!(cut[7], 255);

        // An explicit `.OPA` always wins over both defaults.
        let masked = palette_to_rgba(&pixels, &act, Some(&[0, 255]), (2, 1));
        assert_eq!(masked[3], 0);
        assert_eq!(masked[7], 255);
    }

    #[test]
    fn cloudy2_sky_remaps_shifted_palette() {
        require_assets!();
        let mut act = vec![0u8; 768];
        // Synthetic CLOUDY2 pattern: colors at 192..=195 while pixels are 230..=233.
        for i in 0..4 {
            act[(192 + i) * 3] = 100 + i as u8 * 10;
            act[(192 + i) * 3 + 1] = 140;
            act[(192 + i) * 3 + 2] = 190;
        }
        let rgba = palette_to_rgba(&[230, 231, 232, 233], &act, None, (2, 2));
        assert_eq!(&rgba[0..3], &[100, 140, 190]);
        assert_eq!(&rgba[4..7], &[110, 140, 190]);

        let root = ["assets/extracted", "../assets/extracted"]
            .into_iter()
            .map(Path::new)
            .find(|p| p.exists())
            .expect("extracted assets");
        let raw = std::fs::read(root.join("texture/texture/ART/CLOUDY2.RAW")).unwrap();
        let act = std::fs::read(root.join("texture/palette/ART/CLOUDY2.ACT")).unwrap();
        let size = dims(raw.len()).unwrap();
        let decoded = palette_to_rgba(&raw, &act, None, size);
        assert!(horizon_color_from_rgba(&decoded, size.0, size.1).is_some());
    }

    #[test]
    fn ai_lines_numeric_suffix_fallback() {
        require_assets!();
        let root = ["assets/extracted", "../assets/extracted"]
            .into_iter()
            .map(Path::new)
            .find(|p| p.exists())
            .expect("extracted assets");
        let lines = read_ai_lines(&root.join("data/data/AI"), "OBSPARK1");
        assert!(
            !lines.is_empty(),
            "OBSPARK1 should inherit OBSPARK AI lines"
        );
    }
}
