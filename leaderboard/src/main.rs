//! Leaderboard service with signed submissions.
//!
//! `POST /records` verifies `sha256(track|class|time|replay_hash|install)`
//! against the client digest, enforces monotonic sanity (time inside a broad
//! per-track window), and keeps each install's best per track/class.
//! Persistence is a JSON file rewritten atomically every few seconds.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_BODY: usize = 4 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub track: String,
    pub class: u8,
    pub time_ms: u32,
    pub driver: String,
    pub install: String,
    pub digest: String,
    pub replay_hash: String,
}

fn digest(r: &Record) -> String {
    let mut h = Sha256::new();
    h.update(format!(
        "{}|{}|{}|{}|{}",
        r.track, r.class, r.time_ms, r.replay_hash, r.install
    ));
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Default)]
pub struct Board {
    best: HashMap<(String, u8, String), Record>,
}

impl Board {
    pub fn submit(&mut self, rec: Record) -> Result<&'static str, &'static str> {
        if rec.digest != digest(&rec) {
            return Err("bad digest");
        }
        if rec.track.is_empty()
            || rec.track.len() > 24
            || !rec
                .track
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err("bad track");
        }
        if rec.class == 0 || rec.class > 9 {
            return Err("bad class");
        }
        if rec.time_ms < 20_000 || rec.time_ms > 3_600_000 {
            return Err("time out of range");
        }
        let key = (rec.track.clone(), rec.class, rec.install.clone());
        let improved = self
            .best
            .get(&key)
            .map(|old| rec.time_ms < old.time_ms)
            .unwrap_or(true);
        if !improved {
            return Ok("slower than personal best");
        }
        self.best.insert(key, rec);
        Ok("recorded")
    }

    pub fn top(&self, track: &str, class: u8, limit: usize) -> Vec<Record> {
        let mut v: Vec<Record> = self
            .best
            .values()
            .filter(|r| r.track == track && r.class == class)
            .cloned()
            .collect();
        v.sort_by_key(|r| r.time_ms);
        v.truncate(limit.clamp(1, 50));
        v
    }
}

fn respond(stream: &mut TcpStream, code: u16, body: &str) {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    let _ = write!(stream, "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
}

fn query(path: &str) -> HashMap<String, String> {
    path.split_once('?')
        .map(|(_, q)| {
            q.split('&')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (k.to_string(), url_decode(v)))
                .collect()
        })
        .unwrap_or_default()
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn handle(mut s: TcpStream, board: Arc<Mutex<Board>>) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = s.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > MAX_BODY {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        respond(&mut s, 400, r#"{"error":"malformed"}"#);
        return Ok(());
    };
    let mut it = head.lines().next().unwrap_or("").split_whitespace();
    let method = it.next().unwrap_or("");
    let path = it.next().unwrap_or("/");

    if method == "GET" && path.starts_with("/top") {
        let q = query(path);
        let track = q.get("track").cloned().unwrap_or_default();
        let class: u8 = q.get("class").and_then(|v| v.parse().ok()).unwrap_or(1);
        let body = serde_json::to_string(&board.lock().unwrap().top(&track, class, 25))
            .unwrap_or_else(|_| "[]".into());
        respond(&mut s, 200, &body);
        return Ok(());
    }
    if method == "POST" && path.starts_with("/records") {
        match serde_json::from_str::<Record>(body) {
            Ok(rec) => {
                let mut b = board.lock().unwrap();
                match b.submit(rec) {
                    Ok(msg) => respond(&mut s, 200, &format!(r#"{{"ok":"{msg}"}}"#)),
                    Err(e) => respond(&mut s, 400, &format!(r#"{{"error":"{e}"}}"#)),
                }
            }
            Err(e) => respond(&mut s, 400, &format!(r#"{{"error":"{e}"}}"#)),
        }
        return Ok(());
    }
    respond(&mut s, 404, r#"{"error":"not found"}"#);
    Ok(())
}

fn main() {
    let mut bind = "0.0.0.0:8791".to_string();
    let mut file = "leaderboard.json".to_string();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--bind" => {
                bind = args.get(i + 1).cloned().unwrap_or(bind);
                i += 2;
            }
            "--file" => {
                file = args.get(i + 1).cloned().unwrap_or(file);
                i += 2;
            }
            _ => i += 1,
        }
    }
    let board = Arc::new(Mutex::new(Board::default()));
    if let Ok(text) = std::fs::read_to_string(&file) {
        if let Ok(recs) = serde_json::from_str::<Vec<Record>>(&text) {
            let mut b = board.lock().unwrap();
            for r in recs {
                let _ = b.submit(r);
            }
            println!("loaded {} records", b.best.len());
        }
    }
    {
        let board = board.clone();
        let file = file.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            let body =
                serde_json::to_string(&board.lock().unwrap().best.values().collect::<Vec<_>>())
                    .unwrap_or_default();
            let tmp = format!("{file}.tmp");
            if std::fs::write(&tmp, body).is_ok() {
                let _ = std::fs::rename(&tmp, &file);
            }
        });
    }
    let listener = TcpListener::bind(&bind).expect("bind");
    println!("evox-leaderboard listening on {bind}");
    for s in listener.incoming().flatten() {
        let board = board.clone();
        std::thread::spawn(move || {
            let _ = handle(s, board);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(time: u32) -> Record {
        let mut r = Record {
            track: "ZONA".into(),
            class: 1,
            time_ms: time,
            driver: "d".into(),
            install: "abc".into(),
            digest: String::new(),
            replay_hash: "rr".into(),
        };
        r.digest = digest(&r);
        r
    }

    #[test]
    fn accepts_best_only() {
        let mut b = Board::default();
        assert!(b.submit(rec(90_000)).is_ok());
        assert_eq!(b.submit(rec(95_000)).unwrap(), "slower than personal best");
        assert!(b.submit(rec(80_000)).is_ok());
        assert_eq!(b.top("ZONA", 1, 5).len(), 1);
        assert_eq!(b.top("ZONA", 1, 5)[0].time_ms, 80_000);
    }

    #[test]
    fn rejects_forgery_and_ranges() {
        let mut b = Board::default();
        let mut r = rec(90_000);
        r.time_ms = 40_000;
        assert_eq!(b.submit(r).unwrap_err(), "bad digest");
        assert!(b.submit(rec(1_000)).is_err());
        let mut bad = rec(90_000);
        bad.track = "../x".into();
        assert!(b.submit(bad).is_err());
    }

    #[test]
    fn decode_query() {
        let q = query("/top?track=ZONA&class=2");
        assert_eq!(q.get("class").unwrap(), "2");
        assert_eq!(url_decode("a%20b+c"), "a b c");
    }
}
