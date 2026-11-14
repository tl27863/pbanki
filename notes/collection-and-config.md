# Collection and config

## Anki dependency

- `anki` and `anki_proto` come from `ankitects/anki` `branch = "main"` (a git dependency, not pinned).
- The single workspace `Cargo.lock` pins the anki revision for both frontends. Do not `cargo update` it together with other changes.
- `slint` and `slint-build` are `1.18.1` from crates.io. The root `[workspace.dependencies]` sets the Slint version and the base software-renderer features. Members add only features (`unstable-fontique-011` in core, `backend-winit` in desktop).

## Collection setup

`init_session` in `crates/core/src/session.rs` opens the collection.

- It calls `with_desktop_media_paths()`, so `collection.media/` and `collection.mdb` exist. Do not remove this, or media sync fails.
- It imports each `*.apkg` in the collection directory that is not imported yet, with scheduling and media. Then it renames the file to `*.apkg.imported`. Users install decks on the device this way. See `import_pending_packages()`.
- It suspends "Image Occlusion Enhanced" cards.

## Config

- `Config` keeps its load path in the field `path` (serde skips it). `save()` writes to that path.
- `[general]` has these font keys:
  - `card_font`, default `Ryo Gothic PlusN`.
  - `card_fallback_fonts`: `Adobe Ming Std`, `Adobe Heiti Std`, `Noto Sans CJK JP`.
  - `font_dirs`: the PB710 runtime paths, the USB paths, and the PB700 paths.
- `[general] system_panel` is true by default (see `inkview-backend.md`).
- `[general] card_shadows` is false by default: all `box-shadow` is removed from cards, because a blurred shadow costs about 170 ms per page on the PB710 (`blitz-card-render.md`, "Card HTML"). True draws them. Config file only, not in the settings UI; read when the study screen is wired (restart after a change). A config that got `card_shadows = true` from the build of 2026-11-05 keeps it.
- `[general] image_cache_mb` (default 32) and `image_cache_max_image_mb` (default 16): the decoded image cache of the card renderer, all images and one image, in MB of RGBA. 0 turns it off. Config file only; read when the render thread starts.
- `[ankiweb].endpoint` is blank for AnkiWeb. `sync_media` is true by default.
- `[general] full_refresh_every` (default 0): flashing full refresh after this many rated cards. The values are 0 (never), 1, 5 and 10; another value goes to the nearest one. Also in Settings > Device (`eink-refresh.md`, "Periodic full refresh").
- `[general] key_prev_question`, `key_next_question`, `key_prev_answer`, `key_next_answer`: the action of the Prev and Next keys on each card side: `nothing`, `showanswer`, `again`, `hard`, `good`, `easy`. Defaults: `nothing`, `showanswer`, `again`, `good`. Also in Settings > Device, which saves each change at once (`card-loading.md`, "Hardware keys").
- `load_or_create` adds missing font keys, `system_panel`, `card_shadows`, the image cache keys, `full_refresh_every` and the key actions to an existing config and saves it. New keys get their comment from the template (`set_with_comment`). The generated template contains all keys.

## Settings UI

- PocketBook uses the native InkView keyboard (`crates/pocketbook/src/keyboard.rs`). `main.rs` maps it to the settings fields through `EditField`. The desktop uses a normal `LineEdit`.
- The keyboard callback runs on the InkView thread. It sends the result with `slint::invoke_from_event_loop`.
- If the user cancels the keyboard (NULL text), the field does not change.
- The buffer is 512 bytes including the NUL. The keyboard truncates longer initial values on a character boundary.
