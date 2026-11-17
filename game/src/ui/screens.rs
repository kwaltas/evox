//! Screen draw functions. Every screen is a projection of `App` state.

use macroquad::prelude::*;

use crate::app::{App, State};
use crate::parts::{Lock, PartType, PART_TYPES};
use crate::terrain::GRID;
use crate::ui::Focus;

fn vw() -> f32 {
    screen_width()
}
fn vh() -> f32 {
    screen_height()
}

fn bg(app: &App, name: &str) {
    let ui2_name = match name {
        "QUICK_RACE" => Some("bg_qr"),
        "CAREER" => Some("bg_career"),
        "GARAGE" | "GARAGE1" => Some("bg_parts"),
        "SETTINGS" => Some("bg_options"),
        "RESULTS" | "SHOWROOM" => Some("bg_results"),
        "MAIN" => Some("bg_main"),
        _ => None,
    };
    if let Some(u) = ui2_name.and_then(crate::classic::ui2) {
        let k = (vw() / u.width()).max(vh() / u.height());
        draw_texture_ex(
            &u,
            (vw() - u.width() * k) * 0.5,
            (vh() - u.height() * k) * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(u.width() * k, u.height() * k)),
                ..Default::default()
            },
        );
        draw_rectangle(0.0, 0.0, vw(), vh(), Color::new(0.02, 0.03, 0.08, 0.45));
        return;
    }
    let fallback = if name == "GARAGE1"
        || name == "QUICK_RACE"
        || name == "CAREER"
        || name == "GARAGE"
        || name == "SETTINGS"
    {
        "LOADING2"
    } else if name == "SHOWROOM" || name == "RESULTS" {
        "LOADING3"
    } else {
        name
    };
    if let Some(t) = app
        .assets
        .tex_now(fallback)
        .or_else(|| app.assets.tex_now("LOGO480"))
    {
        draw_texture_ex(
            t,
            0.0,
            0.0,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(vw(), vh())),
                ..Default::default()
            },
        );
        draw_rectangle(0.0, 0.0, vw(), vh(), Color::new(0.0, 0.0, 0.02, 0.45));
    }
}

/// Title bar in the original's style: the blue `BAR` band, the screen name
/// in outlined lettering, the parent screen on the right and the EVO 2 logo.
/// (The previous bar drew `LCD_EVO`, which is not in the extracted art, so
/// it showed the generated placeholder square.)
fn header(app: &mut App, title: &str) {
    let h = 70.0;
    draw_rectangle(0.0, 0.0, vw(), h, crate::classic::BAR);
    crate::classic::text_o(&app.ui, title, 28.0, 48.0, 34.0, crate::classic::MENU_HEAD);
    let parent = "Main Menu";
    let mut right = vw() - 28.0;
    if let Some(l) = crate::classic::ui2("logo") {
        let lh = h - 14.0;
        let lw = lh * l.width() / l.height();
        right -= lw;
        draw_texture_ex(
            &l,
            right,
            7.0,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(lw, lh)),
                ..Default::default()
            },
        );
        right -= 18.0;
    }
    let w = crate::classic::width(&app.ui, parent, 26.0);
    crate::classic::text_o(
        &app.ui,
        parent,
        right - w,
        46.0,
        26.0,
        crate::classic::MENU_SEL,
    );
}

/// Background photo of an original screen (`assets/ui2/<name>.png`), dimmed
/// so lettering reads; the extracted `GARAGE1` art when the Evo 2 set is not
/// converted.
fn screen_bg(app: &App, name: &str) {
    if crate::classic::ui2(name).is_some() {
        crate::classic::menu_bg(name, vw(), vh());
    } else {
        bg(app, "GARAGE1");
    }
}

fn menu_column(app: &mut App, items: &[&str]) -> Option<usize> {
    let w = 380.0;
    let h = 46.0;
    let x = vw() * 0.5 - w * 0.5;
    let y0 = vh() * 0.34;
    app.focus.clamp(items.len());
    let mut picked = None;
    for (i, label) in items.iter().enumerate() {
        let r = Rect::new(x, y0 + i as f32 * (h + 10.0), w, h);
        if r.contains(mouse_position().into()) && app.focus.index != i {
            app.focus.index = i;
        }
        if app
            .ui
            .button(&app.assets, r, label, app.focus.index == i, true)
        {
            picked = Some(i);
        }
    }
    picked
}

fn nav_keys(app: &mut App, items: usize) {
    if is_key_pressed(KeyCode::Down) {
        if app.focus.step(1, items) {
            app.audio.ui_move(get_time());
        }
    }
    if is_key_pressed(KeyCode::Up) {
        if app.focus.step(-1, items) {
            app.audio.ui_move(get_time());
        }
    }
}

// ---------------------------------------------------------------------------

/// Publisher and engine cards, then the title art. Times are seconds; the
/// sequence is skippable and falls back to the "no assets" screen.
const SPLASHES: [(f32, &str, bool); 3] = [
    (2.4, "TRILOGO", false),
    (2.0, "LOGO256", false),
    (2.6, "LOGO480", true),
];

pub fn bootsequence(app: &mut App) {
    if crate::classic::intro_frames() > 0 {
        intro_movie(app);
        return;
    }
    let total: f32 = SPLASHES.iter().map(|s| s.0).sum();
    app.boot_t += get_frame_time();

    let mut start = 0.0;
    let mut step = SPLASHES.len() - 1;
    for (i, (dur, _, _)) in SPLASHES.iter().enumerate() {
        if app.boot_t < start + dur {
            step = i;
            break;
        }
        start += dur;
    }
    let (dur, name, full) = SPLASHES[step];
    let t = (app.boot_t - start).clamp(0.0, dur);
    let fade = (t / 0.45).min((dur - t) / 0.6).clamp(0.0, 1.0);

    if let Some(tex) = app.assets.tex_now(name) {
        clear_background(BLACK);
        let alpha = fade * fade * (3.0 - 2.0 * fade);
        let (w, h) = if full {
            let s = (vw() / tex.width()).min(vh() / tex.height());
            (tex.width() * s, tex.height() * s)
        } else {
            let w = (vw().min(vh()) * 0.7).min(tex.width() * 2.0);
            (w, w * tex.height() / tex.width())
        };
        draw_texture_ex(
            tex,
            vw() * 0.5 - w * 0.5,
            vh() * 0.5 - h * 0.5,
            Color::new(1.0, 1.0, 1.0, alpha),
            DrawTextureParams {
                dest_size: Some(vec2(w, h)),
                ..Default::default()
            },
        );
        if app.ui.has_font() {
            app.ui.centered(
                vw() * 0.5,
                vh() - 34.0,
                14.0,
                "PRESS ENTER TO SKIP",
                Color::new(0.85, 0.85, 0.9, 0.5 * alpha.max(0.4)),
            );
        }
    } else {
        boot(app);
        app.boot_t = 0.0;
    }

    let skip = is_key_pressed(KeyCode::Enter)
        || is_key_pressed(KeyCode::Space)
        || is_key_pressed(KeyCode::Escape)
        || is_mouse_button_pressed(MouseButton::Left);
    if skip || app.boot_t >= total {
        app.boot_t = 0.0;
        app.go(State::Title);
    }
}

/// The original's opening movie (GodGames, the Terminal Reality sand pan,
/// the EVO 2 reel) with its soundtrack, skippable after half a second;
/// ported from offroad4x4's intro loop.
fn intro_movie(app: &mut App) {
    let total = crate::classic::intro_frames();
    if app.boot_t == 0.0 {
        crate::classic::intro_reset();
    }
    app.boot_t += get_frame_time().min(0.05);
    let i = (app.boot_t * crate::classic::INTRO_FPS) as u32;
    app.audio.intro(i + 1 < total);
    let tex = crate::classic::intro_frame_ready(i);
    crate::classic::intro_draw(tex.as_ref(), 0.0);
    let skip = app.boot_t > 0.5
        && (is_key_pressed(KeyCode::Escape)
            || is_key_pressed(KeyCode::Enter)
            || is_key_pressed(KeyCode::Space)
            || is_mouse_button_pressed(MouseButton::Left));
    if i + 1 >= total || skip {
        app.audio.intro(false);
        crate::classic::intro_reset();
        app.boot_t = 0.0;
        app.go(State::Title);
    }
}

pub fn boot(app: &mut App) {
    let cx = vw() * 0.5;
    app.ui.centered(
        cx,
        vh() * 0.38,
        46.0,
        "EvoX",
        Color::new(0.95, 0.78, 0.25, 1.0),
    );
    app.ui.centered(
        cx,
        vh() * 0.38 + 64.0,
        18.0,
        "a clean-room 4x4 Evolution source port",
        Color::new(0.8, 0.83, 0.9, 1.0),
    );
    let warn = if app.tracks.is_empty() {
        "No extracted assets. Run: python3 tools/pod.py <game dir> -o assets/extracted"
    } else {
        "Press Enter"
    };
    app.ui.centered(
        cx,
        vh() * 0.62,
        20.0,
        warn,
        Color::new(0.9, 0.75, 0.45, 1.0),
    );
    if !app.truck_errors.is_empty() {
        app.ui.centered(
            cx,
            vh() * 0.68,
            15.0,
            &format!("{} vehicle parse errors", app.truck_errors.len()),
            Color::new(1.0, 0.5, 0.4, 1.0),
        );
    }
    if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Space) {
        app.go(State::Title);
    }
}

