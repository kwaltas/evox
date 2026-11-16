//! UDP multiplayer: snapshots, interpolation, dead reckoning, validation.
//!
//! Server authoritative-ish: clients send inputs at 30 Hz, the host simulates
//! and broadcasts 20 Hz snapshots. Remote cars render from a 120 ms
//! interpolation buffer; when the buffer starves, dead reckoning extends the
//! last known velocity for at most 250 ms. Each packet is bounds-checked before
//! allocation and validated against physical limits.

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use macroquad::prelude::{Quat, Vec3};

pub const MAGIC: [u8; 4] = *b"EVX1";
pub const PROTOCOL: u16 = 1;
pub const MAX_PACKET: usize = 1400;
pub const MAX_PEERS: usize = 16;
pub const TICK_HZ: f32 = 60.0;
pub const SNAPSHOT_HZ: f32 = 20.0;
pub const INTERP_MS: f32 = 120.0;

#[derive(Clone, Debug)]
pub struct CarState {
    pub id: u8,
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
    pub suspension: [f32; 4],
    pub rpm: f32,
    pub gear: u8,
    pub parts_mask: u64,
}

#[derive(Clone, Debug)]
pub enum Packet {
    Hello {
        name: String,
        protocol: u16,
    },
    Welcome {
        id: u8,
        tick: u32,
    },
    Input {
        seq: u32,
        throttle: f32,
        brake: f32,
        steer: f32,
        buttons: u8,
    },
    Snapshot {
        seq: u32,
        tick: u32,
        cars: Vec<CarState>,
    },
    Chat {
        text: String,
    },
    Bye,
    Ping {
        nonce: u32,
    },
    Pong {
        nonce: u32,
    },
}

fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_vec3(out: &mut Vec<u8>, v: Vec3) {
    put_f32(out, v.x);
    put_f32(out, v.y);
    put_f32(out, v.z);
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn u8(&mut self) -> Option<u8> {
        let v = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let s = self.buf.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.buf.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_le_bytes(s.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        let s = self.buf.get(self.pos..self.pos + 8)?;
        self.pos += 8;
        Some(u64::from_le_bytes(s.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_bits(self.u32()?))
    }
    fn vec3(&mut self) -> Option<Vec3> {
        Some(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
    fn quat(&mut self) -> Option<Quat> {
        Some(Quat::from_xyzw(
            self.f32()?,
            self.f32()?,
            self.f32()?,
            self.f32()?,
        ))
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.buf.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }
}

pub fn encode(p: &Packet) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&PROTOCOL.to_le_bytes());
    match p {
        Packet::Hello { name, protocol } => {
            out.push(1);
            out.extend_from_slice(&protocol.to_le_bytes());
            let b = name.as_bytes();
            out.push(b.len().min(24) as u8);
            out.extend_from_slice(&b[..b.len().min(24)]);
        }
        Packet::Welcome { id, tick } => {
            out.push(2);
            out.push(*id);
            out.extend_from_slice(&tick.to_le_bytes());
        }
        Packet::Input {
            seq,
            throttle,
            brake,
            steer,
            buttons,
        } => {
            out.push(3);
            out.extend_from_slice(&seq.to_le_bytes());
            put_f32(&mut out, *throttle);
            put_f32(&mut out, *brake);
            put_f32(&mut out, *steer);
            out.push(*buttons);
        }
        Packet::Snapshot { seq, tick, cars } => {
            out.push(4);
            out.extend_from_slice(&seq.to_le_bytes());
            out.extend_from_slice(&tick.to_le_bytes());
            out.push(cars.len().min(MAX_PEERS) as u8);
            for c in cars.iter().take(MAX_PEERS) {
                out.push(c.id);
                put_vec3(&mut out, c.pos);
                out.extend_from_slice(
                    &[
                        c.rot.x.to_le_bytes(),
                        c.rot.y.to_le_bytes(),
                        c.rot.z.to_le_bytes(),
                        c.rot.w.to_le_bytes(),
                    ]
                    .concat(),
                );
                put_vec3(&mut out, c.vel);
                for s in c.suspension {
                    put_f32(&mut out, s);
                }
                put_f32(&mut out, c.rpm);
                out.push(c.gear);
                out.extend_from_slice(&c.parts_mask.to_le_bytes());
            }
        }
        Packet::Chat { text } => {
            out.push(5);
            let b = text.as_bytes();
            out.push(b.len().min(160) as u8);
            out.extend_from_slice(&b[..b.len().min(160)]);
        }
        Packet::Bye => out.push(6),
        Packet::Ping { nonce } => {
            out.push(7);
            out.extend_from_slice(&nonce.to_le_bytes());
        }
        Packet::Pong { nonce } => {
            out.push(8);
            out.extend_from_slice(&nonce.to_le_bytes());
        }
    }
    out
}

pub fn decode(buf: &[u8]) -> Option<Packet> {
    if buf.len() < 7 || buf[0..4] != MAGIC {
        return None;
    }
    if u16::from_le_bytes([buf[4], buf[5]]) != PROTOCOL {
        return None;
    }
    let mut r = Reader::new(&buf[6..]);
    match r.u8()? {
        1 => {
            let protocol = r.u16()?;
            let n = r.u8()? as usize;
            let name = String::from_utf8_lossy(r.bytes(n)?).to_string();
            Some(Packet::Hello { name, protocol })
        }
        2 => Some(Packet::Welcome {
            id: r.u8()?,
            tick: r.u32()?,
        }),
        3 => Some(Packet::Input {
            seq: r.u32()?,
            throttle: r.f32()?,
            brake: r.f32()?,
            steer: r.f32()?,
            buttons: r.u8()?,
        }),
        4 => {
            let seq = r.u32()?;
            let tick = r.u32()?;
            let n = (r.u8()? as usize).min(MAX_PEERS);
            let mut cars = Vec::with_capacity(n);
            for _ in 0..n {
                let id = r.u8()?;
                let pos = r.vec3()?;
                let rot = r.quat()?;
                let vel = r.vec3()?;
                let mut suspension = [0.0f32; 4];
                for s in &mut suspension {
                    *s = r.f32()?;
                }
                let rpm = r.f32()?;
                let gear = r.u8()?;
                let parts_mask = r.u64()?;
                cars.push(CarState {
                    id,
                    pos,
                    rot,
                    vel,
                    suspension,
                    rpm,
                    gear,
                    parts_mask,
                });
            }
            Some(Packet::Snapshot { seq, tick, cars })
        }
        5 => {
            let n = r.u8()? as usize;
            Some(Packet::Chat {
                text: String::from_utf8_lossy(r.bytes(n)?).to_string(),
            })
        }
        6 => Some(Packet::Bye),
        7 => Some(Packet::Ping { nonce: r.u32()? }),
        8 => Some(Packet::Pong { nonce: r.u32()? }),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SnapshotEntry {
    pub at: Instant,
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
}

/// Per-remote-car jitter buffer.
#[derive(Clone, Debug, Default)]
pub struct RemoteCar {
    pub id: u8,
    pub buffer: Vec<SnapshotEntry>,
    pub last_rx: Option<Instant>,
}

impl RemoteCar {
    pub fn push(&mut self, s: SnapshotEntry) {
        self.last_rx = Some(Instant::now());
        self.buffer.push(s);
        self.buffer.sort_by_key(|e| e.at);
        while self.buffer.len() > 24 {
            self.buffer.remove(0);
        }
    }

    pub fn sample(&self, now: Instant) -> Option<(Vec3, Quat)> {
        let target = now - Duration::from_millis(INTERP_MS as u64);
        if self.buffer.is_empty() {
            return None;
        }
        let first = self.buffer.first()?;
        let last = self.buffer.last()?;
        if target <= first.at {
            return Some((first.pos, first.rot));
        }
        if target >= last.at {
            // Dead reckoning for at most 250 ms.
            let age = now.saturating_duration_since(last.at).as_secs_f32();
            let t = age.min(0.25);
            return Some((last.pos + last.vel * t, last.rot));
        }
        for pair in self.buffer.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if target >= a.at && target <= b.at {
                let span = b.at.saturating_duration_since(a.at).as_secs_f32().max(1e-5);
                let t = target.saturating_duration_since(a.at).as_secs_f32() / span;
                return Some((a.pos.lerp(b.pos, t), a.rot.slerp(b.rot, t)));
            }
        }
        Some((last.pos, last.rot))
    }
}

#[derive(Clone, Debug)]
pub struct Peer {
    pub addr: SocketAddr,
    pub id: u8,
    pub name: String,
    pub last_seen: Instant,
    pub last_seq: u32,
    pub strikes: u8,
    pub car: RemoteCar,
}

#[derive(Default)]
pub struct Lobby {
    pub peers: HashMap<SocketAddr, Peer>,
    pub next_id: u8,
    pub password: Option<String>,
}

impl Lobby {
    pub fn join(&mut self, addr: SocketAddr, name: &str) -> Option<u8> {
        if self.peers.len() >= MAX_PEERS {
            return None;
        }
        if let Some(p) = self.peers.get(&addr) {
            return Some(p.id);
        }
        let id = self.next_id.max(1);
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.peers.insert(
            addr,
            Peer {
                addr,
                id,
                name: name.chars().take(24).collect(),
                last_seen: Instant::now(),
                last_seq: 0,
                strikes: 0,
                car: RemoteCar {
                    id,
                    ..Default::default()
                },
            },
        );
        Some(id)
    }

    pub fn cull(&mut self, timeout: Duration) -> Vec<SocketAddr> {
        let now = Instant::now();
        let dead: Vec<SocketAddr> = self
            .peers
            .iter()
            .filter(|(_, p)| now.duration_since(p.last_seen) > timeout)
            .map(|(a, _)| *a)
            .collect();
        for a in &dead {
            self.peers.remove(a);
        }
        dead
    }
}

/// Reject physically impossible snapshots. Returns a reason on suspicion; a
/// peer is kicked after `MAX_STRIKES`.
pub const MAX_STRIKES: u8 = 6;

pub fn validate(prev: Option<&CarState>, next: &CarState, dt: f32) -> Result<(), &'static str> {
    if !next.pos.is_finite() || !next.vel.is_finite() || !next.rpm.is_finite() {
        return Err("non-finite state");
    }
    if next.pos.length() > 50_000.0 {
        return Err("position out of world");
    }
    if next.vel.length() > 130.0 {
        return Err("impossible speed");
    }
    if let Some(p) = prev {
        let d = (next.pos - p.pos).length();
        if d > 130.0 * dt.max(0.005) + 5.0 {
            return Err("position jump");
        }
    }
    Ok(())
}

pub struct Client {
    pub socket: UdpSocket,
    pub server: SocketAddr,
    pub id: u8,
    pub seq: u32,
    pub cars: HashMap<u8, RemoteCar>,
    pub ping_ms: f32,
    pub connected: bool,
}

impl Client {
    pub fn connect(server: SocketAddr, name: &str) -> std::io::Result<Self> {
        let bind: SocketAddr = if server.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        }
        .parse()
        .unwrap();
        let socket = UdpSocket::bind(bind)?;
        socket.set_nonblocking(true)?;
        socket.connect(server)?;
        let hello = encode(&Packet::Hello {
            name: name.to_string(),
            protocol: PROTOCOL,
        });
        socket.send(&hello)?;
        Ok(Self {
            socket,
            server,
            id: 0,
            seq: 0,
            cars: HashMap::new(),
            ping_ms: 0.0,
            connected: false,
        })
    }

    pub fn send_input(&mut self, throttle: f32, brake: f32, steer: f32, buttons: u8) {
        self.seq = self.seq.wrapping_add(1);
        let p = Packet::Input {
            seq: self.seq,
            throttle,
            brake,
            steer,
            buttons,
        };
        let _ = self.socket.send(&encode(&p));
    }

    pub fn poll(&mut self) -> Vec<(SocketAddr, Packet)> {
        let mut out = Vec::new();
        let mut buf = [0u8; MAX_PACKET];
        while let Ok((n, from)) = self.socket.recv_from(&mut buf) {
            let Some(p) = decode(&buf[..n]) else { continue };
            match &p {
                Packet::Welcome { id, .. } => {
                    self.id = *id;
                    self.connected = true;
                }
                Packet::Snapshot { cars, .. } => {
                    let now = Instant::now();
                    for c in cars {
                        if c.id == self.id {
                            continue;
                        }
                        self.cars
                            .entry(c.id)
                            .or_insert_with(|| RemoteCar {
                                id: c.id,
                                ..Default::default()
                            })
                            .push(SnapshotEntry {
                                at: now,
                                pos: c.pos,
                                rot: c.rot,
                                vel: c.vel,
                            });
                    }
                }
                Packet::Pong { .. } => {}
                _ => {}
            }
            out.push((from, p));
        }
        out
    }

    pub fn interpolated(&self, id: u8, now: Instant) -> Option<(Vec3, Quat)> {
        self.cars.get(&id)?.sample(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec3;

    fn car(id: u8) -> CarState {
        CarState {
            id,
            pos: vec3(1.0, 2.0, 3.0),
            rot: Quat::IDENTITY,
            vel: vec3(0.0, 0.0, -10.0),
            suspension: [0.1, 0.2, 0.3, 0.4],
            rpm: 3200.0,
            gear: 4,
            parts_mask: 0xdead_beef,
        }
    }

    #[test]
    fn snapshot_roundtrip() {
        let p = Packet::Snapshot {
            seq: 7,
            tick: 99,
            cars: vec![car(3)],
        };
        let b = encode(&p);
        assert!(b.len() <= MAX_PACKET);
        let Some(Packet::Snapshot { cars, seq, tick }) = decode(&b) else {
            panic!()
        };
        assert_eq!((seq, tick), (7, 99));
        assert_eq!(cars[0].parts_mask, 0xdead_beef);
        assert_eq!(cars[0].suspension[3], 0.4);
    }

    #[test]
    fn rejects_garbage_and_wrong_version() {
        assert!(decode(b"nope").is_none());
        let mut b = encode(&Packet::Bye);
        b[4] = 9;
        assert!(decode(&b).is_none());
    }

    #[test]
    fn hello_and_chat_bounds() {
        let long = "x".repeat(500);
        let b = encode(&Packet::Chat { text: long.clone() });
        let Some(Packet::Chat { text }) = decode(&b) else {
            panic!()
        };
        assert_eq!(text.len(), 160);
        let b = encode(&Packet::Hello {
            name: long,
            protocol: PROTOCOL,
        });
        assert!(decode(&b).is_some());
    }

    #[test]
    fn interpolation_clamps_and_dead_reckons() {
        let mut c = RemoteCar::default();
        let t0 = Instant::now();
        c.push(SnapshotEntry {
            at: t0,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            vel: vec3(0.0, 0.0, -10.0),
        });
        c.push(SnapshotEntry {
            at: t0 + Duration::from_millis(200),
            pos: vec3(0.0, 0.0, -2.0),
            rot: Quat::IDENTITY,
            vel: vec3(0.0, 0.0, -10.0),
        });
        let (mid, _) = c.sample(t0 + Duration::from_millis(220)).unwrap();
        assert!(mid.z < 0.0 && mid.z > -2.0);
        let (late, _) = c.sample(t0 + Duration::from_millis(900)).unwrap();
        assert!(
            late.z < -2.0 && late.z > -6.0,
            "dead reckoning capped: {late:?}"
        );
    }

    #[test]
    fn validation_catches_teleports() {
        let a = car(1);
        let mut b = car(1);
        b.pos = vec3(500.0, 0.0, 0.0);
        assert!(validate(Some(&a), &b, 0.05).is_err());
        b.pos = vec3(1.0, 2.0, 3.5);
        assert!(validate(Some(&a), &b, 0.05).is_ok());
    }

    #[test]
    fn lobby_caps_players() {
        let mut l = Lobby::default();
        for i in 0..(MAX_PEERS + 4) {
            let addr: SocketAddr = format!("127.0.0.1:{}", 40000 + i).parse().unwrap();
            l.join(addr, "p");
        }
        assert_eq!(l.peers.len(), MAX_PEERS);
    }
}
