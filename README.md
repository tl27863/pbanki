# pbAnki

Anki flashcard app for PocketBook e-readers.

## What is this?

Port of Anki to PocketBook devices using:
- Anki's official rslib backend for full collection compatibility
- Slint UI framework with software rendering for e-ink displays
- inkview-rs bindings for PocketBook SDK integration

## Building

### Desktop (for development)

```bash
cd crates/desktop
cargo run                      # normal desktop window
cargo run --features eink-sim  # simulated e-ink panel (see below)
```

Place your Anki collection at `./pbanki/collection/`

### PocketBook

pbAnki needs PocketBook firmware 6.10 or later. There are two builds:

| Devices | Build | Bundle |
|---|---|---|
| B288/B300 (soft-float): PB700, PB710, 743G, 634, 629, ... | SDK 6.10 bindings | `target/pocketbook/` |
| RK3566 (hard-float): 1030 | SDK 6.11 bindings | `target/pocketbook-rk3566/` |

```bash
make                         # B288/B300 debug build
make release                 # B288/B300 release bundle
make release-profile         # same, with [profile] timing logs
make rk3566                  # RK3566 debug build
make release-rk3566          # RK3566 release bundle
make release-rk3566-profile  # same, with [profile] timing logs
```

Requires rustup and [mise](https://mise.jdx.dev/): run `mise install` once in
the repository to get Zig 0.14.1 and `cargo-zigbuild` 0.24.0 (`mise.toml`). The
Makefile finds them without `mise activate`. rustup installs Rust 1.99.1 with
both armv7 targets from `rust-toolchain.toml` on first use. The InkView dependencies come from the `inkview-rs` fork and are
fetched by Cargo automatically.

## Emulator (PocketBook SDK PC emulator)

The PocketBook SDK includes an x86-64 desktop build of InkView that opens a
window and emulates a device. pbAnki runs on it unchanged (native keyboard,
dialogs, sync, optional status bar), built for x86-64. The emulator runs in a
Docker container whose Debian release matches the SDK libraries.

```bash
make sdk-emulator-setup           # one-time: SDK runtime, image, first build
make sdk-emulator                 # build and run (SDK 6.10, PB700 profile)
make sdk-emulator-setup SDK=6.11  # RK3566 SDK
make sdk-emulator SDK=6.11        # build and run (SDK 6.11, 1030 profile)
```

Host requirements: Docker, X11 or Xwayland (`DISPLAY`), `7z`, `curl`, `git`, and
the toolchain above. The setup downloads the SDK archive once
(about 650 MB for 6.10, 770 MB for 6.11) from the
[Sean-on-Git/PocketBook-SDK](https://github.com/Sean-on-Git/PocketBook-SDK)
re-upload, verifies it against `scripts/sdk-emulator/SHA256SUMS`, and extracts
only the PC runtime libraries and device profiles.

- Device data lives in `target/sdk-emulator/<sdk>/work/system/`. Its
  `mnt/ext1` is mounted as `/mnt/ext1`, so the config and collection use the
  device paths: `.../system/mnt/ext1/applications/pbanki/`.
- On first run, `scripts/sdk-emulator/JLPT_N2_Grammar.apkg` (or
  `PBANKI_DECK_APKG`) is copied into the collection and imported, if present.
- Right-click the window for the emulator menu (device, display mode, events).
- Overrides: `PROFILE=<hwconfig profile>` (e.g. `700`, `743g`, `1030`),
  `RESMODE=<display mode>` (16 = 1264x1680, 12 = 1404x1872), `EMU_NET=host`
  (share the host network, so the dev sync server is at
  `http://127.0.0.1:27701/`), `EMU_APP=<binary>` (run another x86-64 InkView
  app, e.g. the inkview-rs `waveform_probe` example).

The emulator has no e-ink timing (the panel never reports busy), no system tray
app, and its DPI is always 300. Use the device for timing and the tray.

### Simulated e-ink (desktop)

`cargo run --features eink-sim` in `crates/desktop` runs pbAnki on the same
e-ink backend as the device, over a simulated 1264x1680 panel. Updates appear
only after a panel would finish them (partial 490 ms, full 600 ms with a flash,
fast 265 ms), reduced to the waveform's gray levels, and the panel reports busy
in the meantime. Change the timings with
`PBANKI_EINK_SIM_MS="full=600,partial=490,fast=265"`. Click to tap; arrows,
Enter, Backspace, Page Up/Down are the device keys.

### Waveform probe

`make waveform-probe` builds the inkview-rs `waveform_probe` example for the
device (`target/pocketbook-probe/waveform-probe.app`). Copy it to
`applications/` and start it once. It measures how long each InkView update
keeps the panel busy and writes `applications/waveform-probe.log`. It needs the
`../inkview-rs` checkout.

## Installation

Build the bundle for your device (see the table above): `make release`
(B288/B300) or `make release-rk3566` (RK3566). Each creates:

```text
target/pocketbook[-rk3566]/pbanki.app   # shell launcher
target/pocketbook[-rk3566]/pbanki.bin   # ARM executable
```

1. Copy both files to `/mnt/ext1/applications/` keeping their names
2. Make both executable: `chmod +x /mnt/ext1/applications/pbanki.app /mnt/ext1/applications/pbanki.bin`
3. Copy your Anki collection to: `/mnt/ext1/applications/pbanki/collection/`
4. (Optional) Create configuration file at: `/mnt/ext1/applications/pbanki/config.toml`
5. Launch from Applications menu

If pbAnki is already running, close it from the task manager before replacing
either file. `/mnt/ext1` is FAT, and overwriting a running executable can
terminate it with `SIGBUS`. Launching pbAnki while it is already open just
brings the existing window to the front.

`pbanki.app` is a shell launcher, matching KOReader's PocketBook packaging. It
sets `GLIBC_TUNABLES=glibc.rtld.execstack=2`, required by firmware 6.11.1480+
when loading InkView libraries, and writes stdout/stderr to
`/mnt/ext1/applications/pbanki/logs/pbanki.log`. It creates the `pbanki/logs/`
folder if it does not exist. The launcher keeps a single instance via
`/tmp/pbanki.pid`, rotates the log at launch when it exceeds 1 MiB, and records
the start and exit status. If the binary cannot start (for example, the
RK3566 bundle on a B288 device), the log says which bundle the device needs. If
the application directory is not writable, it falls back to `/tmp/pbanki.log`.
It changes into its own directory (normally `/mnt/ext1/applications`) before
starting the binary, so relative `collection_path` values resolve there.

### Importing decks

Place an `.apkg` file in `/mnt/ext1/applications/pbanki/collection/`, then close
and relaunch pbAnki. Launching it while it is already open only brings the
existing window forward. On startup, any unimported `*.apkg` in that folder is
imported with its scheduling and media, then renamed to `*.apkg.imported` so it
is not imported again. No need to extract the package.

## Configuration

pbAnki uses a TOML configuration file. On first launch, a default config will be created at `/mnt/ext1/applications/pbanki/config.toml` (or `./pbanki/config.toml` for desktop builds). The file is read at startup; close and relaunch pbAnki after editing it.

### Configuration Options

```toml
[general]
# Language code for Anki i18n (e.g., "en", "de", "fr", "ja", "es", "pt", "ru", "zh", "ko")
language = "en-GB"

# Collection path (relative to app directory or absolute)
collection_path = "/mnt/ext1/applications/pbanki/collection"

# Font family for card text. PocketBook ships Ryo Gothic PlusN in the Adobe
# fonts folder (/mnt/ext1/system/adobe/fonts on-device, /system/adobe/fonts over USB).
card_font = "Ryo Gothic PlusN"

# Fallback families used when the card contains characters outside card_font.
card_fallback_fonts = ["Adobe Ming Std", "Adobe Heiti Std", "Noto Sans CJK JP"]

# Directories scanned for the fonts above. USB view /system/... maps to
# /mnt/ext1/system/... on-device.
font_dirs = [
    "/mnt/ext1/system/adobe/fonts",
    "/mnt/ext1/system/fonts",
    "/system/adobe/fonts",
    "/adobefonts",
    "/fonts",
    "/ebrmain/fonts",
]

# Show the PocketBook status bar (clock, battery, Wi-Fi) above pbAnki; tapping
# it opens the system tray. Restart pbAnki after changing this.
system_panel = true

[ankiweb]
# AnkiWeb synchronization settings
# Leave empty to disable sync

# AnkiWeb username (email)
username = ""

# Password (plain text - will be encrypted in future versions)
# WARNING: Do not share this file if password is filled
password = ""

# Session token (populated after successful login)
# This avoids storing password long-term
token = ""

# Optional self-hosted sync server URL. Leave empty to use AnkiWeb.
# Example: endpoint = "http://127.0.0.1:27701/"
endpoint = ""

# Sync automatically on app start
auto_sync = false

# Sync automatically after a study session ends
sync_on_exit = false

# Synchronize media files (audio/images) as well as the collection
sync_media = true
```

### Fonts

Card text uses `card_font`, then the families in `card_fallback_fonts` in order.
pbAnki checks each card against the fonts' character maps and changes the card
font when needed. If no configured font covers a character, it is displayed as
`?` instead of letting the renderer stall. The UI itself stays in the system
`Monospace` font.

Font files are discovered by scanning `font_dirs` at startup. The defaults cover
the PB710 runtime paths, the USB mass-storage view, and older PB700 images.
Existing configs are migrated automatically: missing `card_font`,
`card_fallback_fonts`, `font_dirs`, or `system_panel` keys are added on the next
launch without overwriting values you already set.

### Status bar

`system_panel = true` (the default) shows the PocketBook status bar (clock,
battery, Wi-Fi) at the top of the screen, with pbAnki below it. Tapping the bar
opens the system tray, as in the built-in apps. `false` gives pbAnki the whole
screen. Restart pbAnki after changing it. A config written by an earlier
version already contains `system_panel = false`; change it to `true` there.

At runtime, the configured primary and fallback files are linked into
`/tmp/pbanki-fonts`, and Slint registers that directory. This happens for both
PocketBook and desktop builds, so custom `font_dirs` also make fonts available to
Slint during desktop testing. This directory is recreated on every launch and is
not visible over USB; change the config values instead of editing it.

## Syncing

Open the gear button on the home screen to configure sync. The settings page
supports AnkiWeb or a self-hosted sync server, credentials, automatic sync,
sync-after-study, and media sync.

- The reload button on the home screen and **Sync now** in settings both start a
  sync immediately.
- If the server and local collection cannot be merged, the settings page shows
  Upload/Download options. Both directions require an explicit confirmation.
- `sync_on_exit` means after leaving a study session, not when the app exits.
- Media sync defaults to on and stores media in `collection.media`.
- Sync runs synchronously; the UI is blocked until the current sync finishes.

For local development, the companion server project at `../pbanki-sync-server`
uses `http://127.0.0.1:27701/` (desktop, and the SDK emulator with
`EMU_NET=host`) with the dev credentials from that project's `.env`.

### Supported Languages

pbAnki uses Anki's built-in translations, supporting 70+ languages including:
- English (`en`, `en-GB`)
- European: `es`, `fr`, `de`, `it`, `pt-BR`, `pt-PT`, `nl`, `sv-SE`, `da`, `fi`, `cs`, `hu`, `el`, `pl`, `tr`, `ru`, `uk`
- Asian: `ja`, `ko`, `zh-CN`, `zh-TW`, `th`, `vi`, `id`, `hi-IN`
- Middle Eastern: `ar`, `fa`, `he`
- And many more...

## Project Structure

```
Cargo.toml          # Cargo workspace: shared dependency versions, release profile
crates/core/        # pbanki-core: session, study, sync, fonts, config, UI wiring
crates/core/ui/     # Slint UI sources, compiled by crates/core/build.rs
crates/pocketbook/  # pbanki: PocketBook frontend (InkView, Wi-Fi, native keyboard)
crates/desktop/     # pbanki-desktop: desktop frontend (winit, or eink-sim) for development
scripts/            # SDK emulator, PocketBook launcher and device helpers
```

## Current Features

- View deck hierarchies
- Collapse/expand decks
- Show card counts (new/learning/due)
- Study cards with scheduling
- Answer cards with Again/Hard/Good/Easy ratings
- Display interval durations on rating buttons
- Real-time deck count updates after answering
- Asynchronous card loading; a loading screen appears only for slow loads
- Configurable card font and fallback chain with a missing-glyph guard
- Japanese font support on PocketBook (Ryo Gothic PlusN by default)
- E-ink presentation that needs no extra touch at launch, resume, or card changes
- Fast first launch and first card with large CJK fonts (lazy glyph loading)
- Support for type-in cards (`[[type:Back]]`)
- Multi-language support via Anki's i18n system (70+ languages)
- Configurable collection path and language settings
- AnkiWeb or self-hosted sync, including full upload/download and media sync
- Settings page with sync options and automatic sync

## TODOs

- [ ] **Sync progress and abort** - Add background progress reporting and cancellation
- [ ] **MathJax Rendering** - Add support for LaTeX/MathJax formulas in cards
- [ ] **Image Support** - Display images embedded in cards from media folder
- [ ] **UI Polishing** - Improve e-ink optimized interface and user experience

## License

See [LICENSE](LICENSE)
