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
cd dev
cargo run
```

Place your Anki collection at `./pbanki/collection/`

### PocketBook

```bash
make                  # debug build
make release          # release build
make release-profile  # release build with [profile] timing logs
```

Requires: `cargo-zigbuild` and `zig`. The InkView dependencies come from the
`pbanki-upstream-master` branch of the `inkview-rs` fork and are fetched by Cargo
automatically.

## Emulator (PocketBook Era / PB700)

For development without a physical device, this repo can drive the
[katadelos/eink-emulator](https://github.com/katadelos/eink-emulator) QEMU port
of the PocketBook Era (PB700, B288, firmware 6.10.3209).

Host requirements: Python 3.10+, a C compiler, CMake, Ninja, QEMU build
dependencies (`glib`, `pixman`), and `debugfs` from e2fsprogs. The app build also
needs Zig and `cargo-zigbuild`; `make emulator-toolchain` installs Zig 0.14.1,
`cargo-zigbuild` 0.24.0 and Rust 1.99.1 + the `armv7-unknown-linux-gnueabi`
target under `~/.local` and `~/.cargo`.

```bash
make emulator-dev        # interactive session: setup + build + upload + launch
```

`make emulator-dev` is the normal development entry point. It ensures the
emulator exists, builds the ARM app, opens the emulator window, completes the
first-boot wizard if needed, enables Wi-Fi, uploads `pbanki.app` and launches
pbAnki from the app list. Keep the terminal open while using the emulator;
Ctrl+C stops QEMU.

The first run needs the toolchain and emulator setup:

```bash
make emulator-toolchain  # one-time: Zig, cargo-zigbuild, Rust target
make emulator-setup      # clone/build QEMU, download firmware, create PB700 instance
make emulator-run        # start the emulator only (GUI)
```

The first setup downloads a ~453 MiB firmware update and builds a patched QEMU,
so it takes several minutes. `emulator-setup` also completes the PocketBook
first-boot wizard automatically. If the wizard is skipped with
`PBANKI_SKIP_WIZARD=1`, `make emulator-dev` completes it in the visible window.

Build and upload manually when needed:

```bash
make emulator-build      # target/emulator/pbanki.app (stripped)
make emulator-sync       # upload to the running emulator over Wi-Fi
```

`emulator-sync` needs Wi-Fi enabled in the emulator: open
`Settings > Wi-Fi`, turn on `Network Access`, and join `PocketBook-QEMU`.
After syncing, close pbAnki (Home) and relaunch it from `Apps > USER'S`.
The app is listed as `@pbanki`.

The emulator serial console is available through the running instance:

```bash
python3 scripts/emulator/serial.py 'uname -m'
python3 scripts/emulator/serial.py 'wpa_cli -i eth0 status'
```

On first launch, `make emulator-dev` also installs
`scripts/emulator/JLPT_N2_Grammar.apkg` into the emulator's collection. Reinstall
or swap decks manually with:

```bash
make emulator-deck                              # install the bundled deck
PBANKI_DECK_APKG=/path/to/other.apkg make emulator-deck
PBANKI_REINSTALL_DECK=1 make emulator-deck      # replace an existing collection
```

The install replaces `collection.anki2` in the emulator and is skipped once the
`.pbanki-deck` marker exists; quit pbAnki before running it. Only uncompressed
Anki package collections (`collection.anki2`/`collection.anki21`) are supported.

Overrides:

- `EINK_EMULATOR_DIR` — external emulator checkout (default `../eink-emulator`).
- `PBANKI_EMULATOR_MACHINE` — emulator instance name (default `pb700`).
- `PBANKI_EMULATOR_HEADLESS=1` — run without a window (QMP/serial still work).
- `PBANKI_SKIP_BUILD=1` — upload the existing `target/emulator/pbanki.app`.

The emulator is experimental and does not cover e-ink-specific timing, suspend,
Bluetooth or audio. The `src/main.rs` config path can be overridden with
`PBANKI_CONFIG` for host-side testing; the device default is unchanged.

## Installation

Build the release bundle with `make release`. It creates:

```text
target/pocketbook/pbanki.app   # shell launcher
target/pocketbook/pbanki.bin   # ARM executable
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
the start and exit status. If the application directory is not writable, it
falls back to `/tmp/pbanki.log`. It changes into its own directory (normally
`/mnt/ext1/applications`) before starting the binary, so relative
`collection_path` values resolve there.

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
`card_fallback_fonts`, or `font_dirs` keys are added on the next launch without
overwriting values you already set.

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
uses `http://127.0.0.1:27701/` (desktop) or `http://10.0.2.2:27701/` (PB700
emulator) with the dev credentials from that project's `.env`.

### Supported Languages

pbAnki uses Anki's built-in translations, supporting 70+ languages including:
- English (`en`, `en-GB`)
- European: `es`, `fr`, `de`, `it`, `pt-BR`, `pt-PT`, `nl`, `sv-SE`, `da`, `fi`, `cs`, `hu`, `el`, `pl`, `tr`, `ru`, `uk`
- Asian: `ja`, `ko`, `zh-CN`, `zh-TW`, `th`, `vi`, `id`, `hi-IN`
- Middle Eastern: `ar`, `fa`, `he`
- And many more...

## Project Structure

```
common/       # shared Rust logic: API, card jobs, fonts, sync, Slint build
src/          # PocketBook build (uses the patched inkview-rs fork)
dev/          # Desktop build (for development)
ui/           # Slint UI components (shared, not a Cargo package)
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
