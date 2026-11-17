//! Native conversion of the interface TIFFs into the PNGs `classic::ui2`
//! loads. The intake already extracts `texture/ui/ART/*.TIF`; this step turns
//! the menu backgrounds, logo, buttons and cursor into `<data>/assets/ui2/`.
//!
//! Conversion is staged in `assets/.ui2.tmp-<pid>` and renamed into place so a
//! failed run never leaves a partially converted menu.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use ::tiff::decoder::{Decoder, DecodingResult};
use ::tiff::tags::Tag;

#[derive(Default)]
pub struct Stats {
    pub converted: usize,
    pub skipped: usize,
    pub failed: usize,
}

/// Stems `classic::ui2` actually asks for. Converting every UI TIFF would also
/// emit venue cards and preview art that the front end never loads.
fn wanted(stem: &str) -> bool {
    let s = stem.to_ascii_uppercase();
    s.starts_with("BG_")
        || matches!(
            s.as_str(),
            "BUTTON3" | "CURSOR" | "EVO2LOGO" | "LOGO" | "UI_CASH" | "UI_CONTROL" | "UI_TRUCK"
        )
}

fn from_palette(t: super::tiff::PaletteTiff) -> (u32, u32, Vec<u8>) {
    let mut rgba = Vec::with_capacity(t.indices.len() * 4);
    for (i, &idx) in t.indices.iter().enumerate() {
        let p = idx as usize * 3;
        rgba.push(*t.palette.get(p).unwrap_or(&0));
        rgba.push(*t.palette.get(p + 1).unwrap_or(&0));
        rgba.push(*t.palette.get(p + 2).unwrap_or(&0));
        rgba.push(t.mask.get(i).copied().unwrap_or(255));
    }
    (t.width, t.height, rgba)
}

/// Decode one TIFF to RGBA8. Palette TIFFs keep their mask; RGB cards become
/// opaque. The crate handles the common cases, while the mask-bearing
/// two-sample palette TIFFs fall back to the intake decoder.
fn try_crate(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    if let Ok(file) = File::open(path) {
        let mut d =
            Decoder::new(BufReader::new(file)).map_err(|e| format!("{}: {e}", path.display()))?;
        let (w, h) = d
            .dimensions()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let color = d
            .colortype()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let image = d
            .read_image()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let bytes: Vec<u8> = match image {
            DecodingResult::U8(v) => v,
            DecodingResult::U16(v) => v.iter().map(|x| (x >> 8) as u8).collect(),
            _ => Vec::new(),
        };
        let n = (w as usize) * (h as usize);
        let rgba = match color {
            ::tiff::ColorType::RGB(8) if bytes.len() >= n * 3 => {
                let mut out = Vec::with_capacity(n * 4);
                for px in bytes.chunks_exact(3).take(n) {
                    out.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                out
            }
            ::tiff::ColorType::RGBA(8) if bytes.len() >= n * 4 => {
                bytes.chunks_exact(4).take(n).flatten().copied().collect()
            }
            ::tiff::ColorType::Gray(8) if bytes.len() >= n => {
                let mut out = Vec::with_capacity(n * 4);
                for &g in bytes.iter().take(n) {
                    out.extend_from_slice(&[g, g, g, 255]);
                }
                out
            }
            ::tiff::ColorType::Palette(_) if bytes.len() >= n => {
                let cm = d
                    .get_tag_u16_vec(Tag::ColorMap)
                    .map_err(|e| format!("{}: colour map: {e}", path.display()))?;
                if cm.len() < 768 {
                    return Err(format!("{}: short colour map", path.display()));
                }
                let mut out = Vec::with_capacity(n * 4);
                for &idx in bytes.iter().take(n) {
                    let i = idx as usize;
                    out.extend_from_slice(&[
                        (cm[i] >> 8) as u8,
                        (cm[256 + i] >> 8) as u8,
                        (cm[512 + i] >> 8) as u8,
                        255,
                    ]);
                }
                out
            }
            _ => Vec::new(),
        };
        if rgba.len() == n * 4 && n > 0 {
            return Ok((w, h, rgba));
        }
    }
    Err(format!("{}: unsupported TIFF layout", path.display()))
}

fn decode_rgba(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    if let Ok(v) = try_crate(path) {
        return Ok(v);
    }
    // Two-sample palette TIFFs (index + mask) land here.
    super::tiff::decode(path)
        .map(from_palette)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = ::png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(::png::ColorType::Rgba);
    enc.set_depth(::png::BitDepth::Eight);
    let mut writer = enc
        .write_header()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| format!("{}: {e}", path.display()))
}

