//! Master-server client and server browser model.
//!
//! The master speaks line HTTP/1.1 on purpose: it is trivial to host behind
//! any reverse proxy, and the response is JSON. Pings are ICMP-free UDP
//! probes handled by the game's own net stack; the browser shows them sorted.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LobbyEntry {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub players: u8,
    pub max_players: u8,
    pub track: String,
    pub class: u8,
    pub password: bool,
    pub version: String,
    #[serde(skip)]
    pub ping_ms: Option<u32>,
}

impl LobbyEntry {
    pub fn addr(&self) -> Option<SocketAddr> {
        format!("{}:{}", self.host, self.port).parse().ok()
    }
}

pub struct Browser {
    pub url: String,
    pub entries: Vec<LobbyEntry>,
    pub selected: usize,
    pub status: String,
    pub last_fetch: Option<Instant>,
}

impl Browser {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            entries: Vec::new(),
            selected: 0,
            status: "Not refreshed".into(),
            last_fetch: None,
        }
    }

    /// Blocking fetch on a worker thread result; call from a background task.
    pub fn fetch(&mut self) {
        self.status = "Querying master server...".into();
        match http_get(&self.url, "/lobbies") {
            Ok(body) => match serde_json::from_str::<Vec<LobbyEntry>>(&body) {
                Ok(mut list) => {
                    list.retain(|l| l.version == crate::VERSION);
                    list.sort_by(|a, b| b.players.cmp(&a.players));
                    self.entries = list;
                    self.selected = 0;
                    self.status = format!("{} lobbies", self.entries.len());
                    self.last_fetch = Some(Instant::now());
                }
                Err(e) => self.status = format!("Bad master response: {e}"),
            },
            Err(e) => self.status = e,
        }
    }

    pub fn selected_lobby(&self) -> Option<&LobbyEntry> {
        self.entries.get(self.selected)
    }

    pub fn move_selection(&mut self, d: i32) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = (self.selected as i32 + d).clamp(0, self.entries.len() as i32 - 1) as usize;
    }

    pub fn should_refresh(&self) -> bool {
        self.last_fetch
            .map(|t| t.elapsed() > Duration::from_secs(15))
            .unwrap_or(true)
    }
}

/// Parse `host:port[:path]` (http assumed).
fn split_url(url: &str) -> (String, String) {
    let stripped = url.trim_start_matches("http://").trim_end_matches('/');
    match stripped.split_once('/') {
        Some((host, _rest)) => (host.to_string(), _rest.to_string()),
        None => (stripped.to_string(), String::new()),
    }
}

pub fn http_get(base: &str, path: &str) -> Result<String, String> {
    use std::io::{Read, Write};
    let (host, prefix) = split_url(base);
    let mut stream = std::net::TcpStream::connect_timeout(
        &host
            .parse()
            .map_err(|_| format!("bad master address {host}"))?,
        Duration::from_secs(4),
    )
    .map_err(|e| format!("master unreachable: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let path = format!("/{prefix}{path}").replace("//", "/");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    stream
        .take(512 * 1024)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf).to_string();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or("malformed HTTP response")?;
    let code = head.split_whitespace().nth(1).unwrap_or("000");
    if !code.starts_with('2') {
        return Err(format!("master returned {code}"));
    }
    Ok(body.to_string())
}

pub fn http_post(base: &str, path: &str, body: &str) -> Result<String, String> {
    use std::io::{Read, Write};
    let (host, prefix) = split_url(base);
    let mut stream = std::net::TcpStream::connect_timeout(
        &host.parse().map_err(|_| format!("bad address {host}"))?,
        Duration::from_secs(4),
    )
    .map_err(|e| format!("unreachable: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(6))).ok();
    let path = format!("/{prefix}{path}").replace("//", "/");
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    stream
        .take(256 * 1024)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf).to_string();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or("malformed HTTP response")?;
    if !head
        .split_whitespace()
        .nth(1)
        .unwrap_or("000")
        .starts_with('2')
    {
        return Err(format!("server rejected: {body}"));
    }
    Ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_split() {
        assert_eq!(
            split_url("http://127.0.0.1:8080"),
            ("127.0.0.1:8080".into(), "".into())
        );
        assert_eq!(split_url("http://x/y"), ("x".into(), "y".into()));
    }

    #[test]
    fn selection_is_clamped() {
        let mut b = Browser::new("http://127.0.0.1:1");
        b.move_selection(3);
        b.entries = vec![LobbyEntry {
            name: "a".into(),
            host: "127.0.0.1".into(),
            port: 1,
            players: 1,
            max_players: 8,
            track: "ZONA".into(),
            class: 1,
            password: false,
            version: crate::VERSION.into(),
            ping_ms: None,
        }];
        b.move_selection(5);
        assert_eq!(b.selected, 0);
        assert_eq!(b.selected_lobby().unwrap().addr().unwrap().port(), 1);
    }
}
