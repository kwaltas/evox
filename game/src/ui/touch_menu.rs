//! Finger-sized replacements for the menu screens that were laid out for a
//! mouse (Quick Race, Garage). Same data and actions as the desktop screens;
//! only the controls change: fat tap targets, `< value >` steppers instead of
//! tiny arrows, swipe-to-scroll lists, no hover or keyboard focus.
//!
//! Used when `ui::touch_ui()` is on (always on Android).

use macroquad::prelude::*;

use crate::app::App;
use crate::classic::{menu_frame, text_c, text_o, ui2, MENU_HEAD, MENU_SEL};
use crate::ui::{swipe_rows, tap, vh, vw};

const PLATE_IDLE: Color = Color::new(0.80, 0.84, 0.98, 0.92);
const PLATE_HOT: Color = Color::new(1.35, 1.15, 0.65, 1.0);
const CARD: Color = Color::new(0.04, 0.05, 0.09, 0.92);
const EDGE: Color = Color::new(0.5, 0.58, 0.78, 0.8);

/// Menu plate with the retail button art, brighter while a finger is down.
fn plate(r: Rect, hot: bool) {
    if let Some(b) = ui2("button3") {
        draw_texture_ex(
            &b,
            r.x,
            r.y,
            if hot { PLATE_HOT } else { PLATE_IDLE },
            DrawTextureParams {
                dest_size: Some(vec2(r.w, r.h)),
                ..Default::default()
            },
        );
    } else {
        draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.1, 0.12, 0.2, 0.9));
    }
}

/// Big labelled button. True when tapped (and enabled).
fn big_button(app: &App, r: Rect, label: &str, size: f32, enabled: bool) -> bool {
    plate(r, enabled && crate::ui::held(r));
    let c = if enabled {
        Color::new(1.0, 0.95, 0.62, 1.0)
    } else {
        Color::new(0.55, 0.55, 0.6, 1.0)
    };
    text_c(
        &app.ui,
        label,
        r.x + r.w * 0.5,
        r.y + r.h * 0.5 + size * 0.45,
        size,
        c,
    );
    enabled && tap(r)
}

/// `<  value  >` control. Tapping the left third steps back, anywhere else
/// steps forward. Returns -1, 0 or +1.
fn stepper(app: &App, r: Rect, caption: &str, value: &str) -> i32 {
    plate(r, crate::ui::held(r));
    let mid = r.y + r.h * 0.5;
    let tri = r.h * 0.22;
    let (lx, rx) = (r.x + r.w * 0.09, r.x + r.w * 0.91);
    draw_triangle(
        vec2(lx - tri * 0.6, mid),
        vec2(lx + tri * 0.6, mid - tri),
        vec2(lx + tri * 0.6, mid + tri),
        MENU_SEL,
    );
    draw_triangle(
        vec2(rx + tri * 0.6, mid),
        vec2(rx - tri * 0.6, mid - tri),
        vec2(rx - tri * 0.6, mid + tri),
        MENU_SEL,
    );
    if !caption.is_empty() {
        text_c(
            &app.ui,
            caption,
            r.x + r.w * 0.5,
            r.y + r.h * 0.30,
            15.0,
            MENU_HEAD,
        );
    }
    text_c(
        &app.ui,
        value,
        r.x + r.w * 0.5,
        r.y + r.h * if caption.is_empty() { 0.62 } else { 0.78 },
        22.0,
        Color::new(1.0, 0.95, 0.62, 1.0),
    );
    if !tap(r) {
        return 0;
    }
    let p = crate::ui::mouse_for_tap();
    if p.x < r.x + r.w * 0.33 {
        -1
    } else {
        1
    }
}

/// Scrollbar thumb for a list of `total` rows of which `shown` are visible.
fn scrollbar(r: Rect, first: usize, shown: usize, total: usize) {
    if total <= shown {
        return;
    }
    draw_rectangle(
        r.right() + 6.0,
        r.y,
        6.0,
        r.h,
        Color::new(1.0, 1.0, 1.0, 0.12),
    );
    let th = (r.h * shown as f32 / total as f32).max(24.0);
    let ty = r.y + (r.h - th) * first as f32 / (total - shown) as f32;
    draw_rectangle(
        r.right() + 6.0,
        ty,
        6.0,
        th,
        Color::new(0.95, 0.78, 0.25, 0.9),
    );
}