enum Input {
    Tiff(PathBuf),
    Raw {
        stem: String,
        raw: PathBuf,
        act: PathBuf,
        opa: Option<PathBuf>,
    },
}

impl Input {
    fn stem(&self) -> String {
        match self {
            Input::Tiff(p) => p
                .file_stem()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default(),
            Input::Raw { stem, .. } => stem.to_ascii_lowercase(),
        }
    }

    fn decode(&self) -> Result<(u32, u32, Vec<u8>), String> {
        match self {
            Input::Tiff(p) => decode_rgba(p),
            Input::Raw {
                stem,
                raw,
                act,
                opa,
            } => decode_raw(stem, raw, act, opa.as_deref()),
        }
    }
}

fn dims(n: usize) -> Option<(u32, u32)> {
    match n {
        65536 => Some((256, 256)),
        16384 => Some((128, 128)),
        4096 => Some((64, 64)),
        1024 => Some((32, 32)),
        256 => Some((16, 16)),
        307200 => Some((640, 480)),
        76096 => Some((232, 328)),
        57024 => Some((264, 216)),
        38704 => Some((248, 156)),
        29040 => Some((220, 132)),
        1536 => Some((32, 48)),
        _ => {
            let side = (n as f64).sqrt() as usize;
            (side * side == n).then_some((side as u32, side as u32))
        }
    }
}

fn decode_raw(
    stem: &str,
    raw: &Path,
    act: &Path,
    opa: Option<&Path>,
) -> Result<(u32, u32, Vec<u8>), String> {
    let indices = std::fs::read(raw).map_err(|e| format!("{}: {e}", raw.display()))?;
    let (w, h) = dims(indices.len())
        .ok_or_else(|| format!("{}: unknown image size {}", stem, indices.len()))?;
    let palette = std::fs::read(act).map_err(|e| format!("{}: {e}", act.display()))?;
    if palette.len() < 768 {
        return Err(format!("{}: short palette", act.display()));
    }
    let mask = match opa {
        Some(p) => std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?,
        None => vec![255u8; indices.len()],
    };
    Ok(from_palette(super::tiff::PaletteTiff {
        width: w,
        height: h,
        indices,
        palette: palette[..768].to_vec(),
        mask,
    }))
}

