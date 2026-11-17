//! Standalone first-boot intake: locate the per-user asset root, verify that a
//! complete retail tree is present, and otherwise ask for the user's 4x4
//! Evolution 2 ISO or mounted disc and extract it natively.

pub mod extract;
pub mod iso;
pub mod pod;
pub mod tiff;
pub mod verify;

use std::path::{Path, PathBuf};

use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

#[derive(Clone, Debug)]
pub enum Source {
    Iso(PathBuf),
    Folder(PathBuf),
}

impl Source {
    pub fn display(&self) -> String {
        match self {
            Source::Iso(p) => p.display().to_string(),
            Source::Folder(p) => p.display().to_string(),
        }
    }
}

/// Per-user writable data root. `EVOX_ASSET_ROOT` is the QA/dev override.
pub fn data_root() -> PathBuf {
    if let Ok(p) = std::env::var("EVOX_ASSET_ROOT") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    match std::env::consts::OS {
        "macos" => PathBuf::from(home).join("Library/Application Support/Evox"),
        "windows" => std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(&home).join("AppData/Local"))
            .join("Evox"),
        _ => std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(&home).join(".local/share"))
            .join("evox"),
    }
}

fn has_ext(dir: &Path, ext: &str) -> bool {
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case(ext))
        })
}

/// A tree is usable only when every loader-critical library is populated.
/// An empty `assets/extracted` (or a partial run) must not reach `App::new`.
pub fn is_complete(root: &Path) -> bool {
    tree_ready(&root.join("assets/extracted"))
}

/// Readiness of an already-resolved extracted tree (used for staging too).
pub(crate) fn tree_ready(ext: &Path) -> bool {
    has_ext(&ext.join("track/track/LEVELS"), "lvl")
        && has_ext(&ext.join("model/vehicle/TRUCK"), "trk")
        && has_ext(&ext.join("audio/sfx/SOUND"), "wav")
        && ext.join("texture/ui/ART/4X4FONT.RAW").is_file()
}

/// Resolve this boot's root and whether it already holds complete assets.
/// Development checkouts keep using their in-tree `assets/extracted`; a fresh
/// install falls back to the per-user data directory.
pub fn resolve_root(override_dir: Option<&Path>) -> (PathBuf, bool) {
    if let Some(p) = override_dir {
        return (p.to_path_buf(), is_complete(p));
    }
    if let Ok(cwd) = std::env::current_dir() {
        if is_complete(&cwd) {
            return (cwd, true);
        }
    }
    let root = data_root();
    let ready = is_complete(&root);
    (root, ready)
}

fn pick_source() -> Result<Source, String> {
    // QA/automation hook: skip the modal picker when the caller already knows
    // which ISO or folder to intake.
    if let Ok(p) = std::env::var("EVOX_INTAKE_SOURCE") {
        if !p.is_empty() {
            let path = PathBuf::from(p);
            return Ok(
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("iso"))
                {
                    Source::Iso(path)
                } else {
                    Source::Folder(path)
                },
            );
        }
    }
    if let Some(p) = FileDialog::new()
        .set_title("Select your 4x4 Evolution 2 ISO")
        .add_filter("4x4 Evolution 2 ISO", &["iso"])
        .pick_file()
    {
        return Ok(Source::Iso(p));
    }
    if let Some(p) = FileDialog::new()
        .set_title("Select the mounted 4x4 Evo 2 disc or a folder containing its .POD files")
        .pick_folder()
    {
        return Ok(Source::Folder(p));
    }
    Err("no 4x4 Evolution 2 data selected".into())
}

fn error_dialog(message: &str) {
    let _ = MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title("EvoX needs 4x4 Evolution 2 data")
        .set_description(message)
        .set_buttons(MessageButtons::Ok)
        .show();
}

/// First-boot outcome: either the tree is ready, or extraction must run in the
/// loader window.
pub enum Boot {
    Ready(PathBuf),
    Intake(Job),
}

pub struct Job {
    root: PathBuf,
    plan: extract::Plan,
}

/// Ensure a complete tree exists, prompting on first boot. Extraction is
/// handed to the loader so the game window can show progress.
pub fn begin(override_dir: Option<&Path>) -> Result<Boot, String> {
    let (root, ready) = resolve_root(override_dir);
    if ready {
        return Ok(Boot::Ready(root));
    }

    let source = match pick_source() {
        Ok(s) => s,
        Err(e) => {
            error_dialog(&format!(
                "{e}.\n\nEvoX ships no game data: it needs the POD archives from your \
                 own 4x4 Evolution 2 disc."
            ));
            return Err(e);
        }
    };

    let plan = extract::prepare(&source).inspect_err(|e| error_dialog(e))?;
    if !plan.required_missing.is_empty() {
        let msg = format!(
            "This source is missing required archives: {}.\n\nIt does not look like a \
             complete 4x4 Evolution 2 disc.",
            plan.required_missing.join(", ")
        );
        error_dialog(&msg);
        return Err(msg);
    }
    if !plan.unknown.is_empty() {
        eprintln!(
            "intake: {} archives are not in the retail manifest: {}",
            plan.unknown.len(),
            plan.unknown.join(", ")
        );
    }
    if !plan.mismatches.is_empty() {
        let msg = format!(
            "{} archive(s) do not match the retail checksums:\n\n{}\n\nContinue anyway?",
            plan.mismatches.len(),
            plan.mismatches.join("\n")
        );
        let choice = MessageDialog::new()
            .set_level(MessageLevel::Warning)
            .set_title("Checksum mismatch")
            .set_description(&msg)
            .set_buttons(MessageButtons::OkCancel)
            .show();
        if choice != MessageDialogResult::Ok {
            return Err("extraction cancelled at checksum warning".into());
        }
    }

    Ok(Boot::Intake(Job { root, plan }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("evox-root-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn empty_and_partial_trees_are_not_complete() {
        let root = scratch("empty");
        assert!(!is_complete(&root));

        let ext = root.join("assets/extracted");
        std::fs::create_dir_all(ext.join("track/track/LEVELS")).unwrap();
        std::fs::write(ext.join("track/track/LEVELS/TEST.LVL"), b"x").unwrap();
        assert!(!is_complete(&root), "levels alone must not count");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn fully_populated_tree_is_complete() {
        let root = scratch("full");
        let ext = root.join("assets/extracted");
        for (dir, file) in [
            ("track/track/LEVELS", "TEST.LVL"),
            ("model/vehicle/TRUCK", "TEST.TRK"),
            ("audio/sfx/SOUND", "ENG_STOCK.WAV"),
            ("texture/ui/ART", "4X4FONT.RAW"),
        ] {
            let d = ext.join(dir);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(file), b"x").unwrap();
        }
        assert!(is_complete(&root));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn override_wins_even_when_incomplete() {
        let root = scratch("override");
        let (got, ready) = resolve_root(Some(&root));
        assert_eq!(got, root);
        assert!(!ready);
        let _ = std::fs::remove_dir_all(root);
    }
}