/// Render the selected truck into the shared off-screen bay and blit it.
fn truck_card(app: &mut App, truck: &crate::vehicle::Truck, card: Rect, grid: u32) {
    draw_rectangle(card.x, card.y, card.w, card.h, CARD);
    let mut shown = truck.clone();
    shown.paint_index = app.setup.paint % shown.paints.len().max(1);
    let spin = 0.85 + get_time() as f32 * 0.45;
    if let Some(rt) = &app.garage_rt {
        rt.texture.set_filter(FilterMode::Linear);
        let eye = vec3(0.0, 1.75, 6.4);
        set_camera(&Camera3D {
            position: eye,
            target: vec3(0.0, 0.95, 0.0),
            up: Vec3::Y,
            fovy: 36.0_f32.to_radians(),
            aspect: Some(card.w / card.h),
            render_target: Some(rt.clone()),
            ..Default::default()
        });
        clear_background(Color::new(0.05, 0.07, 0.11, 1.0));
        draw_grid(
            grid,
            0.5,
            Color::new(0.18, 0.22, 0.28, 1.0),
            Color::new(0.11, 0.13, 0.18, 1.0),
        );
        app.render
            .draw_truck_preview(&mut app.assets, &shown, Vec3::ZERO, spin, eye);
        crate::ui::restore_menu_camera();
        draw_texture_ex(
            &rt.texture,
            card.x,
            card.y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(card.w, card.h)),
                flip_y: true,
                ..Default::default()
            },
        );
    }
    draw_rectangle_lines(card.x, card.y, card.w, card.h, 1.5, EDGE);
}

/// The garage's trucks, optionally filtered by make (case-insensitive).
fn catalog(app: &App, brand: Option<&str>) -> Vec<crate::vehicle::Truck> {
    app.garage_files()
        .into_iter()
        .filter(|t| brand.is_none_or(|b| t.make.eq_ignore_ascii_case(b)))
        .cloned()
        .collect()
}

fn brands(app: &App) -> Vec<String> {
    let mut out = vec!["All".to_string()];
    let mut seen = std::collections::BTreeSet::new();
    for t in app.garage_files() {
        let m = t.make.trim();
        if !m.is_empty() && seen.insert(m.to_ascii_lowercase()) {
            out.push(m.to_string());
        }
    }
    out[1..].sort_by_key(|b| b.to_ascii_lowercase());
    out
}

