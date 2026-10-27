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
make          # debug build
make release  # release build
```

Requires: `cargo-zigbuild` and `zig`

## Emulator (PocketBook Era / PB700)

For development without a physical device, this repo can drive the
[katadelos/eink-emulator](https://github.com/katadelos/eink-emulator) QEMU port
of the PocketBook Era (PB700, B288, firmware 6.10.3209).

Host requirements: Python 3.10+, a C compiler, CMake, Ninja, QEMU build
dependencies (`glib`, `pixman`), and `debugfs` from e2fsprogs. The app build also
needs Zig and `cargo-zigbuild`; `make emulator-toolchain` installs Zig 0.14.1,
`cargo-zigbuild` 0.24.0 and Rust 1.89.0 + the `armv7-unknown-linux-gnueabi`
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

1. Copy binary to device: `/mnt/ext1/applications/pbanki.app`
2. Copy your Anki collection to: `/mnt/ext1/applications/pbanki/collection/`
3. (Optional) Create configuration file at: `/mnt/ext1/applications/pbanki/config.toml`
4. Launch from Applications menu

## Configuration

pbAnki uses a TOML configuration file. On first launch, a default config will be created at `/mnt/ext1/applications/pbanki/config.toml` (or `./pbanki/config.toml` for desktop builds).

### Configuration Options

```toml
[general]
# Language code for Anki i18n (e.g., "en", "de", "fr", "ja", "es", "pt", "ru", "zh", "ko")
language = "en"

# Collection path (relative to app directory or absolute)
collection_path = "/mnt/ext1/applications/pbanki/collection"

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

# Sync automatically on app start
auto_sync = false

# Sync automatically after session ends
sync_on_exit = false
```

### Supported Languages

pbAnki uses Anki's built-in translations, supporting 70+ languages including:
- English (`en`, `en-GB`)
- European: `es`, `fr`, `de`, `it`, `pt-BR`, `pt-PT`, `nl`, `sv-SE`, `da`, `fi`, `cs`, `hu`, `el`, `pl`, `tr`, `ru`, `uk`
- Asian: `ja`, `ko`, `zh-CN`, `zh-TW`, `th`, `vi`, `id`, `hi-IN`
- Middle Eastern: `ar`, `fa`, `he`
- And many more...

## Project Structure

```
src/      # PocketBook build (uses inkview-rs)
dev/      # Desktop build (for development)
ui/       # Slint UI components (shared)
```

## Current Features

- View deck hierarchies
- Collapse/expand decks
- Show card counts (new/learning/due)
- Study cards with scheduling
- Answer cards with Again/Hard/Good/Easy ratings
- Display interval durations on rating buttons
- Real-time deck count updates after answering
- Support for type-in cards (`[[type:Back]]`)
- Multi-language support via Anki's i18n system (70+ languages)
- Configurable collection path and language settings

## TODOs

- [ ] **AnkiWeb Sync** - Implement synchronization with AnkiWeb
- [ ] **MathJax Rendering** - Add support for LaTeX/MathJax formulas in cards
- [ ] **Image Support** - Display images embedded in cards from media folder
- [ ] **UI Polishing** - Improve e-ink optimized interface and user experience

## License

See [LICENSE](LICENSE)
