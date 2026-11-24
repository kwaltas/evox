//! Read-only InstallShield 5 cabinet (`data1.hdr` + `data1.cab`) support.
//!
//! The retail 4x4 Evolution 2 disc ships its `.POD` archives inside one of
//! these cabinets instead of loose on the disc. Layout notes (file table,
//! file descriptor offsets, the 16-bit chunk framing and the obfuscation byte
//! transform) follow the public format documentation in the MIT-licensed
//! `unshield` project; this is an independent Rust implementation.
//!
//! Only single-volume cabinets are handled, which is all the retail disc uses.

use std::io::{Read, Seek, SeekFrom};

use flate2::{Decompress, FlushDecompress, Status};

const SIGNATURE: u32 = 0x2863_5349; // "ISc("
const FILE_OBFUSCATED: u16 = 0x02;
const FILE_COMPRESSED: u16 = 0x04;
const FILE_INVALID: u16 = 0x08;
/// A compressed chunk never inflates beyond this (the writer uses 32 KiB).
const CHUNK_OUT: usize = 1 << 17;

/// One file stored in the cabinet.
#[derive(Clone, Debug)]
pub struct Entry {
    /// File name without its directory (`TRUCK.POD`).
    pub name: String,
    pub expanded: u64,
    pub compressed: u64,
    pub offset: u64,
    pub flags: u16,
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, String> {
    d.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| "InstallShield header is truncated".to_string())
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, String> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| "InstallShield header is truncated".to_string())
}

fn cstr_at(d: &[u8], at: usize) -> Result<String, String> {
    let tail = d
        .get(at..)
        .ok_or_else(|| "InstallShield file name is out of bounds".to_string())?;
    let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
}

/// Parse `data1.hdr` into the list of stored files.
pub fn parse_header(h: &[u8]) -> Result<Vec<Entry>, String> {
    if u32_at(h, 0)? != SIGNATURE {
        return Err("not an InstallShield cabinet header".into());
    }
    let version = u32_at(h, 4)?;
    let major = if version >> 24 == 1 {
        (version >> 12) & 0xf
    } else {
        return Err(format!(
            "unsupported InstallShield version {version:#x} (only version 5 is supported)"
        ));
    };
    if major != 0 && major != 5 {
        return Err(format!(
            "unsupported InstallShield major version {major} (only 5 is supported)"
        ));
    }
    let desc = u32_at(h, 12)? as usize;
    let table_off = u32_at(h, desc + 0x0c)? as usize;
    let dir_count = u32_at(h, desc + 0x1c)? as usize;
    let file_count = u32_at(h, desc + 0x28)? as usize;
    if file_count == 0 || file_count > 100_000 || dir_count > 100_000 {
        return Err("InstallShield header has an implausible file count".into());
    }
    let base = desc + table_off;

    let mut out = Vec::with_capacity(file_count);
    for i in 0..file_count {
        let slot = base + 4 * (dir_count + i);
        let rel = u32_at(h, slot)? as usize;
        let p = base + rel;
        let name_off = u32_at(h, p)? as usize;
        let flags = u16_at(h, p + 8)?;
        let expanded = u32_at(h, p + 0x0a)? as u64;
        let compressed = u32_at(h, p + 0x0e)? as u64;
        let offset = u32_at(h, p + 0x26)? as u64;
        let name = cstr_at(h, base + name_off)?;
        out.push(Entry {
            name,
            expanded,
            compressed,
            offset,
            flags,
        });
    }
    Ok(out)
}

/// The cabinet's byte-wise obfuscation (rotate/xor/subtract with a running seed).
fn deobfuscate(buf: &mut [u8], seed: &mut u32) {
    for b in buf.iter_mut() {
        *b = (*b ^ 0xd5)
            .rotate_right(2)
            .wrapping_sub((*seed % 0x47) as u8);
        *seed = seed.wrapping_add(1);
    }
}

