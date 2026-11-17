//! Front end and race HUD ported from offroad4x4 (`hud.rs`, `render.rs`):
//! the original opening movie, the title card, the blue-bar menu frame over
//! the Evo 2 background art, and the race instruments laid out like the
//! original (timing top-left, checkpoint finder top-centre, course map
//! top-right, speedometer with odometer drums bottom-left, tachometer with
//! gear and drive mode bottom-right).
//!
//! All lettering goes through evox's `Ui` (the extracted `4X4FONT` with its
//! measured glyph table); offroad4x4's own font path is not used. The gauge
//! labels, digits and gear rings come from the `ODOM` instrument sheet, which
//! evox decodes from `texture/ui/ART` (identical to offroad4x4 `odom.png`).
//!
//! Optional converted art lives beside the extracted data:
//! `assets/intro/f0001.png..` + `intro.wav` (the Bink movie at 15 fps,
//! 320x240) and `assets/ui2/*.png` (Evo 2 menu backgrounds, buttons, logo).
//! Each piece falls back to evox's own screens when absent.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};

use macroquad::prelude::*;

use crate::terrain::{shade_sun, Track, SUN};
use crate::ui::Ui;

pub const INTRO_DIR: &str = "assets/intro";
pub const UI2_DIR: &str = "assets/ui2";
pub const INTRO_FPS: f32 = 15.0;

pub const YELLOW: Color = Color::new(1.0, 0.86, 0.0, 1.0);
pub const GOOD: Color = Color::new(0.47, 0.94, 0.55, 1.0);
pub const BAD: Color = Color::new(1.0, 0.36, 0.32, 1.0);
pub const ACCENT: Color = Color::new(1.0, 0.72, 0.18, 1.0);
pub const DIM: Color = Color::new(0.75, 0.78, 0.82, 1.0);
pub const BAR: Color = Color::new(0.14, 0.15, 0.62, 0.62);
pub const MENU_SEL: Color = Color::new(1.0, 0.62, 0.10, 1.0);
pub const MENU_HEAD: Color = Color::new(0.80, 0.84, 1.0, 1.0);

// ------------------------------------------------------------------ text --
// offroad4x4 sizes are in its own font units with `y` on the baseline; evox's
// `Ui::text` takes the top edge. These convert one to the other.

const SIZE_K: f32 = 0.78;

/// Text with its baseline at `y` (offroad4x4 `draw_text` convention).
pub fn text(ui: &Ui, s: &str, x: f32, y: f32, size: f32, c: Color) {
    let es = size * SIZE_K;
    ui.text(x, y - es * 1.15, es, s, c);
}

pub fn width(ui: &Ui, s: &str, size: f32) -> f32 {
    ui.width(size * SIZE_K, s)
}

/// Text with a dark outline, like the original's menu lettering.
pub fn text_o(ui: &Ui, s: &str, x: f32, y: f32, size: f32, c: Color) {
    let k = Color::new(0.0, 0.0, 0.08, 0.9 * c.a);
    for (dx, dy) in [
        (-2.0, 0.0),
        (2.0, 0.0),
        (0.0, -2.0),
        (0.0, 2.0),
        (1.5, 1.5),
        (-1.5, 1.5),
    ] {
        text(ui, s, x + dx, y + dy, size, k);
    }
    text(ui, s, x, y, size, c);
}

pub fn text_c(ui: &Ui, s: &str, cx: f32, y: f32, size: f32, c: Color) {
    let w = width(ui, s, size);
    text(
        ui,
        s,
        cx - w * 0.5 + 2.0,
        y + 2.0,
        size,
        Color::new(0.0, 0.0, 0.0, 0.6 * c.a),
    );
    text(ui, s, cx - w * 0.5, y, size, c);
}

pub fn text_r(ui: &Ui, s: &str, rx: f32, y: f32, size: f32, c: Color) {
    text(ui, s, rx - width(ui, s, size), y, size, c);
}

pub fn fmt_time(t: f32) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor() as u32;
    let s = t - m as f32 * 60.0;
    format!("{}:{:05.2}", m, s)
}

// ------------------------------------------------------------------- art --

thread_local! {
    static UI2: RefCell<HashMap<String, Option<Texture2D>>> = RefCell::new(HashMap::new());
}

fn load_png(path: &str) -> Option<Texture2D> {
    let bytes = std::fs::read(path).ok()?;
    let img = Image::from_file_with_format(&bytes, Some(ImageFormat::Png)).ok()?;
    let t = Texture2D::from_image(&img);
    t.set_filter(FilterMode::Linear);
    Some(t)
}

/// Front-end art of the original (`assets/ui2/<name>.png`), loaded once.
pub fn ui2(name: &str) -> Option<Texture2D> {
    UI2.with(|c| {
        c.borrow_mut()
            .entry(name.to_string())
            .or_insert_with(|| load_png(&format!("{UI2_DIR}/{name}.png")))
            .clone()
    })
}

pub fn has_menu_art() -> bool {
    ui2("bg_main").is_some()
}

