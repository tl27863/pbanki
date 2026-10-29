# Vendored fontdue 0.9.3 (pbanki patch)

Upstream: https://github.com/mooman219/fontdue, crates.io `fontdue` 0.9.3
(MIT OR Apache-2.0 OR Zlib; license files kept). Wired in through
`[patch.crates-io]` in `Cargo.toml`, `common/Cargo.toml`, and `dev/Cargo.toml`,
so it replaces fontdue for both Slint's runtime software renderer and the
build-time Slint compiler.

Change: `Font::from_bytes` no longer outlines every cmap/GSUB glyph up front.
It keeps an owned copy of the font bytes and one `OnceLock<Glyph>` per glyph;
`metrics_indexed`/`rasterize_indexed*` generate the outline on first use
(`Font::glyph`). Upstream's eager pass took 5.5 s for Ryo Gothic PlusN and
16 s for Adobe Ming Std on the PB710, inside Slint's first `render()` on the
UI thread. Side effects: glyphs outside cmap/GSUB now have real outlines
instead of empty ones, `FontSettings::load_substitutions` has no effect, and
the crate links `std` (for `OnceLock`).

Drop this copy if Slint stops using fontdue or upstream adds lazy loading.
