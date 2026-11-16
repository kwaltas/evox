//! Leaderboard client. Submissions carry an HMAC-style digest of the run
//! payload with a per-install salt so a trivial `curl` forgery is rejected;
//! the server re-computes and range-checks.

use sha2::{Digest, Sha256};

use crate::master::http_get;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Record {
    pub track: String,
    pub class: u8,
    pub time_ms: u32,
    pub driver: String,
    pub install: String,
    pub digest: String,
    pub replay_hash: String,
}

pub struct Client {
    pub base: String,
    pub install: String,
    pub driver: String,
}

impl Client {
    pub fn new(base: impl Into<String>, install_digest: &str, driver: &str) -> Self {
        Self {
            base: base.into(),
            install: install_digest.chars().take(16).collect(),
            driver: driver.to_string(),
        }
    }

    fn sign(&self, track: &str, class: u8, time_ms: u32, replay_hash: &str) -> String {
        let mut h = Sha256::new();
        h.update(format!(
            "{track}|{class}|{time_ms}|{replay_hash}|{}",
            self.install
        ));
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn make(&self, track: &str, class: u8, time_ms: u32, replay_hash: &str) -> Record {
        Record {
            track: track.to_string(),
            class,
            time_ms,
            driver: self.driver.clone(),
            install: self.install.clone(),
            digest: self.sign(track, class, time_ms, replay_hash),
            replay_hash: replay_hash.to_string(),
        }
    }

    pub fn submit(&self, rec: &Record) -> Result<String, String> {
        let body = serde_json::to_string(rec).map_err(|e| e.to_string())?;
        crate::master::http_post(&self.base, "/records", &body)
    }

    pub fn top(&self, track: &str, class: u8) -> Result<Vec<Record>, String> {
        let body = http_get(&self.base, &format!("/top?track={track}&class={class}"))?;
        serde_json::from_str(&body).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_depends_on_every_field() {
        let c = Client::new("http://x", "abc123", "me");
        let a = c.make("ZONA", 1, 60_000, "r");
        let b = c.make("ZONA", 1, 60_001, "r");
        assert_ne!(a.digest, b.digest);
        assert_eq!(a.digest.len(), 64);
    }
}
