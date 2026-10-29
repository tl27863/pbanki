//! Opt-in timing instrumentation, enabled with the `profile` cargo feature.
//!
//! Without the feature, `profile_log!` and `ProfileTimer` compile to nothing,
//! so release builds carry no profiling code or log traffic. Lines are
//! prefixed with `[profile]`.

use std::time::Duration;

#[cfg(feature = "profile")]
use std::time::Instant;

use crate::fonts::FontCatalog;

/// Logs a `[profile]` line when the `profile` feature is enabled.
#[cfg(feature = "profile")]
#[macro_export]
macro_rules! profile_log {
    ($($arg:tt)*) => {
        eprintln!("[profile] {}", format_args!($($arg)*))
    };
}

/// Logs a `[profile]` line when the `profile` feature is enabled.
#[cfg(not(feature = "profile"))]
#[macro_export]
macro_rules! profile_log {
    ($($arg:tt)*) => {
        if false {
            // Keeps the arguments type-checked and "used" without emitting code.
            eprintln!("[profile] {}", format_args!($($arg)*))
        }
    };
}

/// A stopwatch that is a zero-sized no-op without the `profile` feature.
#[derive(Clone, Copy)]
pub struct ProfileTimer {
    #[cfg(feature = "profile")]
    started: Instant,
}

impl ProfileTimer {
    #[inline(always)]
    pub fn start() -> Self {
        Self {
            #[cfg(feature = "profile")]
            started: Instant::now(),
        }
    }

    /// Time since `start()`, or zero without the `profile` feature.
    #[inline(always)]
    pub fn elapsed(&self) -> Duration {
        #[cfg(feature = "profile")]
        {
            self.started.elapsed()
        }
        #[cfg(not(feature = "profile"))]
        {
            Duration::ZERO
        }
    }
}

/// Measures fontdue first-use cost for every font Slint will load, off the UI
/// thread.
///
/// Slint parses each font with `fontdue::Font::from_bytes` on first use inside
/// `render()` on the UI thread, then rasterizes the glyphs it draws. This runs
/// the same parse plus a rasterization of `SAMPLE_TEXT` for the registered
/// card fonts and for whatever the UI family (`ui_family`, e.g. "Monospace")
/// resolves to, and logs the times and glyph counts.
///
/// The thread starts `BENCHMARK_DELAY` after launch at the lowest CPU
/// priority, so it does not compete with the first frames. No-op without the
/// `profile` feature.
pub fn spawn_font_benchmark(catalog: &FontCatalog, ui_family: &str) {
    #[cfg(feature = "profile")]
    {
        let faces = catalog.registered_faces();
        let ui_family = ui_family.to_owned();
        let spawned = std::thread::Builder::new()
            .name("pbanki-font-bench".into())
            .spawn(move || {
                std::thread::sleep(benchmark::BENCHMARK_DELAY);
                // On Linux PRIO_PROCESS with id 0 applies to the calling thread.
                unsafe {
                    libc::setpriority(libc::PRIO_PROCESS, 0, 19);
                }
                benchmark::run(faces, &ui_family)
            });
        if let Err(err) = spawned {
            profile_log!("fontdue benchmark thread failed to start: {err}");
        }
    }
    #[cfg(not(feature = "profile"))]
    {
        let _ = (catalog, ui_family);
    }
}

#[cfg(feature = "profile")]
mod benchmark {
    use std::path::PathBuf;
    use std::time::Instant;

    use fontique::{
        Attributes, Collection, CollectionOptions, GenericFamily, QueryFamily, QueryStatus,
        SourceCache,
    };

    /// Start delay after launch; the first present happens well before this.
    pub(super) const BENCHMARK_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

    /// Rasterized after parsing: about a card's worth of mixed text.
    const SAMPLE_TEXT: &str = "The quick brown fox jumps over the lazy dog 0123456789 \
        あいうえおかきくけこさしすせそたちつてとなにぬねのはひふへほまみむめもやゆよらりるれろわをん \
        アイウエオカキクケコサシスセソタチツテト 日本語の文法を勉強しています。両方とも試験に出る表現です。";

    /// Slint's software renderer pixel size: 20 px font at scale factor 3.
    const SAMPLE_PX: f32 = 60.0;

    pub(super) fn run(faces: Vec<(String, PathBuf, u32)>, ui_family: &str) {
        let started = Instant::now();
        for (family, path, index) in faces {
            let read_started = Instant::now();
            let data = match std::fs::read(&path) {
                Ok(data) => data,
                Err(err) => {
                    profile_log!("fontdue '{family}' {}: read failed: {err}", path.display());
                    continue;
                }
            };
            let read = read_started.elapsed();
            parse(&format!("'{family}' {}", path.display()), &data, index, read);
        }

        resolve_ui_font(ui_family);
        profile_log!("fontdue benchmark finished in {:?}", started.elapsed());
    }

    /// Repeats Slint's `FontRequest::query_fontique` for `family` against the
    /// system collection: the named family, then sans-serif and system-ui.
    fn resolve_ui_font(family: &str) {
        let resolve_started = Instant::now();
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: true,
        });
        let mut cache = SourceCache::default();
        let mut query = collection.query(&mut cache);
        query.set_families([
            QueryFamily::Named(family),
            QueryFamily::Generic(GenericFamily::SansSerif),
            QueryFamily::Generic(GenericFamily::SystemUi),
        ]);
        query.set_attributes(Attributes::default());
        let mut found = None;
        query.matches_with(|font| {
            found = Some(font.clone());
            QueryStatus::Stop
        });
        drop(query);

        let Some(font) = found else {
            profile_log!("fontdue UI family '{family}': no system font matched");
            return;
        };
        let name = collection
            .family_name(font.family.0)
            .unwrap_or("<unknown>")
            .to_owned();
        let resolve = resolve_started.elapsed();
        profile_log!(
            "fontdue UI family '{family}' resolves to '{name}' index={} bytes={} (resolve {resolve:?})",
            font.index,
            font.blob.data().len()
        );
        parse(
            &format!("UI '{family}' -> '{name}'"),
            font.blob.data(),
            font.index,
            std::time::Duration::ZERO,
        );
    }

    fn parse(label: &str, data: &[u8], index: u32, read: std::time::Duration) {
        let parse_started = Instant::now();
        // Same settings as Slint's software renderer (`systemfonts.rs`).
        let result = fontdue::Font::from_bytes(
            data,
            fontdue::FontSettings {
                collection_index: index,
                scale: 40.,
                ..Default::default()
            },
        );
        let parse = parse_started.elapsed();
        match result {
            Ok(font) => {
                let raster_started = Instant::now();
                let mut drawn = 0usize;
                for character in SAMPLE_TEXT.chars().filter(|c| !c.is_whitespace()) {
                    let index = font.lookup_glyph_index(character);
                    if index != 0 {
                        let _ = font.rasterize_indexed(index, SAMPLE_PX);
                        drawn += 1;
                    }
                }
                let raster = raster_started.elapsed();
                profile_log!(
                    "fontdue {label}: read={read:?} parse={parse:?} sample_raster={raster:?} \
                     ({drawn} glyphs) glyphs={} chars={} bytes={}",
                    font.glyph_count(),
                    font.chars().len(),
                    data.len()
                )
            }
            Err(err) => profile_log!("fontdue {label}: parse failed after {parse:?}: {err}"),
        }
    }
}
