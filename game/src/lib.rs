//! EvoX engine root (the desktop `main.rs` and the Android cdylib both call into it):  `evox`, `evox --self-test 0`, `evox --resolution WxH`.

#![allow(dead_code)]
#![allow(
    clippy::too_many_arguments,
    clippy::collapsible_if,
    clippy::chunks_exact_to_as_chunks,
    clippy::excessive_precision,
    clippy::field_reassign_with_default,
    clippy::if_same_then_else,
    clippy::items_after_test_module,
    clippy::let_and_return,
    clippy::manual_flatten,
    clippy::map_identity,
    clippy::needless_borrow,
    clippy::question_mark,
    clippy::single_match,
    clippy::unnecessary_lazy_evaluations,
    clippy::unnecessary_min_or_max,
    clippy::unnecessary_sort_by,
    clippy::useless_format,
    clippy::bool_comparison
)]

use macroquad::prelude::*;

/// Skip a unit test when the git-ignored `assets/extracted` tree is absent, so
/// CI (which checks out source only) is green while a workstation with the
/// retail data still runs every asset assertion. Tests call it first thing.
#[cfg(test)]
#[macro_export]
macro_rules! require_assets {
    () => {
        if !std::path::Path::new("assets/extracted").exists()
            && !std::path::Path::new("../assets/extracted").exists()
        {
            eprintln!("skip: assets/extracted is not extracted");
            return;
        }
    };
}

mod ai;
#[cfg(target_os = "android")]
mod android_log;
mod app;
mod assets;
mod audio;
mod career;
mod classic;
mod collide;
mod editor;
mod intake;
mod leaderboard;
mod master;
mod net;
mod paint;
mod parts;
mod physics;
mod race;
mod render;
mod save;
mod self_test;
mod smf;
mod terrain;
mod touch;
mod ui;
mod vehicle;
mod vfx;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Window size from `--resolution WxH`. It used to be `--screen WxH`, which
/// collided with `--screen <state>` (boot/shop/...) used by the QA hooks.
fn parse_resolution(args: &[String]) -> (u32, u32) {
    for (i, a) in args.iter().enumerate() {
        if a == "--resolution" {
            if let Some(v) = args.get(i + 1) {
                if let Some((w, h)) = v.split_once('x') {
                    if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
                        return (w, h);
                    }
                }
            }
        }
    }
    (1280, 720)
}

/// Android entry: miniquad's `MainActivity` calls this symbol once the Java
/// side is up (the binary is built as a `cdylib`, see `android/build.sh`).
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn quad_main() {
    android_log::redirect_stdio();
    main();
}

/// Command line. Android has none, so QA runs pass it as `EVOX_ARGS`.
fn cli_args() -> Vec<String> {
    #[cfg(target_os = "android")]
    {
        let mut v = vec!["evox".to_string()];
        if let Ok(a) = std::env::var("EVOX_ARGS") {
            v.extend(a.split_whitespace().map(str::to_string));
        }
        v
    }
    #[cfg(not(target_os = "android"))]
    std::env::args().collect()
}

pub fn main() {
    let args = cli_args();
    if let Some(i) = args.iter().position(|a| a == "--self-test") {
        let level: u32 = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(0);
        std::process::exit(self_test::run(level));
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("evox {VERSION}");
        println!("  --self-test <level>   run diagnostics and exit (0 = fast)");
        println!("  --resolution WxH      window size");
        println!("  --race TRACK          boot straight into a race (QA)");
        println!("  --assets DIR          override extracted asset root");
        println!("  --debug               HUD debug overlay");
        return;
    }

    let (w, h) = parse_resolution(&args);
    let debug = args.iter().any(|a| a == "--debug");

    // A positional directory is a legacy asset root. Apply it before the
    // intake check so a checkout can still point at an out-of-tree tree.
    let assets_override = args
        .iter()
        .position(|a| a == "--assets")
        .and_then(|i| args.get(i + 1))
        .map(std::path::PathBuf::from);
    if assets_override.is_none() {
        if let Some(dir) = args.get(1).filter(|a| !a.starts_with('-')) {
            let p = std::path::Path::new(dir);
            if p.is_dir() {
                let _ = std::env::set_current_dir(p);
            }
        }
    }
    #[cfg(target_os = "android")]
    let mut need_source: Option<std::path::PathBuf> = None;
    let job = match intake::begin(assets_override.as_deref()) {
        Ok(intake::Boot::Ready(root)) => {
            if let Err(e) = std::env::set_current_dir(&root) {
                eprintln!("intake: cannot enter {}: {e}", root.display());
                return;
            }
            None
        }
        Ok(intake::Boot::Intake(job)) => Some(job),
        #[cfg(target_os = "android")]
        Ok(intake::Boot::NeedSource(root)) => {
            need_source = Some(root);
            None
        }
        Err(e) => {
            eprintln!("intake: {e}");
            return;
        }
    };

    let conf = Conf {
        window_title: format!("EvoX {VERSION}"),
        window_width: w as i32,
        window_height: h as i32,
        high_dpi: true,
        window_resizable: true,
        fullscreen: false,
        sample_count: 4,
        ..Default::default()
    };

    #[cfg(target_os = "android")]
    macroquad::Window::from_config(conf, run(debug, job, need_source));
    #[cfg(not(target_os = "android"))]
    macroquad::Window::from_config(conf, run(debug, job, None));
}

