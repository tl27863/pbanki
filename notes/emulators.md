# Emulators and probes

## SDK emulator

The SDK emulator replaces the old QEMU PB700 emulator. It runs the x86-64 InkView of the SDK (a GTK2 window) in Docker.

Use it:

1. Run `make sdk-emulator-setup` once. Add `SDK=6.11` for the 6.11 SDK.
2. Run `make sdk-emulator` (build and run). Use the same `SDK=` value.

Setup:

- The scripts are in `scripts/sdk-emulator/`. The Docker images are `pbanki-sdk-emulator:<sdk>`. 6.10 uses Debian bookworm and 6.11 uses Debian trixie. These match the ICU, jsoncpp, and glibc versions of the SDK libraries.
- pbAnki is built for `x86_64-unknown-linux-gnu.2.34` (6.10) or `.2.38` (6.11).
- The SDK archives come from the Sean-on-Git re-upload. `scripts/sdk-emulator/SHA256SUMS` pins them. The setup extracts only `local/lib` and `local/share/config` to `target/sdk/<sdk>/runtime/`.
- The `system/` data tree comes from the git tree of the same repository (commit `715cda6`).

Defaults:

| SDK | hwconfig profile (`PROFILE`) | Display mode (`RESMODE`) |
|---|---|---|
| 6.10 | `700` | 16 (1264×1680 at 300 dpi) |
| 6.11 | `1030` (RK3566) | 12 (1404×1872) |

Other variables:

- `EMU_NET=host` uses the host network. The dev sync server binds 127.0.0.1, so the default bridge network cannot reach it.
- `EMU_APP=<x86-64 binary>` runs another InkView binary instead of pbAnki.
- `PROFILE_CARGO=release` builds the release profile. The default is the dev profile.

How it works:

- The PC InkView reads everything relative to `./system`. The working directory is `target/sdk-emulator/<sdk>/work`.
- `system/mnt/ext1` is also mounted as `/mnt/ext1`, so device paths work without changes.
- The Display Mode sets the window size, not the hwconfig profile. `run.sh` writes `RESMODE` to `emu_resmode` in `system/config/global.cfg`.
  - Values follow the `ACDisplayMode…` enum order: 2 = 758×1024, 9 = 1448×1072, 12 = 1404×1872, 16 = 1264×1680, 17 = 1264×1680 shown at 1/2.
- `PROFILE` is copied to `system/config/hwconfig`. It sets the device ID, platform, and EPDC.
- The emulator always reports 300 dpi. Thus the 1030 profile (227 dpi) logs the DPI-mismatch warning.

What works (checked on 6.10 and 6.11, Nov 2026): deck import, deck entry, answer, rating, Home, Settings, the native keyboard (`OpenKeyboardEx`), and sync login to the dev server with `EMU_NET=host`. `QueryNetwork` reports a connection, so no network override is necessary.

What the emulator does not do:

- E-ink timing. `IsUpdateInProcess` never reports busy. Thus the presenter waits and the dynamic path do not run. Use eink-sim or the device.
- The system tray, the scanner, and other tasks.
- The release launcher and ARM code generation (see `build.md`).

Input automation:

- GTK ignores synthetic X events. Use XTEST: `xdotool mousemove --window W x y; xdotool click 1`.
- In the compressed view, window coordinates are half of the framebuffer coordinates.

To test the status bar setting off, add `show_status_bar=0` to `target/sdk-emulator/<sdk>/work/system/config/global.cfg` (see `inkview-backend.md`).

The bundled deck `scripts/sdk-emulator/JLPT_N2_Grammar.apkg` is local only (`.git/info/exclude`).

## eink-sim

- Run: `cd crates/desktop && cargo run --features eink-sim`. `PBANKI_EINK_SIM_MS` sets the timings.
- `SimScreen` (`crates/desktop/src/eink_sim.rs`) implements `inkview_slint::EinkScreen`. It runs the same presenter and damage policy as the device backend.
- It has a BB8 framebuffer and a queue of pending updates. It applies each update to the visible frame after a delay for its kind. It quantizes to 16, 4, or 2 grays. Full updates show black for the first half.
- The eink-sim frontend sets `FrontendHooks.full_present`, as the PocketBook frontend does. The winit desktop frontend does not set it.
- minifb reads input once per frame. Thus scripted clicks must hold the button: `xdotool mousedown 1; sleep 0.08; xdotool mouseup 1`.

## Waveform probe

- `make waveform-probe` builds the inkview-rs `waveform_probe` example from `../inkview-rs` for the base ABI. The output is `target/pocketbook-probe/waveform-probe.app`.
- On the device, it writes `applications/waveform-probe.log`.
- In the emulator: `EMU_APP=<x86-64 build> make sdk-emulator`.
- The probe runs itself again with `GLIBC_TUNABLES=glibc.rtld.execstack=2`, so it runs as a bare `.app` on 6.11.1480+. It writes panics to its log file.
- For the PB710 results, see `performance.md`.