pub fn quick_race(app: &mut App) {
    let (sw, sh) = (vw(), vh());
    menu_frame(
        &app.ui,
        "Quick Race",
        "Main Menu",
        &app.profile.name,
        "bg_qr",
    );

    let top = sh * 0.125;
    let bottom = sh * 0.845;

    // ---- track list: six fat rows, swipe to scroll -----------------------
    const ROWS: usize = 6;
    let list = Rect::new(sw * 0.02, top, sw * 0.42, bottom - top);
    let row_h = list.h / ROWS as f32;
    let n = app.tracks.len();
    let max_first = n.saturating_sub(ROWS);
    let steps = swipe_rows(list, row_h);
    app.touch_first = (app.touch_first as i32 + steps).clamp(0, max_first as i32) as usize;
    if app.setup.track.is_empty() {
        if let Some((name, _)) = app.tracks.first() {
            app.setup.track = name.clone();
        }
    }
    for i in 0..ROWS {
        let idx = app.touch_first + i;
        let Some((name, _)) = app.tracks.get(idx).cloned() else {
            break;
        };
        let r = Rect::new(list.x, list.y + i as f32 * row_h + 3.0, list.w, row_h - 6.0);
        let sel = app.setup.track.eq_ignore_ascii_case(&name);
        plate(r, sel || crate::ui::held(r));
        text_o(
            &app.ui,
            &super::screens::track_label(app, &name),
            r.x + 22.0,
            r.y + r.h * 0.5 + 12.0,
            30.0,
            if sel {
                MENU_SEL
            } else {
                Color::new(1.0, 1.0, 1.0, 1.0)
            },
        );
        if tap(r) {
            app.setup.track = name;
            app.focus.index = idx;
            app.audio.ui_move(get_time());
        }
    }
    scrollbar(list, app.touch_first, ROWS, n);

    // ---- right column -----------------------------------------------------
    let x0 = sw * 0.48;
    let w = sw - x0 - sw * 0.02;
    let gap = 10.0;
    let half = (w - gap) * 0.5;

    // Cards: track preview + 3D truck.
    let card_h = 150.0;
    let trk = Rect::new(x0, top, half, card_h);
    let veh = Rect::new(x0 + half + gap, top, half, card_h);
    draw_rectangle(trk.x, trk.y, trk.w, trk.h, CARD);
    if let Some(p) = app.assets.track_preview(&app.setup.track) {
        draw_texture_ex(
            &p,
            trk.x + 2.0,
            trk.y + 2.0,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(trk.w - 4.0, trk.h - 30.0)),
                ..Default::default()
            },
        );
    }
    draw_rectangle_lines(trk.x, trk.y, trk.w, trk.h, 1.5, EDGE);
    text_c(
        &app.ui,
        &super::screens::track_label(app, &app.setup.track),
        trk.x + trk.w * 0.5,
        trk.y + trk.h - 8.0,
        20.0,
        MENU_SEL,
    );

    let brand_list = brands(app);
    app.setup.brand = app.setup.brand.min(brand_list.len() - 1);
    let brand = brand_list[app.setup.brand].clone();
    let cat = catalog(app, (app.setup.brand != 0).then_some(brand.as_str()));
    if !cat.is_empty() {
        app.selected_truck = app.selected_truck.min(cat.len() - 1);
        truck_card(app, &cat[app.selected_truck].clone(), veh, 16);
    }

    // Vehicle stepper (big).
    let mut y = top + card_h + gap;
    let veh_bar = Rect::new(x0, y, w, 84.0);
    let (name, sub) = cat.get(app.selected_truck).map_or_else(
        || ("No vehicles".to_string(), String::new()),
        |t| {
            (
                format!("{} {}", t.make, t.model),
                format!(
                    "{} hp  |  {} lb  |  {}/{}",
                    t.engine.max_hp as i32,
                    t.dry_weight_lb as i32,
                    app.selected_truck + 1,
                    cat.len()
                ),
            )
        },
    );
    let d = stepper(app, veh_bar, &sub, &name);
    if d != 0 && !cat.is_empty() {
        let n = cat.len() as i32;
        app.selected_truck = (app.selected_truck as i32 + d).rem_euclid(n) as usize;
        app.audio.ui_move(get_time());
    }
    y += veh_bar.h + gap;

    // Brand | Paint.
    let t_h = 72.0;
    let d = stepper(app, Rect::new(x0, y, half, t_h), "BRAND", &brand);
    if d != 0 {
        let n = brand_list.len() as i32;
        app.setup.brand = (app.setup.brand as i32 + d).rem_euclid(n) as usize;
        app.selected_truck = 0;
        app.setup.paint = 0;
        app.audio.ui_move(get_time());
    }
    let paint_r = Rect::new(x0 + half + gap, y, half, t_h);
    let paints = cat.get(app.selected_truck).map_or(0, |t| t.paints.len());
    let paint_name = cat
        .get(app.selected_truck)
        .and_then(|t| t.paints.get(app.setup.paint % t.paints.len().max(1)))
        .map(|p| (p.name.clone(), p.swatch));
    let paint_label = match &paint_name {
        Some((n, _)) if !n.is_empty() => {
            format!("{}  {}/{}", n, app.setup.paint % paints.max(1) + 1, paints)
        }
        Some(_) => format!("{}/{}", app.setup.paint % paints.max(1) + 1, paints),
        None => "factory".to_string(),
    };
    let d = stepper(app, paint_r, "PAINT", &paint_label);
    if let Some((_, s)) = &paint_name {
        draw_rectangle(
            paint_r.x + paint_r.w * 0.5 - 20.0,
            paint_r.y + paint_r.h - 12.0,
            40.0,
            7.0,
            Color::from_rgba(s[0], s[1], s[2], 255),
        );
    }
    if d != 0 && paints > 0 {
        app.setup.paint = (app.setup.paint as i32 + d).rem_euclid(paints as i32) as usize;
        app.audio.ui_move(get_time());
    }
    y += t_h + gap;

    // Class | Laps | Rivals.
    let third = (w - 2.0 * gap) / 3.0;
    let d = stepper(
        app,
        Rect::new(x0, y, third, t_h),
        "CLASS",
        &app.setup.class.to_string(),
    );
    app.setup.class = (app.setup.class as i32 + d).clamp(1, 5) as u8;
    let d = stepper(
        app,
        Rect::new(x0 + third + gap, y, third, t_h),
        "LAPS",
        &app.setup.laps.to_string(),
    );
    app.setup.laps = (app.setup.laps as i32 + d).clamp(1, 5) as u32;
    let d = stepper(
        app,
        Rect::new(x0 + 2.0 * (third + gap), y, third, t_h),
        "RIVALS",
        &app.setup.opponents.to_string(),
    );
    app.setup.opponents = (app.setup.opponents as i32 + d).clamp(1, 7) as usize;
    y += t_h + gap;

    // START.
    let ready = !app.setup.track.is_empty() && !cat.is_empty();
    let start = Rect::new(x0, y, w, (bottom - y).clamp(70.0, 96.0));
    if big_button(app, start, "START RACE", 40.0, ready) {
        app.audio.ui_click(get_time());
        app.start_loading(None);
    }
}

