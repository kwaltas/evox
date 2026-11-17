//! First-boot intake: scan a retail ISO/folder, verify PODs, extract them into
//! the per-user asset root.
//!
//! This is the Rust counterpart of `tools/setup_evo2.py` + `tools/pod.py`
//! (minus InstallShield cabinet support and UI PNG baking). Extraction happens
//! in a sibling staging directory and is renamed into place only after the
//! tree validates, so a failed run never leaves a half-usable install.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::iso::{self, IsoPod};
use super::pod::{classify, sanitize, Pod};
use super::tiff;
use super::Source;

const REQUIRED: [&str; 4] = ["startup.pod", "truck.pod", "ui.pod", "sound.pod"];
const MAX_SCAN_DEPTH: usize = 8;

pub enum Progress {
    Started {
        total: usize,
        bytes: u64,
    },
    Archive {
        index: usize,
        total: usize,
        name: String,
    },
    Files {
        files: usize,
        bytes: u64,
    },
    Art {
        done: usize,
        total: usize,
    },
    Done {
        files: usize,
        bytes: u64,
    },
}

enum Archive {
    File {
        path: PathBuf,
        name: String,
        size: u64,
    },
    Iso {
        iso: PathBuf,
        name: String,
        size: u64,
    },
}

impl Archive {
    fn name(&self) -> &str {
        match self {
            Archive::File { name, .. } => name,
            Archive::Iso { name, .. } => name,
        }
    }

    fn file_name(&self) -> &str {
        self.name().rsplit('/').next().unwrap_or(self.name())
    }

    fn size(&self) -> u64 {
        match self {
            Archive::File { size, .. } => *size,
            Archive::Iso { size, .. } => *size,
        }
    }
}

/// Read one archive, reopening the ISO only when the archive comes from one.
/// Keeping `ISOFile` handles in the plan would make it non-`Send` (`Rc`), and
/// the extraction worker must own the plan.
fn read_archive(
    archive: &Archive,
    iso_cache: &mut Option<(PathBuf, Vec<IsoPod>)>,
) -> Result<Vec<u8>, String> {
    match archive {
        Archive::File { path, .. } => {
            std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
        }
        Archive::Iso {
            iso: iso_path,
            name,
            ..
        } => {
            if iso_cache
                .as_ref()
                .map(|(p, _)| p != iso_path)
                .unwrap_or(true)
            {
                *iso_cache = Some((iso_path.clone(), iso::list_pods(iso_path)?));
            }
            let (_, pods) = iso_cache.as_mut().unwrap();
            let pod = pods
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| format!("{name}: archive disappeared from the ISO"))?;
            pod.read()
        }
    }
}

pub struct Plan {
    archives: Vec<Archive>,
    source: Source,
    /// Keeps a fallback-mounted disc attached until extraction ends.
    _mount: Option<iso::MountedIso>,
    pub required_missing: Vec<String>,
    pub mismatches: Vec<String>,
    pub unknown: Vec<String>,
    pub total_bytes: u64,
}

impl Plan {
    pub fn archive_count(&self) -> usize {
        self.archives.len()
    }
}

pub struct Report {
    pub archives: usize,
    pub files: usize,
    pub bytes: u64,
    pub art: tiff::ArtStats,
}

fn collect_pods(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if depth > MAX_SCAN_DEPTH {
        return Ok(());
    }
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_pods(&path, depth + 1, out)?;
            continue;
        }
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pod"))
            && path.metadata().map(|m| m.len() > 64).unwrap_or(false)
        {
            out.push(path);
        }
    }
    Ok(())
}

fn folder_archives(dir: &Path) -> Result<Vec<Archive>, String> {
    let mut pods = Vec::new();
    collect_pods(dir, 0, &mut pods)?;
    pods.sort();
    if pods.is_empty() {
        let cab = std::fs::read_dir(dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("hdr"))
            });
        return Err(if cab {
            "this disc stores the game in InstallShield cabinets; native CAB \
             extraction is not supported yet. Install the game, then choose \
             the folder that contains the .POD files."
                .into()
        } else {
            format!("{}: no 4x4 Evolution 2 .POD archives found", dir.display())
        });
    }
    Ok(pods
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            Archive::File { path, name, size }
        })
        .collect())
}