// ----------------------------------------------------------------- intro --
// The original's opening movie, decoded into 320x240 frames at 15 fps. A few
// frames are kept in flight so playback does not hitch while PNGs decode.

thread_local! {
    static INTRO_TEX: RefCell<VecDeque<(u32, Texture2D)>> = const { RefCell::new(VecDeque::new()) };
    static INTRO_COUNT: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

fn intro_path(i: u32) -> String {
    format!("{INTRO_DIR}/f{:04}.png", i + 1)
}

/// Number of movie frames on disk (0 = no movie converted).
pub fn intro_frames() -> u32 {
    INTRO_COUNT.with(|c| {
        if let Some(n) = c.get() {
            return n;
        }
        let mut n = 0;
        while n < 10_000 && std::path::Path::new(&intro_path(n)).exists() {
            n += 1;
        }
        c.set(Some(n));
        n
    })
}

pub fn intro_audio_path() -> String {
    format!("{INTRO_DIR}/intro.wav")
}

pub fn intro_reset() {
    INTRO_TEX.with(|c| c.borrow_mut().clear());
}

/// Frame `i`, loading ahead; `None` while it is still decoding.
pub fn intro_frame_ready(i: u32) -> Option<Texture2D> {
    let total = intro_frames();
    if total == 0 {
        return None;
    }
    let want = i.min(total - 1);
    INTRO_TEX.with(|c| {
        let mut q = c.borrow_mut();
        let stale = q.front().is_none_or(|(f, _)| want < *f || want > *f + 8);
        if stale {
            q.clear();
            for k in want..(want + 3).min(total) {
                match load_png(&intro_path(k)) {
                    Some(t) => q.push_back((k, t)),
                    None => break,
                }
            }
        } else {
            while q.back().is_some_and(|(b, _)| *b < want + 2) {
                let k = q.back().map_or(want, |(b, _)| b + 1);
                if k >= total {
                    break;
                }
                match load_png(&intro_path(k)) {
                    Some(t) => q.push_back((k, t)),
                    None => break,
                }
            }
            while q.front().is_some_and(|(f, _)| *f + 2 < want) {
                q.pop_front();
            }
        }
        q.iter().find(|(k, _)| *k == want).map(|(_, t)| t.clone())
    })
}

/// Draw one movie frame letterboxed over black.
pub fn intro_draw(t: Option<&Texture2D>, fade: f32) {
    clear_background(BLACK);
    let (sw, sh) = (screen_width(), screen_height());
    let k = (sw / 320.0).min(sh / 240.0);
    let (w, h) = (320.0 * k, 240.0 * k);
    if let Some(t) = t {
        draw_texture_ex(
            t,
            (sw - w) * 0.5,
            (sh - h) * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(w, h)),
                ..Default::default()
            },
        );
    }
    if fade > 0.0 {
        draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.0, 0.0, 0.0, fade));
    }
}

// ----------------------------------------------------------------- title --

/// The original's title screen: the EVO 2 logo over black and a blinking
/// prompt. Falls back to the extracted `LOGO480` card.
pub fn title(ui: &Ui, fallback: Option<&Texture2D>) {
    clear_background(BLACK);
    let (sw, sh) = (screen_width(), screen_height());
    if let Some(l) = ui2("logo") {
        let w = (sw * 0.54).min(sh * 0.84);
        let h = w * l.height() / l.width();
        draw_texture_ex(
            &l,
            (sw - w) * 0.5,
            (sh - h) * 0.46,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(w, h)),
                ..Default::default()
            },
        );
    } else if let Some(t) = fallback {
        let s = (sw / t.width()).min(sh / t.height());
        let (w, h) = (t.width() * s, t.height() * s);
        draw_texture_ex(
            t,
            (sw - w) * 0.5,
            sh * 0.46 - h * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(w, h)),
                ..Default::default()
            },
        );
    }
    let msg = "PRESS ANY KEY";
    let w = width(ui, msg, 26.0);
    let blink = 0.45 + 0.55 * ((get_time() * 2.2).sin() as f32 * 0.5 + 0.5);
    text(
        ui,
        msg,
        (sw - w) * 0.5,
        sh * 0.90,
        26.0,
        Color::new(0.9, 0.92, 1.0, blink),
    );
}

// ------------------------------------------------------------ menu frame --

/// Background art of an Evo 2 screen, cover-scaled and dimmed so the
/// lettering reads.
pub fn menu_bg(name: &str, sw: f32, sh: f32) {
    if let Some(t) = ui2(name) {
        let k = (sw / t.width()).max(sh / t.height());
        draw_texture_ex(
            &t,
            (sw - t.width() * k) * 0.5,
            (sh - t.height() * k) * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(t.width() * k, t.height() * k)),
                ..Default::default()
            },
        );
    }
    draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.03, 0.04, 0.10, 0.42));
}

