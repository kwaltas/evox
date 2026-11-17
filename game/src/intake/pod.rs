//! Terminal Reality POD archive reader and the path rules `tools/pod.py`
//! applies when filing extracted entries.
//!
//! Retail 4x4 Evo 2 ships two revisions: `POD\0` (v1) and `POD2` (v2). Both
//! use the same 20-byte TOC entry; only the header layout differs. Every field
//! is bounds-checked here, and decompression is capped so a malformed archive
//! cannot exhaust memory.

use std::io::Read;

const MAGIC_V1: &[u8; 4] = b"POD\0";
const MAGIC_V2: &[u8; 4] = b"POD2";
const ENTRY_SIZE: usize = 20;
const COMPRESSED_BIT: u32 = 0x8000_0000;
const SIZE_MASK: u32 = 0x7fff_ffff;
const MAX_SANE_FILES: u32 = 200_000;
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Entry {
    pub index: usize,
    pub name: String,
    raw_size: u32,
    offset: u32,
    pub timestamp: u64,
}

impl Entry {
    pub fn size(&self) -> usize {
        (self.raw_size & SIZE_MASK) as usize
    }

    pub fn compressed(&self) -> bool {
        self.raw_size & COMPRESSED_BIT != 0
    }
}

#[derive(Clone, Debug)]
pub struct Pod {
    pub version: u8,
    pub entries: Vec<Entry>,
    data: Vec<u8>,
}

fn u32_at(data: &[u8], off: usize) -> Result<u32, String> {
    data.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| format!("truncated header at 0x{off:x}"))
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

impl Pod {
    pub fn parse(data: Vec<u8>) -> Result<Pod, String> {
        if data.len() < 8 {
            return Err("file shorter than a header".into());
        }
        let (version, count_at, toc_at): (u8, usize, usize) = if data.starts_with(MAGIC_V2) {
            (2, 0x58, 0x60)
        } else if data.starts_with(MAGIC_V1) {
            (1, 0x04, 0x0c)
        } else {
            return Err(format!("bad magic {:?}", &data[..4]));
        };
        if version == 2 {
            let pad = u32_at(&data, 0x54)?;
            if pad > 1 {
                return Err(format!("unexpected v2 pad word 0x{pad:x}"));
            }
        }
        let count = u32_at(&data, count_at)?;
        if count > MAX_SANE_FILES {
            return Err(format!("implausible file count {count}"));
        }
        let toc_end = toc_at
            .checked_add(count as usize * ENTRY_SIZE)
            .ok_or("TOC size overflow")?;
        if toc_end > data.len() {
            return Err("TOC runs past end of file".into());
        }

        let mut raw = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let at = toc_at + i * ENTRY_SIZE;
            raw.push((
                u32_at(&data, at)?,
                u32_at(&data, at + 4)?,
                u32_at(&data, at + 8)?,
                u32_at(&data, at + 12)? as u64 | ((u32_at(&data, at + 16)? as u64) << 32),
            ));
        }

        let name_base = toc_end;
        let first_data = raw
            .iter()
            .filter(|(_, size, _, _)| size & SIZE_MASK != 0)
            .map(|(_, _, off, _)| *off as usize)
            .min()
            .unwrap_or(data.len());
        if first_data > data.len() {
            return Err("payload offset outside archive".into());
        }
        if name_base > first_data {
            return Err("name block overlaps first payload".into());
        }

