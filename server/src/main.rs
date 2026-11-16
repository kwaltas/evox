//! Master server: a tiny lobby registry.
//!
//! HTTP/1.1 with JSON, no framework, so it cross-compiles to a 1 MB static
//! binary and survives `scratch` containers. Lobbies expire unless refreshed.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PROTOCOL_VERSION: &str = "0.1.0";
const MAX_BODY: usize = 8 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lobby {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub players: u8,
    pub max_players: u8,
    pub track: String,
    pub class: u8,
    pub password: bool,
    pub version: String,
}

struct Entry {
    lobby: Lobby,
    seen: Instant,
    token: u64,
}

#[derive(Default)]
pub struct Registry {
    lobbies: HashMap<String, Entry>,
    next_token: u64,
    pub ttl: Duration,
}

impl Registry {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            next_token: 1,
            ..Default::default()
        }
    }

    pub fn register(&mut self, mut lobby: Lobby) -> Result<u64, &'static str> {
        if lobby.name.trim().is_empty() || lobby.name.len() > 48 {
            return Err("name must be 1..48 chars");
        }
        if lobby.port == 0 || lobby.max_players == 0 || lobby.max_players > 16 {
            return Err("bad port/max_players");
        }
        if lobby.players > lobby.max_players {
            return Err("players > max_players");
        }
        if lobby.host.parse::<std::net::IpAddr>().is_err() {
            return Err("host must be a literal IP");
        }
        lobby.version = PROTOCOL_VERSION.to_string();
        let key = format!("{}:{}", lobby.host, lobby.port);
        let token = self.next_token;
        self.next_token += 1;
        self.lobbies.insert(
            key,
            Entry {
                lobby,
                seen: Instant::now(),
                token,
            },
        );
        Ok(token)
    }

    pub fn unregister(&mut self, key: &str, token: u64) -> bool {
        match self.lobbies.get(key) {
            Some(e) if e.token == token => self.lobbies.remove(key).is_some(),
            _ => false,
        }
    }

    pub fn sweep(&mut self) -> usize {
        let now = Instant::now();
        let before = self.lobbies.len();
        self.lobbies
            .retain(|_, e| now.duration_since(e.seen) < self.ttl);
        before - self.lobbies.len()
    }

    pub fn list(&self) -> Vec<&Lobby> {
        let mut v: Vec<&Lobby> = self.lobbies.values().map(|e| &e.lobby).collect();
        v.sort_by(|a, b| b.players.cmp(&a.players).then(a.name.cmp(&b.name)));
        v
    }
}

fn respond(stream: &mut TcpStream, code: u16, body: &str) {
    let reason = match code {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

pub fn handle(mut stream: TcpStream, reg: Arc<Mutex<Registry>>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    // Read headers.
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > MAX_BODY {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let Some((head, rest)) = text.split_once("\r\n\r\n") else {
        respond(&mut stream, 400, r#"{"error":"malformed"}"#);
        return Ok(());
    };
    let mut parts = head.lines().next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/").to_string();

    if method == "GET" && path == "/health" {
        respond(
            &mut stream,
            200,
            &format!(r#"{{"ok":true,"version":"{VERSION}"}}"#),
        );
        return Ok(());
    }
    if method == "GET" && path == "/lobbies" {
        let reg = reg.lock().unwrap();
        let body = serde_json::to_string(&reg.list()).unwrap_or_else(|_| "[]".into());
        respond(&mut stream, 200, &body);
        return Ok(());
    }
    if method == "POST" && path == "/register" {
        match serde_json::from_str::<Lobby>(rest) {
            Ok(lobby) => {
                let mut reg = reg.lock().unwrap();
                match reg.register(lobby) {
                    Ok(token) => respond(&mut stream, 201, &format!(r#"{{"token":{token}}}"#)),
                    Err(e) => respond(&mut stream, 400, &format!(r#"{{"error":"{e}"}}"#)),
                }
            }
            Err(e) => respond(&mut stream, 400, &format!(r#"{{"error":"{e}"}}"#)),
        }
        return Ok(());
    }
    if method == "DELETE" && path.starts_with("/register/") {
        let rest = path.trim_start_matches("/register/");
        let mut it = rest.rsplitn(2, '/');
        let token: u64 = it.next().and_then(|t| t.parse().ok()).unwrap_or(0);
        let key = it.next().unwrap_or("").to_string();
        let mut reg = reg.lock().unwrap();
        if reg.unregister(&key, token) {
            respond(&mut stream, 200, r#"{"ok":true}"#);
        } else {
            respond(&mut stream, 404, r#"{"error":"unknown lobby"}"#);
        }
        return Ok(());
    }
    respond(&mut stream, 404, r#"{"error":"not found"}"#);
    Ok(())
}

fn main() {
    let mut bind = "0.0.0.0:8790".to_string();
    let mut ttl = 45u64;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--bind" => {
                bind = args.get(i + 1).cloned().unwrap_or(bind);
                i += 2;
            }
            "--ttl" => {
                ttl = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(ttl);
                i += 2;
            }
            "--help" => {
                println!("evox-master --bind ADDR --ttl SECONDS");
                return;
            }
            _ => i += 1,
        }
    }
    let listener = TcpListener::bind(&bind).expect("bind");
    println!("evox-master {VERSION} listening on {bind}, ttl {ttl}s");
    let reg = Arc::new(Mutex::new(Registry::new(Duration::from_secs(ttl))));
    {
        let reg = reg.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(10));
            let swept = reg.lock().unwrap().sweep();
            if swept > 0 {
                println!("swept {swept} expired lobbies");
            }
        });
    }
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let reg = reg.clone();
                std::thread::spawn(move || {
                    let _ = handle(s, reg);
                });
            }
            Err(e) => eprintln!("accept: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lobby(name: &str) -> Lobby {
        Lobby {
            name: name.into(),
            host: "127.0.0.1".into(),
            port: 7777,
            players: 1,
            max_players: 8,
            track: "ZONA".into(),
            class: 1,
            password: false,
            version: String::new(),
        }
    }

    #[test]
    fn register_ttl_and_unregister() {
        let mut r = Registry::new(Duration::from_millis(1));
        let token = r.register(lobby("a")).unwrap();
        assert_eq!(r.list().len(), 1);
        assert_eq!(r.list()[0].version, PROTOCOL_VERSION);
        assert!(!r.unregister("127.0.0.1:7777", token + 1));
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(r.sweep(), 1);
        assert!(r.list().is_empty());
    }

    #[test]
    fn rejects_bad_lobbies() {
        let mut r = Registry::new(Duration::from_secs(10));
        let mut l = lobby("");
        assert!(r.register(l.clone()).is_err());
        l = lobby("ok");
        l.port = 0;
        assert!(r.register(l.clone()).is_err());
        l = lobby("ok");
        l.host = "not-an-ip".into();
        assert!(r.register(l).is_err());
    }

    #[test]
    fn duplicate_host_replaces() {
        let mut r = Registry::new(Duration::from_secs(10));
        r.register(lobby("one")).unwrap();
        r.register(lobby("two")).unwrap();
        assert_eq!(r.list().len(), 1);
        assert_eq!(r.list()[0].name, "two");
    }
}