/// The original's blue bars: screen name top left, parent screen top right,
/// profile and BACK at the bottom, date and time bottom right.
pub fn menu_frame(ui: &Ui, left: &str, right: &str, profile: &str, bg: &str) {
    let (sw, sh) = (screen_width(), screen_height());
    clear_background(BLACK);
    menu_bg(bg, sw, sh);
    draw_rectangle(0.0, 0.0, sw, sh * 0.105, BAR);
    draw_rectangle(0.0, sh * 0.86, sw, sh * 0.14, BAR);
    text_o(ui, left, sw * 0.045, sh * 0.072, 34.0, MENU_HEAD);
    text_o(
        ui,
        right,
        sw * 0.955 - width(ui, right, 34.0),
        sh * 0.072,
        34.0,
        MENU_SEL,
    );
    text(ui, "Profile", sw * 0.045, sh * 0.905, 20.0, MENU_HEAD);
    text_o(ui, profile, sw * 0.045, sh * 0.945, 26.0, WHITE);
    let bx = sw * 0.045 + width(ui, profile, 26.0) + 24.0;
    draw_rectangle(
        bx,
        sh * 0.958,
        86.0,
        24.0,
        Color::new(0.75, 0.78, 0.9, 0.85),
    );
    text(
        ui,
        "BACK",
        bx + 16.0,
        sh * 0.958 + 19.0,
        24.0,
        Color::new(0.1, 0.1, 0.3, 1.0),
    );
    let now = clock_text();
    text(
        ui,
        &now,
        sw * 0.955 - width(ui, &now, 20.0),
        sh * 0.905,
        20.0,
        MENU_HEAD,
    );
}

/// Local date and time, refreshed every few seconds (asked from `date`).
fn clock_text() -> String {
    thread_local! {
        static CLOCK: RefCell<(f64, String)> = const { RefCell::new((-100.0, String::new())) };
    }
    CLOCK.with(|c| {
        let mut c = c.borrow_mut();
        let t = get_time();
        if t - c.0 > 5.0 {
            c.0 = t;
            c.1 = std::process::Command::new("date")
                .arg("+%a %b %d %Y   %I:%M %p")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
        }
        c.1.clone()
    })
}

pub fn wrap_text(t: &str, max_chars: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for w in t.split_whitespace() {
        if lines
            .last()
            .is_some_and(|l| l.len() + w.len() + 1 > max_chars)
        {
            lines.push(String::new());
        }
        // `lines` starts with one entry and only grows.
        let Some(l) = lines.last_mut() else { break };
        if !l.is_empty() {
            l.push(' ');
        }
        l.push_str(w);
    }
    lines
}

/// Row rectangle of menu item `i` (for mouse hit-testing).
pub fn menu_row_rect(i: usize) -> Rect {
    let (sw, sh) = (screen_width(), screen_height());
    let y = sh * 0.165 + i as f32 * sh * 0.058;
    Rect::new(sw * 0.030, y - 24.0, sw * 0.30, 33.0)
}

/// "Select Mode": the list down the left with the original's buttons and
/// cursor, the logo top right, the description of the selected item lower
/// right.
pub fn menu_list(ui: &Ui, items: &[&str], sel: usize, desc: &str, profile: &str) {
    let (sw, sh) = (screen_width(), screen_height());
    menu_frame(ui, "Select Mode", "Main Menu", profile, "bg_main");
    if let Some(l) = ui2("logo") {
        let w = sw * 0.20;
        let h = w * l.height() / l.width();
        draw_texture_ex(
            &l,
            sw * 0.955 - w,
            sh * 0.125,
            Color::new(1.0, 1.0, 1.0, 0.92),
            DrawTextureParams {
                dest_size: Some(vec2(w, h)),
                ..Default::default()
            },
        );
    }
    for (i, label) in items.iter().enumerate() {
        let r = menu_row_rect(i);
        let y = r.y + 24.0;
        let is_sel = sel == i;
        if let Some(b) = ui2("button3") {
            draw_texture_ex(
                &b,
                r.x,
                r.y,
                if is_sel {
                    WHITE
                } else {
                    Color::new(0.85, 0.87, 1.0, 0.9)
                },
                DrawTextureParams {
                    dest_size: Some(vec2(r.w, r.h)),
                    ..Default::default()
                },
            );
        }
        if is_sel {
            if let Some(c) = ui2("cursor") {
                let h = 26.0;
                let w = h * c.width() / c.height();
                draw_texture_ex(
                    &c,
                    r.x - w - 10.0,
                    y - 21.0,
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(vec2(w, h)),
                        ..Default::default()
                    },
                );
            }
        }
        text_o(
            ui,
            label,
            sw * 0.045,
            y,
            32.0,
            if is_sel { MENU_SEL } else { WHITE },
        );
    }
    let lines = wrap_text(desc, 52);
    draw_rectangle(
        sw * 0.39,
        sh * 0.685,
        sw * 0.59,
        lines.len() as f32 * 24.0 + 22.0,
        Color::new(0.02, 0.03, 0.08, 0.55),
    );
    let mut dy = sh * 0.705 + 6.0;
    for l in lines {
        text_o(ui, &l, sw * 0.40, dy, 22.0, WHITE);
        dy += 24.0;
    }
    let hint = "Up/Down  ENTER  ESC";
    text(
        ui,
        hint,
        sw * 0.955 - width(ui, hint, 20.0),
        sh * 0.95,
        20.0,
        MENU_HEAD,
    );
}

