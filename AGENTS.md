# AGENTS.md

Anki flashcard app for PocketBook e-readers: Rust frontends over Anki's `rslib` backend, Slint software-rendered UI.

## Layout

Three **separate, standalone Cargo packages** (there is no workspace and no root `[workspace]`). Each has its own `Cargo.lock`:
- `common/` — shared logic: `api.rs` (session/scheduling), `config.rs`, `sync.rs`, `utils.rs` (pagination). Also owns the Slint build.
- `dev/` — desktop build for development. Entrypoint `dev/src/main.rs`, binary name `dev`.
- `src/` — PocketBook build via `inkview-rs`. Entrypoint `src/main.rs`, binary name `pbanki`.
- `ui/` — Slint sources, NOT a crate. Compiled by `common/build.rs` from `../ui/main.slint`; `common/src/lib.rs` does `slint::include_modules!()`.

Both frontends call the same `common` API (`init_session`, `update_deck_tree`, `next_card`, `rate_card`). Port behavior changes to both `src/main.rs` and `dev/src/main.rs`.

## Commands

- Desktop dev run: `cd dev && cargo run` (creates/reads collection at `./pbanki/collection/`, config at `./pbanki/config.toml`).
- PocketBook build: `make` (dev) / `make release`; runs `cargo zigbuild --target armv7-unknown-linux-gnueabi.2.23 -p pbanki --features=sdk-6-8`. Requires `cargo-zigbuild` + `zig` (provided by `shell.nix`).
- Emulator (PB700, external QEMU firmware emulator): `make emulator-dev` is the one-command interactive flow (setup + build + visible window + wizard + Wi-Fi + upload + launch pbAnki). Individual steps: `make emulator-setup`, `make emulator-run`, `make emulator-build`, `make emulator-sync`. See `scripts/emulator/` and README. First setup downloads ~453 MiB firmware and builds patched QEMU; `make emulator-toolchain` installs Zig/cargo-zigbuild/Rust 1.89 target under `~/.local`/`~/.cargo`.
- `PBANKI_CONFIG` overrides the config path in `src/main.rs` (used for host-side testing); default stays `/mnt/ext1/applications/pbanki/config.toml`.
- Toolchain is pinned to `1.89.0` in `rust-toolchain.toml` intentionally — newer versions may fail clippy. Do not bump casually.
- No test suite, lint config, or CI exists in this repo.

## Gotchas

- `anki`/`anki_proto` come from `ankitects/anki` `branch = "main"` (git dep, unpinned), while `slint` is pinned to rev `44208ff`. The `third_party/anki` submodule is not the dep source; deps resolve via Cargo git.
- Slint must use the software renderer features listed in each `Cargo.toml`; `common/build.rs` embeds resources `EmbedForSoftwareRenderer`. Keep these in sync across the three manifests when touching Slint.
- `Config::save()` (config.rs) hardcodes the device path `/mnt/ext1/applications/pbanki/config.toml`, ignoring the load path. The desktop path still works only because it's read via `load_or_create`.
- Sync is partial: `sync_media` and `full_sync` in `common/src/sync.rs` return "not implemented" errors. TODO items live in README.
- `common/src/api.rs` suspends "Image Occlusion Enhanced" cards at session init.
- `mount.sh` mounts the device over CIFS to `./target/.../debug/applications/`.
- Emulator checkout is external (`EINK_EMULATOR_DIR`, default `../eink-emulator`) and not vendored. `scripts/emulator/setup.sh` fixes the upstream QEMU submodule's SSH URL to HTTPS. `enable-wifi.py` uses the quick-settings Wi-Fi toggle and configures `PocketBook-QEMU` through `wpa_cli` over the serial console; do not navigate Settings > Wi-Fi (firmware versions put Wi-Fi on different rows). Sync also uses the serial console, not SSH. `sync-app.sh` uploads to a `.new` file and renames it so a running pbAnki keeps working.
