//! Immediate-mode widget kit drawn with the extracted Evo 2 interface art.
//!
//! Keyboard focus is authoritative (the original shipped with keyboard and
//! pad-first menus); mouse hover can move focus and click activates. Widgets
//! take a `&mut Focus` so the app owns navigation state and screens stay
//! stateless.

use macroquad::prelude::*;

use crate::assets::Assets;

pub mod screens;
pub mod touch_menu;

#[derive(Default)]
pub struct Focus {
    pub index: usize,
}

impl Focus {
    pub fn reset(&mut self) {
        self.index = 0;
    }

    /// Move with wrap-around; returns true when the focus changed.
    pub fn step(&mut self, delta: i32, count: usize) -> bool {
        if count == 0 {
            return false;
        }
        let next = (self.index as i32 + delta).rem_euclid(count as i32);
        let changed = next as usize != self.index;
        self.index = next as usize;
        changed
    }

    pub fn clamp(&mut self, count: usize) {
        if count == 0 {
            self.index = 0;
        } else {
            self.index = self.index.min(count - 1);
        }
    }
}

/// Glyph rectangles in the extracted `4X4FONT` sheet, indexed from code 33
/// (`!`) in reading order. The sheet is packed at roughly 12 px per glyph, so
/// each rect is `(x, y, width)` inside a 20 px row. Space is tabulated as a
/// fixed advance. Entries were checked against the ink runs of the sheet's
/// opacity mask (`@`, `W`-`Z`, `[`-`` ` ``, `g`, `j`-`l` had slipped onto
/// their neighbours).
const GLYPHS: [(u16, u16, u16); 92] = [
    (3, 0, 3),
    (12, 0, 7),
    (24, 0, 9),
    (37, 0, 9),
    (51, 0, 13),
    (70, 0, 11),
    (86, 0, 3),
    (94, 0, 5),
    (104, 0, 4),
    (113, 0, 6),
    (124, 0, 9),
    (138, 0, 3),
    (147, 0, 5),
    (157, 0, 3),
    (165, 0, 5),
    (177, 0, 9),
    (191, 0, 6),
    (204, 0, 9),
    (218, 0, 8),
    (231, 0, 9),
    (245, 0, 8),
    (1, 20, 9),
    (15, 20, 8),
    (28, 20, 9),
    (42, 20, 8),
    (52, 20, 3),
    (62, 20, 3),
    (71, 20, 8),
    (84, 20, 9),
    (98, 20, 9),
    (112, 20, 9),
    (126, 20, 16),
    (145, 20, 11),
    (162, 20, 10),
    (177, 20, 11),
    (194, 20, 10),
    (210, 20, 9),
    (225, 20, 9),
    (239, 20, 11),
    (1, 40, 10),
    (13, 40, 3),
    (18, 40, 9),
    (29, 40, 10),
    (41, 40, 9),
    (52, 40, 12),
    (66, 40, 10),
    (78, 40, 12),
    (92, 40, 9),
    (103, 40, 12),
    (117, 40, 11),
    (130, 40, 11),
    (143, 40, 10),
    (155, 40, 9),
    (167, 40, 11),
    (182, 40, 15),
    (202, 40, 10),
    (217, 40, 10),
    (232, 40, 10),
    (1, 60, 5),
    (10, 60, 4),
    (18, 60, 5),
    (29, 60, 8),
    (42, 60, 10),
    (56, 60, 4),
    (64, 60, 9),
    (78, 60, 9),
    (92, 60, 9),
    (106, 60, 8),
    (120, 60, 8),
    (133, 60, 6),
    (143, 60, 9),
    (158, 60, 7),
    (172, 60, 4),
    (179, 60, 5),
    (190, 60, 8),
    (204, 60, 3),
    (212, 60, 13),
    (229, 60, 8),
    (243, 60, 9),
    (1, 80, 9),
    (15, 80, 9),
    (30, 80, 6),
    (40, 80, 8),
    (53, 80, 6),
    (64, 80, 8),
    (77, 80, 9),
    (90, 80, 14),
    (108, 80, 9),
    (121, 80, 9),
    (135, 80, 8),
    (145, 80, 6),
    (157, 80, 2),
];

