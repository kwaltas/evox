//! Persistent profile, integrity, and UGC mod watching.
//!
//! The save is JSON with a SHA-256 trailer. Writes are atomic (temp + rename)
//! and the previous file is kept as `.bak`. A corrupt checksum is reported but
//! not fatal: the loader returns a recoverable error so the UI can offer the
//! backup instead of wiping progression.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::career::Career;

pub const SAVE_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    pub master_volume: f32,
    pub music_volume: f32,
    pub invert_y: bool,
    pub metric: bool,
    pub camera_fov: f32,
    pub vsync: bool,
    pub resolution_scale: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub version: u32,
    pub name: String,
    pub budget: i64,
    pub owned: HashSet<String>,
    pub equipped: HashMap<String, String>, // truck file -> part code per slot flattened at save time
    pub garage: Vec<String>,               // owned truck files
    pub career: Career,
    pub settings: Settings,
    pub best_times: HashMap<String, f32>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            version: SAVE_VERSION,
            name: "Driver".into(),
            budget: 25_000,
            owned: HashSet::new(),
            equipped: HashMap::new(),
            garage: vec!["MMLA3.TRK".into()],
            career: Career::default(),
            settings: Settings {
                master_volume: 0.8,
                music_volume: 0.45,
                invert_y: false,
                metric: true,
                camera_fov: 70.0,
                vsync: true,
                resolution_scale: 1.0,
            },
            best_times: HashMap::new(),
        }
    }
}

fn checksum(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug)]
pub enum SaveError {
    Io(String),
    Parse(String),
    Integrity,
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Io(e) => write!(f, "save io: {e}"),
            SaveError::Parse(e) => write!(f, "save parse: {e}"),
            SaveError::Integrity => write!(f, "save checksum mismatch"),
        }
    }
}

pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn save(&self, profile: &Profile) -> Result<(), SaveError> {
        let body = serde_json::to_vec_pretty(profile).map_err(|e| SaveError::Io(e.to_string()))?;
        let sum = checksum(&body);
        let mut out = body;
        out.extend_from_slice(b"\n");
        out.extend_from_slice(sum.as_bytes());

        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| SaveError::Io(e.to_string()))?;
        }
        if self.path.exists() {
            let _ = std::fs::copy(&self.path, self.path.with_extension("bak"));
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &out).map_err(|e| SaveError::Io(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| SaveError::Io(e.to_string()))?;
        Ok(())
    }

    pub fn load(&self) -> Result<Profile, SaveError> {
        let raw = std::fs::read(&self.path).map_err(|e| SaveError::Io(e.to_string()))?;
        Self::decode(&raw)
    }

    pub fn load_backup(&self) -> Result<Profile, SaveError> {
        let raw = std::fs::read(self.path.with_extension("bak"))
            .map_err(|e| SaveError::Io(e.to_string()))?;
        Self::decode(&raw)
    }

    fn decode(raw: &[u8]) -> Result<Profile, SaveError> {
        let text = std::str::from_utf8(raw).map_err(|e| SaveError::Parse(e.to_string()))?;
        let (body, sum) = text
            .rsplit_once('\n')
            .ok_or_else(|| SaveError::Parse("missing checksum".into()))?;
        if checksum(body.as_bytes()) != sum.trim() {
            return Err(SaveError::Integrity);
        }
        let profile: Profile =
            serde_json::from_str(body).map_err(|e| SaveError::Parse(e.to_string()))?;
        if profile.version > SAVE_VERSION {
            return Err(SaveError::Parse(format!(
                "save version {} is newer",
                profile.version
            )));
        }
        Ok(profile)
    }
}

/// New/changed files in the user content folders, polled off the render thread.
#[derive(Debug, Clone)]
pub struct ModFile {
    pub path: PathBuf,
    pub kind: ModKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModKind {
    Track,
    Vehicle,
}

pub fn watch_mods(root: impl AsRef<Path>) -> Receiver<ModFile> {
    let root = root.as_ref().to_path_buf();
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new()
        .name("mod-watch".into())
        .spawn(move || {
            let mut seen: HashMap<PathBuf, std::time::SystemTime> = HashMap::new();
            loop {
                for (dir, kind) in [
                    (root.join("tracks"), ModKind::Track),
                    (root.join("vehicles"), ModKind::Vehicle),
                ] {
                    let Ok(rd) = std::fs::read_dir(&dir) else {
                        continue;
                    };
                    for p in rd.filter_map(|e| e.ok()).map(|e| e.path()) {
                        let ext_ok = p
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|e| {
                                matches!(e.to_ascii_lowercase().as_str(), "pod" | "trk" | "lte")
                            })
                            .unwrap_or(false);
                        if !ext_ok || !p.is_file() {
                            continue;
                        }
                        let mtime = p
                            .metadata()
                            .and_then(|m| m.modified())
                            .unwrap_or(std::time::UNIX_EPOCH);
                        if seen.insert(p.clone(), mtime) != Some(mtime) {
                            let _ = tx.send(ModFile { path: p, kind });
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        });
    // Hot-reloading mods is optional; a refused thread must not take the game
    // down with it. Say so and carry on without the watcher.
    if let Err(e) = spawned {
        eprintln!("mods: watcher thread could not start ({e}); hot reload disabled");
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!("evox-save-{}", std::process::id()))
    }

    #[test]
    fn roundtrip_and_tamper() {
        let path = tmp().with_extension("json");
        let store = Store::new(&path);
        let mut p = Profile::default();
        p.budget = 1234;
        store.save(&p).unwrap();
        store.save(&p).unwrap(); // second write creates the .bak copy
        assert_eq!(store.load().unwrap().budget, 1234);

        let text = std::fs::read_to_string(&path).unwrap();
        let tampered = text.replacen("1234", "99999", 1);
        std::fs::write(&path, tampered).unwrap();
        assert!(matches!(store.load(), Err(SaveError::Integrity)));
        assert_eq!(store.load_backup().unwrap().budget, 1234);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sha256_known_answer() {
        assert_eq!(
            checksum(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
