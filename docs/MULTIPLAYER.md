# Multiplayer

## Ports

| Service | Protocol | Default | Exposure |
|---|---|---|---|
| Game host | UDP | 7777 | public, forwarded |
| Master registry | TCP/HTTP | 8790 | public, TLS via proxy |
| Leaderboard | TCP/HTTP | 8791 | public, TLS via proxy |

Forward **UDP 7777** only to the player hosting. The master and leaderboard can
live on a small VPS; neither sees gameplay traffic.

### Router checklist

1. Reserve a static LAN address for the host machine.
2. Forward UDP 7777 to that address.
3. Allow the game binary through the OS firewall (UDP inbound).
4. If CGNAT is in play, use a tunnel (WireGuard to the VPS) or IPv6.

## Flow

```
client            master              host
  │  GET /lobbies     │                  │
  │──────────────────►│                  │
  │  [{host,port}]    │                  │
  │◄──────────────────│                  │
  │  UDP Hello ──────────────────────────►
  │◄───────────────────────── Welcome{id} │
  │  UDP Input(30Hz) ───────────────────► │
  │◄────────────── Snapshot(20Hz) ────────│
```

The host runs the authoritative simulation. Clients render remote cars 120 ms
behind real time from a jitter buffer; starving buffers extrapolate for 250 ms.

## Registration

The game (or a wrapper script) announces lobbies:

```bash
curl -X POST http://master:8790/register \
  -H 'content-type: application/json' \
  -d '{"name":"Friday trucks","host":"203.0.113.9","port":7777,
       "players":1,"max_players":8,"track":"ZONA","class":2,"password":false}'
```

Refresh every ~20 s; entries expire after 45 s (configurable `--ttl`). Keep the
returned token to `DELETE /register/<host>:<port>/<token>` on shutdown.

## Validation and anti-cheat

`net::validate` rejects non-finite state, speeds above 130 m/s, and position
jumps beyond `130 m/s × dt + 5 m`. Six strikes disconnect a peer. The snapshot
also carries a parts bitmask so the host can refuse illegal loadouts.

Server-side re-simulation (Phase 9B) is not yet enabled: the host trusts client
inputs, which is adequate for friendly races but not for prize events.

## Chat

`Packet::Chat` is length-prefixed to 160 bytes, UTF-8 lossy. No markup, no
history — it is a race lobby, not IRC.

## Troubleshooting

- **No lobbies in browser**: master URL is compiled as `127.0.0.1:8790`; change
  `MASTER_URL` in `game/src/app.rs` for a hosted instance.
- **Connected but no cars**: snapshots are dropped when the parts masks or
  protocol version differ; update both peers.
- **One-way audio of state**: the host must have UDP 7777 reachable; a client
  that can send but not receive is usually a firewall on the host side.