/// Height, in UI units, that menus are authored for. On a phone the screen is
/// far denser than the 640x480-class menus the retail art and text sizes were
/// made for, so menu screens are drawn through a camera that shows this many
/// units of height; everything (layout, text, hit tests) then scales together.
const UI_REF_H: f32 = 700.0;

/// UI scale factor: real pixels per UI unit. 1.0 on desktop; on Android the
/// screen height divided by `UI_REF_H`. `EVOX_UI_REF=<height>` forces a
/// reference height on any platform (desktop checks of the phone layout).
pub fn k() -> f32 {
    let r = std::env::var("EVOX_UI_REF")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .or(if cfg!(target_os = "android") {
            Some(UI_REF_H)
        } else {
            None
        });
    r.map_or(1.0, |r| (screen_height() / r.max(100.0)).max(1.0))
}

/// Screen width in UI units.
pub fn vw() -> f32 {
    screen_width() / k()
}

/// Screen height in UI units.
pub fn vh() -> f32 {
    screen_height() / k()
}

/// Pointer position in UI units.
pub fn mouse() -> Vec2 {
    Vec2::from(mouse_position()) / k()
}

/// Draw the following UI through the scaled camera (no-op at scale 1).
pub fn begin_scaled() {
    if k() > 1.0 {
        // `from_display_rect` is built for render targets (y up); the screen
        // here is y down like every other UI coordinate, so flip it back.
        let mut cam = Camera2D::from_display_rect(Rect::new(0.0, 0.0, vw(), vh()));
        cam.zoom.y = -cam.zoom.y;
        set_camera(&cam);
    }
}

/// After a menu screen drew into an off-screen target (3D truck preview), put
/// the UI camera back: plain default camera at scale 1, the scaled one on phones.
pub fn restore_menu_camera() {
    if k() > 1.0 {
        begin_scaled();
    } else {
        set_default_camera();
    }
}

/// Back to real pixels.
pub fn end_scaled() {
    set_default_camera();
}

// ---------------------------------------------------------------- touch --

/// Finger-friendly menus: big targets, tap-on-release, swipe-to-scroll.
/// Always on for Android; `EVOX_TOUCH_UI=1` turns it on elsewhere.
pub fn touch_ui() -> bool {
    cfg!(target_os = "android") || std::env::var("EVOX_TOUCH_UI").is_ok_and(|v| v == "1")
}

/// Pointer travel (UI units) beyond which a press is a drag, not a tap.
const TAP_SLOP: f32 = 14.0;

#[derive(Default)]
struct Pointer {
    down: bool,
    start: Vec2,
    last: Vec2,
    travelled: f32,
    /// This frame's vertical movement while a press is held.
    dy: f32,
    /// `(start, end, travelled)` on the frame the press ended.
    released: Option<(Vec2, Vec2, f32)>,
    /// Row accumulator for `swipe_rows`.
    acc: f32,
}

static POINTER: std::sync::Mutex<Pointer> = std::sync::Mutex::new(Pointer {
    down: false,
    start: Vec2::ZERO,
    last: Vec2::ZERO,
    travelled: 0.0,
    dy: 0.0,
    released: None,
    acc: 0.0,
});

/// Advance the pointer state; call once per frame before any UI is built.
pub fn tick() {
    let pos = mouse();
    let mut p = POINTER.lock().unwrap();
    p.released = None;
    p.dy = 0.0;
    if is_mouse_button_pressed(MouseButton::Left) {
        p.down = true;
        p.start = pos;
        p.last = pos;
        p.travelled = 0.0;
        p.acc = 0.0;
    } else if p.down && is_mouse_button_down(MouseButton::Left) {
        p.dy = pos.y - p.last.y;
        p.travelled = p.travelled.max((pos - p.start).length());
        p.last = pos;
    }
    if p.down && is_mouse_button_released(MouseButton::Left) {
        p.down = false;
        p.released = Some((p.start, pos, p.travelled));
    }
}

/// A click/tap on `r` this frame. Touch UI confirms on release (press and
/// release both inside `r`, barely moved) so a swipe never activates a row;
/// with a mouse it fires on press while hovering, as before.
pub fn tap(r: Rect) -> bool {
    if touch_ui() {
        let p = POINTER.lock().unwrap();
        p.released
            .is_some_and(|(s, e, t)| t < TAP_SLOP && r.contains(s) && r.contains(e))
    } else {
        r.contains(mouse()) && is_mouse_button_pressed(MouseButton::Left)
    }
}