/// Title card: the original's EVO 2 logo and "PRESS ANY KEY"; moves on by
/// itself after four seconds like the original.
pub fn title(app: &mut App) {
    crate::classic::title(&app.ui, app.assets.tex_now("LOGO480"));
    app.boot_t += get_frame_time();
    let pressed = get_last_key_pressed().is_some() || is_mouse_button_pressed(MouseButton::Left);
    if pressed || app.boot_t > 4.0 {
        app.boot_t = 0.0;
        if pressed {
            app.audio.ui_click(get_time());
        }
        app.go(State::MainMenu);
    }
}

/// Descriptions of the main-menu items (the original's texts where the item
/// exists in the original).
const MENU_DESC: [&str; 8] = [
    "Quick Race allows you to run a race on any track. You have the option to select a truck from the quick race list or load a truck from your personal garage.",
    "Career allows you to work through a career in a truck. You start by buying a stock vehicle. You can compete in a series of races to earn money.",
    "The Garage keeps your truck. From here you can visit the Parts Shop and fit the upgrade modules you have bought.",
    "Multiplayer allows you to compete against other players located anywhere in the world or on a local network connection.",
    "Options allows you to setup the game according to your preferences. You can adjust controls, game play, and sound settings.",
    "Mods lists the POD packs found in the mods folder and loads the tracks and trucks they carry.",
    "The people behind the original game and this reconstruction.",
    "Exit the game.",
];

pub fn main_menu(app: &mut App) {
    if crate::classic::has_menu_art() {
        main_menu_classic(app);
        return;
    }
    screen_bg(app, "bg_main");
    header(app, "MAIN MENU");
    app.ui.text(
        28.0,
        96.0,
        17.0,
        &format!("Driver: {}", app.profile.name),
        Color::new(0.8, 0.85, 0.9, 1.0),
    );
    app.ui.money(vw() - 220.0, 100.0, app.profile.budget);
    let items = [
        "Quick Race",
        "Career Series",
        "Garage & Parts",
        "Multiplayer",
        "Settings",
        "Mods",
        "Credits",
        "Quit",
    ];
    nav_keys(app, items.len());
    if let Some(i) = menu_column(app, &items) {
        app.audio.ui_click(get_time());
        match i {
            0 => app.go(State::QuickRace),
            1 => app.go(State::Career),
            2 => app.go(State::Garage),
            3 => app.go(State::Multiplayer),
            4 => app.go(State::Settings),
            5 => app.go(State::Mods),
            6 => app.go(State::Credits),
            _ => app.go(State::Quit),
        }
    }
    if let Some(e) = &app.load_error {
        app.ui.text(
            28.0,
            vh() - 58.0,
            16.0,
            &format!("Save recovered: {e}"),
            Color::new(1.0, 0.62, 0.5, 1.0),
        );
    }
    app.ui.hint("F3 Debug   F1 Save");
    if is_key_pressed(KeyCode::F1) {
        app.save_profile();
        app.toast("Profile saved");
    }
}

/// Main menu in the original's "Select Mode" layout (offroad4x4
/// `draw_menu_list`): blue bars, Evo 2 background, button strips and cursor
/// down the left, description lower right.
fn main_menu_classic(app: &mut App) {
    let items = [
        "Quick Race",
        "Career",
        "Garage",
        "Multiplayer",
        "Options",
        "Mods",
        "Credits",
        "Exit",
    ];
    nav_keys(app, items.len());
    app.focus.clamp(items.len());
    let mouse: Vec2 = mouse_position().into();
    let mut picked = None;
    for i in 0..items.len() {
        if crate::classic::menu_row_rect(i).contains(mouse) {
            if app.focus.index != i {
                app.focus.index = i;
            }
            if is_mouse_button_pressed(MouseButton::Left) {
                picked = Some(i);
            }
        }
    }
    if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Space) {
        picked = Some(app.focus.index);
    }
    let sel = app.focus.index;
    crate::classic::menu_list(
        &app.ui,
        &items,
        sel,
        MENU_DESC[sel.min(7)],
        &app.profile.name,
    );
    if let Some(i) = picked {
        app.audio.ui_click(get_time());
        match i {
            0 => app.go(State::QuickRace),
            1 => app.go(State::Career),
            2 => app.go(State::Garage),
            3 => app.go(State::Multiplayer),
            4 => app.go(State::Settings),
            5 => app.go(State::Mods),
            6 => app.go(State::Credits),
            _ => app.go(State::Quit),
        }
    }
    if let Some(e) = &app.load_error {
        app.ui.text(
            28.0,
            vh() * 0.80,
            16.0,
            &format!("Save recovered: {e}"),
            Color::new(1.0, 0.62, 0.5, 1.0),
        );
    }
    if is_key_pressed(KeyCode::F1) {
        app.save_profile();
        app.toast("Profile saved");
    }
}

