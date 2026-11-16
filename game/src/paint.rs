//! Factory paint: the retail `colorAdjustTexture` (`core/colorvar.cpp`).
//!
//! A truck's `colorList[]` entries are not colours to fill with but HSV
//! *adjustments* applied to the body texture's palette, masked per pixel by
//! the texture's second channel (Evo 1 ships it as `<stem>.OPA`, Evo 2 as the
//! second TIFF sample). Reverse-engineered from `decomp_4x42_full.c`:
//!
//! * `loadTruckTextures` (190 692-190 750) takes entry `colorIndex`
//!   (`+0x2074`) and passes `hue / 360`, `sat / 100`, `val / 100`
//!   (`DAT_0065b5c0` = 360.0, `DAT_0065b5c8` = 100.0 in `4x42.exe`) to
//!   `FUN_004273d0`. Fields 3-5 (RGB) are only the swatch shown in menus;
//!   field 6 is the colour's name (file version ≥ 6).
//! * `FUN_004273d0` (17 454-17 900) converts the 256-entry palette to HSV
//!   (`FUN_004269a0`: V = max, S = (max−min)/max, H in turns, −1 for greys),
//!   counts palette indices under mask > 250, and takes the hue of the most
//!   used one as the reference. Every entry's hue is clamped to
//!   `reference ± 0.05` (`DAT_0062ff01/05`), shifted by the adjustment and
//!   wrapped into 0..1; saturation and value are shifted and clamped to 0..1;
//!   then back to RGB (`FUN_004267d0`).
//! * Per pixel: mask 255 takes the adjusted palette, mask ≤ 128 the original,
//!   in between a linear blend by `(mask − 128) / 127`; the mask is written
//!   to the alpha byte in every case.
//!
//! A zero adjustment (all three 0) keeps the original palette, which is how
//! fixed liveries such as the yellow ARB Wrangler (`numColors 0`) render.

/// One `colorList[]` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Paint {
    /// Hue shift, degrees (field 0).
    pub hue_deg: f32,
    /// Saturation shift, percent (field 1).
    pub sat_pct: f32,
    /// Value (brightness) shift, percent (field 2).
    pub val_pct: f32,
    /// Menu swatch (fields 3-5), 0-255.
    pub swatch: [u8; 3],
    /// Colour name (field 6, `NULL` when unnamed).
    pub name: String,
}

impl Paint {
    /// Parse `h,s,v[,r,g,b[,name]]`.
    pub fn parse(line: &str) -> Option<Self> {
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        let f = |i: usize| parts.get(i).and_then(|s| s.parse::<f32>().ok());
        let (h, s, v) = (f(0)?, f(1)?, f(2)?);
        let c = |i: usize| f(i).map(|x| x.clamp(0.0, 255.0) as u8).unwrap_or(0);
        let name = parts
            .get(6)
            .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case("NULL"))
            .map(|n| n.to_string())
            .unwrap_or_default();
        Some(Self {
            hue_deg: h,
            sat_pct: s,
            val_pct: v,
            swatch: [c(3), c(4), c(5)],
            name,
        })
    }

    /// Adjustment in the units `FUN_004273d0` works in (turns, 0..1, 0..1).
    pub fn shift(&self) -> (f32, f32, f32) {
        (
            self.hue_deg / 360.0,
            self.sat_pct / 100.0,
            self.val_pct / 100.0,
        )
    }

    /// Stable cache key for a painted texture.
    pub fn key(&self) -> String {
        format!(
            "{:.2}:{:.2}:{:.2}",
            self.hue_deg, self.sat_pct, self.val_pct
        )
    }
}

/// Hue window around the reference paint hue (`DAT_0062ff01` = 0.05).
const HUE_WINDOW: f32 = 0.05;
/// Mask above this is counted as body paint when picking the reference hue.
const PAINT_MASK_MIN: u8 = 250;
/// Mask at or below this keeps the original colour.
const KEEP_MASK_MAX: u8 = 128;

/// `FUN_004269a0`: RGB (0..1) → (hue in turns or −1 for grey, sat, value).
pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    if max == 0.0 || delta == 0.0 {
        return (-1.0, if max == 0.0 { 0.0 } else { delta / max }, max);
    }
    let s = delta / max;
    let mut h = if r == max {
        (g - b) / delta
    } else if g == max {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    } * 60.0;
    if h < 0.0 {
        h += 360.0;
    }
    (h / 360.0, s, max)
}

/// `FUN_004267d0`: (hue in turns, sat, value) → RGB (0..1).
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    if s == 0.0 {
        return (v, v, v);
    }
    let h = if h >= 1.0 { 0.0 } else { h.max(0.0) } * 6.0;
    let sector = h.floor();
    let f = h - sector;
    let p = (1.0 - s) * v;
    let q = (1.0 - s * f) * v;
    let t = (1.0 - (1.0 - f) * s) * v;
    match sector as i32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