        let mut entries = Vec::with_capacity(raw.len());
        for (i, (name_off, size, off, timestamp)) in raw.into_iter().enumerate() {
            let start = name_base
                .checked_add(name_off as usize)
                .ok_or_else(|| format!("entry {i}: name offset overflow"))?;
            if start >= first_data {
                return Err(format!("entry {i}: name offset outside name block"));
            }
            let end = data[start..first_data]
                .iter()
                .position(|&b| b == 0)
                .map(|p| start + p)
                .ok_or_else(|| format!("entry {i}: unterminated name"))?;
            let name = latin1(&data[start..end]);
            if size & SIZE_MASK != 0 {
                let len = (size & SIZE_MASK) as usize;
                if (off as usize) < first_data || off as usize >= data.len() {
                    return Err(format!(
                        "entry {i} ({name}): payload offset outside archive"
                    ));
                }
                if size & COMPRESSED_BIT == 0 && (off as usize).saturating_add(len) > data.len() {
                    return Err(format!("entry {i} ({name}): payload runs past end"));
                }
            }
            entries.push(Entry {
                index: i,
                name,
                raw_size: size,
                offset: off,
                timestamp,
            });
        }
        Ok(Pod {
            version,
            entries,
            data,
        })
    }

    /// Payload bytes. A compressed entry's stored size is the compressed
    /// length, so the decoder stops at stream end; output is capped.
    pub fn payload(&self, entry: &Entry) -> Result<Vec<u8>, String> {
        let start = entry.offset as usize;
        let end = start
            .checked_add(entry.size())
            .ok_or_else(|| format!("{}: payload size overflow", entry.name))?;
        let blob = self
            .data
            .get(start..end)
            .ok_or_else(|| format!("{}: payload outside archive", entry.name))?;
        if !entry.compressed() {
            return Ok(blob.to_vec());
        }
        let mut out = Vec::new();
        let mut dec = flate2::read::DeflateDecoder::new(blob).take(MAX_ENTRY_BYTES + 1);
        dec.read_to_end(&mut out)
            .map_err(|e| format!("{}: deflate failed: {e}", entry.name))?;
        if out.len() as u64 > MAX_ENTRY_BYTES {
            return Err(format!(
                "{}: decompressed payload is implausibly large",
                entry.name
            ));
        }
        Ok(out)
    }
}

/// `tools/pod.py::classify`: (class, group) from the archive stem and the
/// entry's internal path.
pub fn classify(pod_stem: &str, name: &str) -> (&'static str, &'static str) {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let (mut class, mut group) = match ext.as_str() {
        "smf" | "trk" => ("model", "vehicle"),
        "lte" | "trk2" | "rtd" | "rtl" | "sit" | "sdw" | "clr" | "lvl" => ("track", "track"),
        "raw" => ("texture", "texture"),
        "act" => ("texture", "palette"),
        "opa" => ("texture", "opacity"),
        "tga" | "bmp" | "pbm" | "png" | "jpg" | "dds" => ("texture", "texture"),
        "tif" => ("texture", "unknown"),
        "wav" | "sfx" => ("audio", "sfx"),
        "mp3" => ("audio", "music"),
        "bik" | "avi" => ("video", "video"),
        "rpl" => ("video", "replay"),
        "txt" => ("data", "data"),
        "ini" => ("data", "config"),
        "loc" | "vox" => ("data", "localization"),
        "rsp" => ("data", "script"),
        "lst" => ("data", "index"),
        "bin" => ("data", "binary"),
        _ => ("unknown", "unknown"),
    };
    let stem = pod_stem.to_ascii_lowercase();
    let up = name.to_ascii_uppercase();
    if matches!(class, "texture" | "model" | "data")
        && matches!(stem.as_str(), "ui" | "uitrack" | "startup" | "parts")
        && (up.starts_with("ART\\") || up.starts_with("UI") || !up.contains('/'))
    {
        group = "ui";
    }
    if up.starts_with("UIREPLAY") || up.starts_with("REPLAY") {
        group = "replay";
        if class == "unknown" {
            class = "video";
        }
    }
    if stem == "series" && class == "data" {
        group = "career";
    }
    if group == "unknown" {
        group = match stem.as_str() {
            "ui" | "uitrack" | "startup" | "parts" => "ui",
            "series" => "career",
            "ai" => "ai",
            "truck" => "vehicle",
            "life" => "props",
            "sound" => "sfx",
            "driver" => "vehicle",
            _ => "unknown",
        };
    }
    (class, group)
}

