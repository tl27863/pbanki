# Fonts

## Card fonts on the PocketBook

- In USB mass-storage view, the fonts are in `/system/fonts` and `/system/adobe/fonts`. On the device, the runtime paths are `/mnt/ext1/system/fonts` and `/mnt/ext1/system/adobe/fonts`.
- PB700 images use `/fonts` and `/adobefonts` (not `/ebrmain/fonts`).
- `/mnt/ext1/system/adobe/fonts/RyoGothicPlusN-Medium.otf` (2.3 MB, typographic family `Ryo Gothic PlusN`) covers Japanese. It is the default card font.
- `/mnt/ext1/system/fonts/NotoSansCJK-Regular.ttc` is a 19 MB CFF collection.
- `fireflysung.ttf` does not have `両`.

## Font catalog and registration

- `crates/core/src/fonts.rs` makes a catalog from `Config.general.font_dirs`. It reads only the sfnt/TTC table directory and the `name` table of each file, not the full file.
- `register_catalog_fonts` registers the primary and fallback files with Slint. `app::start` calls it before `MainWindow::new`.
  - It memory-maps the files into the shared fontique collection of Slint through `slint::fontique_011::shared_collection()`.
  - This needs the feature `unstable-fontique-011` in `crates/core/Cargo.toml`. It also needs the Slint platform, so call it after `set_platform`.
  - It orders the fonts by the configured family. It puts them first in the Hira, Kana, Hani, and Hang fallback chains.
- `fonts.rs` also checks card text against the cmap of each font. `next_card` selects the first font that covers the full card. If no font does, it replaces the missing glyphs with `?`.
- Card text uses `Config.general.card_font` plus `card_fallback_fonts`. The UI stays `Monospace` (Roboto on the PB710).

Do not use `SLINT_FONT_PATH` or `SLINT_DEFAULT_FONT`. In Slint 1.18 they make the registered fonts the generic sans-serif chain. `Monospace` is not a real family, so it falls back to sans-serif. It then rendered in Ryo Gothic instead of Roboto. The lint rule `pbanki-no-slint-font-env` checks this.

## Slint 1.18 fonts

- Slint 1.16+ rasterizes glyphs with swash (lazy, one glyph at a time) instead of fontdue. Thus the old vendored fontdue patch is removed.
  - The eager outlines of fontdue took 5.5 s for Ryo Gothic and 16 s for Adobe Ming on the PB710.
  - If a future Slint uses fontdue again, check the first-render time.
- fontique 0.11 links fontconfig on Linux. The root manifest enables `fontique/fontconfig-dlopen`, so the armv7 cross build needs no fontconfig sysroot.
- Without `libfontconfig.so.1` on the device, Slint skips the system fonts. The registered card fonts still work and become the UI fallback.
- The PB710 has `libfontconfig.so.1`. `Monospace` resolves to Roboto.

## Glyph embedding

- `crates/core/build.rs` uses `EmbedForSoftwareRenderer`. It rasterizes each literal font size in the UI before run time. It also embeds the SVG icons, which need it, because runtime SVG decoding is not enabled.
- A constant `0px` size (for example an unset `font-size` on a std-widgets `CheckBox`) is rasterized at the design size of the font. The result was 380 MB of generated code and an 80 MB binary.
- Thus `crates/core/ui/screens/settings.slint` sets each CheckBox to `12px` (the default of Slint). The lint rule `pbanki-checkbox-font-size` checks this.
- The size problem shows only when no CheckBox sets the size. With Slint 1.18, one unset CheckBox among set ones still built at 2.8 MB. All unset gave 264 MB. Thus one missing size is a hidden bug.
- After UI changes, make sure that `target/*/build/pbanki-core-*/out/main.rs` stays at about 3 MB and has no `pixel_size : 0`.

## Embedded bitmap font

- The embedded bitmap font is the default sans font of the build host (Noto Sans here). It has only ASCII plus the characters from `.slint` literals.
- With `systemfonts`, Slint uses a bitmap font only in two cases: the requested family is equal to its name, or as the last fallback when the requested family resolves to nothing or no family is requested.
- Thus `MainWindow` sets `default-font-family: Constants.font_family_standard`. Std-widgets labels (CheckBox, Button) then also use the runtime UI font and show translated characters.