/// Display spelling for a shipped level id. The ids are upper-case file
/// stems; the `.LVL`/`SERIES` data carries no long name, so the handful of
/// run-together ones are expanded here and everything else is title-cased.
fn track_title(id: &str) -> String {
    let up = id.to_ascii_uppercase();
    let fixed = match up.as_str() {
        "CON2" => "Con 2",
        "CZ2000" => "CZ 2000",
        "FM109" => "FM 109",
        "ISLAND2" => "Island 2",
        "JYARD" => "Junkyard",
        "MCROSS" => "Moto Cross",
        "OILFLD" => "Oil Field",
        "SHRTAIR" => "Short Air",
        "SHTARTC" => "Short Arctic",
        "SHTBAJA" => "Short Baja",
        "SHTSWAM" => "Short Swamp",
        "THEHILL" => "The Hill",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_string();
    }
    let mut out = String::with_capacity(id.len());
    for (i, c) in id.chars().enumerate() {
        if i == 0 {
            out.extend(c.to_uppercase());
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Quick Race in the original's track-select layout: the authentic photo
/// background, full-width button strips for the level list, cursor down the
/// left of the highlight, and the vehicle/session panel on the right.
pub fn quick_race(app: &mut App) {
    use crate::classic::{menu_frame, text, text_o, width, MENU_HEAD, MENU_SEL};

    let (sw, sh) = (vw(), vh());
    menu_frame(
        &app.ui,
        "Quick Race",
        "Main Menu",
        &app.profile.name,
        "bg_qr",
    );

    let list_x = sw * 0.045;
    let list_w = sw * 0.44;
    let list_y = sh * 0.17;
    let row_h = 30.0;
    let rows = (((sh * 0.84 - list_y) / row_h).floor() as usize).max(3);

    let px = sw * 0.55;
    let pw = sw * 0.40;

    // ---- session panel --------------------------------------------------
    let catalog: Vec<crate::vehicle::Truck> = app.garage_files().into_iter().cloned().collect();
    // Preview cards at top of right panel (`list_y .. list_y + 150.0`),
    // Vehicle cycler at `list_y + 180.0`, session rows below, Start button at
    // the bottom. The layout is derived top-down so the panel height follows
    // the content and `START RACE` can never fall off the bottom edge at
    // 1280x720 (nor at a shorter window).
    // Short windows get a tighter row pitch and button so the stack still
    // clears the footer.
    let row_pitch = if sh < 700.0 { 30.0 } else { 38.0 };
    let start_h = if sh < 700.0 { 40.0 } else { 48.0 };
    let veh_bar_y = list_y + 180.0;
    let mut opt_y = veh_bar_y + 66.0;
    let mut opt_hit: Vec<Rect> = vec![Rect::new(px, veh_bar_y, pw, 56.0)];
    // 1 class, 2 laps, 3 rivals, 4 paint; 5 is START RACE.
    for _ in 1..5 {
        opt_hit.push(Rect::new(px, opt_y, pw, 32.0));
        opt_y += row_pitch;
    }
    let start_y = opt_y + 8.0;
    let panel = Rect::new(
        px - 14.0,
        list_y - 14.0,
        pw + 28.0,
        (start_y + start_h + 16.0) - (list_y - 14.0),
    );
    draw_rectangle(
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        Color::new(0.02, 0.03, 0.08, 0.68),
    );
    draw_rectangle_lines(
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        1.5,
        Color::new(0.55, 0.62, 0.80, 0.55),
    );

    let mut adjust: Option<i32> = None;
    let mut confirm = false;
    app.setup.opt = app.setup.opt.min(5);
    let list_focus = app.setup.focus == 0;

    // ---- keyboard -------------------------------------------------------
    if is_key_pressed(KeyCode::Tab) {
        app.setup.focus ^= 1;
        app.audio.ui_move(get_time());
    }
    let (up, down) = (
        is_key_pressed(KeyCode::Up) || is_key_pressed(KeyCode::W),
        is_key_pressed(KeyCode::Down) || is_key_pressed(KeyCode::S),
    );
    let (left, right) = (
        is_key_pressed(KeyCode::Left) || is_key_pressed(KeyCode::A),
        is_key_pressed(KeyCode::Right) || is_key_pressed(KeyCode::D),
    );
    if list_focus {
        if up || down {
            app.focus.step(if down { 1 } else { -1 }, app.tracks.len());
            if let Some((name, _)) = app.tracks.get(app.focus.index) {
                app.setup.track = name.clone();
            }
            app.audio.ui_move(get_time());
        }
        if left || right {
            adjust = Some(if right { 1 } else { -1 });
        }
        if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Space) {
            app.setup.focus = 1;
        }
    } else {
        if up || down {
            let d = if down { 1 } else { -1 };
            app.setup.opt = (app.setup.opt as i32 + d).rem_euclid(6) as u8;
            app.audio.ui_move(get_time());
        }
        if left || right {
            if app.setup.opt == 5 {
                confirm = right;
            } else {
                adjust = Some(if right { 1 } else { -1 });
            }
        }
        if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Space) {
            if app.setup.opt == 5 {
                confirm = true;
            } else {
                adjust = Some(1);
            }
        }
    }
    if let Some(d) = adjust {
        match app.setup.opt {
            0 if !list_focus => {
                let n = catalog.len().max(1) as i32;
                app.selected_truck = (app.selected_truck as i32 + d).rem_euclid(n) as usize;
                app.audio.ui_move(get_time());
            }
            1 => {
                app.setup.class = (app.setup.class as i32 + d).clamp(1, 5) as u8;
            }
            2 => {
                app.setup.laps = (app.setup.laps as i32 + d).clamp(1, 5) as u32;
            }
            3 => {
                app.setup.opponents = (app.setup.opponents as i32 + d).clamp(1, 7) as usize;
            }
            4 => {
                let n = catalog
                    .get(app.selected_truck)
                    .map_or(0, |t| t.paints.len()) as i32;
                if n > 0 {
                    app.setup.paint = (app.setup.paint as i32 + d).rem_euclid(n) as usize;
                    app.audio.ui_move(get_time());
                }
            }
            _ => {
                if list_focus && !catalog.is_empty() {
                    let n = catalog.len() as i32;
                    app.selected_truck = (app.selected_truck as i32 + d).rem_euclid(n) as usize;
                    app.audio.ui_move(get_time());
                }
            }
        }
    }

    // ---- track list -----------------------------------------------------
    if app.tracks.is_empty() {
        text(
            &app.ui,
            "No tracks extracted",
            list_x,
            list_y + 24.0,
            20.0,
            Color::new(1.0, 0.6, 0.5, 1.0),
        );
    }
    app.focus.clamp(app.tracks.len().max(1));
    let start = (app.focus.index / rows) * rows;
    for (n, (name, _)) in app.tracks.iter().enumerate().skip(start).take(rows) {
        let r = Rect::new(
            list_x,
            list_y + (n - start) as f32 * row_h,
            list_w,
            row_h - 4.0,
        );
        let sel = app.setup.track.eq_ignore_ascii_case(name)
            || (app.setup.track.is_empty() && app.focus.index == n);
        let focused = app.focus.index == n && list_focus;
        if let Some(b) = crate::classic::ui2("button3") {
            draw_texture_ex(
                &b,
                r.x,
                r.y,
                if sel || focused {
                    WHITE
                } else {
                    Color::new(0.88, 0.90, 1.0, 0.88)
                },
                DrawTextureParams {
                    dest_size: Some(vec2(r.w, r.h)),
                    ..Default::default()
                },
            );
        }
        if focused {
            if let Some(c) = crate::classic::ui2("cursor") {
                let h = 24.0;
                let w = h * c.width() / c.height();
                draw_texture_ex(
                    &c,
                    r.x - w - 8.0,
                    r.y + r.h * 0.5 - h * 0.5,
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(vec2(w, h)),
                        ..Default::default()
                    },
                );
            }
        }
        text_o(
            &app.ui,
            &track_title(name),
            r.x + 14.0,
            r.y + r.h - 6.0,
            22.0,
            if sel { MENU_SEL } else { WHITE },
        );

        let hit = app.ui.text_click(r);
        if r.contains(mouse_position().into()) {
            app.focus.index = n;
            app.setup.focus = 0;
        }
        if hit {
            app.focus.index = n;
            app.setup.track = name.clone();
            app.audio.ui_move(get_time());
        }
    }
    if app.setup.track.is_empty() {
        if let Some((name, _)) = app.tracks.get(app.focus.index) {
            app.setup.track = name.clone();
        }
    }

    // ---- track preview + 3D truck preview cards -------------------------
    let card_h = 144.0;
    let card_gap = 12.0;
    let card_w = (pw - card_gap) * 0.5;
    let trk_card = Rect::new(px, list_y, card_w, card_h);
    let veh_card = Rect::new(px + card_w + card_gap, list_y, card_w, card_h);

    draw_rectangle(
        trk_card.x,
        trk_card.y,
        trk_card.w,
        trk_card.h,
        Color::new(0.04, 0.05, 0.09, 0.92),
    );
    // Retail substitutes NOTRACK.RAW when a level has no 64 KiB preview
    // (see `Assets::track_preview`), but the missing file stays visible as
    // a label instead of passing the placeholder off as real art.
    let preview = app.assets.track_preview(&app.setup.track);
    if let Some(preview) = &preview {
        draw_texture_ex(
            preview,
            trk_card.x + 2.0,
            trk_card.y + 2.0,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(trk_card.w - 4.0, trk_card.h - 28.0)),
                ..Default::default()
            },
        );
    }
    if !app.assets.has_own_track_preview(&app.setup.track) {
        app.ui.text(
            trk_card.x + 8.0,
            trk_card.y + 10.0,
            10.0,
            &format!(
                "{}.RAW missing — NOTRACK.RAW",
                app.setup.track.to_ascii_uppercase()
            ),
            Color::new(1.0, 0.55, 0.45, 1.0),
        );
    }
    draw_rectangle_lines(
        trk_card.x,
        trk_card.y,
        trk_card.w,
        trk_card.h,
        1.5,
        Color::new(0.5, 0.58, 0.78, 0.8),
    );
    app.ui.centered(
        trk_card.x + trk_card.w * 0.5,
        trk_card.y + trk_card.h - 22.0,
        15.0,
        &track_title(&app.setup.track),
        MENU_SEL,
    );

    if !catalog.is_empty() {
        app.selected_truck = app.selected_truck.min(catalog.len() - 1);
        // Preview in the selected factory paint.
        let mut shown = catalog[app.selected_truck].clone();
        shown.paint_index = app.setup.paint % shown.paints.len().max(1);
        let cur = &shown;
        let spin = 0.85 + get_time() as f32 * 0.45;
        if let Some(rt) = &app.garage_rt {
            rt.texture.set_filter(FilterMode::Linear);
            let eye = vec3(0.0, 1.75, 6.4);
            set_camera(&Camera3D {
                position: eye,
                target: vec3(0.0, 0.95, 0.0),
                up: Vec3::Y,
                fovy: 36.0_f32.to_radians(),
                aspect: Some(veh_card.w / veh_card.h),
                render_target: Some(rt.clone()),
                ..Default::default()
            });
            clear_background(Color::new(0.05, 0.07, 0.11, 1.0));
            draw_grid(
                16,
                0.5,
                Color::new(0.18, 0.22, 0.28, 1.0),
                Color::new(0.11, 0.13, 0.18, 1.0),
            );
            app.render
                .draw_truck_preview(&mut app.assets, cur, Vec3::ZERO, spin, eye);
            set_default_camera();
            draw_texture_ex(
                &rt.texture,
                veh_card.x,
                veh_card.y,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(veh_card.w, veh_card.h)),
                    flip_y: true,
                    ..Default::default()
                },
            );
        }
        draw_rectangle_lines(
            veh_card.x,
            veh_card.y,
            veh_card.w,
            veh_card.h,
            1.5,
            Color::new(0.5, 0.58, 0.78, 0.8),
        );
    }

    // ---- vehicle cycler -------------------------------------------------
    let vsel = !list_focus && app.setup.opt == 0;
    let veh_label = if catalog.is_empty() {
        "Vehicle".to_string()
    } else {
        format!("Vehicle  {}/{}", app.selected_truck + 1, catalog.len())
    };
    text(
        &app.ui,
        &veh_label,
        px,
        veh_bar_y - 6.0,
        20.0,
        if vsel { MENU_SEL } else { MENU_HEAD },
    );
    let bar = opt_hit[0];
    draw_rectangle(
        bar.x,
        bar.y,
        bar.w,
        bar.h,
        Color::new(0.06, 0.08, 0.15, 0.92),
    );
    draw_rectangle_lines(
        bar.x,
        bar.y,
        bar.w,
        bar.h,
        1.5,
        if vsel {
            MENU_SEL
        } else {
            Color::new(0.5, 0.58, 0.78, 0.8)
        },
    );
    if !catalog.is_empty() {
        app.selected_truck = app.selected_truck.min(catalog.len() - 1);
        let cur = &catalog[app.selected_truck];
        let left_r = Rect::new(bar.x + 4.0, bar.y + 10.0, 36.0, 38.0);
        let right_r = Rect::new(bar.x + bar.w - 40.0, bar.y + 10.0, 36.0, 38.0);
        if app.ui.button(&app.assets, left_r, "<", vsel, true) {
            let n = catalog.len() as i32;
            app.selected_truck = (app.selected_truck as i32 - 1).rem_euclid(n) as usize;
            app.audio.ui_move(get_time());
        }
        if app.ui.button(&app.assets, right_r, ">", vsel, true) {
            let n = catalog.len() as i32;
            app.selected_truck = (app.selected_truck as i32 + 1).rem_euclid(n) as usize;
            app.audio.ui_move(get_time());
        }
        let name = format!("{} {}", cur.make, cur.model);
        app.ui.centered(
            bar.x + bar.w * 0.5,
            bar.y + 6.0,
            20.0,
            &name,
            Color::new(1.0, 0.95, 0.62, 1.0),
        );
        let stats = format!(
            "{}  |  {} hp  |  {} lb",
            cur.class_name, cur.engine.max_hp as i32, cur.dry_weight_lb as i32
        );
        app.ui.centered(
            bar.x + bar.w * 0.5,
            bar.y + 37.0,
            14.0,
            &stats,
            Color::new(0.78, 0.84, 0.94, 1.0),
        );
    }

    // ---- class / laps / opponents --------------------------------------
    let cur_paint = catalog.get(app.selected_truck).and_then(|t| {
        (!t.paints.is_empty()).then(|| {
            let i = app.setup.paint % t.paints.len();
            (i, t.paints.len(), t.paints[i].clone())
        })
    });
    let paint_label = match &cur_paint {
        Some((i, n, p)) if !p.name.is_empty() => format!("Paint  {}  ({}/{})", p.name, i + 1, n),
        Some((i, n, _)) => format!("Paint  {}/{}", i + 1, n),
        None => "Paint  factory livery".to_string(),
    };
    let opts: [(u8, String); 4] = [
        (1, format!("Class  {}", app.setup.class)),
        (2, format!("Laps  {}", app.setup.laps)),
        (3, format!("Rivals  {}", app.setup.opponents)),
        (4, paint_label),
    ];
    // Swatch of the selected factory paint (`colorList[]` fields 3-5).
    if let Some((_, _, p)) = &cur_paint {
        let r = opt_hit[4];
        let sw_x = r.x + pw * 0.62;
        draw_rectangle(
            sw_x,
            r.y + 6.0,
            44.0,
            22.0,
            Color::from_rgba(p.swatch[0], p.swatch[1], p.swatch[2], 255),
        );
        draw_rectangle_lines(sw_x, r.y + 6.0, 44.0, 22.0, 1.5, WHITE);
    }
    for (k, (idx, label)) in opts.iter().enumerate() {
        let r = opt_hit[(*idx) as usize];
        let sel = !list_focus && app.setup.opt == *idx;
        if app.ui.text_click(r) {
            app.setup.focus = 1;
            app.setup.opt = *idx;
            app.audio.ui_move(get_time());
        }
        text(
            &app.ui,
            label,
            r.x + 2.0,
            r.y + 24.0,
            22.0,
            if sel { MENU_SEL } else { WHITE },
        );
        if sel {
            let w = width(&app.ui, "<", 22.0);
            text_o(
                &app.ui,
                "<",
                r.x + pw - w * 2.0 - 14.0,
                r.y + 24.0,
                20.0,
                MENU_SEL,
            );
            text_o(&app.ui, ">", r.x + pw - w - 4.0, r.y + 24.0, 20.0, MENU_SEL);
        }
        let _ = k;
    }

    // ---- start ----------------------------------------------------------
    let ready = !app.setup.track.is_empty() && !catalog.is_empty();
    let start_r = Rect::new(px, start_y, pw, start_h);
    if app.ui.button(
        &app.assets,
        start_r,
        "START RACE",
        !list_focus && app.setup.opt == 5,
        ready,
    ) || (confirm && ready)
    {
        app.audio.ui_click(get_time());
        app.start_loading(None);
    }

    // Keep the cycler inside the catalog as soon as one exists.
    if app.setup.track.is_empty() && !app.tracks.is_empty() {
        app.setup.track = app.tracks[app.focus.index.min(app.tracks.len() - 1)]
            .0
            .clone();
    }

    let hint = "Up/Down  ENTER  TAB  ESC";
    let hint_y = sh * 0.95;
    // On short windows the session panel reaches the footer; put the hint in
    // the (empty) track-list column instead of over the panel.
    if panel.bottom() + 18.0 > hint_y {
        text(&app.ui, hint, list_x, hint_y, 20.0, MENU_HEAD);
    } else {
        text(
            &app.ui,
            hint,
            sw * 0.955 - width(&app.ui, hint, 20.0),
            hint_y,
            20.0,
            MENU_HEAD,
        );
    }
    let _ = (opt_hit, list_w);
}