/// Convert the interface TIFFs below `extracted` and atomically install them
/// as `<assets>/ui2`. Returns counts; individual unreadable cards are logged
/// and skipped so one bad TIFF cannot fail the whole intake.
pub fn convert(
    extracted: &Path,
    assets: &Path,
    mut progress: impl FnMut(usize, usize),
) -> Result<Stats, String> {
    let mut inputs: Vec<Input> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // `unknown/ui/ART` is the layout older Python extractions produced; native
    // intake files the TIFFs under `texture/ui/ART`.
    let dirs = [
        extracted.join("texture/ui/ART"),
        extracted.join("unknown/ui/ART"),
    ];
    for art in &dirs {
        if let Ok(rd) = std::fs::read_dir(art) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("tif"))
                    && p.file_stem()
                        .map(|s| wanted(&s.to_string_lossy()))
                        .unwrap_or(false)
                {
                    let stem = p
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_ascii_lowercase())
                        .unwrap_or_default();
                    seen.insert(stem);
                    inputs.push(Input::Tiff(p));
                }
            }
        }
    }
    // Some interface pieces (BUTTON3, CURSOR) ship as RAW + ACT + OPA rather
    // than TIFFs.
    for art in &dirs {
        if let Ok(rd) = std::fs::read_dir(art) {
            for e in rd.flatten() {
                let raw = e.path();
                if !raw
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("raw"))
                {
                    continue;
                }
                let Some(stem) = raw.file_stem().map(|s| s.to_string_lossy().to_string()) else {
                    continue;
                };
                if !wanted(&stem) || seen.contains(&stem.to_ascii_lowercase()) {
                    continue;
                }
                let act = raw.with_extension("ACT");
                if !act.exists() {
                    continue;
                }
                let opa = raw.with_extension("OPA");
                seen.insert(stem.to_ascii_lowercase());
                inputs.push(Input::Raw {
                    stem,
                    raw,
                    act,
                    opa: opa.exists().then_some(opa),
                });
            }
        }
    }
    inputs.sort_by_key(|i| i.stem());
    let total = inputs.len();
    let mut stats = Stats::default();
    if total == 0 {
        return Ok(stats);
    }

    let tmp = assets.join(format!(".ui2.tmp-{}", std::process::id()));
    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;

    for (i, input) in inputs.iter().enumerate() {
        progress(i, total);
        let dest = tmp.join(format!("{}.png", input.stem()));
        match input
            .decode()
            .and_then(|(w, h, rgba)| write_png(&dest, w, h, &rgba))
        {
            Ok(()) => stats.converted += 1,
            Err(e) => {
                eprintln!("intake: ui2 {e}");
                stats.failed += 1;
            }
        }
    }
    progress(total, total);

    if stats.converted == 0 {
        let _ = std::fs::remove_dir_all(&tmp);
        stats.skipped = total;
        return Ok(stats);
    }

    let ui2 = assets.join("ui2");
    let old = assets.join(format!(".ui2.old-{}", std::process::id()));
    if ui2.exists() {
        std::fs::rename(&ui2, &old)
            .map_err(|e| format!("{}: could not move old menu art aside: {e}", ui2.display()))?;
    }
    if let Err(e) = std::fs::rename(&tmp, &ui2) {
        if old.exists() {
            let _ = std::fs::rename(&old, &ui2);
        }
        return Err(format!("{}: {e}", ui2.display()));
    }
    if old.exists() {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_wanted_rgb_tiff_to_png() {
        let td = std::env::temp_dir().join(format!("evox-ui2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&td);
        let art = td.join("extracted/texture/ui/ART");
        std::fs::create_dir_all(&art).unwrap();
        let assets = td.join("assets");
        std::fs::create_dir_all(&assets).unwrap();

        let path = art.join("BG_TEST.TIF");
        let pixels: Vec<u8> = vec![
            255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0, 10, 20, 30, 40, 50, 60,
        ];
        {
            let mut file = File::create(&path).unwrap();
            let mut enc = ::tiff::encoder::TiffEncoder::new(&mut file).unwrap();
            enc.write_image::<::tiff::encoder::colortype::RGB8>(3, 2, &pixels)
                .unwrap();
        }
        // Non-menu art must be ignored.
        std::fs::write(art.join("ALASKA.TIF"), b"not a tiff").unwrap();

        let stats = convert(&td.join("extracted"), &assets, |_, _| {}).unwrap();
        assert_eq!(stats.converted, 1);
        let png = assets.join("ui2/bg_test.png");
        assert!(png.exists());
        let dec = ::png::Decoder::new(BufReader::new(File::open(&png).unwrap()));
        let mut reader = dec.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (3, 2));
        assert_eq!(&buf[..4], &[255, 0, 0, 255]);
        assert!(!assets
            .join(format!(".ui2.tmp-{}", std::process::id()))
            .exists());
        let _ = std::fs::remove_dir_all(td);
    }

    #[test]
    fn converts_retail_ui_art() {
        crate::require_assets!();
        let root = ["assets/extracted", "../assets/extracted"]
            .into_iter()
            .map(Path::new)
            .find(|p| p.exists())
            .expect("extracted assets");
        let td = std::env::temp_dir().join(format!("evox-ui2-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&td);
        std::fs::create_dir_all(&td).unwrap();
        let stats = convert(root, &td, |_, _| {}).unwrap();
        assert!(stats.converted >= 30, "converted {}", stats.converted);
        assert_eq!(stats.failed, 0, "unreadable interface TIFFs");
        for name in ["bg_main.png", "button3.png", "cursor.png", "logo.png"] {
            assert!(td.join("ui2").join(name).exists(), "missing {name}");
        }
        let _ = std::fs::remove_dir_all(td);
    }
}