/// Where the last tap ended (the pointer position when nothing was released).
pub fn mouse_for_tap() -> Vec2 {
    POINTER
        .lock()
        .unwrap()
        .released
        .map_or_else(mouse, |(_, e, _)| e)
}

/// True while a press has travelled far enough to be a swipe.
pub fn is_dragging() -> bool {
    let p = POINTER.lock().unwrap();
    p.down && p.travelled >= TAP_SLOP
}

static BACK_REQUEST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Ask `App::back` to step out of the current screen (the on-screen "Main
/// Menu" label is the touch equivalent of Esc).
pub fn request_back() {
    BACK_REQUEST.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Consume a pending back request.
pub fn take_back() -> bool {
    BACK_REQUEST.swap(false, std::sync::atomic::Ordering::SeqCst)
}

/// Is a finger held on `r` right now (for pressed-state highlights)?
pub fn held(r: Rect) -> bool {
    let p = POINTER.lock().unwrap();
    p.down && r.contains(p.start) && r.contains(p.last)
}

/// Whole rows dragged this frame by a press that began inside `r`: positive
/// when the content should move down the list (finger dragged up).
pub fn swipe_rows(r: Rect, row_h: f32) -> i32 {
    let mut p = POINTER.lock().unwrap();
    if !(p.down && r.contains(p.start)) {
        return 0;
    }
    if p.travelled < TAP_SLOP {
        return 0;
    }
    let dy = p.dy;
    p.acc -= dy;
    let steps = (p.acc / row_h.max(1.0)).trunc();
    p.acc -= steps * row_h;
    steps as i32
}

pub struct Ui {
    pub scale: f32,
    font: Option<Texture2D>,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            scale: 1.0,
            font: None,
        }
    }
}

impl Ui {
    pub fn set_font(&mut self, tex: Texture2D) {
        self.font = Some(tex);
    }

    pub fn has_font(&self) -> bool {
        self.font.is_some()
    }

    /// Advance width of a string at `size` pixels.
    pub fn width(&self, size: f32, text: &str) -> f32 {
        let s = size * self.scale / 16.0;
        text.chars()
            .map(|c| {
                let code = c as u32;
                if code == 32 {
                    4.0 * s
                } else if let Some(g) = GLYPHS.get(code.saturating_sub(33) as usize) {
                    (g.2 as f32 + 1.0) * s
                } else {
                    5.0 * s
                }
            })
            .sum()
    }

    pub fn text(&self, x: f32, y: f32, size: f32, text: &str, color: Color) {
        let Some(font) = &self.font else {
            draw_text(text, x, y, size * self.scale, color);
            return;
        };
        let s = size * self.scale / 16.0;
        let mut cx = x;
        for c in text.chars() {
            let code = c as u32;
            if code == 32 {
                cx += 4.0 * s;
                continue;
            }
            let Some(g) = GLYPHS.get(code.saturating_sub(33) as usize) else {
                cx += 5.0 * s;
                continue;
            };
            let (gx, gy, gw) = (g.0 as f32, g.1 as f32, g.2 as f32);
            // The sheet stores lowercase letters 3 px higher in their cells
            // than capitals and digits (baseline 11 vs 14, measured from the
            // opacity mask); drop them onto the common baseline.
            let drop = if c.is_ascii_lowercase() { 3.0 * s } else { 0.0 };
            draw_texture_ex(
                font,
                cx,
                y + s * 2.0 + drop,
                color,
                DrawTextureParams {
                    source: Some(Rect::new(gx, gy, gw, 20.0)),
                    dest_size: Some(vec2(gw * s, 20.0 * s)),
                    ..Default::default()
                },
            );
            cx += (gw + 1.0) * s;
        }
    }

    /// `text` cut to `max_w` pixels at `size`, ending in "..." when cut.
    pub fn fit(&self, size: f32, text: &str, max_w: f32) -> String {
        if self.width(size, text) <= max_w {
            return text.to_string();
        }
        let dots = self.width(size, "...");
        let mut out = String::new();
        for c in text.chars() {
            out.push(c);
            if self.width(size, &out) + dots > max_w {
                out.pop();
                break;
            }
        }
        format!("{}...", out.trim_end())
    }