/// Recoloured RGBA for an indexed texture. `pixels` are palette indices,
/// `act` the 768-byte palette, `mask` the `.OPA` paint mask (never transparency).
/// Every pixel is emitted 100% opaque (`alpha = 255`) so unpainted pickup bed
/// liners, trim, and bumpers never turn semi-transparent.
pub fn recolor(pixels: &[u8], act: &[u8], mask: &[u8], paint: &Paint) -> Vec<u8> {
    let original: Vec<[u8; 3]> = (0..256)
        .map(|i| [act[i * 3], act[i * 3 + 1], act[i * 3 + 2]])
        .collect();
    let adjusted = adjusted_palette(pixels, &original, mask, paint.shift());
    let mut out = Vec::with_capacity(pixels.len() * 4);
    for (i, &p) in pixels.iter().enumerate() {
        let m = mask.get(i).copied().unwrap_or(255);
        let a = original[p as usize];
        let b = adjusted[p as usize];
        let rgb = if m == 255 {
            b
        } else if m <= KEEP_MASK_MAX {
            if a[0].max(a[1]).max(a[2]) <= 96 {
                [0, 1, 2].map(|k| (a[k] as f32 * 0.75).round() as u8)
            } else {
                a
            }
        } else {
            let t = (m - KEEP_MASK_MAX) as f32 / (255 - KEEP_MASK_MAX) as f32;
            [0, 1, 2].map(|k| (a[k] as f32 + (b[k] as f32 - a[k] as f32) * t).round() as u8)
        };
        out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    out
}

/// The palette after `colorAdjustTexture`'s per-entry HSV shift.
pub fn adjusted_palette(
    pixels: &[u8],
    original: &[[u8; 3]],
    mask: &[u8],
    (dh, ds, dv): (f32, f32, f32),
) -> Vec<[u8; 3]> {
    if dh == 0.0 && ds == 0.0 && dv == 0.0 {
        return original.to_vec();
    }
    let hsv: Vec<(f32, f32, f32)> = original
        .iter()
        .map(|c| {
            rgb_to_hsv(
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
            )
        })
        .collect();
    // Reference hue: the palette entry used most under the paint mask.
    let mut counts = [0u32; 256];
    for (i, &p) in pixels.iter().enumerate() {
        if mask.get(i).is_some_and(|&m| m > PAINT_MASK_MIN) {
            counts[p as usize] += 1;
        }
    }
    let reference = (0..256).max_by_key(|&i| counts[i]).unwrap_or(0);
    let ref_h = hsv[reference].0;
    hsv.iter()
        .map(|&(h, s, v)| {
            let mut h = h.min(ref_h + HUE_WINDOW).max(ref_h - HUE_WINDOW) + dh;
            if h > 1.0 {
                h -= 1.0;
            }
            if h < 0.0 {
                h += 1.0;
            }
            let s = (s + ds).clamp(0.0, 1.0);
            let v = (v + dv).clamp(0.0, 1.0);
            let (r, g, b) = hsv_to_rgb(h, s, v);
            [r, g, b].map(|c| (c * 255.0).round().clamp(0.0, 255.0) as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_roundtrip() {
        for c in [
            [1.0, 0.0, 0.0],
            [0.2, 0.6, 0.3],
            [0.9, 0.8, 0.1],
            [0.3, 0.3, 0.9],
        ] {
            let (h, s, v) = rgb_to_hsv(c[0], c[1], c[2]);
            let (r, g, b) = hsv_to_rgb(h, s, v);
            assert!((r - c[0]).abs() < 1e-4 && (g - c[1]).abs() < 1e-4 && (b - c[2]).abs() < 1e-4);
        }
        assert_eq!(rgb_to_hsv(0.5, 0.5, 0.5).0, -1.0, "greys have no hue");
    }

    #[test]
    fn parses_retail_entries() {
        let p = Paint::parse("345,95.42,32.5,255,0,0,NULL").unwrap();
        assert_eq!(p.swatch, [255, 0, 0]);
        assert!(p.name.is_empty());
        assert!((p.shift().0 - 345.0 / 360.0).abs() < 1e-6);
        let named = Paint::parse("10,20,30,1,2,3,Desert Tan").unwrap();
        assert_eq!(named.name, "Desert Tan");
        assert!(Paint::parse("12,3").is_none());
    }

    #[test]
    fn grey_body_turns_to_the_swatch_and_trim_stays() {
        // Wrangler Sport red: a grey base becomes red under the paint mask,
        // the unmasked pixel keeps its colour.
        let mut act = vec![0u8; 768];
        act[3..6].copy_from_slice(&[140, 140, 140]); // index 1: grey body
        act[6..9].copy_from_slice(&[20, 20, 20]); // index 2: black trim
        let pixels = [1u8, 1, 2];
        let mask = [255u8, 255, 128];
        let red = Paint::parse("345,95.42,32.5,255,0,0,NULL").unwrap();
        let out = recolor(&pixels, &act, &mask, &red);
        let (h, s, _) = rgb_to_hsv(
            out[0] as f32 / 255.0,
            out[1] as f32 / 255.0,
            out[2] as f32 / 255.0,
        );
        assert!(s > 0.8, "body not saturated: {:?}", &out[0..3]);
        assert!(!(0.05..=0.92).contains(&h), "body hue {h} is not red");
        assert_eq!(
            &out[8..11],
            &[15, 15, 15],
            "dark trim should be darkened 0.75x"
        );
        assert_eq!(out[11], 255, "vehicle pixels must be 100% opaque");
    }

    #[test]
    fn zero_shift_keeps_the_livery() {
        let mut act = vec![0u8; 768];
        act[3..6].copy_from_slice(&[230, 200, 20]); // ARB yellow
        let out = recolor(&[1], &act, &[255], &Paint::parse("0,0,0").unwrap());
        assert_eq!(&out[0..4], &[230, 200, 20, 255]);
    }
}