// ------------------------------------------------------------------- HUD --

/// A piece of the original's instrument sheet (`ODOM`).
/// Source rects are authored against the retail 256x256 `ODOM` sheet; the HD
/// override is a 4x upscale of the same layout, so scale the sub-rect with the
/// loaded texture instead of assuming retail pixels.
fn sheet(odom: Option<&Texture2D>, src: Rect, x: f32, y: f32, k: f32, c: Color) {
    if let Some(t) = odom {
        let sx = t.width() / 256.0;
        let sy = t.height() / 256.0;
        draw_texture_ex(
            t,
            x,
            y,
            c,
            DrawTextureParams {
                dest_size: Some(vec2(src.w * k, src.h * k)),
                source: Some(Rect::new(src.x * sx, src.y * sy, src.w * sx, src.h * sy)),
                ..Default::default()
            },
        );
    }
}

/// Angle (degrees, standard math convention) of a needle for `value` on a
/// 270-degree dial that sweeps from lower-left (0) to lower-right (max).
fn dial_angle(value: f32, max: f32) -> f32 {
    if max <= 0.0 {
        return 225.0;
    }
    225.0 - 270.0 * (value / max).clamp(0.0, 1.0)
}

/// Retail gauge face (`MPH128` / `RPM128`) with the colour-keyed `NEEDLE`
/// sprite rotated over it. Returns false when the face texture is missing so
/// callers can fall back to the vector dial.
fn dial_art(
    face: Option<&Texture2D>,
    needle: Option<&Texture2D>,
    cx: f32,
    cy: f32,
    r: f32,
    value: f32,
    max: f32,
) -> bool {
    let Some(face) = face else {
        return false;
    };
    draw_texture_ex(
        face,
        cx - r,
        cy - r,
        WHITE,
        DrawTextureParams {
            dest_size: Some(vec2(r * 2.0, r * 2.0)),
            ..Default::default()
        },
    );
    if let Some(needle) = needle {
        // The blade grows upwards from a hub at 54/64 of the sprite height.
        // Drawing the 64x64 sprite at `r` px keeps the blade just inside the
        // 128px face scaled to 2r.
        // Crop to the blade's column: the sprite's full-width horizontal
        // balance bar otherwise reads as a second needle across the face.
        let src = Rect::new(22.0, 0.0, 20.0, 64.0);
        let sw = r * src.w / 64.0;
        draw_texture_ex(
            needle,
            cx - sw * 0.5,
            cy - r * (54.0 / 64.0),
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(sw, r)),
                source: Some(src),
                rotation: (90.0 - dial_angle(value, max)).to_radians(),
                pivot: Some(vec2(cx, cy)),
                ..Default::default()
            },
        );
        draw_circle(cx, cy, r * 0.05, Color::new(0.35, 0.14, 0.0, 0.9));
    }
    true
}

/// A round instrument drawn the way the original does: white ticks and
/// numbers straight over the scene, and the orange needle. The scale sweeps
/// 270 degrees from lower-left to lower-right.
fn dial(
    ui: &Ui,
    cx: f32,
    cy: f32,
    r: f32,
    max: f32,
    step: f32,
    label_every: f32,
    red_from: f32,
    value: f32,
    scale_label: f32,
) {
    let ang = |v: f32| dial_angle(v, max).to_radians();
    let shadow = Color::new(0.0, 0.0, 0.0, 0.55);
    let n = (max / step).round() as i32;
    for i in 0..=n {
        let v = i as f32 * step;
        let a = ang(v);
        let (c, sn) = (a.cos(), a.sin());
        let major = (v / label_every - (v / label_every).round()).abs() < 1e-3;
        let r0 = r - if major { 14.0 } else { 8.0 };
        let col = if v >= red_from {
            Color::new(0.95, 0.15, 0.1, 1.0)
        } else {
            WHITE
        };
        let w = if major { 3.5 } else { 2.0 };
        draw_line(
            cx + c * r0 + 1.5,
            cy - sn * r0 + 1.5,
            cx + c * r + 1.5,
            cy - sn * r + 1.5,
            w,
            shadow,
        );
        draw_line(cx + c * r0, cy - sn * r0, cx + c * r, cy - sn * r, w, col);
        if major {
            let t = format!("{:.0}", v / scale_label);
            let tw = width(ui, &t, 18.0);
            let (tx, ty) = (cx + c * (r - 30.0) - tw * 0.5, cy - sn * (r - 30.0) + 6.0);
            text(ui, &t, tx + 1.5, ty + 1.5, 18.0, shadow);
            text(ui, &t, tx, ty, 18.0, col);
        }
    }
    // needle: the orange of the original's needle texture
    let a = ang(value.clamp(0.0, max));
    let d = vec2(a.cos(), -a.sin());
    let nn = vec2(-d.y, d.x);
    let c = vec2(cx, cy);
    let orange = Color::new(0.96, 0.38, 0.01, 1.0);
    let o = vec2(1.5, 1.5);
    draw_triangle(
        c + nn * 4.0 + o,
        c - nn * 4.0 + o,
        c + d * (r - 6.0) + o,
        shadow,
    );
    draw_triangle(c + nn * 4.0, c - nn * 4.0, c + d * (r - 6.0), orange);
    draw_triangle(c + nn * 4.0, c - nn * 4.0, c - d * 14.0, orange);
    draw_circle(cx, cy, 6.0, Color::new(0.56, 0.22, 0.0, 1.0));
}