/// Read `n` bytes at the current position, deobfuscating when required.
fn read_exact<R: Read>(r: &mut R, n: usize, obf: bool, seed: &mut u32) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)
        .map_err(|e| format!("reading the cabinet failed: {e}"))?;
    if obf {
        deobfuscate(&mut buf, seed);
    }
    Ok(buf)
}

/// Extract one file fully into memory.
pub fn extract<R: Read + Seek>(cab: &mut R, e: &Entry) -> Result<Vec<u8>, String> {
    if e.flags & FILE_INVALID != 0 || e.offset == 0 {
        return Err(format!("{}: invalid cabinet entry", e.name));
    }
    cab.seek(SeekFrom::Start(e.offset))
        .map_err(|err| format!("{}: seek failed: {err}", e.name))?;
    let obf = e.flags & FILE_OBFUSCATED != 0;
    let mut seed = 0u32;
    let mut out = Vec::with_capacity(e.expanded as usize);

    if e.flags & FILE_COMPRESSED == 0 {
        let mut left = e.expanded as usize;
        while left > 0 {
            let n = left.min(1 << 20);
            out.extend(read_exact(cab, n, obf, &mut seed)?);
            left -= n;
        }
    } else {
        let mut left = e.compressed as i64;
        let mut chunk = vec![0u8; CHUNK_OUT];
        while left > 0 {
            let len = read_exact(cab, 2, obf, &mut seed)?;
            let len = u16::from_le_bytes([len[0], len[1]]) as usize;
            if len == 0 {
                return Err(format!("{}: zero-length compressed chunk", e.name));
            }
            let mut input = read_exact(cab, len, obf, &mut seed)?;
            // The writer's streams end on a sync marker; one extra zero byte
            // lets the inflater finish cleanly.
            input.push(0);
            let mut inflater = Decompress::new(false);
            let before = inflater.total_out();
            let status = inflater
                .decompress(&input, &mut chunk, FlushDecompress::Finish)
                .map_err(|err| format!("{}: inflate failed: {err}", e.name))?;
            if status != Status::StreamEnd {
                return Err(format!("{}: compressed chunk did not terminate", e.name));
            }
            let produced = (inflater.total_out() - before) as usize;
            out.extend_from_slice(&chunk[..produced]);
            left -= 2 + len as i64;
        }
    }
    if out.len() as u64 != e.expanded {
        return Err(format!(
            "{}: expanded to {} bytes, expected {}",
            e.name,
            out.len(),
            e.expanded
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set `EVOX_TEST_ISO` to a retail 2048-byte ISO to exercise the whole
    /// cabinet path: every POD must inflate and match the retail MD5 manifest.
    #[test]
    fn retail_disc_cabinet_matches_manifest() {
        let Ok(iso) = std::env::var("EVOX_TEST_ISO") else {
            eprintln!("skip: EVOX_TEST_ISO not set");
            return;
        };
        let plan = crate::intake::extract::prepare(&crate::intake::Source::Iso(iso.into()))
            .expect("prepare");
        assert!(plan.archive_count() > 40, "got {}", plan.archive_count());
        assert!(
            plan.required_missing.is_empty(),
            "{:?}",
            plan.required_missing
        );
        assert!(plan.mismatches.is_empty(), "{:?}", plan.mismatches);
    }

    #[test]
    fn rejects_foreign_headers() {
        assert!(parse_header(b"MSCF\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0").is_err());
        assert!(parse_header(&[]).is_err());
    }

    #[test]
    fn deobfuscation_is_stateful() {
        let mut a = [0u8; 4];
        let mut s = 0;
        deobfuscate(&mut a[..2], &mut s);
        deobfuscate(&mut a[2..], &mut s);
        let mut b = [0u8; 4];
        let mut s2 = 0;
        deobfuscate(&mut b, &mut s2);
        assert_eq!(a, b);
        assert_eq!(s, 4);
    }
}