    pub fn centered(&self, cx: f32, y: f32, size: f32, text: &str, color: Color) {
        self.text(cx - self.width(size, text) * 0.5, y, size, text, color);
    }

    /// Nine-slice panel from an extracted frame texture.
    /// Nine-slice plate from the original menu art: the Evo 2 `button3`
    /// strip when converted, else the extracted Evo 1 `BUTTON3`. (`frame`
    /// names a specific extracted plate; the old `EDBTN*` names were the
    /// track editor's toolbar icons and never resolved.)
    pub fn panel(&self, assets: &Assets, frame: &str, r: Rect, tint: Color) {
        let te = crate::classic::ui2("button3")
            .or_else(|| assets.tex_now(frame).cloned())
            .or_else(|| assets.tex_now("BUTTON3").cloned());
        let Some(tex) = te else {
            draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.05, 0.06, 0.08, tint.a));
            return;
        };
        let tex = &tex;
        let (tw, th) = (tex.width(), tex.height());
        let b = (tw * 0.28).min(r.w * 0.3).min(th);
        let cols = [(0.0, b), (b, (tw - 2.0 * b).max(1.0)), (tw - b, b)];
        let rows = [(0.0, b), (b, (th - 2.0 * b).max(1.0)), (th - b, b)];
        let mut dx = 0.0;
        for (ci, (sx, sw)) in cols.iter().enumerate() {
            let dw = match ci {
                1 => r.w - 2.0 * b,
                _ => b,
            };
            let mut dy = 0.0;
            for (ri, (sy, sh)) in rows.iter().enumerate() {
                let dh = match ri {
                    1 => r.h - 2.0 * b,
                    _ => b,
                };
                if dw > 0.0 && dh > 0.0 {
                    draw_texture_ex(
                        tex,
                        r.x + dx,
                        r.y + dy,
                        tint,
                        DrawTextureParams {
                            dest_size: Some(vec2(dw, dh)),
                            source: Some(Rect::new(*sx, *sy, *sw, *sh)),
                            ..Default::default()
                        },
                    );
                }
                dy += dh;
            }
            dx += dw;
        }
    }

    /// Returns true when activated this frame. `focused` drives the highlight.
    pub fn button(
        &self,
        assets: &Assets,
        r: Rect,
        label: &str,
        focused: bool,
        enabled: bool,
    ) -> bool {
        let hover = r.contains(mouse());
        // One plate, state shown by tint (the original menu buttons).
        let frame = "BUTTON3";
        let tint = if !enabled {
            Color::new(0.45, 0.47, 0.52, 0.85)
        } else if focused || hover {
            Color::new(1.35, 1.12, 0.62, 1.0)
        } else {
            Color::new(0.62, 0.66, 0.74, 0.95)
        };
        self.panel(assets, frame, r, tint);
        let color = if !enabled {
            Color::new(0.55, 0.55, 0.6, 1.0)
        } else if focused || hover {
            Color::new(1.0, 0.95, 0.55, 1.0)
        } else {
            Color::new(0.92, 0.92, 0.95, 1.0)
        };
        self.centered(r.x + r.w * 0.5, r.y + r.h * 0.5 - 17.0, 24.0, label, color);
        enabled && (focused && is_key_pressed(KeyCode::Enter) || tap(r))
    }

    /// Small row item for lists (garage parts, lobbies, tracks).
    pub fn row(
        &self,
        assets: &Assets,
        r: Rect,
        title: &str,
        sub: &str,
        focused: bool,
        enabled: bool,
    ) -> bool {
        let bg = if focused {
            Color::new(0.16, 0.22, 0.34, 0.92)
        } else {
            Color::new(0.04, 0.05, 0.07, 0.72)
        };
        draw_rectangle(r.x, r.y, r.w, r.h, bg);
        if focused {
            draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, Color::new(0.95, 0.78, 0.25, 1.0));
            let c = assets.tex_now("CURSOR");
            if let Some(t) = c {
                draw_texture_ex(
                    t,
                    r.x - 30.0,
                    r.y + r.h * 0.5 - 12.0,
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(vec2(24.0, 24.0)),
                        ..Default::default()
                    },
                );
            }
        }
        let dim = if enabled {
            WHITE
        } else {
            Color::new(0.55, 0.55, 0.6, 1.0)
        };
        // One line per row: title left, note right-aligned. When both do not
        // fit, the note keeps at least 30 % of the row and each side is cut
        // with "..." instead of drawing over the other.
        let pad = 12.0;
        let gap = 18.0;
        let inner = r.w - 2.0 * pad;
        let title_w = self.width(20.0, title);
        let sub_room = (inner - title_w - gap).max(inner * 0.30);
        let sub = self.fit(14.0, sub, sub_room);
        let sub_w = if sub.is_empty() {
            0.0
        } else {
            self.width(14.0, &sub) + gap
        };
        let title = self.fit(20.0, title, inner - sub_w);
        let ty = r.y + (r.h - 28.0) * 0.5;
        self.text(r.x + pad, ty, 20.0, &title, dim);
        if !sub.is_empty() {
            let w = self.width(14.0, &sub);
            self.text(
                r.x + r.w - w - pad,
                r.y + (r.h - 20.0) * 0.5,
                14.0,
                &sub,
                Color::new(0.72, 0.76, 0.84, dim.a),
            );
        }
        enabled && (focused && is_key_pressed(KeyCode::Enter) || tap(r))
    }

    pub fn tab(
        &self,
        assets: &Assets,
        r: Rect,
        icon: &str,
        label: &str,
        active: bool,
        focused: bool,
    ) -> bool {
        let tint = if active {
            Color::new(1.0, 0.92, 0.55, 1.0)
        } else if focused {
            WHITE
        } else {
            Color::new(0.62, 0.66, 0.74, 1.0)
        };
        if active {
            draw_rectangle(
                r.x,
                r.y + r.h - 3.0,
                r.w,
                3.0,
                Color::new(0.95, 0.78, 0.25, 1.0),
            );
        }
        if let Some(t) = assets.tex_now(icon) {
            let s = r.w.min(44.0);
            draw_texture_ex(
                t,
                r.x + (r.w - s) * 0.5,
                r.y + 4.0,
                tint,
                DrawTextureParams {
                    dest_size: Some(vec2(s, s)),
                    ..Default::default()
                },
            );
        }
        self.centered(r.x + r.w * 0.5, r.y + r.h - 8.0, 13.0, label, tint);
        tap(r)
    }

    pub fn stat_bar(
        &self,
        x: f32,
        y: f32,
        w: f32,
        label: &str,
        value: f32,
        max: f32,
        good_high: bool,
        color: Color,
    ) {
        self.text(x, y, 14.0, label, Color::new(0.8, 0.84, 0.9, 1.0));
        let t = (value / max.max(1.0)).clamp(0.0, 1.0);
        draw_rectangle(x + 92.0, y + 5.0, w, 10.0, Color::new(0.1, 0.11, 0.14, 1.0));
        let c = if good_high {
            color
        } else {
            Color::new(1.0, 0.45, 0.35, 1.0)
        };
        draw_rectangle(x + 92.0, y + 5.0, w * t, 10.0, c);
    }

    pub fn money(&self, x: f32, y: f32, cents: i64) {
        let s = format!("${}", cents);
        self.text(x, y, 26.0, &s, Color::new(0.55, 0.95, 0.55, 1.0));
    }

    pub fn text_click(&self, r: Rect) -> bool {
        tap(r)
    }

    pub fn hint(&self, text: &str) {
        let s = format!("Enter Select   Esc Back   Arrows Move   {text}");
        draw_rectangle(
            0.0,
            vh() - 30.0,
            vw(),
            30.0,
            Color::new(0.0, 0.0, 0.0, 0.55),
        );
        self.text(
            14.0,
            vh() - 26.0,
            15.0,
            &s,
            Color::new(0.75, 0.78, 0.85, 1.0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_cuts_with_dots_and_keeps_short_text() {
        let ui = Ui::default();
        assert_eq!(ui.fit(20.0, "Garage", 500.0), "Garage");
        let cut = ui.fit(20.0, "Chevrolet Blazer TrailBlazer 4-Dr", 120.0);
        assert!(cut.ends_with("..."), "{cut}");
        assert!(
            ui.width(20.0, &cut) <= 120.0,
            "{cut} is wider than the room"
        );
    }
}