/// Speedometer with its label and the odometer drums from the sheet: mph
/// and miles like the original, or km/h and km with metric units.
#[allow(clippy::too_many_arguments)]
pub fn speedo(
    ui: &Ui,
    odom: Option<&Texture2D>,
    face: Option<&Texture2D>,
    needle: Option<&Texture2D>,
    cx: f32,
    cy: f32,
    r: f32,
    speed_ms: f32,
    odo_m: f32,
    metric: bool,
) {
    // The retail face is a fixed 0-140 MPH scale, so the instrument cluster is
    // imperial whenever it is available. The metric vector dial remains the
    // fallback when the extracted face art is missing.
    let (k, max, every, unit_m, label_y) = if metric && face.is_none() {
        (3.6, 240.0, 40.0, 1000.0, 40.0)
    } else {
        (2.236_936, 140.0, 20.0, 1609.344, 21.0)
    };
    let speed = speed_ms.abs() * k;
    // The cluster overlaps the tach: an opaque disc under the (translucent)
    // retail face makes the speedometer physically occlude the dial behind it.
    if face.is_some() {
        draw_circle(cx, cy, r * 0.985, Color::new(0.012, 0.014, 0.012, 1.0));
    }
    let tex_dial = dial_art(face, needle, cx, cy, r, speed, max);
    if !tex_dial {
        dial(ui, cx, cy, r, max, 10.0, every, 999.0, speed, 1.0);
        sheet(
            odom,
            Rect::new(183.0, label_y, 30.0, 11.0),
            cx - 15.0,
            cy + 26.0,
            1.0,
            WHITE,
        );
    }
    let tenths = ((odo_m / unit_m) * 10.0).floor() as u64;
    let digits = [100000u64, 10000, 1000, 100, 10, 1].map(|p| (tenths / 10 / p) % 10);
    let rows = [1.0, 17.0, 31.0, 46.0, 62.0, 77.0, 91.0, 107.0, 121.0, 136.0];
    // Odometer drums sit directly on the face, like the counter moulded into
    // the retail speedometer; no backing plate.
    let zoom = 1.35;
    let (ox, oy) = (cx - 40.0 * zoom, cy + 68.0);
    let ink = Color::new(0.93, 0.95, 0.93, 0.95);
    for (i, d) in digits.iter().enumerate() {
        sheet(
            odom,
            Rect::new(10.0 * i as f32, rows[*d as usize], 10.0, 13.0),
            ox + 10.0 * zoom * i as f32,
            oy,
            zoom,
            ink,
        );
    }
    sheet(
        odom,
        Rect::new(62.0, rows[0], 5.0, 13.0),
        ox + 62.0 * zoom,
        oy,
        zoom,
        ink,
    );
    sheet(
        odom,
        Rect::new(70.0, rows[(tenths % 10) as usize], 10.0, 13.0),
        ox + 70.0 * zoom,
        oy,
        zoom,
        ink,
    );
}

/// Tachometer with "x1000 / min", the gear ring (P R N 1-5 on the sheet,
/// indexed like evox's gearbox) and the drive-mode label.
#[allow(clippy::too_many_arguments)]
pub fn tach(
    ui: &Ui,
    odom: Option<&Texture2D>,
    face: Option<&Texture2D>,
    needle: Option<&Texture2D>,
    cx: f32,
    cy: f32,
    r: f32,
    rpm: f32,
    redline: f32,
    gear: usize,
) {
    let max = ((redline / 1000.0).ceil() + 1.0) * 1000.0;
    // The retail RPM face is labelled 0-8 x1000.
    let tex_dial = face.is_some() && dial_art(face, needle, cx, cy, r, rpm, 8000.0);
    if !tex_dial {
        dial(
            ui,
            cx,
            cy,
            r,
            max,
            500.0,
            1000.0,
            redline,
            rpm.max(700.0),
            1000.0,
        );
        sheet(
            odom,
            Rect::new(183.0, 1.0, 70.0, 13.0),
            cx - 35.0,
            cy + 26.0,
            1.0,
            WHITE,
        );
    }
    // Just the numeral from the ODOM gear cell: the white ring baked into the
    // cell reads as a hard badge, unlike the retail indicator.
    let row = gear.min(7) as f32;
    sheet(
        odom,
        Rect::new(136.0, 7.0 + 28.0 * row, 9.0, 10.0),
        cx - 9.0,
        cy + 36.0,
        2.0,
        WHITE,
    );
}

