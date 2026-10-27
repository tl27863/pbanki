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
- Emulator (PB700, external QEMU firmware emulator): `make emulator-dev` is the one-command interactive flow (setup + build + visible window + wizard + Wi-Fi + upload + deck + launch pbAnki). Individual steps: `make emulator-setup`, `make emulator-run`, `make emulator-build`, `make emulator-sync`, `make emulator-deck`. See `scripts/emulator/` and README. First setup downloads ~453 MiB firmware and builds patched QEMU; `make emulator-toolchain` installs Zig/cargo-zigbuild/Rust 1.89 target under `~/.local`/`~/.cargo`.
- `PBANKI_CONFIG` overrides the config path in `src/main.rs` (used for host-side testing); default stays `/mnt/ext1/applications/pbanki/config.toml`.
- Toolchain is pinned to `1.89.0` in `rust-toolchain.toml` intentionally — newer versions may fail clippy. Do not bump casually.
- If `make` cannot find `zig`, add `~/.local/opt/zig-x86_64-linux-0.14.1` and `~/.local/bin` to `PATH` (see `scripts/emulator/install-toolchain.sh`).
- `make release` stages a PocketBook bundle in `target/pocketbook/`: `pbanki.app` is a shell launcher and `pbanki.bin` is the ARM binary. The launcher sets `GLIBC_TUNABLES=glibc.rtld.execstack=2` for firmware 6.11.1480+ (same workaround as KOReader) and logs to `pbanki.log`. Copy both files to `/mnt/ext1/applications/`; do not copy the raw binary as `pbanki.app`.
- No test suite, lint config, or CI exists in this repo.

## Dev sync server

- External Docker Compose project at `../pbanki-sync-server` (not vendored): `zorrn/anki-sync-server:25.09.2` pinned by digest, data in `./data`, dev credentials `pbanki`/`devpass` in its `.env`.
- Run from here: `make -C ../pbanki-sync-server up`; stop with `down`, follow logs with `logs`, verify with `smoke`.
- Endpoint: desktop/host `http://127.0.0.1:27701/`; PB700 emulator `http://10.0.2.2:27701/`. Set `SYNC_BIND_ADDR=0.0.0.0` in the server `.env` for LAN/device access.
- The client supports `[ankiweb].endpoint` and full upload/download, so a fresh server can be bootstrapped from the UI's full-sync prompt. The server starts empty; choose Upload on the first sync.

## Gotchas

- `anki`/`anki_proto` come from `ankitects/anki` `branch = "main"` (git dep, unpinned), while `slint` is pinned to rev `44208ff`. The `third_party/anki` submodule is not the dep source; deps resolve via Cargo git.
- Slint must use the software renderer features listed in each `Cargo.toml`; `common/build.rs` embeds resources `EmbedForSoftwareRenderer`. Keep these in sync across the three manifests when touching Slint.
- `Config` stores its loaded path in the serde-skipped `path` field; `save()` writes there. `[ankiweb].endpoint` is blank for AnkiWeb, and `sync_media` defaults to true.
- Sync now supports normal merge, full upload/download, and media sync in `common/src/sync.rs`. Full sync temporarily takes the collection out of `LearnSession.collection` (an `Option`) and reopens it with `Collection::as_builder()`. Sync is blocking on the UI thread; progress/abort is not implemented.
- `sync_on_exit` means after leaving a study session (LearnScreen home), not on application exit.
- Sync settings live in `ui/screens/settings.slint`; the home gear opens it and the reload icon triggers sync. Shared callback wiring and sync orchestration live in `common/src/frontend.rs`; PocketBook adds the native InkView keyboard (`src/keyboard.rs`) and desktop uses normal `LineEdit` input.
- `init_session` calls `with_desktop_media_paths()` so `collection.media/` and `collection.mdb` exist. Do not remove this or media sync will fail.
- On startup, `init_session` imports any unimported `*.apkg` in the collection directory (including scheduling/media) and renames it to `*.apkg.imported`. This is how users install decks on-device; see `import_pending_packages()` in `common/src/api.rs`.
- `common/src/api.rs` suspends "Image Occlusion Enhanced" cards at session init.
- `mount.sh` mounts the device over CIFS to `./target/.../debug/applications/`.
- Emulator checkout is external (`EINK_EMULATOR_DIR`, default `../eink-emulator`) and not vendored. `scripts/emulator/setup.sh` fixes the upstream QEMU submodule's SSH URL to HTTPS. `enable-wifi.py` uses the quick-settings Wi-Fi toggle and configures `PocketBook-QEMU` through `wpa_cli` over the serial console; do not navigate Settings > Wi-Fi (firmware versions put Wi-Fi on different rows). Sync also uses the serial console, not SSH. `sync-app.sh` uploads to a `.new` file and renames it so a running pbAnki keeps working. `install-deck.sh` replaces the emulator collection with `scripts/emulator/JLPT_N2_Grammar.apkg` unless the `.pbanki-deck` marker exists; it must run while pbAnki is stopped.