pub fn career(app: &mut App) {
    screen_bg(app, "bg_career");
    header(app, "CAREER SERIES");
    let list = app.career.series.clone();
    app.focus.clamp(list.len().max(1));
    let mut start = false;
    // Scroll so the selection stays inside the list area above the
    // earnings line (11 series do not fit a 720 px screen).
    let row_h = 62.0;
    let visible = (((vh() - 100.0 - 84.0) / row_h) as usize).max(1);
    let first = app.focus.index.saturating_sub(visible - 1);
    for (i, s) in list.iter().enumerate().skip(first).take(visible) {
        let r = Rect::new(28.0, 100.0 + (i - first) as f32 * row_h, vw() - 56.0, 56.0);
        let locked = !app.career.is_unlocked(s.tier);
        let sub = format!(
            "Tier {}  |  {} events  |  fee ${}  purse ${}  |  max class {}",
            s.tier,
            s.events.len(),
            s.entry_fee,
            s.purse,
            s.max_class
        );
        if app.ui.row(
            &app.assets,
            r,
            &format!("{}  ({})", s.name, if locked { "LOCKED" } else { "OPEN" }),
            &sub,
            app.focus.index == i,
            !locked,
        ) {
            app.audio.ui_click(get_time());
            let class = app.setup.class;
            match app.career.start(i, &mut app.profile.budget, class) {
                Ok(()) => {
                    app.profile.career = app.career.clone();
                    app.save_profile();
                    start = true;
                }
                Err(e) => app.toast(format!("{:?}", e)),
            }
        }
    }
    if start {
        // Career uses the series' first event.
        if let Some(i) = list.iter().position(|s| !app.career.is_unlocked(s.tier)) {
            let _ = i;
        }
        let idx = app.career.active;
        app.start_loading(idx);
    }
    app.ui.text(
        28.0,
        vh() - 52.0,
        17.0,
        &format!(
            "Career earnings: ${}   Completed: {}",
            app.profile.career.career_earnings,
            app.profile.career.completed_series.len()
        ),
        Color::new(0.85, 0.9, 0.8, 1.0),
    );
    nav_keys(app, list.len());
    app.ui.hint("");
}

