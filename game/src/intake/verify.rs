//! MD5 verification against the checked-in retail manifest.

use std::collections::HashMap;

use md5::{Digest, Md5};

const MANIFEST: &str = include_str!("../../../assets/pods_evo2.md5");

pub fn manifest() -> HashMap<String, String> {
    MANIFEST
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?;
            Some((name.to_ascii_lowercase(), hash.to_ascii_lowercase()))
        })
        .collect()
}

pub fn md5_hex(bytes: &[u8]) -> String {
    let digest = Md5::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_populated() {
        let m = manifest();
        assert_eq!(m.len(), 52);
        assert!(m.contains_key("ui.pod"));
    }

    #[test]
    fn md5_matches_known_vector() {
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }
}