/// Retail drive-mode label (`2WD` / `4WDL` / `4WDH`) from the ODOM sheet.
pub fn drive_mode(odom: Option<&Texture2D>, x: f32, y: f32, mode: u8) {
    let row = match mode {
        0 => 59.0,
        2 => 78.0,
        _ => 97.0,
    };
    sheet(odom, Rect::new(183.0, row, 46.0, 11.0), x, y, 1.5, WHITE);
}

type RadarCache = Option<(String, i32, i32, i32, Texture2D)>;

thread_local! {
    static RADAR_MODE: Cell<bool> = const { Cell::new(true) };
    static RADAR_TEX: RefCell<RadarCache> = const { RefCell::new(None) };
}

fn radar_texture(track: &Track, player: Vec3, fwd: Vec2, course: &[Vec3]) -> Texture2D {
    const N: usize = 156;
    const RANGE_M: f32 = 220.0;
    let right = vec2(-fwd.y, fwd.x);
    let heading_bucket = (fwd.y.atan2(fwd.x) / 0.10).round() as i32;
    let key = (
        track.id.clone(),
        (player.x / 12.0) as i32,
        (player.z / 12.0) as i32,
        heading_bucket,
    );
    if let Some((id, x, z, h, tex)) = RADAR_TEX.with(|c| c.borrow().clone()) {
        if id == key.0 && x == key.1 && z == key.2 && h == key.3 {
            return tex;
        }
    }
    let mut img = Image::gen_image_color(N as u16, N as u16, Color::new(0.0, 0.0, 0.0, 0.0));
    let half = N as f32 * 0.5;
    for py in 0..N {
        for px in 0..N {
            let dx = (px as f32 + 0.5 - half) / half;
            let dy = (py as f32 + 0.5 - half) / half;
            let r = (dx * dx + dy * dy).sqrt();
            if r > 1.0 {
                continue;
            }
            // Heading-up: screen up is the chassis forward vector, screen
            // right is its perpendicular. The compass letters rotate instead.
            let wx = player.x + right.x * dx * RANGE_M - fwd.x * dy * RANGE_M;
            let wz = player.z + right.y * dx * RANGE_M - fwd.y * dy * RANGE_M;
            let hill = track.height_at(wx, wz);
            let mut col = shade_sun(
                track.ground_color_at(wx, wz),
                track.normal_at_world(wx, wz),
                SUN,
            );
            if track.water_height > 0.0 && hill < track.water_height {
                let w = track.water_color;
                col = Color::new(
                    col.r * 0.35 + w.r * 0.65,
                    col.g * 0.35 + w.g * 0.65,
                    col.b * 0.35 + w.b * 0.65,
                    1.0,
                );
            }
            let a = ((1.0 - r) / 0.06).clamp(0.0, 1.0);
            img.set_pixel(px as u32, py as u32, Color::new(col.r, col.g, col.b, a));
        }
    }

    // Bake the course into the local map like retail's road/tree radar.
    let road = Color::new(0.90, 0.86, 0.67, 1.0);
    let n = N as f32;
    for seg in course.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let steps =
            (((b.x - a.x).powi(2) + (b.z - a.z).powi(2)).sqrt() / 3.0).clamp(2.0, 80.0) as usize;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let wx = a.x + (b.x - a.x) * t;
            let wz = a.z + (b.z - a.z) * t;
            let d = vec2(wx - player.x, wz - player.z);
            let sx = d.dot(right) / RANGE_M;
            let sy = -d.dot(fwd) / RANGE_M;
            if sx * sx + sy * sy > 0.94 {
                continue;
            }
            let px = (sx * 0.5 + 0.5) * n;
            let py = (sy * 0.5 + 0.5) * n;
            for oy in -1..=1 {
                for ox in -1..=1 {
                    let x = px as i32 + ox;
                    let y = py as i32 + oy;
                    if x >= 0 && y >= 0 && x < N as i32 && y < N as i32 {
                        img.set_pixel(x as u32, y as u32, road);
                    }
                }
            }
        }
    }

    let tex = Texture2D::from_image(&img);
    tex.set_filter(FilterMode::Linear);
    RADAR_TEX.with(|c| {
        *c.borrow_mut() = Some((key.0, key.1, key.2, key.3, tex.clone()));
    });
    tex
}

