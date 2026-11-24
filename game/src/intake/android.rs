//! Android first-boot intake: ask the user for their 4x4 Evolution 2 ISO with
//! the system document picker, then hand it to the normal extraction loader.
//!
//! The picker lives in Java (`MainActivity.pickIso`, injected by
//! `android/build.sh`). Java opens the chosen document, detaches its file
//! descriptor and reports it through `EvoxNative.isoPicked`; the ISO reader
//! then duplicates it (path `fd:<n>`), so no storage permission is needed and
//! nothing is copied.

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::channel;
use std::thread;

use macroquad::prelude::*;

use super::{extract, Job, Source};

const BG: Color = Color::new(0.035, 0.04, 0.06, 1.0);
const ACCENT: Color = Color::new(0.95, 0.72, 0.25, 1.0);

/// No answer yet.
const WAITING: i32 = -3;
/// The user backed out of the picker.
const CANCELLED: i32 = -2;
/// Java could not open the chosen document.
const OPEN_FAILED: i32 = -1;

static PICKED: AtomicI32 = AtomicI32::new(WAITING);

/// Called from `MainActivity.onActivityResult` (UI thread).
#[no_mangle]
pub extern "C" fn Java_com_evox_game_EvoxNative_isoPicked(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
    fd: i32,
) {
    PICKED.store(fd, Ordering::SeqCst);
}

/// Ask the activity to open the document picker.
fn request_pick() -> Result<(), String> {
    use macroquad::miniquad::native::android::{attach_jni_env, ACTIVITY};
    unsafe {
        let env = attach_jni_env();
        let activity = ACTIVITY;
        if env.is_null() || activity.is_null() {
            return Err("the Android activity is not ready".into());
        }
        let jni = &**env;
        let class = jni.GetObjectClass.ok_or("JNI GetObjectClass missing")?(env, activity);
        let method = jni.GetMethodID.ok_or("JNI GetMethodID missing")?(
            env,
            class,
            c"pickIso".as_ptr(),
            c"()V".as_ptr(),
        );
        if method.is_null() {
            return Err("MainActivity.pickIso is missing from this build".into());
        }
        jni.CallVoidMethod.ok_or("JNI CallVoidMethod missing")?(env, activity, method);
    }
    Ok(())
}

fn centered(text: &str, y: f32, size: f32, color: Color) {
    let d = measure_text(text, None, size as u16, 1.0);
    draw_text(text, (screen_width() - d.width) * 0.5, y, size, color);
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// Button rectangle (x, y, w, h) for the current screen size.
fn button() -> (f32, f32, f32, f32) {
    let w = screen_width();
    let h = screen_height();
    let bw = (w * 0.5).max(280.0);
    let bh = h * 0.14;
    ((w - bw) * 0.5, h * 0.62, bw, bh)
}

fn draw_pick_screen(note: Option<&str>) {
    let h = screen_height();
    let s = h / 540.0;
    clear_background(BG);
    centered("4x4 Evolution 2", h * 0.20, 44.0 * s, WHITE);
    centered(
        "Necesitas tu propia copia del juego. Elige el archivo .ISO del disco.",
        h * 0.32,
        22.0 * s,
        GRAY,
    );
    centered(
        "Se hace una sola vez; tus datos no salen del telefono.",
        h * 0.38,
        18.0 * s,
        GRAY,
    );
    if let Some(note) = note {
        let mut y = h * 0.47;
        for line in wrap(note, 60) {
            centered(&line, y, 20.0 * s, Color::new(1.0, 0.5, 0.4, 1.0));
            y += 26.0 * s;
        }
    }
    let (bx, by, bw, bh) = button();
    draw_rectangle(bx, by, bw, bh, ACCENT);
    let label = "Cargar ISO";
    let d = measure_text(label, None, (28.0 * s) as u16, 1.0);
    draw_text(
        label,
        bx + (bw - d.width) * 0.5,
        by + bh * 0.5 + d.height * 0.4,
        28.0 * s,
        BG,
    );
}

fn draw_busy_screen(title: &str, detail: &str) {
    let h = screen_height();
    let s = h / 540.0;
    clear_background(BG);
    centered(title, h * 0.40, 30.0 * s, WHITE);
    centered(detail, h * 0.48, 18.0 * s, GRAY);
    // Indeterminate bar: a block sweeping across.
    let w = screen_width();
    let bar_w = w * 0.6;
    let bar_x = (w - bar_w) * 0.5;
    let bar_y = h * 0.58;
    draw_rectangle(
        bar_x,
        bar_y,
        bar_w,
        16.0 * s,
        Color::new(0.12, 0.13, 0.16, 1.0),
    );
    let t = (get_time() as f32 * 0.8).fract();
    let seg = bar_w * 0.25;
    draw_rectangle(bar_x + (bar_w - seg) * t, bar_y, seg, 16.0 * s, ACCENT);
}

/// Run the picker until the user supplies a valid ISO, then return the
/// extraction job for `loader::run`. Never returns on a cancelled picker: it
/// shows the button again instead.
pub async fn acquire(root: PathBuf) -> Result<Job, String> {
    let mut auto = true;
    let mut note: Option<String> = None;
    loop {
        draw_pick_screen(note.as_deref());
        let (bx, by, bw, bh) = button();
        let (mx, my) = mouse_position();
        let tapped = is_mouse_button_pressed(MouseButton::Left)
            && mx >= bx
            && mx <= bx + bw
            && my >= by
            && my <= by + bh;
        if auto || tapped {
            auto = false;
            PICKED.store(WAITING, Ordering::SeqCst);
            request_pick()?;
            let fd = loop {
                let v = PICKED.load(Ordering::SeqCst);
                if v != WAITING {
                    break v;
                }
                draw_busy_screen("Elige tu ISO", "Selecciona el archivo en el selector");
                next_frame().await;
            };
            match fd {
                CANCELLED => note = Some("No se eligio ningun archivo.".into()),
                OPEN_FAILED => {
                    note = Some(
                        "No se pudo abrir ese archivo. Si esta en la nube, descargalo \
                         primero al telefono."
                            .into(),
                    )
                }
                fd => match verify(fd).await {
                    Ok(plan) => return Ok(Job { root, plan }),
                    Err(e) => note = Some(e),
                },
            }
        }
        next_frame().await;
    }
}

/// List and checksum the PODs on a worker thread so the window stays live.
async fn verify(fd: i32) -> Result<extract::Plan, String> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let source = Source::Iso(PathBuf::from(format!("fd:{fd}")));
        let _ = tx.send(extract::prepare(&source));
    });
    let plan = loop {
        match rx.try_recv() {
            Ok(r) => break r,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                draw_busy_screen(
                    "Verificando el disco",
                    "Comprobando los archivos del juego (puede tardar un par de minutos)",
                );
                next_frame().await;
            }
            Err(_) => return Err("la verificacion se interrumpio".into()),
        }
    }?;
    if !plan.required_missing.is_empty() {
        return Err(format!(
            "Este ISO no parece un disco completo de 4x4 Evolution 2. Faltan: {}.",
            plan.required_missing.join(", ")
        ));
    }
    if !plan.mismatches.is_empty() {
        // The desktop build asks whether to continue; a modded or re-ripped
        // disc is still usable, so import it and leave the evidence in logcat.
        eprintln!(
            "intake: {} archive(s) differ from the retail checksums: {}",
            plan.mismatches.len(),
            plan.mismatches.join("; ")
        );
    }
    Ok(plan)
}
