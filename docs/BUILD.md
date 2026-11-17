# Build

## Requirements

- Rust stable 1.78+ (CI pins `stable`; MSRV is whatever `macroquad` needs)
- Python 3.9+ and Pillow, for asset intake only
- Platform toolchain: MSVC on Windows, Xcode CLT on macOS, `libx11`/`libxi`/`libgl` on Linux

## Local build

```bash
git clone <repo> evox && cd evox
cargo build --release -p evox
python3 tools/setup_evo2.py --source /path/to/evo2/pods --out assets/extracted
./target/release/evox
```

The binary resolves assets relative to the working directory (`assets/...`) and
stores the profile in `$EVOX_DATA_DIR` or `~/.evox`.

On Linux the client needs a working GL/Vulkan driver; the server crates are
pure std and run headless anywhere.

## Test gate

```bash
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo run -p evox -- --self-test 0
```

`--self-test 0` exits non-zero on any failure; CI uses it as the merge gate
once the fixture assets are present. Without assets it still checks physics and
packaging and prints `[--]` for skipped groups.

## Release artefacts

`release.yml` produces:

- `evox-windows-x86_64.zip` (MSVC, Authenticode step optional)
- `evox-macos-universal.zip` (x86_64 + aarch64, ad-hoc signed then notarised)
- `evox-linux-x86_64.AppImage`

Artefacts contain no game data. Ship `assets/pods_evo2.md5` alongside so the
intake tool can verify the user's disc.

## Docker

```bash
docker compose -f deploy/docker-compose.yml up -d
```

Builds `evox-master` and `evox-leaderboard` on a scratch base and fronts them
behind Caddy for TLS. See `docs/MULTIPLAYER.md` for ports.

## Cross-compilation notes

`macroquad` links platform backends; cross-building from macOS to Windows
needs `cargo-xwin` and to Linux needs a cross linker plus `libx11-dev` for the
target. The server crates cross-compile cleanly with `--target` alone.