/// `tools/pod.py::sanitize`: strips traversal and unsafe characters while
/// keeping the archive's directory structure.
pub fn sanitize(name: &str) -> String {
    let mut parts = Vec::new();
    for chunk in name.replace('\\', "/").split('/') {
        let chunk = chunk.trim().replace("..", "");
        let chunk = chunk.trim_matches('.');
        if chunk.is_empty() {
            continue;
        }
        let safe: String = chunk
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        parts.push(safe);
    }
    if parts.is_empty() {
        "unnamed.bin".into()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::DeflateEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn make_pod(names: &[&str], payloads: &[&[u8]], v2: bool, compress: bool) -> Vec<u8> {
        let mut head = Vec::new();
        head.extend_from_slice(if v2 { MAGIC_V2 } else { MAGIC_V1 });
        if v2 {
            let mut comment = b"unit test".to_vec();
            comment.resize(80, 0);
            head.extend_from_slice(&comment);
            head.extend_from_slice(&0u32.to_le_bytes());
        }
        head.extend_from_slice(&(names.len() as u32).to_le_bytes());
        head.extend_from_slice(&(names.len() as u32).to_le_bytes());
        let toc_at = head.len();
        let name_base = toc_at + names.len() * ENTRY_SIZE;
        let mut names_block = Vec::new();
        let mut name_offsets = Vec::new();
        for n in names {
            name_offsets.push(names_block.len() as u32);
            names_block.extend_from_slice(n.as_bytes());
            names_block.push(0);
        }
        let data_at = name_base + names_block.len();

        let mut toc = Vec::new();
        let mut body = Vec::new();
        for (i, p) in payloads.iter().enumerate() {
            let stored = if compress {
                let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
                enc.write_all(p).unwrap();
                enc.finish().unwrap()
            } else {
                p.to_vec()
            };
            let raw = stored.len() as u32 | if compress { COMPRESSED_BIT } else { 0 };
            toc.extend_from_slice(&name_offsets[i].to_le_bytes());
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

    #[test]
    fn parses_both_revisions_and_deflate() {
        let names = ["ART\\TEX.ACT", "MODELS\\TRUCK.SMF", "SOUND\\HIT.WAV"];
        let payloads: [&[u8]; 3] = [&[7u8; 256], b"SMF-data", b"RIFF-wave"];
        for v2 in [true, false] {
            for compress in [false, true] {
                let bytes = make_pod(&names, &payloads, v2, compress);
                let pod = Pod::parse(bytes).unwrap();
                assert_eq!(pod.version, if v2 { 2 } else { 1 });
                assert_eq!(pod.entries.len(), 3);
                for (e, want) in pod.entries.iter().zip(payloads.iter()) {
                    assert_eq!(pod.payload(e).unwrap(), *want);
                }
            }
        }
    }

    #[test]
    fn classification_matches_intake_tree() {
        assert_eq!(classify("ui", "ART\\BG_MAIN.TIF"), ("texture", "ui"));
        assert_eq!(classify("truck", "ART\\JEEP.TIF"), ("texture", "vehicle"));
        assert_eq!(classify("truck", "ART\\TEX.ACT"), ("texture", "palette"));
        assert_eq!(classify("sound", "SOUND\\HIT.WAV"), ("audio", "sfx"));
        assert_eq!(classify("aspen", "LEVELS\\ASPEN.LVL"), ("track", "track"));
    }

    #[test]
    fn sanitize_strips_traversal() {
        assert_eq!(sanitize("..\\..\\x\\a b.raw"), "x/a_b.raw");
        assert_eq!(sanitize("../../etc/passwd"), "etc/passwd");
        assert_eq!(sanitize(""), "unnamed.bin");
    }

    #[test]
    fn rejects_bad_magic_and_truncation() {
        assert!(Pod::parse(b"JUNK".to_vec()).is_err());
        let mut bad = b"POD2".to_vec();
        bad.resize(40, 0);
        assert!(Pod::parse(bad).is_err());
    }
}