/// Locate archives and verify their MD5s without writing anything. The caller
/// can show mismatches and ask for explicit confirmation.
pub fn prepare(source: &Source) -> Result<Plan, String> {
    let mut mount = None;
    let archives: Vec<Archive> = match source {
        Source::Iso(path) => match iso::list_pods(path) {
            Ok(pods) => pods
                .into_iter()
                .map(|p| Archive::Iso {
                    iso: path.clone(),
                    name: p.name,
                    size: p.size as u64,
                })
                .collect(),
            Err(pure_error) => {
                let m = iso::mount_fallback(path).map_err(|e| format!("{pure_error}; {e}"))?;
                let list = folder_archives(m.point())?;
                mount = Some(m);
                list
            }
        },
        Source::Folder(dir) => folder_archives(dir)?,
    };

    let expected = super::verify::manifest();
    let mut names = BTreeSet::new();
    let mut mismatches = Vec::new();
    let mut unknown = Vec::new();
    let mut total_bytes = 0;
    let mut iso_cache = None;
    for archive in &archives {
        let name = archive.file_name().to_ascii_lowercase();
        names.insert(name.clone());
        total_bytes += archive.size();
        let bytes = read_archive(archive, &mut iso_cache)?;
        let got = super::verify::md5_hex(&bytes);
        match expected.get(&name) {
            Some(want) if want != &got => {
                mismatches.push(format!("{}: md5 {got} != {want}", archive.file_name()))
            }
            None => unknown.push(archive.file_name().to_string()),
            _ => {}
        }
    }
    let required_missing = REQUIRED
        .iter()
        .filter(|n| !names.contains(**n))
        .map(|n| (*n).to_string())
        .collect();
    Ok(Plan {
        archives,
        source: source.clone(),
        _mount: mount,
        required_missing,
        mismatches,
        unknown,
        total_bytes,
    })
}

/// Extract the prepared archives into `<root>/assets/extracted`, then swap it
/// into place. Progress is reported through `progress` so the loader screen can
/// animate while the worker owns the plan.
fn validate_tree(root: &Path) -> bool {
    super::tree_ready(root)
}