/// Top-right radar. Retail defaults to a circular, heading-up local radar with
/// rotating N/S/E/W markers and the course baked into the terrain; `M` toggles
/// the full-course wireframe.
pub fn minimap(
    ui: &Ui,
    track: &Track,
    course: &[Vec3],
    rivals: &[Vec3],
    player: Vec3,
    forward: Vec3,
    x: f32,
    y: f32,
    size: f32,
) {
    if is_key_pressed(KeyCode::M) {
        RADAR_MODE.with(|c| c.set(!c.get()));
    }
    let (cx, cy) = (x + size * 0.5, y + size * 0.5);
    let mut fwd = vec2(forward.x, forward.z).normalize_or_zero();
    if fwd.length_squared() < 1e-6 {
        fwd = vec2(0.0, -1.0);
    }
    let right = vec2(-fwd.y, fwd.x);
    if RADAR_MODE.with(|c| c.get()) {
        draw_circle(cx, cy, size * 0.53, Color::new(0.01, 0.02, 0.05, 0.8));
        let tex = radar_texture(track, player, fwd, course);
        draw_texture_ex(
            &tex,
            x,
            y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(size, size)),
                ..Default::default()
            },
        );
        draw_circle_lines(cx, cy, size * 0.5, 2.5, Color::new(0.88, 0.85, 0.75, 0.95));
        draw_circle_lines(
            cx,
            cy,
            size * 0.5 - 4.0,
            1.0,
            Color::new(0.1, 0.12, 0.18, 0.8),
        );
        for i in 0..36 {
            let a = i as f32 / 36.0 * std::f32::consts::TAU;
            let (s, c) = a.sin_cos();
            let long = i % 9 == 0;
            let r0 = size * 0.5 - if long { 8.0 } else { 4.0 };
            let r1 = size * 0.5 - 1.0;
            draw_line(
                cx + s * r0,
                cy - c * r0,
                cx + s * r1,
                cy - c * r1,
                if long { 2.0 } else { 1.0 },
                Color::new(0.85, 0.82, 0.72, 0.9),
            );
        }

        // Compass letters rotate with the chassis: "up" is always forward.
        for (label, d) in [
            ("N", vec2(0.0, -1.0)),
            ("E", vec2(1.0, 0.0)),
            ("S", vec2(0.0, 1.0)),
            ("W", vec2(-1.0, 0.0)),
        ] {
            let sx = d.dot(right);
            let sy = -d.dot(fwd);
            let px = cx + sx * size * 0.44;
            let py = cy + sy * size * 0.44;
            text_o(
                ui,
                label,
                px - width(ui, label, 17.0) * 0.5,
                py + 6.0,
                17.0,
                WHITE,
            );
        }

        // Local trucks: dots at their offset from the player, inside the ring.
        for o in rivals {
            let d = vec2(o.x - player.x, o.z - player.z);
            let dx = d.dot(right) / 220.0 * size * 0.5;
            let dy = -d.dot(fwd) / 220.0 * size * 0.5;
            if dx * dx + dy * dy <= (size * 0.46).powi(2) {
                draw_circle(cx + dx, cy + dy, 4.0, Color::new(0.95, 0.18, 0.12, 1.0));
            }
        }
        // Player chevron, always pointing up in the heading-up map.
        draw_triangle(
            vec2(cx, cy - 9.0),
            vec2(cx - 6.0, cy + 6.0),
            vec2(cx + 6.0, cy + 6.0),
            Color::new(0.08, 0.32, 0.08, 1.0),
        );
        draw_triangle(
            vec2(cx, cy - 6.0),
            vec2(cx - 4.0, cy + 4.0),
            vec2(cx + 4.0, cy + 4.0),
            GOOD,
        );
        return;
    }

    // Wireframe fallback (toggled with `M`).
    if course.len() < 2 {
        return;
    }
    let (mut lo, mut hi) = (vec2(f32::MAX, f32::MAX), vec2(f32::MIN, f32::MIN));
    for p in course {
        lo = lo.min(vec2(p.x, p.z));
        hi = hi.max(vec2(p.x, p.z));
    }
    let mid = (lo + hi) * 0.5;
    let span = ((hi - lo).max_element() * 0.5).max(60.0);
    let map = |p: Vec3| -> Vec2 {
        vec2(
            x + size * 0.5 + (p.x - mid.x) / span * size * 0.48,
            y + size * 0.5 - (p.z - mid.y) / span * size * 0.48,
        )
    };
    let step = 3.min(course.len() - 1).max(1);
    for (w, col) in [(5.0, Color::new(0.0, 0.0, 0.0, 0.55)), (2.5, WHITE)] {
        let mut i = 0;
        while i < course.len() {
            let a = map(course[i]);
            let b = map(course[(i + step) % course.len()]);
            draw_line(a.x, a.y, b.x, b.y, w, col);
            i += step;
        }
    }
    let s0 = map(course[0]);
    draw_rectangle(s0.x - 4.0, s0.y - 4.0, 8.0, 8.0, WHITE);
    for o in rivals {
        let p = map(*o);
        draw_circle(p.x, p.y, 4.5, Color::new(0.9, 0.15, 0.1, 1.0));
    }
    let p = map(player);
    draw_circle(p.x, p.y, 5.5, BLACK);
    draw_circle(p.x, p.y, 4.5, YELLOW);
}

/// Arrow pointing at the next checkpoint: green when you're heading towards
/// it, red when not.
/// Wrong-way threshold matching retail 4x4 Evolution (turns red past 110 deg).
const FINDER_RED_RAD: f32 = 110.0 * std::f32::consts::PI / 180.0;

fn finder_is_ahead(ang: f32) -> bool {
    ang.abs() <= FINDER_RED_RAD
}

