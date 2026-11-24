//! On-screen driving controls for touch devices.
//!
//! Active on Android, and on desktop when `EVOX_TOUCH=1` (the left mouse button
//! then acts as a finger, for layout work). The buttons feed the same inputs
//! the keyboard does, so physics and the race code are untouched.
//!
//! Layout (landscape): steering arrows bottom-left, brake and gas bottom-right,
//! handbrake above the brake, shifters on the left edge, and a column of
//! menu / rescue / 4WD-mode buttons on the right edge below the minimap.

use std::sync::Mutex;

use macroquad::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Btn {
    Left = 0,
    Right,
    Gas,
    Brake,
    Hand,
    Rescue,
    Mode,
    Menu,
    ShiftUp,
    ShiftDown,
}

const COUNT: usize = 10;

struct State {
    down: [bool; COUNT],
    prev: [bool; COUNT],
}

static STATE: Mutex<State> = Mutex::new(State {
    down: [false; COUNT],
    prev: [false; COUNT],
});

/// Whether touch controls are in use this run.
pub fn active() -> bool {
    cfg!(target_os = "android") || std::env::var("EVOX_TOUCH").is_ok_and(|v| v == "1")
}

/// Button rectangles for the current screen size.
fn layout() -> [Rect; COUNT] {
    let w = screen_width();
    let h = screen_height();
    let s = 0.26 * h; // steering / brake size
    let big = 0.34 * h; // gas
    let small = 0.13 * h;
    let pad = 0.04 * h;
    let gap = 0.02 * w;

    let left = Rect::new(0.19 * w, h - s - pad, s, s);
    let right = Rect::new(left.x + s + gap, left.y, s, s);
    let gas = Rect::new(w - big - 0.03 * w, h - big - pad, big, big);
    let brake = Rect::new(gas.x - s - gap, h - s - pad, s, s);
    let hand = Rect::new(
        brake.x + (s - small * 1.6) * 0.5,
        brake.y - small - 0.025 * h,
        small * 1.6,
        small,
    );

    let col_x = w - small * 1.5 - 0.01 * w;
    let col = |i: f32| {
        Rect::new(
            col_x,
            0.19 * h + i * (small * 0.92 + 0.015 * h),
            small * 1.5,
            small * 0.92,
        )
    };
    let menu = col(0.0);
    let rescue = col(1.0);
    let mode = col(2.0);

    let edge_x = 0.02 * w;
    let shift_up = Rect::new(edge_x, 0.30 * h, small * 1.4, small);
    let shift_down = Rect::new(edge_x, 0.30 * h + small + 0.02 * h, small * 1.4, small);

    let mut r = [Rect::new(0.0, 0.0, 0.0, 0.0); COUNT];
    r[Btn::Left as usize] = left;
    r[Btn::Right as usize] = right;
    r[Btn::Gas as usize] = gas;
    r[Btn::Brake as usize] = brake;
    r[Btn::Hand as usize] = hand;
    r[Btn::Rescue as usize] = rescue;
    r[Btn::Mode as usize] = mode;
    r[Btn::Menu as usize] = menu;
    r[Btn::ShiftUp as usize] = shift_up;
    r[Btn::ShiftDown as usize] = shift_down;
    r
}

/// Current finger positions.
fn points() -> Vec<Vec2> {
    let mut p: Vec<Vec2> = touches()
        .into_iter()
        .filter(|t| !matches!(t.phase, TouchPhase::Ended | TouchPhase::Cancelled))
        .map(|t| t.position)
        .collect();
    if !cfg!(target_os = "android") && is_mouse_button_down(MouseButton::Left) {
        p.push(mouse_position().into());
    }
    p
}

/// Refresh button state; call once per frame before reading inputs.
pub fn update() {
    if !active() {
        return;
    }
    let pts = points();
    let rects = layout();
    let mut st = STATE.lock().unwrap();
    st.prev = st.down;
    for (i, r) in rects.iter().enumerate() {
        st.down[i] = pts.iter().any(|p| r.contains(*p));
    }
}

/// Is the button held right now?
pub fn held(b: Btn) -> bool {
    STATE.lock().unwrap().down[b as usize]
}

/// Did the button go down this frame?
pub fn pressed(b: Btn) -> bool {
    let st = STATE.lock().unwrap();
    st.down[b as usize] && !st.prev[b as usize]
}

/// Draw the controls over the race view.
pub fn draw() {
    if !active() || std::env::var("EVOX_TOUCH_HIDE").is_ok() {
        return;
    }
    let rects = layout();
    let st = STATE.lock().unwrap();
    let labels = [
        "", "", "GAS", "BRAKE", "HAND", "RESCUE", "4WD", "MENU", "SHIFT +", "SHIFT -",
    ];
    for (i, r) in rects.iter().enumerate() {
        let on = st.down[i];
        let fill = if on {
            Color::new(0.95, 0.72, 0.25, 0.55)
        } else {
            Color::new(1.0, 1.0, 1.0, 0.16)
        };
        draw_rectangle(r.x, r.y, r.w, r.h, fill);
        draw_rectangle_lines(r.x, r.y, r.w, r.h, 3.0, Color::new(1.0, 1.0, 1.0, 0.55));
        let c = vec2(r.x + r.w * 0.5, r.y + r.h * 0.5);
        let white = Color::new(1.0, 1.0, 1.0, 0.9);
        if i == Btn::Left as usize || i == Btn::Right as usize {
            let d = if i == Btn::Left as usize { -1.0 } else { 1.0 };
            let a = r.w * 0.22;
            draw_triangle(
                vec2(c.x + d * a, c.y),
                vec2(c.x - d * a, c.y - a),
                vec2(c.x - d * a, c.y + a),
                white,
            );
        } else {
            let size = (r.h * 0.26).clamp(18.0, 44.0);
            let d = measure_text(labels[i], None, size as u16, 1.0);
            draw_text(
                labels[i],
                c.x - d.width * 0.5,
                c.y + d.height * 0.4,
                size,
                white,
            );
        }
    }
}
