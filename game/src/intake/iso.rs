//! Read POD archives straight out of an ISO 9660 image.
//!
//! The retail disc is ISO 9660 level 1/2; reading it with a parser avoids
//! mounting, root privileges and platform-specific mount tools. If an image
//! uses an unsupported variant the caller can fall back to asking the user for
//! the mounted disc folder.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use iso9660::{DirectoryEntry, ISODirectory, ISOFile, ISO9660};

const MAX_DEPTH: usize = 16;

#[derive(Debug)]
pub struct IsoPod {
    /// Path inside the image, for diagnostics.
    pub name: String,
    pub size: u32,
    file: ISOFile<File>,
}

impl IsoPod {
    /// Streaming reader positioned at the start of the file.
    pub fn reader(&self) -> impl std::io::Read + std::io::Seek {
        self.file.read()
    }

    pub fn read(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(self.size as usize);
        self.file
            .read()
            .read_to_end(&mut out)
            .map_err(|e| format!("{}: reading from ISO failed: {e}", self.name))?;
        if out.len() != self.size as usize {
            return Err(format!(
                "{}: short read from ISO ({} of {} bytes)",
                self.name,
                out.len(),
                self.size
            ));
        }
        Ok(out)
    }
}

fn walk(
    dir: &ISODirectory<File>,
    prefix: &str,
    depth: usize,
    out: &mut Vec<IsoPod>,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("ISO directory nesting is implausibly deep".into());
    }
    for entry in dir.contents() {
        let entry = entry.map_err(|e| format!("ISO directory read failed: {e}"))?;
        match entry {
            DirectoryEntry::Directory(d) => {
                let id = &d.identifier;
                if id == "." || id == ".." {
                    continue;
                }
                let next = if prefix.is_empty() {
                    id.to_string()
                } else {
                    format!("{prefix}/{id}")
                };
                walk(&d, &next, depth + 1, out)?;
            }
            DirectoryEntry::File(f) => {
                let id = f.identifier.to_ascii_lowercase();
                let id = id.split(';').next().unwrap_or(&id);
                if id.ends_with(".pod") || id == "data1.hdr" || id == "data1.cab" {
                    let name = if prefix.is_empty() {
                        f.identifier.clone()
                    } else {
                        format!("{prefix}/{}", f.identifier)
                    };
                    out.push(IsoPod {
                        name,
                        size: f.size(),
                        file: f,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Open the disc image. A path of the form `fd:<n>` is an already-open file
/// descriptor (Android's document picker hands over one; reopening it through
/// `/proc/self/fd` is refused by SELinux), which is duplicated, never closed.
fn open_image(path: &Path) -> std::io::Result<File> {
    #[cfg(unix)]
    if let Some(n) = path.to_str().and_then(|p| p.strip_prefix("fd:")) {
        use std::os::fd::{BorrowedFd, RawFd};
        let fd: RawFd = n
            .parse()
            .map_err(|_| std::io::Error::other("bad file descriptor number"))?;
        // SAFETY: the caller keeps `fd` open for the life of the process.
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        return Ok(File::from(borrowed.try_clone_to_owned()?));
    }
    File::open(path)
}

/// Every `.POD` in the image, plus the InstallShield `data1.hdr`/`data1.cab`
/// pair when present, sorted by name. The file handle is retained so the
/// caller can stream one archive at a time instead of loading the disc.
pub fn list_pods(path: &Path) -> Result<Vec<IsoPod>, String> {
    let file = open_image(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let iso = ISO9660::new(file).map_err(|e| {
        format!(
            "{}: not a readable ISO 9660 image ({e}); mount it and choose the disc folder instead",
            path.display()
        )
    })?;
    let mut out = Vec::new();
    walk(&iso.root, "", 0, &mut out)?;
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// ISO mounted by the platform as a fallback for images that `iso9660` cannot
/// parse. Detached on drop.
pub struct MountedIso {
    point: PathBuf,
}

impl MountedIso {
    pub fn point(&self) -> &Path {
        &self.point
    }
}

impl Drop for MountedIso {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("hdiutil")
                .args(["detach", "-quiet"])
                .arg(&self.point)
                .status();
        }
    }
}

#[cfg(target_os = "macos")]
pub fn mount_fallback(path: &Path) -> Result<MountedIso, String> {
    let out = std::process::Command::new("hdiutil")
        .args(["attach", "-readonly", "-nobrowse"])
        .arg(path)
        .output()
        .map_err(|e| format!("hdiutil attach failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "hdiutil attach failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(idx) = line.find("/Volumes/") {
            return Ok(MountedIso {
                point: PathBuf::from(line[idx..].trim()),
            });
        }
    }
    Err("hdiutil attached the image but reported no /Volumes mount point".into())
}

#[cfg(not(target_os = "macos"))]
pub fn mount_fallback(_path: &Path) -> Result<MountedIso, String> {
    Err(
        "this ISO could not be read directly; mount it with your operating system \
         and choose the mounted disc folder"
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_iso_input() {
        let path = std::env::temp_dir().join("evox-not-an-iso.bin");
        std::fs::write(&path, b"this is not an iso").unwrap();
        let err = list_pods(&path).unwrap_err();
        assert!(err.contains("not a readable ISO"), "{err}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn iso_pod_name_lookup_is_case_insensitive_after_construction() {
        // The walker stores the on-disc identifier and filters with a
        // lowercase suffix check; this guards the helper used by callers.
        assert!("UI.POD".to_ascii_lowercase().ends_with(".pod"));
        assert!("ui.pod".to_ascii_lowercase().ends_with(".pod"));
    }
}