async fn run(debug: bool, job: Option<intake::Job>, need_source: Option<std::path::PathBuf>) {
    // Retail meshes exceed Macroquad's default 5k-index draw call, which
    // silently clamps body geometry. Raise the batch budget before drawing.
    macroquad::window::gl_set_drawcall_buffer_capacity(65_535, 200_000);
    // Android: the window must exist before the user can pick the ISO.
    #[cfg(target_os = "android")]
    let job = match need_source {
        Some(root) => match intake::android::acquire(root).await {
            Ok(j) => Some(j),
            Err(e) => {
                eprintln!("intake: {e}");
                return;
            }
        },
        None => job,
    };
    #[cfg(not(target_os = "android"))]
    let _ = need_source;
    if let Some(job) = job {
        if let Err(e) = intake::loader::run(job).await {
            eprintln!("intake: {e}");
            return;
        }
    }
    let mut app = app::App::new().await;
    app.show_debug = debug;
    // QA hooks: EVOX_TRUCK=<TRK stem> picks a truck, EVOX_PAINT=<n> a paint.
    if let Ok(stem) = std::env::var("EVOX_TRUCK") {
        let want = format!("{}.TRK", stem.to_ascii_uppercase());
        if let Some(i) = app
            .garage_files()
            .iter()
            .position(|t| t.file.to_ascii_uppercase().ends_with(&want))
        {
            app.selected_truck = i;
        }
    }
    if let Some(p) = std::env::var("EVOX_PAINT")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        app.setup.paint = p;
    }
    // QA hook: deterministic boot into a single race without menu navigation.
    let args = cli_args();
    if let Some(i) = args.iter().position(|a| a == "--race") {
        if let Some(track) = args.get(i + 1) {
            app.setup.track = track.clone();
            app.setup.opponents = 3;
            app.start_loading(None);
        }
    }

    // QA hook: jump straight to a screen for screenshots.
    if let Some(i) = args.iter().position(|a| a == "--screen") {
        if let Some(s) = args.get(i + 1) {
            app.state = match s.as_str() {
                "boot" | "splash" | "intro" => app::State::BootSequence,
                "fallback_boot" => app::State::Boot,
                "title" => app::State::Title,
                "quickrace" => app::State::QuickRace,
                "career" => app::State::Career,
                "garage" | "dealership" => app::State::Garage,
                "shop" => {
                    app.open_shop();
                    app::State::Shop
                }
                "main" | "menu" => app::State::MainMenu,
                "options" | "settings" => app::State::Settings,
                "credits" => app::State::Credits,
                "mods" => app::State::Mods,
                "multiplayer" => app::State::Multiplayer,
                _ => app.state,
            };
        }
    }

    // QA hook: EVOX_SHOT=out.png [EVOX_SHOT_T=seconds] saves one frame and exits.
    // When `--race` is active, `EVOX_SHOT_T` measures in-race simulation time
    // (`race.time`) so `t=1s`, `t=5s`, `t=15s` are exact race timestamps.
    let is_race_cli = args.iter().any(|a| a == "--race");
    let shot = std::env::var("EVOX_SHOT").ok();
    let shot_t: f64 = std::env::var("EVOX_SHOT_T")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5.0);
    let t0 = get_time();
    loop {
        let dt = get_frame_time().min(0.1);
        app.update(dt);
        app.draw();
        if let Some(path) = &shot {
            let ready = if is_race_cli {
                app.state == app::State::Race
                    && app.race.as_ref().is_some_and(|r| r.time as f64 >= shot_t)
            } else {
                get_time() - t0 >= shot_t
            };
            if ready {
                get_screen_data().export_png(path);
                break;
            }
        }
        if app.quit {
            app.save_profile();
            break;
        }
        next_frame().await;
    }
}
