//! In-window extraction progress. The worker owns the plan and reports over an
//! mpsc channel; this loop draws with macroquad and never blocks on disk I/O.

use std::sync::mpsc::channel;
use std::thread;

use macroquad::prelude::*;

use super::extract::{self, Progress};
use super::Job;

const BG: Color = Color::new(0.035, 0.04, 0.06, 1.0);
const ACCENT: Color = Color::new(0.95, 0.72, 0.25, 1.0);

pub async fn run(job: Job) -> Result<(), String> {
    let root = job.root;
    let plan = job.plan;
    let worker_root = root.clone();
    let (tx, rx) = channel();
    let worker = thread::spawn(move || {
        let mut send = |p| {
            let _ = tx.send(p);
        };
        extract::run(plan, &worker_root, &mut send)
    });

    let mut progress = Progress::Started { total: 0, bytes: 0 };
    let result = loop {
        while let Ok(p) = rx.try_recv() {
            progress = p;
        }
        draw(&progress);
        if worker.is_finished() {
            break worker
                .join()
                .unwrap_or_else(|_| Err("the extraction worker panicked".to_string()));
        }
        next_frame().await;
    };
    match result {
        Ok(report) => {
            std::env::set_current_dir(&root)
                .map_err(|e| format!("cannot enter {}: {e}", root.display()))?;
            eprintln!(
                "intake: extracted {} files ({:.1} MiB), art converted {} / skipped {} / failed {}",
                report.files,
                report.bytes as f64 / 1_048_576.0,
                report.art.converted,
                report.art.skipped,
                report.art.failed
            );
            Ok(())
        }
        Err(e) => {
            error_screen(&e).await;
            Err(e)
        }
    }
}

fn status(p: &Progress) -> (String, f32, String) {
    match p {
        Progress::Started { total, bytes } => (
            format!(
                "Found {total} POD archives ({:.1} MiB)",
                *bytes as f64 / 1_048_576.0
            ),
            0.02,
            "Verifying the retail archives".into(),
        ),
        Progress::Archive { index, total, name } => (
            format!("Archive {index} of {total}"),
            if *total == 0 {
                0.0
            } else {
                *index as f32 / *total as f32
            },
            name.clone(),
        ),
        Progress::Files { files, bytes } => (
            format!(
                "{files} files extracted ({:.1} MiB)",
                *bytes as f64 / 1_048_576.0
            ),
            0.5,
            "Extracting".into(),
        ),
        Progress::Art { done, total } => (
            format!("Vehicle art {done}/{total}"),
            if *total == 0 {
                0.99
            } else {
                0.9 + 0.09 * (*done as f32 / *total as f32)
            },
            "Baking palette TIFFs to RAW/ACT/OPA".into(),
        ),
        Progress::UiArt { done, total } => (
            format!("Menu art {done}/{total}"),
            if *total == 0 {
                0.99
            } else {
                0.99 + 0.009 * (*done as f32 / *total as f32)
            },
            "Converting interface TIFFs to PNG".into(),
        ),
        Progress::Done { files, bytes } => (
            format!(
                "Done: {files} files, {:.1} MiB",
                *bytes as f64 / 1_048_576.0
            ),
            1.0,
            "Checking the extracted tree".into(),
        ),
    }
}

fn draw(p: &Progress) {
    let w = screen_width();
    let h = screen_height();
    clear_background(BG);

    let title = "4x4 Evolution 2 - importing game data";
    let td = measure_text(title, None, 30, 1.0);
    draw_text(title, (w - td.width) * 0.5, h * 0.32, 30.0, WHITE);

    let (headline, frac, detail) = status(p);
    let hd = measure_text(&headline, None, 20, 1.0);
    draw_text(&headline, (w - hd.width) * 0.5, h * 0.44, 20.0, ACCENT);
    let dd = measure_text(&detail, None, 16, 1.0);
    draw_text(&detail, (w - dd.width) * 0.5, h * 0.49, 16.0, GRAY);

    let bar_w = (w * 0.62).max(320.0);
    let bar_x = (w - bar_w) * 0.5;
    let bar_y = h * 0.56;
    draw_rectangle(bar_x, bar_y, bar_w, 18.0, Color::new(0.12, 0.13, 0.16, 1.0));
    draw_rectangle(bar_x, bar_y, bar_w * frac.clamp(0.0, 1.0), 18.0, ACCENT);
    draw_rectangle_lines(
        bar_x,
        bar_y,
        bar_w,
        18.0,
        2.0,
        Color::new(0.5, 0.5, 0.55, 1.0),
    );

    let note = "This runs once. Your own retail data stays on this machine.";
    let nd = measure_text(note, None, 14, 1.0);
    draw_text(note, (w - nd.width) * 0.5, h * 0.66, 14.0, GRAY);
}

async fn error_screen(message: &str) {
    loop {
        let w = screen_width();
        let h = screen_height();
        clear_background(BG);
        let title = "Import failed";
        let td = measure_text(title, None, 30, 1.0);
        draw_text(
            title,
            (w - td.width) * 0.5,
            h * 0.30,
            30.0,
            Color::new(1.0, 0.5, 0.4, 1.0),
        );
        let mut y = h * 0.40;
        for line in wrap(message, 78) {
            let d = measure_text(&line, None, 16, 1.0);
            draw_text(&line, (w - d.width) * 0.5, y, 16.0, WHITE);
            y += 22.0;
        }
        let hint = "Press Enter or Escape to close";
        let hd = measure_text(hint, None, 16, 1.0);
        draw_text(hint, (w - hd.width) * 0.5, h * 0.82, 16.0, GRAY);
        if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::Escape) {
            break;
        }
        next_frame().await;
    }
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

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn wraps_long_messages() {
        let lines = wrap("one two three four five six", 9);
        assert!(lines.iter().all(|l| l.len() <= 9), "{lines:?}");
        assert!(lines.join(" ").contains("six"));
    }
}