pub fn garage(app: &mut App) {
    crate::classic::menu_frame(
        &app.ui,
        "Garage & Dealership",
        "Main Menu",
        &app.profile.name,
        "bg_parts",
    );
    let catalog: Vec<crate::vehicle::Truck> = app.garage_files().into_iter().cloned().collect();
    let slots = catalog.len() + 2;
    app.focus.clamp(slots);
    nav_keys(app, slots);
    if app.focus.index < catalog.len() {
        app.selected_truck = app.focus.index;
    }
    // Scroll the visible window with the selection inside the menu_frame bounds.
    let list_y = 128.0;
    let row_h = 34.0;
    let rows = ((vh() - 250.0) / row_h).max(3.0) as usize;
    let start = (app.focus.index.min(catalog.len().saturating_sub(1)) / rows) * rows;
    for (i, t) in catalog.iter().enumerate().skip(start).take(rows) {
        let name = format!("{} {}", t.make, t.model);
        let owned = app.is_owned(t);
        let r = Rect::new(
            36.0,
            list_y + (i - start) as f32 * row_h,
            336.0,
            row_h - 4.0,
        );
        let sub = if owned {
            "owned".to_string()
        } else {
            format!("{} hp", t.engine.max_hp as i32)
        };
        let hit = app
            .ui
            .row(&app.assets, r, &name, &sub, app.focus.index == i, true);
        if hit || app.ui.text_click(r) {
            app.selected_truck = i;
            app.focus.index = i;
        }
        if !owned {
            // Dim non-owned rows so ownership still reads at a glance.
            draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.0, 0.0, 0.0, 0.22));
        }
    }
    let truck = app.current_truck().cloned();
    if let Some(t) = &truck {
        let px = 400.0;
        // Turntable rendered to an off-screen bay, then blitted, so the bay
        // lines up with the 2D layout regardless of viewport conventions.
        let (bw, bh) = (440.0f32, 310.0f32);
        let spin = 0.85 + get_time() as f32 * 0.45;
        if let Some(rt) = &app.garage_rt {
            rt.texture.set_filter(FilterMode::Linear);
            let eye = vec3(0.0, 1.75, 6.4);
            set_camera(&Camera3D {
                position: eye,
                target: vec3(0.0, 0.95, 0.0),
                up: Vec3::Y,
                fovy: 36.0_f32.to_radians(),
                aspect: Some(bw / bh),
                render_target: Some(rt.clone()),
                ..Default::default()
            });
            clear_background(Color::new(0.05, 0.07, 0.11, 1.0));
            draw_grid(
                20,
                0.5,
                Color::new(0.18, 0.22, 0.28, 1.0),
                Color::new(0.11, 0.13, 0.18, 1.0),
            );
            let mut painted = t.clone();
            painted.paint_index = app.setup.paint % painted.paints.len().max(1);
            app.render
                .draw_truck_preview(&mut app.assets, &painted, Vec3::ZERO, spin, eye);
            set_default_camera();
            draw_texture_ex(
                &rt.texture,
                px,
                128.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(bw, bh)),
                    flip_y: true,
                    ..Default::default()
                },
            );
            draw_rectangle_lines(px, 128.0, bw, bh, 1.5, Color::new(0.5, 0.58, 0.78, 0.8));
        }
        draw_rectangle(px, 448.0, bw, 190.0, Color::new(0.02, 0.03, 0.08, 0.72));
        draw_rectangle_lines(px, 448.0, bw, 190.0, 1.5, Color::new(0.5, 0.58, 0.78, 0.65));
        app.ui.text(
            px + 20.0,
            458.0,
            26.0,
            &format!("{} {}", t.make, t.model),
            Color::new(1.0, 0.95, 0.62, 1.0),
        );
        app.ui.text(
            px + 20.0,
            490.0,
            16.0,
            &format!(
                "{}  |  {}  |  {} hp  |  {} lb",
                t.class_name, t.model_year, t.engine.max_hp as i32, t.dry_weight_lb as i32
            ),
            Color::new(0.82, 0.87, 0.94, 1.0),
        );
        app.ui.stat_bar(
            px + 20.0,
            524.0,
            240.0,
            "Acceleration",
            t.acceleration * 3.0,
            1.0,
            true,
            Color::new(0.4, 0.9, 0.5, 1.0),
        );
        app.ui.stat_bar(
            px + 20.0,
            550.0,
            240.0,
            "Top speed",
            t.top_speed * 12.0,
            1.0,
            true,
            Color::new(0.4, 0.7, 1.0, 1.0),
        );
        app.ui.stat_bar(
            px + 20.0,
            576.0,
            240.0,
            "Handling",
            t.handling * 3.0,
            1.0,
            true,
            Color::new(1.0, 0.8, 0.4, 1.0),
        );
        app.ui.money(px + 20.0, 604.0, app.profile.budget);
    }
    if app.ui.button(
        &app.assets,
        Rect::new(vw() - 380.0, 460.0, 320.0, 52.0),
        "PARTS SHOP",
        app.focus.index == catalog.len(),
        truck.is_some(),
    ) {
        app.audio.ui_click(get_time());
        app.open_shop();
    }
    if app.ui.button(
        &app.assets,
        Rect::new(vw() - 380.0, 526.0, 320.0, 52.0),
        "TEST DRIVE",
        app.focus.index == catalog.len() + 1,
        truck.is_some(),
    ) {
        app.audio.ui_click(get_time());
        if app.setup.track.is_empty() {
            app.setup.track = app.tracks.first().map(|t| t.0.clone()).unwrap_or_default();
        }
        app.start_loading(None);
    }
}

pub fn shop(app: &mut App) {
    screen_bg(app, "bg_parts");
    header(app, "PARTS SHOP");
    let Some(mut shop) = app.shop.take() else {
        app.go(State::Garage);
        return;
    };
    shop.tick(1.0 / 60.0);
    // Category tabs.
    let tab_w = (vw() - 56.0) / 9.0;
    for (i, ty) in PART_TYPES.iter().enumerate() {
        let r = Rect::new(28.0 + i as f32 * tab_w, 74.0, tab_w - 6.0, 74.0);
        let active = shop.selected_type == *ty;
        if app
            .ui
            .tab(&app.assets, r, ty.ui_icon(), ty.label(), active, false)
        {
            shop.selected_type = *ty;
            shop.cursor = 0;
            app.audio.ui_click(get_time());
        }
    }
    if is_key_pressed(KeyCode::RightBracket) || is_key_pressed(KeyCode::E) {
        shop.cycle_type(1);
    }
    if is_key_pressed(KeyCode::LeftBracket) || is_key_pressed(KeyCode::Q) {
        shop.cycle_type(-1);
    }

    let items: Vec<_> = shop.filtered().into_iter().cloned().collect();
    shop.cursor = shop.cursor.min(items.len());
    let action_focus = shop.cursor >= items.len();
    let list_y = 162.0;
    let row_h = 58.0;
    let rows = ((vh() - list_y - 96.0) / row_h) as usize;
    let offset = (shop.cursor / rows.max(1)) * rows.max(1);
    for (n, p) in items.iter().enumerate().skip(offset).take(rows) {
        let r = Rect::new(
            28.0,
            list_y + (n - offset) as f32 * row_h,
            vw() * 0.52,
            row_h - 6.0,
        );
        let lock = shop.buy_lock(p);
        let owned = shop.owned.contains(&p.code);
        let equipped = shop
            .equipped
            .get(&p.slot)
            .map(|c| c == &p.code)
            .unwrap_or(false);
        let sub = if equipped {
            "INSTALLED".to_string()
        } else if owned {
            format!("OWNED  |  {}", p.description.lines().next().unwrap_or(""))
        } else if let Some(l) = &lock {
            l.text()
        } else {
            format!(
                "${}  |  {}",
                p.cost,
                p.description.lines().next().unwrap_or("")
            )
        };
        let sub: String = sub.chars().take(82).collect();
        if app.ui.row(
            &app.assets,
            r,
            &format!("{}  ${}", p.name, p.cost),
            &sub,
            shop.cursor == n,
            owned || lock.is_none(),
        ) {
            shop.cursor = n;
            if owned {
                if equipped {
                    shop.unequip(p.slot);
                } else if let Err(l) = shop.equip(&p.code) {
                    app.toast(l.text());
                }
            } else if let Err(l) = shop.buy(&p.code) {
                app.toast(l.text());
            } else {
                app.audio.ui_click(get_time());
                app.profile.budget = shop.budget;
                app.profile.owned = shop.owned.clone();
                for (slot, code) in &shop.equipped {
                    app.profile
                        .equipped
                        .insert(format!("{}:{}", shop.truck, slot.label()), code.clone());
                }
                app.save_profile();
            }
        }
    }
    // Detail panel.
    let px = vw() * 0.56;
    let pw = vw() - px - 28.0;
    app.ui.panel(
        &app.assets,
        "BUTTON3",
        Rect::new(px, 162.0, pw, vh() - 260.0),
        Color::new(1.0, 1.0, 1.0, 0.6),
    );
    if let Some(p) = items.get(shop.cursor.min(items.len().saturating_sub(1))) {
        app.ui.text(
            px + 20.0,
            198.0,
            26.0,
            &p.name,
            Color::new(1.0, 0.92, 0.6, 1.0),
        );
        let mut y = 232.0;
        for line in wrap(&p.description, (pw / 9.5) as usize).iter().take(5) {
            app.ui
                .text(px + 20.0, y, 15.0, line, Color::new(0.85, 0.88, 0.95, 1.0));
            y += 19.0;
        }
        let mut sy = 340.0;
        for (label, v, good) in p.mods.rows().iter().take(6) {
            app.ui.text(px + 20.0, sy, 17.0, label, WHITE);
            let s = if *v > 0.0 {
                format!("{v:+.3}  ({})", if *good { "better" } else { "worse" })
            } else {
                format!("{v:+.3}  ({})", if *good { "worse" } else { "better" })
            };
            app.ui.text(
                px + 170.0,
                sy,
                17.0,
                &s,
                if *v > 0.0 {
                    Color::new(0.55, 0.95, 0.6, 1.0)
                } else {
                    Color::new(1.0, 0.55, 0.45, 1.0)
                },
            );
            sy += 22.0;
        }
        sy += 8.0;
        if let Some(req) = &p.prereq {
            for line in wrap(&format!("Requires: {}", req.render()), (pw / 10.0) as usize)
                .iter()
                .take(2)
            {
                app.ui
                    .text(px + 20.0, sy, 15.0, line, Color::new(0.9, 0.85, 0.6, 1.0));
                sy += 18.0;
            }
        }
        if let Some(l) = shop.buy_lock(p) {
            if !matches!(l, crate::parts::Lock::AlreadyEquipped) {
                app.ui.text(
                    px + 20.0,
                    sy + 4.0,
                    17.0,
                    &format!("Locked: {}", l.text()),
                    Color::new(1.0, 0.62, 0.5, 1.0),
                );
            }
        }
        let equipped = shop
            .equipped
            .get(&p.slot)
            .map(|c| c == &p.code)
            .unwrap_or(false);
        let action = if equipped {
            "REMOVE"
        } else if shop.owned.contains(&p.code) {
            "INSTALL"
        } else {
            "BUY"
        };
        if app.ui.button(
            &app.assets,
            Rect::new(px + 20.0, vh() - 92.0, pw - 40.0, 46.0),
            action,
            action_focus,
            equipped || shop.owned.contains(&p.code) || shop.buy_lock(p).is_none(),
        ) {
            if equipped {
                shop.unequip(p.slot);
            } else if shop.owned.contains(&p.code) {
                if let Err(l) = shop.equip(&p.code) {
                    app.toast(l.text());
                }
            } else if let Err(l) = shop.buy(&p.code) {
                app.toast(l.text());
            } else {
                app.profile.budget = shop.budget;
                app.profile.owned = shop.owned.clone();
                app.save_profile();
            }
        }
    }
    draw_rectangle(
        0.0,
        vh() - 38.0,
        vw(),
        38.0,
        Color::new(0.02, 0.03, 0.05, 0.9),
    );
    // `Ui::text` takes the top edge: keep both lines inside the 38 px bar.
    app.ui.text(
        28.0,
        vh() - 35.0,
        22.0,
        &format!("${}", shop.budget),
        Color::new(0.55, 0.95, 0.55, 1.0),
    );
    let keys = "Up/Down Browse   Enter Buy/Install   Q/E Category   Esc Back";
    app.ui.text(
        vw() - 28.0 - app.ui.width(16.0, keys),
        vh() - 31.0,
        16.0,
        keys,
        Color::new(0.8, 0.82, 0.9, 1.0),
    );
    if let Some((m, _)) = &shop.message {
        app.toast(m.clone());
    }
    if is_key_pressed(KeyCode::Down) && shop.cursor < items.len() {
        shop.cursor += 1;
        app.audio.ui_move(get_time());
    }
    if is_key_pressed(KeyCode::Up) && shop.cursor > 0 {
        shop.cursor -= 1;
        app.audio.ui_move(get_time());
    }
    app.shop = Some(shop);
}

