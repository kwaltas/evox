//! Minimal decoder for the uncompressed palette TIFFs Evo 2 stores vehicle
//! art in. Port of `tools/setup_evo2.py::read_palette_tif`: sample 0 is the
//! colour index, sample 1 (when present) is the paint mask shipped as `.OPA`.

use std::path::{Path, PathBuf};

pub struct PaletteTiff {
    pub width: u32,
    pub height: u32,
    pub indices: Vec<u8>,
    pub palette: Vec<u8>,
    pub mask: Vec<u8>,
}

fn u16_at(d: &[u8], off: usize) -> Result<u16, String> {
    d.get(off..off + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| "truncated TIFF".to_string())
}

fn u32_at(d: &[u8], off: usize) -> Result<u32, String> {
    d.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| "truncated TIFF".to_string())
}

fn values(d: &[u8], typ: u16, count: u32, field: usize) -> Result<Vec<u32>, String> {
    let size = match typ {
        1 | 2 => 1usize,
        3 => 2,
        4 => 4,
        other => return Err(format!("unsupported TIFF field type {other}")),
    };
    let total = size
        .checked_mul(count as usize)
        .ok_or_else(|| "TIFF field overflow".to_string())?;
    let at = if total <= 4 {
        field
    } else {
        u32_at(d, field)? as usize
    };
    let raw = d
        .get(at..at + total)
        .ok_or_else(|| "TIFF field outside file".to_string())?;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let v = match size {
            1 => raw[i] as u32,
            2 => u16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]) as u32,
            _ => u32::from_le_bytes([raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2], raw[i * 4 + 3]]),
        };
        out.push(v);
    }
    Ok(out)
}

pub fn decode(path: &Path) -> Result<PaletteTiff, String> {
    let d = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if d.len() < 8 || &d[..4] != b"II*\0" {
        return Err(format!("{}: not a little-endian TIFF", path.display()));
    }
    let ifd = u32_at(&d, 4)? as usize;
    let n = u16_at(&d, ifd)? as usize;
    let mut tags: std::collections::HashMap<u16, (u16, u32, usize)> =
        std::collections::HashMap::new();
    for k in 0..n {
        let at = ifd + 2 + 12 * k;
        let tag = u16_at(&d, at)?;
        let typ = u16_at(&d, at + 2)?;
        let count = u32_at(&d, at + 4)?;
        tags.insert(tag, (typ, count, at + 8));
    }
    let tag = |id: u16| -> Option<&(u16, u32, usize)> { tags.get(&id) };
    let one = |id: u16| -> Result<u32, String> {
        let (typ, count, field) = *tag(id).ok_or_else(|| format!("missing TIFF tag {id}"))?;
        values(&d, typ, count, field)?
            .first()
            .copied()
            .ok_or_else(|| format!("empty TIFF tag {id}"))
    };

    let width = one(256)?;
    let height = one(257)?;
    if one(259)? != 1 || one(262)? != 3 {
        return Err("not an uncompressed palette TIFF".into());
    }
    let spp = match tag(277) {
        Some(_) => one(277)? as usize,
        None => 1,
    };
    let (ct, cc, cf) = *tag(273).ok_or("missing TIFF strip offset")?;
    let strips = values(&d, ct, cc, cf)?;
    let (lt, lc, lf) = *tag(279).ok_or("missing TIFF strip length")?;
    let lengths = values(&d, lt, lc, lf)?;
    let mut data = Vec::new();
    for (off, len) in strips.iter().zip(lengths.iter()) {
        let o = *off as usize;
        let l = *len as usize;
        data.extend_from_slice(
            d.get(o..o + l)
                .ok_or_else(|| "TIFF strip outside file".to_string())?,
        );
    }
    let need = width as usize * height as usize * spp;
    if data.len() < need {
        return Err("truncated TIFF pixel data".into());
    }
    let (mt, mc, mf) = *tag(320).ok_or("missing TIFF colour map")?;
    let cm = values(&d, mt, mc, mf)?;
    if cm.len() < 768 {
        return Err("short TIFF colour map".into());
    }
    let mut palette = Vec::with_capacity(768);
    for i in 0..256 {
        palette.push((cm[i] >> 8) as u8);
        palette.push((cm[256 + i] >> 8) as u8);
        palette.push((cm[512 + i] >> 8) as u8);
    }
    let mut indices = Vec::with_capacity(width as usize * height as usize);
    let mut mask = Vec::with_capacity(width as usize * height as usize);
    for i in 0..(width as usize * height as usize) {
        indices.push(data[i * spp]);
        mask.push(if spp >= 2 { data[i * spp + 1] } else { 0xff });
    }
    Ok(PaletteTiff {
        width,
        height,
        indices,
        palette,
        mask,
    })
}

pub struct ArtStats {
    pub converted: usize,
    pub skipped: usize,
    pub failed: usize,
}