pub fn garage(app: &mut App) {
    let (sw, sh) = (vw(), vh());
    menu_frame(
        &app.ui,
        "Garage & Dealership",
        "Main Menu",
        &app.profile.name,
        "bg_parts",
    );

    let top = sh * 0.125;
    let bottom = sh * 0.845;
    let cat = catalog(app, None);
    app.focus.clamp(cat.len().max(1));

    const ROWS: usize = 6;
    let list = Rect::new(sw * 0.02, top, sw * 0.44, bottom - top);
    let row_h = list.h / ROWS as f32;
    let max_first = cat.len().saturating_sub(ROWS);
    let steps = swipe_rows(list, row_h);
    app.touch_first = (app.touch_first as i32 + steps).clamp(0, max_first as i32) as usize;
    // First visit: bring the selected truck into view.
    if app.selected_truck < app.touch_first || app.selected_truck >= app.touch_first + ROWS {
        if !crate::ui::is_dragging() {
            app.touch_first = app.selected_truck.min(max_first);
        }
    }
    for i in 0..ROWS {
        let idx = app.touch_first + i;
        let Some(t) = cat.get(idx) else { break };
        let r = Rect::new(list.x, list.y + i as f32 * row_h + 3.0, list.w, row_h - 6.0);
        let sel = app.selected_truck == idx;
        plate(r, sel || crate::ui::held(r));
        let owned = app.is_owned(t);
        text_o(
            &app.ui,
            &format!("{} {}", t.make, t.model),
            r.x + 22.0,
            r.y + r.h * 0.5 - 2.0,
            26.0,
            if sel {
                MENU_SEL
            } else {
                Color::new(1.0, 1.0, 1.0, 1.0)
            },
        );
        text_o(
            &app.ui,
            &if owned {
                "owned".to_string()
            } else {
                format!("{} hp", t.engine.max_hp as i32)
            },
            r.x + 22.0,
            r.y + r.h * 0.5 + 24.0,
            18.0,
            Color::new(0.78, 0.84, 0.94, 1.0),
        );
        if tap(r) {
            app.selected_truck = idx;
            app.focus.index = idx;
            app.audio.ui_move(get_time());
        }
    }
    scrollbar(list, app.touch_first, ROWS, cat.len());

    // Right column: bay, stats, buttons.
    let x0 = sw * 0.50;
    let w = sw - x0 - sw * 0.02;
    let truck = cat.get(app.selected_truck).cloned();
    let bay = Rect::new(x0, top, w, 230.0);
    if let Some(t) = &truck {
        truck_card(app, t, bay, 20);
        let info = Rect::new(x0, bay.bottom() + 8.0, w, 74.0);
        draw_rectangle(
            info.x,
            info.y,
            info.w,
            info.h,
            Color::new(0.02, 0.03, 0.08, 0.72),
        );
        draw_rectangle_lines(info.x, info.y, info.w, info.h, 1.5, EDGE);
        text_o(
            &app.ui,
            &format!("{} {}", t.make, t.model),
            info.x + 16.0,
            info.y + 32.0,
            26.0,
            Color::new(1.0, 0.95, 0.62, 1.0),
        );
        text_o(
            &app.ui,
            &format!(
                "{}  |  {} hp  |  {} lb",
                t.class_name, t.engine.max_hp as i32, t.dry_weight_lb as i32
            ),
            info.x + 16.0,
            info.y + 60.0,
            17.0,
            Color::new(0.82, 0.87, 0.94, 1.0),
        );
        text_o(
            &app.ui,
            &format!("${}", app.profile.budget),
            info.right() - 150.0,
            info.y + 32.0,
            24.0,
            Color::new(0.5, 1.0, 0.6, 1.0),
        );
    }
    let by = bay.bottom() + 8.0 + 74.0 + 10.0;
    let bh = ((bottom - by - 10.0) * 0.5).clamp(56.0, 84.0);
    if big_button(
        app,
        Rect::new(x0, by, w, bh),
        "PARTS SHOP",
        34.0,
        truck.is_some(),
    ) {
        app.audio.ui_click(get_time());
        app.open_shop();
    }
    if big_button(
        app,
        Rect::new(x0, by + bh + 10.0, w, bh),
        "TEST DRIVE",
        34.0,
        truck.is_some(),
    ) {
        app.audio.ui_click(get_time());
        if app.setup.track.is_empty() {
            app.setup.track = app.tracks.first().map(|t| t.0.clone()).unwrap_or_default();
        }
        app.start_loading(None);
    }
}
