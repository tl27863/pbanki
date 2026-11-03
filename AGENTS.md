# AGENTS.md

pbAnki is an Anki flashcard app for PocketBook e-readers. Rust frontends use Anki's `rslib` backend. The UI is Slint with the software renderer.

## Directory

The repository is a Cargo workspace with one `Cargo.lock` and one `target/`. The root `Cargo.toml` holds the shared dependency versions (anki, Slint, InkView, tokio) and the release profile. Members only add features.

- `crates/core/` — package `pbanki-core`, the shared library. All shared behavior goes here.
  - `build.rs` compiles `ui/main.slint`. `lib.rs` calls `slint::include_modules!()`.
  - `app.rs` — startup for both frontends. `app::start(config, native_keyboard, hooks)` returns an `App`. `App::run` starts the auto-sync and the event loop.
  - `session.rs` — `LearnSession`, `init_session`, `reopen_collection`, `reset_session_memory`.
  - `study.rs` — card loading and rating on the worker thread (`LoadedCard`, `next_card`, `rate_card`).
  - `card_render/` — card pages rendered with Blitz: `mod.rs` (`CardRenderer`, the render thread), `html.rs` (card document, type-in and audio tags), `paginate.rs` (page breaks from line boxes), `raster.rs` (card fonts, media, pixels). See `notes/blitz-card-render.md`.
  - `sync/` — `mod.rs` (sync, full sync, media sync), `client.rs` (`SyncManager`), `auth.rs` (login, endpoint), `backup.rs` (backup before a full download).
  - `frontend/` — Slint wiring: `mod.rs` (`FrontendHooks`, `init_ui`, `wire_callbacks`), `card_jobs.rs`, `card_pages.rs` (page store for `page-image`), `study.rs`, `settings.rs`, `sync.rs` (`SyncHooks`), `models.rs` (view models, `SyncState` constructors).
  - `config.rs`, `fonts.rs`, `logging.rs` (log subscriber), `profile.rs`.
  - `ui/` — Slint sources.
- `crates/pocketbook/` — package and binary `pbanki`, the PocketBook frontend over `inkview-rs`.
  - `src/main.rs` — platform setup, hooks, device log, panel and update-policy options.
  - `src/network.rs` — Wi-Fi. `src/keyboard.rs` — native keyboard wrapper.
  - Features `sdk-6-10` (B288/B300) and `sdk-6-11` (RK3566). Enable exactly one.
- `crates/desktop/` — package `pbanki-desktop`, the winit frontend for development. `src/eink_sim.rs` is the e-ink simulator (feature `eink-sim`).
- `bench/blitz-render/` — headless Blitz render benchmark, outside the workspace (see `notes/build.md`).
- `scripts/pocketbook/` — the release launcher `pbanki.app`, the bench launcher `blitz-bench.app`, and SSH/dropbear helper scripts for the device (see `SSH-SETUP.md`).
- `scripts/sdk-emulator/` — the Docker SDK emulator.
- `lint/`, `sgconfig.yml` — ast-grep rules.
- `notes/` — project notes (index below).

Both frontends start through `pbanki_core::app::start`. A frontend `main.rs` adds only platform parts. Keep UI callbacks non-blocking.

## Quick start

Desktop development run (paths are relative to the working directory):

```sh
cd crates/desktop && cargo run
cd crates/desktop && cargo run --features eink-sim   # simulated e-ink panel
```

Tests and lint:

```sh
cargo test -p pbanki-core
cargo test -p pbanki --features sdk-6-10
cargo test -p pbanki-desktop --features eink-sim
make lint
```

PocketBook builds (firmware 6.10+). Run `mise install` once first.

```sh
make release          # B288/B300, soft-float (PB710) -> target/pocketbook/
make release-rk3566   # RK3566, hard-float -> target/pocketbook-rk3566/
```

SDK emulator: run `make sdk-emulator-setup` once, then `make sdk-emulator`. Add `SDK=6.11` for the RK3566 SDK.

Dev sync server: `make -C ../pbanki-sync-server up`. The endpoint is `http://127.0.0.1:27701/`.

## Rules

- Do not access the user's PocketBook (for example `/run/media/uu02d/PB710` or `/mnt/ext1`) unless the user asks. The user deploys builds and shares the session logs from `pbanki/logs/`.
- Do not `cargo update` together with other changes.
- Read the related note before you change the e-ink backend, the card jobs, card rendering, fonts, or sync.
- You can use ast-grep for structural search and codemods (`ast-grep run -l rust -p '<pattern>'`). For Slint, add `-l slint --custom-languages allow`.
- If a change adds an invariant that clippy cannot check, add an ast-grep rule in `lint/rules/` with tests in `lint/rule-tests/`. Then run `make lint`. See `notes/lint.md`.

## Notes

| File | Topic |
|---|---|
| `notes/build.md` | Toolchain, PocketBook builds, release bundle, tests, clippy |
| `notes/emulators.md` | SDK emulator, eink-sim, waveform probe |
| `notes/inkview-backend.md` | The inkview-rs fork, rendering, system panel, device info |
| `notes/eink-refresh.md` | Update path, full presents, update policy, and why they are like this |
| `notes/logging.md` | Log output, levels, `PBANKI_LOG` filter |
| `notes/eink-logs.md` | `[eink …]` log lines and how to read them |
| `notes/performance.md` | Profiling, PB710 baselines, waveform probe results |
| `notes/inkview-single-thread-pump.md` | Fallback design for InkView update stalls (not implemented) |
| `notes/eink-presenter-refactor.md` | Blitz phase 1: the e-ink presenter moved out of inkview-slint into `inkview-eink` (done) |
| `notes/blitz-card-render.md` | Blitz card pages in the study session: render thread, pagination, colors, audio placeholders |
| `notes/card-loading.md` | Worker jobs, `busy`, loading screens, explicit redraws |
| `notes/sync.md` | Sync, Wi-Fi, sync hooks, dev sync server |
| `notes/fonts.md` | Card fonts, Slint fonts, glyph embedding |
| `notes/collection-and-config.md` | Collection setup, package import, config file |
| `notes/lint.md` | ast-grep rules and workflow |
| `notes/deep-research/` | Dated research reports |