/// Bake the vehicle TIFFs (`texture/vehicle/ART`, legacy `unknown/vehicle/ART`)
/// and the level/scenery TIFFs (`texture/unknown/ART`, legacy
/// `unknown/unknown/ART`: terrain skins, rocks, cliffs, pipes, ...) into the
/// `RAW/ACT/OPA` triplet the loader reads. Without the scenery set the props
/// and some terrains render untextured white. UI TIFFs are excluded: they
/// belong to the interface, not the terrain/vehicle texture library.
pub fn convert_art(root: &Path, mut progress: impl FnMut(usize, usize)) -> ArtStats {
    let tex = root.join("texture/texture/ART");
    let act = root.join("texture/palette/ART");
    let opa = root.join("texture/opacity/ART");
    let mut sources: Vec<PathBuf> = Vec::new();
    for dir in [
        root.join("texture/vehicle/ART"),
        root.join("unknown/vehicle/ART"),
        root.join("texture/unknown/ART"),
        root.join("unknown/unknown/ART"),
    ] {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("tif")) {
                    sources.push(p);
                }
            }
        }
    }
    sources.sort();
    let mut stats = ArtStats {
        converted: 0,
        skipped: 0,
        failed: 0,
    };
    let total = sources.len();
    for (i, tif) in sources.iter().enumerate() {
        progress(i, total);
        let stem = tif
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_uppercase())
            .unwrap_or_default();
        if stem.ends_with("_BUMP") {
            continue;
        }
        if tex.join(format!("{stem}.RAW")).exists() {
            stats.skipped += 1;
            continue;
        }
        match decode(tif) {
            Ok(t) => {
                let _ = std::fs::create_dir_all(&tex);
                let _ = std::fs::create_dir_all(&act);
                let _ = std::fs::create_dir_all(&opa);
                if std::fs::write(tex.join(format!("{stem}.RAW")), &t.indices).is_err()
                    || std::fs::write(act.join(format!("{stem}.ACT")), &t.palette).is_err()
                    || std::fs::write(opa.join(format!("{stem}.OPA")), &t.mask).is_err()
                {
                    stats.failed += 1;
                } else {
                    stats.converted += 1;
                }
            }
            Err(e) => {
                eprintln!("intake: {}: {e}", tif.display());
                stats.failed += 1;
            }
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_tiff(w: u16, h: u16, spp: u16) -> Vec<u8> {
        let tags = [256u16, 257, 259, 262, 273, 277, 279, 320];
        let ifd_at = 8usize;
        let entries = tags.len();
        let data_at = ifd_at + 2 + entries * 12 + 4;
        let pixels: Vec<u8> = (0..w as usize * h as usize)
            .flat_map(|i| {
                if spp == 2 {
                    vec![(i % 7) as u8, (200 - i) as u8]
                } else {
                    vec![(i % 7) as u8]
                }
            })
            .collect();
        let pixel_at = data_at;
        let mut cmap = Vec::new();
        for i in 0..768u32 {
            cmap.extend_from_slice(&(i as u16).to_le_bytes());
        }
        let cmap_at = pixel_at + pixels.len();

        let mut out = b"II*\0".to_vec();
        out.extend_from_slice(&(ifd_at as u32).to_le_bytes());
        out.extend_from_slice(&(entries as u16).to_le_bytes());
        for tag in tags {
            out.extend_from_slice(&tag.to_le_bytes());
            match tag {
                256 => {
                    out.extend_from_slice(&3u16.to_le_bytes());
                    out.extend_from_slice(&1u32.to_le_bytes());
                    out.extend_from_slice(&(w as u32).to_le_bytes());
                }
                257 => {
                    out.extend_from_slice(&3u16.to_le_bytes());
                    out.extend_from_slice(&1u32.to_le_bytes());
                    out.extend_from_slice(&(h as u32).to_le_bytes());
                }
                259 => {
                    out.extend_from_slice(&3u16.to_le_bytes());
                    out.extend_from_slice(&1u32.to_le_bytes());
                    out.extend_from_slice(&1u32.to_le_bytes());
                }
                262 => {
                    out.extend_from_slice(&3u16.to_le_bytes());
                    out.extend_from_slice(&1u32.to_le_bytes());
                    out.extend_from_slice(&3u32.to_le_bytes());
                }
                273 => {
                    out.extend_from_slice(&4u16.to_le_bytes());
                    out.extend_from_slice(&1u32.to_le_bytes());
                    out.extend_from_slice(&(pixel_at as u32).to_le_bytes());
                }
                277 => {
                    out.extend_from_slice(&3u16.to_le_bytes());
                    out.extend_from_slice(&1u32.to_le_bytes());
                    out.extend_from_slice(&(spp as u32).to_le_bytes());
                }
                279 => {
                    out.extend_from_slice(&4u16.to_le_bytes());
                    out.extend_from_slice(&1u32.to_le_bytes());
                    out.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
                }
                _ => {
                    out.extend_from_slice(&3u16.to_le_bytes());
                    out.extend_from_slice(&768u32.to_le_bytes());
                    out.extend_from_slice(&(cmap_at as u32).to_le_bytes());
                }
            }
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&pixels);
        out.extend_from_slice(&cmap);
        out
    }

    #[test]
    fn decodes_indices_palette_and_mask() {
        let td = std::env::temp_dir().join(format!("evox-tiff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&td);
        std::fs::create_dir_all(&td).unwrap();
        let path = td.join("TEST.TIF");
        std::fs::write(&path, build_tiff(4, 2, 2)).unwrap();
        let t = decode(&path).unwrap();
        assert_eq!((t.width, t.height), (4, 2));
        assert_eq!(t.indices, vec![0, 1, 2, 3, 4, 5, 6, 0]);
        assert_eq!(t.mask, vec![200, 199, 198, 197, 196, 195, 194, 193]);
        assert_eq!(&t.palette[..6], &[0, 1, 2, 0, 1, 2]);
        let _ = std::fs::remove_dir_all(td);
    }

    #[test]
    fn one_sample_tiff_gets_opaque_mask() {
        let td = std::env::temp_dir().join(format!("evox-tiff1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&td);
        std::fs::create_dir_all(&td).unwrap();
        let path = td.join("ONE.TIF");
        std::fs::write(&path, build_tiff(2, 2, 1)).unwrap();
        let t = decode(&path).unwrap();
        assert_eq!(t.mask, vec![255; 4]);
        let _ = std::fs::remove_dir_all(td);
    }
}