pub fn settings(app: &mut App) {
    screen_bg(app, "bg_options");
    header(app, "SETTINGS");
    let s = &mut app.profile.settings;
    let mut y = 110.0;
    let rows: [(&str, String); 5] = [
        ("Master volume", format!("{:.0}%", s.master_volume * 100.0)),
        ("Music volume", format!("{:.0}%", s.music_volume * 100.0)),
        ("Field of view", format!("{:.0}", s.camera_fov)),
        (
            "Units",
            if s.metric {
                "Metric".into()
            } else {
                "Imperial".into()
            },
        ),
        (
            "Invert steering axis",
            if s.invert_y {
                "Yes".into()
            } else {
                "No".into()
            },
        ),
    ];
    for (label, value) in rows {
        app.ui.text(40.0, y, 20.0, label, WHITE);
        app.ui
            .text(420.0, y, 20.0, &value, Color::new(1.0, 0.9, 0.55, 1.0));
        y += 42.0;
    }
    if is_key_pressed(KeyCode::Left) || is_key_pressed(KeyCode::Right) {
        let d = if is_key_pressed(KeyCode::Right) {
            1.0
        } else {
            -1.0
        };
        match app.focus.index % 5 {
            0 => s.master_volume = (s.master_volume + d * 0.05).clamp(0.0, 1.0),
            1 => s.music_volume = (s.music_volume + d * 0.05).clamp(0.0, 1.0),
            2 => s.camera_fov = (s.camera_fov + d * 5.0).clamp(55.0, 100.0),
            3 => s.metric = !s.metric,
            _ => s.invert_y = !s.invert_y,
        }
        app.audio.master = s.master_volume;
        app.audio.music_volume = s.music_volume;
    }
    nav_keys(app, 5);
    if app.ui.button(
        &app.assets,
        Rect::new(40.0, y + 20.0, 240.0, 48.0),
        "SAVE & BACK",
        true,
        true,
    ) {
        app.save_profile();
        app.toast("Settings saved");
        app.state = State::MainMenu;
    }
    app.ui.hint("");
}

pub fn multiplayer(app: &mut App) {
    screen_bg(app, "bg_start");
    header(app, "MULTIPLAYER");
    app.ui.text(
        40.0,
        130.0,
        20.0,
        "Host on UDP 7777, then share your IP or register with a master server.",
        Color::new(0.85, 0.88, 0.95, 1.0),
    );
    app.ui.text(
        40.0,
        170.0,
        17.0,
        &format!("Master: {}", crate::app::MASTER_URL),
        Color::new(0.7, 0.75, 0.85, 1.0),
    );
    let items = [
        "Server Browser",
        "Refresh Master",
        "Direct Connect (127.0.0.1:7777)",
    ];
    if let Some(i) = menu_column(app, &items) {
        match i {
            0 => {
                app.browser.fetch();
                app.go(State::Browser)
            }
            1 => app.toast("Refresh happens in the browser"),
            _ => app.go(State::Browser),
        }
    }
    app.ui.hint("");
}

pub fn browser(app: &mut App) {
    screen_bg(app, "bg_start");
    header(app, "SERVER BROWSER");
    app.ui.text(
        28.0,
        92.0,
        16.0,
        &app.browser.status,
        Color::new(0.8, 0.85, 0.9, 1.0),
    );
    let entries = app.browser.entries.clone();
    app.focus.clamp(entries.len().max(1));
    for (i, e) in entries.iter().enumerate() {
        let sub = format!(
            "{}:{}{}  |  {}/{}  |  {}  |  class {}",
            e.host,
            e.port,
            if e.password { " *" } else { "" },
            e.players,
            e.max_players,
            e.track,
            e.class
        );
        let r = Rect::new(28.0, 110.0 + i as f32 * 48.0, vw() - 56.0, 44.0);
        if app
            .ui
            .row(&app.assets, r, &e.name, &sub, app.focus.index == i, true)
        {
            app.browser.selected = i;
            if let Some(addr) = e.addr() {
                match crate::net::Client::connect(addr, &app.profile.name) {
                    Ok(c) => {
                        app.online = Some(c);
                        app.toast(format!("Connecting to {addr}"));
                    }
                    Err(err) => app.toast(format!("{err}")),
                }
            }
        }
    }
    if entries.is_empty() {
        app.ui.text(
            32.0,
            140.0,
            18.0,
            "No lobbies. Start a master server (cargo run -p evox-master).",
            Color::new(1.0, 0.7, 0.55, 1.0),
        );
    }
    if app.browser.should_refresh() && entries.is_empty() == false {
        // passive; refresh happens on demand to stay frame-safe
    }
    nav_keys(app, entries.len());
    app.ui.hint("R Refresh");
    if is_key_pressed(KeyCode::R) {
        app.browser.fetch();
    }
}

pub fn loading(app: &mut App) {
    let cx = vw() * 0.5;
    if let Some(t) = app.assets.tex_now("LOADING1") {
        let w = vw() * 0.5;
        let h = w * t.height() / t.width();
        draw_texture_ex(
            t,
            cx - w * 0.5,
            vh() * 0.32,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(w, h)),
                ..Default::default()
            },
        );
    }
    app.ui.centered(
        cx,
        vh() * 0.66,
        26.0,
        "Loading track...",
        Color::new(0.95, 0.9, 0.6, 1.0),
    );
    let w = vw() * 0.4;
    draw_rectangle(
        cx - w * 0.5,
        vh() * 0.7,
        w,
        14.0,
        Color::new(0.1, 0.1, 0.14, 1.0),
    );
    draw_rectangle(
        cx - w * 0.5,
        vh() * 0.7,
        w * (app.loading_step / 0.35).clamp(0.0, 1.0),
        14.0,
        Color::new(0.95, 0.75, 0.25, 1.0),
    );
}