/// Retail HUD checkpoint compass arrow: 3D-shaded bevel arrow turning green
/// toward the next checkpoint and red when facing more than 110 deg away.
pub fn finder(target: Vec3, pos: Vec3, forward: Vec3, right: Vec3, cx: f32, cy: f32) {
    let to = vec3(target.x - pos.x, 0.0, target.z - pos.z);
    let fwd = vec3(forward.x, 0.0, forward.z).normalize_or_zero();
    let r = vec3(right.x, 0.0, right.z).normalize_or_zero();
    let ang = to.dot(r).atan2(to.dot(fwd)); // 0 = straight ahead, + = to the right
    let (lit_col, shade_col, rim_col) = if finder_is_ahead(ang) {
        (
            Color::new(0.25, 0.96, 0.34, 1.0),
            Color::new(0.06, 0.56, 0.14, 1.0),
            Color::new(0.04, 0.32, 0.08, 1.0),
        )
    } else {
        (
            Color::new(0.98, 0.24, 0.18, 1.0),
            Color::new(0.60, 0.08, 0.06, 1.0),
            Color::new(0.35, 0.04, 0.04, 1.0),
        )
    };
    let d = vec2(ang.sin(), -ang.cos());
    let n = vec2(-d.y, d.x);
    let c = vec2(cx, cy);
    // Drop shadow beneath the 3D faceted arrow.
    let sc = c + vec2(3.0, 4.0);
    let shad = Color::new(0.0, 0.0, 0.0, 0.52);
    let s_tip = sc + d * 36.0;
    let s_base = sc - d * 24.0;
    draw_triangle(s_tip, sc + n * 23.0, sc - n * 23.0, shad);
    draw_triangle(sc + n * 9.5, sc - n * 9.5, s_base + n * 9.5, shad);
    draw_triangle(s_base + n * 9.5, s_base - n * 9.5, sc - n * 9.5, shad);

    // 3D beveled arrow facets: bright left facet, darker right facet, crisp rim.
    let tip = c + d * 35.0;
    let wing_l = c - n * 22.0;
    let wing_r = c + n * 22.0;
    let stem_l = c - n * 9.0;
    let stem_r = c + n * 9.0;
    let base_l = c - d * 24.0 - n * 9.0;
    let base_r = c - d * 24.0 + n * 9.0;
    let base_c = c - d * 24.0;

    // Head: left bright facet + right shaded facet.
    draw_triangle(tip, c, wing_l, lit_col);
    draw_triangle(tip, wing_r, c, shade_col);
    // Stem: left bright facet + right shaded facet.
    draw_triangle(c, base_c, stem_l, lit_col);
    draw_triangle(stem_l, base_c, base_l, lit_col);
    draw_triangle(stem_r, c, base_r, shade_col);
    draw_triangle(c, base_c, base_r, shade_col);
    // Subtle center spine highlight.
    draw_line(tip.x, tip.y, base_c.x, base_c.y, 1.5, rim_col);
}

/// Timing block top-left, yellow labels and white values, no panel.
pub fn timing(ui: &Ui, rows: &[(&str, String)], delta: Option<f32>) {
    let (x, x2) = (22.0, 150.0);
    let mut y = 38.0;
    for (i, (label, value)) in rows.iter().enumerate() {
        text_o(ui, label, x, y, 24.0, YELLOW);
        text_o(ui, value, x2, y, 24.0, WHITE);
        if i == 0 {
            if let Some(d) = delta {
                let (t, c) = if d <= 0.0 {
                    (format!("-{:.2}", -d), GOOD)
                } else {
                    (format!("+{:.2}", d), BAD)
                };
                text_o(ui, &t, x2 + 120.0, y, 24.0, c);
            }
        }
        y += 28.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_format_matches_original() {
        assert_eq!(fmt_time(0.0), "0:00.00");
        assert_eq!(fmt_time(83.456), "1:23.46");
        assert_eq!(fmt_time(-3.0), "0:00.00");
    }

    #[test]
    fn wrap_keeps_lines_short() {
        let l = wrap_text("Quick Race allows you to run a race on any track.", 20);
        assert!(l.iter().all(|s| s.len() <= 20));
        assert_eq!(
            l.join(" "),
            "Quick Race allows you to run a race on any track."
        );
    }

    #[test]
    fn gauge_needle_sweeps_270_degrees() {
        assert!((dial_angle(0.0, 140.0) - 225.0).abs() < 1e-4);
        assert!((dial_angle(70.0, 140.0) - 90.0).abs() < 1e-4);
        assert!((dial_angle(140.0, 140.0) + 45.0).abs() < 1e-4);
        assert!((dial_angle(200.0, 140.0) + 45.0).abs() < 1e-4);
    }

    #[test]
    fn finder_turns_red_past_110_degrees() {
        assert!(finder_is_ahead(0.0));
        assert!(finder_is_ahead(105.0_f32.to_radians()));
        assert!(!finder_is_ahead(115.0_f32.to_radians()));
        assert!(!finder_is_ahead(-130.0_f32.to_radians()));
    }
}