pub fn run(plan: Plan, root: &Path, mut progress: impl FnMut(Progress)) -> Result<Report, String> {
    let assets = root.join("assets");
    let target = assets.join("extracted");
    let staging = assets.join(format!(".extracted.tmp-{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    }
    std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;

    let total = plan.archives.len();
    progress(Progress::Started {
        total,
        bytes: plan.total_bytes,
    });

    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut iso_cache = None;
    for (i, archive) in plan.archives.iter().enumerate() {
        progress(Progress::Archive {
            index: i + 1,
            total,
            name: archive.file_name().to_string(),
        });
        let raw = read_archive(archive, &mut iso_cache)?;
        let stem = Path::new(archive.file_name())
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let pod = Pod::parse(raw)?;
        for entry in &pod.entries {
            if entry.name.is_empty() {
                continue;
            }
            let payload = pod.payload(entry)?;
            let (class, group) = classify(&stem, &entry.name);
            let rel = sanitize(&entry.name);
            let dest = staging.join(class).join(group).join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::write(&dest, &payload).map_err(|e| format!("{}: {e}", dest.display()))?;
            files += 1;
            bytes += payload.len() as u64;
            if files.is_multiple_of(256) {
                progress(Progress::Files { files, bytes });
            }
        }
    }

    let art = tiff::convert_art(&staging, |done, total| {
        progress(Progress::Art { done, total });
    });

    if !validate_tree(&staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(
            "the archives extracted but the tree is incomplete (missing levels, trucks, \
             sounds or 4X4FONT); the source is not a complete 4x4 Evolution 2 disc"
                .into(),
        );
    }

    let backup = assets.join(format!(".extracted.old-{}", std::process::id()));
    if target.exists() {
        std::fs::rename(&target, &backup).map_err(|e| {
            format!(
                "{}: could not move the previous tree aside: {e}",
                target.display()
            )
        })?;
    }
    if let Err(e) = std::fs::rename(&staging, &target) {
        if backup.exists() {
            let _ = std::fs::rename(&backup, &target);
        }
        return Err(format!("{}: {e}", target.display()));
    }
    if backup.exists() {
        let _ = std::fs::remove_dir_all(&backup);
    }

    let install = serde_json::json!({
        "game": "4x4-evo-2",
        "source": plan.source.display(),
        "archives": plan.archive_count(),
        "bytes": plan.total_bytes,
        "verified": plan.mismatches.is_empty(),
        "unknown_archives": plan.unknown,
        "extracted_files": files,
        "extracted_bytes": bytes,
        "art_converted": art.converted,
        "art_skipped": art.skipped,
        "art_failed": art.failed,
    });
    let install_path = assets.join("install.json");
    std::fs::write(
        &install_path,
        serde_json::to_vec_pretty(&install).unwrap_or_default(),
    )
    .map_err(|e| format!("{}: {e}", install_path.display()))?;
    let marker = target.join(".evox-intake.json");
    let _ = std::fs::write(
        &marker,
        serde_json::to_vec_pretty(&install).unwrap_or_default(),
    );

    progress(Progress::Done { files, bytes });
    Ok(Report {
        archives: plan.archive_count(),
        files,
        bytes,
        art,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::DeflateEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn make_pod(names: &[&str], payloads: &[&[u8]], v2: bool) -> Vec<u8> {
        const ENTRY: usize = 20;
        let mut head = Vec::new();
        head.extend_from_slice(if v2 { b"POD2" } else { b"POD\0" });
        if v2 {
            let mut c = b"unit".to_vec();
            c.resize(80, 0);
            head.extend_from_slice(&c);
            head.extend_from_slice(&0u32.to_le_bytes());
        }
        head.extend_from_slice(&(names.len() as u32).to_le_bytes());
        head.extend_from_slice(&(names.len() as u32).to_le_bytes());
        let toc_at = head.len();
        let name_base = toc_at + names.len() * ENTRY;
        let mut names_block = Vec::new();
        let mut offsets = Vec::new();
        for n in names {
            offsets.push(names_block.len() as u32);
            names_block.extend_from_slice(n.as_bytes());
            names_block.push(0);
        }
        let data_at = name_base + names_block.len();
        let mut toc = Vec::new();
        let mut body = Vec::new();
        for (i, p) in payloads.iter().enumerate() {
            let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
            enc.write_all(p).unwrap();
            let stored = enc.finish().unwrap();
            let raw = stored.len() as u32 | 0x8000_0000;
            toc.extend_from_slice(&offsets[i].to_le_bytes());
            toc.extend_from_slice(&raw.to_le_bytes());
            toc.extend_from_slice(&(data_at as u32 + body.len() as u32).to_le_bytes());
            toc.extend_from_slice(&0u64.to_le_bytes());
            body.extend_from_slice(&stored);
        }
        let mut out = head;
        out.extend_from_slice(&toc);
        out.extend_from_slice(&names_block);
        out.extend_from_slice(&body);
        out
    }

    fn archive(path: PathBuf) -> Archive {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Archive::File { path, name, size }
    }

    #[test]
    fn extracts_a_complete_minimal_tree_and_converts_art() {
        let td = std::env::temp_dir().join(format!("evox-intake-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&td);
        std::fs::create_dir_all(&td).unwrap();
        let root = td.join("root");
        let src = td.join("src");
        std::fs::create_dir_all(&src).unwrap();

        let pods = [
            (
                "TEST.POD",
                make_pod(&["LEVELS\\TEST.LVL"], &[b"level"], true),
            ),
            (
                "TEST2.POD",
                make_pod(&["TRUCK\\TEST.TRK"], &[b"truck"], true),
            ),
            (
                "TEST3.POD",
                make_pod(&["SOUND\\ENG_STOCK.WAV"], &[b"wave"], true),
            ),
            ("UI.POD", make_pod(&["ART\\4X4FONT.RAW"], &[b"font"], true)),
        ];
        let mut archives = Vec::new();
        for (name, body) in pods {
            let p = src.join(name);
            std::fs::write(&p, body).unwrap();
            archives.push(archive(p));
        }
        let plan = Plan {
            archives,
            source: Source::Folder(src.clone()),
            _mount: None,
            required_missing: Vec::new(),
            mismatches: Vec::new(),
            unknown: Vec::new(),
            total_bytes: 0,
        };
        let report = run(plan, &root, |_| {}).unwrap();
        assert_eq!(report.files, 4);
        assert!(root
            .join("assets/extracted/track/track/LEVELS/TEST.LVL")
            .exists());
        assert!(root
            .join("assets/extracted/texture/ui/ART/4X4FONT.RAW")
            .exists());
        let _ = std::fs::remove_dir_all(td);
    }
}