pub fn race(app: &mut App) {
    let Some(mut owned) = app.race.take() else {
        return;
    };
    let race = &mut owned;
    let cam = race.camera();
    let p = race.player.pos;
    let lead = race.player.vel * 0.9;
    let focus = p + lead;
    let budget = (2 + (race.player.vel.length() / 8.0) as usize).min(8);
    draw_world(app, race, &cam, focus, budget);
    app.render.fog_end = 0.0;
    set_default_camera();
    if race.track.water_height > 0.5 && cam.position.y < race.track.water_height {
        draw_rectangle(0.0, 0.0, vw(), vh(), Color::new(0.06, 0.24, 0.36, 0.42));
    }
    // HUD laid out like the original (ported from offroad4x4 `hud::draw`).
    let ui = &app.ui;
    let odom = app.assets.tex_now("ODOM");
    let (sw, sh) = (vw(), vh());
    let racing = race.phase != crate::race::Phase::Finished;
    // top-left: timing
    let cur = race.time - race.lap_start;
    let last = if race.last_lap > 0.0 {
        crate::classic::fmt_time(race.last_lap)
    } else {
        "-:--.--".to_string()
    };
    let best = if race.best_lap > 0.0 {
        crate::classic::fmt_time(race.best_lap)
    } else {
        "-:--.--".to_string()
    };
    let delta = (race.best_lap > 0.0 && race.last_lap > 0.0).then(|| race.last_lap - race.best_lap);
    crate::classic::timing(
        ui,
        &[
            ("CURRENT", crate::classic::fmt_time(cur)),
            ("PREVIOUS", last),
            ("FASTEST", best),
            (
                "LAP",
                format!(
                    "{}/{}",
                    race.lap.min(crate::race::MAX_LAPS),
                    crate::race::MAX_LAPS
                ),
            ),
            (
                "PLACE",
                format!("{}/{}", race.position, race.opponents.len() + 1),
            ),
        ],
        delta.filter(|d| d.abs() > 0.004),
    );
    // top-centre: course-following checkpoint finder
    if racing {
        if let Some(target) = race.compass_target().or_else(|| race.checkpoint_target()) {
            crate::classic::finder(
                target,
                race.player.pos,
                race.player.forward(),
                race.player.right(),
                sw * 0.5,
                52.0,
            );
        }
    }
    // top-right: course outline with every truck
    let rivals: Vec<Vec3> = race.opponents.iter().map(|o| o.vehicle.pos).collect();
    crate::classic::minimap(
        &race.course,
        &rivals,
        race.player.pos,
        sw - 196.0,
        14.0,
        180.0,
    );
    // bottom: speedometer (mph, odometer) left, tachometer and gear right
    crate::classic::speedo(
        ui,
        odom,
        128.0,
        sh - 118.0,
        96.0,
        race.player.vel.length(),
        race.odo_m,
        app.profile.settings.metric,
    );
    crate::classic::tach(
        ui,
        odom,
        sw - 128.0,
        sh - 118.0,
        96.0,
        race.player.rpm,
        race.player.truck.engine.redline.max(1000.0),
        race.player.gear,
        race.drive_mode,
    );
    if race.phase == crate::race::Phase::Countdown {
        app.ui.centered(
            vw() * 0.5,
            vh() * 0.4,
            84.0,
            &format!("{}", race.countdown.ceil() as i32),
            Color::new(1.0, 0.9, 0.4, 1.0),
        );
    }
    if race.phase == crate::race::Phase::Finished {
        app.ui.centered(
            vw() * 0.5,
            vh() * 0.24,
            52.0,
            "FINISH",
            Color::new(1.0, 0.92, 0.5, 1.0),
        );
        app.ui.centered(
            vw() * 0.5,
            vh() * 0.32,
            18.0,
            "replay camera rolling - Esc to exit",
            WHITE,
        );
    }
    if race.paused {
        app.ui
            .centered(vw() * 0.5, vh() * 0.5, 40.0, "PAUSED", WHITE);
    }
    // Controls only on the pause screen, like the original: the bar would
    // cover the gauges' lower arc.
    if race.paused {
        app.ui.hint(
            "W/S Throttle/Brake   A/D Steer   Space Handbrake   E/Q Shift   P Pause   F2 Finish",
        );
    }
    if is_key_pressed(KeyCode::P) {
        race.paused = !race.paused;
    }
    // QA shortcut: force the flag so career chaining can be exercised without
    // a full lap.
    if is_key_pressed(KeyCode::F2) && race.phase != crate::race::Phase::Finished {
        race.phase = crate::race::Phase::Finished;
        race.finished_at = Some(race.time);
        race.replay.timer = 2.0;
    }
    app.race = Some(owned);
}

/// Shared 3D world draw: sky, terrain chunks, props, skids, vehicles, water,
/// particles and checkpoint gates, for both the race and the editor. The camera
/// and `focus` (chunk-ring centre) are caller-supplied so the editor can fly
/// anywhere while the race leads with velocity.
fn draw_world(
    app: &mut App,
    race: &mut crate::race::Race,
    cam: &Camera3D,
    focus: Vec3,
    budget: usize,
) {
    clear_background(race.track.fog_color);
    app.render.cam_pos = cam.position;
    app.render.fog_color = race.track.fog_color;
    app.render.fog_start = 135.0;
    app.render.fog_end = 520.0;
    set_camera(cam);
    if std::env::var("EVOX_NOSKY").is_err() {
        let sky = app.assets.track_sky(&mut race.track);
        app.render
            .draw_sky_dome(cam.position, sky.as_ref(), race.track.fog_color);
    }

    // Terrain chunks: build ahead of the focus and draw what exists.
    let cs = 32usize;
    let cx = (focus.x / race.track.scale).clamp(0.0, GRID as f32 - 1.0) as usize / cs;
    let cz = (focus.z / race.track.scale).clamp(0.0, GRID as f32 - 1.0) as usize / cs;
    let own_x = cx;
    let own_z = cz;
    const DRAW_R: i32 = 3;
    let tex = app.assets.terrain_texture(&race.track);
    app.chunks
        .update(&race.track, cx, cz, cs, tex.as_ref(), DRAW_R + 3, budget);
    if !app.chunks.has(own_x, own_z) {
        app.chunks
            .update(&race.track, own_x, own_z, cs, tex.as_ref(), 0, 1);
    }
    let chunk_fog = app.render.bind_world_material(false);
    for dz in -DRAW_R..=DRAW_R {
        for dx in -DRAW_R..=DRAW_R {
            let nx = cx as i32 + dx;
            let nz = cz as i32 + dz;
            if nx < 0 || nz < 0 {
                continue;
            }
            if let Some(m) = app.chunks.mesh(nx as usize, nz as usize) {
                draw_mesh(m);
            }
        }
    }
    if chunk_fog {
        app.render.unbind_world_material();
    }
    // Props: retail `.SIT` box models (trees, rocks, buildings).
    let prop_cutoff = 300.0 * 300.0;
    let mut drawn = 0usize;
    for prop in &race.track.props {
        if (prop.pos - focus).length_squared() > prop_cutoff || drawn >= 200 {
            continue;
        }
        if prop.is_unported_traffic() {
            continue;
        }
        app.render
            .draw_model(&mut app.assets, &prop.model, prop.pos, prop.yaw, 1.0);
        drawn += 1;
    }
    // Tire tread marks projected flat onto terrain beneath all vehicles.
    let skid_mat = app.render.bind_translucent_material();
    race.vfx.draw_skids_3d(&race.track);
    if skid_mat {
        app.render.unbind_world_material();
    }
    // Cars: grounded blob shadows underneath, then retail SMF bodies + wheels.
    for o in &race.opponents {
        app.render.draw_blob_shadow(&race.track, &o.vehicle);
        app.render.draw_vehicle(&mut app.assets, &o.vehicle);
    }
    app.render.draw_blob_shadow(&race.track, &race.player);
    app.render.draw_vehicle(&mut app.assets, &race.player);

    // Translucent water surface for flooded river/lake crossings.
    app.render.draw_water(&race.track, cam.position, race.time);

    // 3D dust, mud, and water spray billboards in world space near the wheels.
    let particle_mat = app.render.bind_translucent_material();
    race.vfx.draw_particles_3d(&cam, &race.track);
    if particle_mat {
        app.render.unbind_world_material();
    }

    // 3D vertical waypoint beams for active and next checkpoint gates.
    if race.phase != crate::race::Phase::Finished {
        let total = race.checkpoint_total().max(1);
        let active_idx = race.checkpoint % total;
        let next_idx = (race.checkpoint + 1) % total;
        for (idx, gate_pos) in race.checkpoint_gates() {
            let col = if idx == active_idx {
                Some(Color::new(0.18, 0.96, 0.32, 0.36))
            } else if idx == next_idx {
                Some(Color::new(0.96, 0.80, 0.16, 0.20))
            } else {
                None
            };
            if let Some(col) = col {
                let ground_y = race.track.height_at_world(gate_pos.x, gate_pos.z);
                let top_y = ground_y + 36.0;
                let w = 0.85;
                let verts = vec![
                    Vertex {
                        position: vec3(gate_pos.x - w, ground_y, gate_pos.z),
                        uv: Vec2::ZERO,
                        color: [
                            (col.r * 255.0) as u8,
                            (col.g * 255.0) as u8,
                            (col.b * 255.0) as u8,
                            (col.a * 255.0) as u8,
                        ],
                        normal: Vec4::new(0.0, 0.0, 1.0, 0.0),
                    },
                    Vertex {
                        position: vec3(gate_pos.x + w, ground_y, gate_pos.z),
                        uv: Vec2::ZERO,
                        color: [
                            (col.r * 255.0) as u8,
                            (col.g * 255.0) as u8,
                            (col.b * 255.0) as u8,
                            (col.a * 255.0) as u8,
                        ],
                        normal: Vec4::new(0.0, 0.0, 1.0, 0.0),
                    },
                    Vertex {
                        position: vec3(gate_pos.x + w, top_y, gate_pos.z),
                        uv: Vec2::ZERO,
                        color: [
                            (col.r * 255.0) as u8,
                            (col.g * 255.0) as u8,
                            (col.b * 255.0) as u8,
                            24,
                        ],
                        normal: Vec4::new(0.0, 0.0, 1.0, 0.0),
                    },
                    Vertex {
                        position: vec3(gate_pos.x - w, top_y, gate_pos.z),
                        uv: Vec2::ZERO,
                        color: [
                            (col.r * 255.0) as u8,
                            (col.g * 255.0) as u8,
                            (col.b * 255.0) as u8,
                            24,
                        ],
                        normal: Vec4::new(0.0, 0.0, 1.0, 0.0),
                    },
                    Vertex {
                        position: vec3(gate_pos.x, ground_y, gate_pos.z - w),
                        uv: Vec2::ZERO,
                        color: [
                            (col.r * 255.0) as u8,
                            (col.g * 255.0) as u8,
                            (col.b * 255.0) as u8,
                            (col.a * 255.0) as u8,
                        ],
                        normal: Vec4::new(1.0, 0.0, 0.0, 0.0),
                    },
                    Vertex {
                        position: vec3(gate_pos.x, ground_y, gate_pos.z + w),
                        uv: Vec2::ZERO,
                        color: [
                            (col.r * 255.0) as u8,
                            (col.g * 255.0) as u8,
                            (col.b * 255.0) as u8,
                            (col.a * 255.0) as u8,
                        ],
                        normal: Vec4::new(1.0, 0.0, 0.0, 0.0),
                    },
                    Vertex {
                        position: vec3(gate_pos.x, top_y, gate_pos.z + w),
                        uv: Vec2::ZERO,
                        color: [
                            (col.r * 255.0) as u8,
                            (col.g * 255.0) as u8,
                            (col.b * 255.0) as u8,
                            24,
                        ],
                        normal: Vec4::new(1.0, 0.0, 0.0, 0.0),
                    },
                    Vertex {
                        position: vec3(gate_pos.x, top_y, gate_pos.z - w),
                        uv: Vec2::ZERO,
                        color: [
                            (col.r * 255.0) as u8,
                            (col.g * 255.0) as u8,
                            (col.b * 255.0) as u8,
                            24,
                        ],
                        normal: Vec4::new(1.0, 0.0, 0.0, 0.0),
                    },
                ];
                let pillar = Mesh {
                    vertices: verts,
                    indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
                    texture: None,
                };
                let used = app.render.bind_translucent_material();
                draw_mesh(&pillar);
                if used {
                    app.render.unbind_world_material();
                }
            }
        }
    }
}

/// Free-fly track editor: the frozen race world viewed through the editor
/// camera, with the terrain brush cursor and a small HUD.
pub fn editor(app: &mut App) {
    let Some(mut owned) = app.race.take() else {
        return;
    };
    let race = &mut owned;
    let (cam, cursor) = match app.editor.as_ref() {
        Some(ed) => (ed.camera(), ed.cursor),
        None => {
            app.race = Some(owned);
            return;
        }
    };
    let focus = cursor.unwrap_or(cam.position);
    draw_world(app, race, &cam, focus, 8);
    if let Some(ed) = app.editor.as_ref() {
        ed.draw_cursor(&race.track, Color::new(1.0, 0.55, 0.10, 1.0));
    }
    app.render.fog_end = 0.0;
    set_default_camera();

    // Editor HUD.
    let (sw, sh) = (vw(), vh());
    draw_rectangle(0.0, 0.0, sw, 40.0, Color::new(0.0, 0.0, 0.0, 0.6));
    let ui = &app.ui;
    ui.text(
        12.0,
        26.0,
        16.0,
        "TRACK EDITOR — WASD fly  RMB look  Shift faster  F5 save  F12 back to race",
        Color::new(0.7, 1.0, 0.7, 1.0),
    );
    if let (Some(c), Some(ed)) = (cursor, app.editor.as_ref()) {
        let info = format!(
            "cursor ({:.1}, {:.1}, {:.1})  props +{}",
            c.x,
            c.y,
            c.z,
            ed.placed.len()
        );
        ui.text(12.0, sh - 24.0, 15.0, &info, Color::new(0.9, 0.9, 0.6, 1.0));
    }

    app.race = Some(owned);
}

pub fn results(app: &mut App) {
    screen_bg(app, "bg_results");
    header(app, "RESULTS");
    let rows = app.results.clone();
    for (i, (name, t)) in rows.iter().enumerate() {
        let r = Rect::new(40.0, 100.0 + i as f32 * 40.0, vw() - 80.0, 36.0);
        let sub = format!("{t:.2}s");
        let _ = app.ui.row(&app.assets, r, name, &sub, i == 0, true);
    }
    // Career races chain into the next queued event instead of dropping to the
    // menu; only the final event returns control.
    let advanced = matches!(
        app.career_progress,
        Some(crate::career::Progress::Advanced { .. })
    );
    let label = match &app.career_progress {
        Some(crate::career::Progress::Advanced { track, event }) => {
            format!("NEXT EVENT {}: {}", event + 1, track)
        }
        _ => "CONTINUE".to_string(),
    };
    if let Some(crate::career::Progress::Complete(award)) = &app.career_progress {
        app.ui.centered(
            vw() * 0.5,
            vh() - 160.0,
            20.0,
            &format!(
                "Final P{}  |  purse ${}  |  {}",
                award.final_place,
                award.purse,
                match award.unlocked_tier {
                    Some(t) => format!("tier {t} unlocked"),
                    None if award.passed => "already unlocked".to_string(),
                    None => "no tier advance".to_string(),
                }
            ),
            Color::new(0.95, 0.9, 0.6, 1.0),
        );
    }
    if app.ui.button(
        &app.assets,
        Rect::new(vw() * 0.5 - 190.0, vh() - 120.0, 380.0, 50.0),
        &label,
        true,
        true,
    ) {
        app.race = None;
        if advanced {
            app.start_next_event();
        } else {
            app.go(State::MainMenu);
        }
    }
}

pub fn credits(app: &mut App) {
    screen_bg(app, "bg_logo");
    header(app, "CREDITS");
    let lines = [
        "EvoX - clean-room source port of 4x4 Evolution",
        "",
        "Original game: Terminal Reality, 2000",
        "POD / SMF / TRK reverse engineering: community",
        "Rust, Macroquad, and the open-source toolchain",
        "",
        "No copyrighted assets are shipped. Point the setup",
        "wizard at your own CD image; assets are rebuilt locally.",
        "",
        "Press Esc",
    ];
    for (i, l) in lines.iter().enumerate() {
        app.ui.text(
            40.0,
            130.0 + i as f32 * 30.0,
            18.0,
            l,
            Color::new(0.85, 0.88, 0.95, 1.0),
        );
    }
}

pub fn mods(app: &mut App) {
    screen_bg(app, "bg_full");
    header(app, "MODS");
    app.ui.text(
        40.0,
        120.0,
        18.0,
        "Drop .TRK vehicles in assets/mods/vehicles or .LTE/.POD tracks in assets/mods/tracks.",
        Color::new(0.85, 0.88, 0.95, 1.0),
    );
    app.ui.text(
        40.0,
        150.0,
        18.0,
        &format!("Watcher events: {}", app.mod_rx_seen),
        WHITE,
    );
    app.ui.text(
        40.0,
        180.0,
        16.0,
        "Vehicles appear in the garage immediately; tracks appear in Quick Race.",
        Color::new(0.75, 0.8, 0.88, 1.0),
    );
    for (i, t) in app.trucks.iter().rev().take(18).enumerate() {
        app.ui.text(
            40.0,
            216.0 + i as f32 * 24.0,
            16.0,
            &format!("{} {} ({})", t.make, t.model, t.file),
            Color::new(0.8, 0.85, 0.9, 1.0),
        );
    }
    app.ui.hint("Rescan");
    if is_key_pressed(KeyCode::R) {
        app.mod_rx_seen += 1;
        app.toast("Watcher runs continuously");
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if line.len() + word.len() + 1 > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

fn draw_arc(cx: f32, cy: f32, r: f32, start: f32, end: f32, segs: usize, thick: f32, color: Color) {
    let mut prev = (cx + r * start.cos(), cy + r * start.sin());
    for i in 1..=segs {
        let a = start + (end - start) * i as f32 / segs as f32;
        let p = (cx + r * a.cos(), cy + r * a.sin());
        draw_line(prev.0, prev.1, p.0, p.1, thick, color);
        prev = p;
    }
}

pub fn unused_lock(_: Lock, _: PartType, _: &mut Focus) {}
