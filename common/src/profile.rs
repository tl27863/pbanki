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

/// Logs, off the UI thread, how long reading each registered card font takes
/// and which font the UI family (`ui_family`, e.g. "Monospace") resolves to.
///
/// Slint reads every registered font file completely when its font collection
/// is created, and rasterizes glyphs lazily with swash, so the file reads are
/// the per-font startup cost worth tracking.
///
/// The thread starts `PROBE_DELAY` after launch at the lowest CPU priority,
/// so it does not compete with the first frames. No-op without the `profile`
/// feature.
pub fn spawn_font_probe(catalog: &FontCatalog, ui_family: &str) {
    #[cfg(feature = "profile")]
    {
        let faces = catalog.registered_faces();
        let ui_family = ui_family.to_owned();
        let spawned = std::thread::Builder::new()
            .name("pbanki-font-probe".into())
            .spawn(move || {
                std::thread::sleep(probe::PROBE_DELAY);
                // On Linux PRIO_PROCESS with id 0 applies to the calling thread.
                unsafe {
                    libc::setpriority(libc::PRIO_PROCESS, 0, 19);
                }
                probe::run(faces, &ui_family)
            });
        if let Err(err) = spawned {
            profile_log!("font probe thread failed to start: {err}");
        }
    }
    #[cfg(not(feature = "profile"))]
    {
        let _ = (catalog, ui_family);
    }
}

#[cfg(feature = "profile")]
mod probe {
    use std::path::PathBuf;
    use std::time::Instant;

    use fontique::{
        Attributes, Collection, CollectionOptions, GenericFamily, QueryFamily, QueryStatus,
        SourceCache,
    };

    /// Start delay after launch; the first present happens well before this.
    pub(super) const PROBE_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

    pub(super) fn run(faces: Vec<(String, PathBuf, u32)>, ui_family: &str) {
        let started = Instant::now();
        for (family, path, index) in faces {
            let read_started = Instant::now();
            match std::fs::read(&path) {
                Ok(data) => profile_log!(
                    "font '{family}' {} index={index}: read={:?} bytes={}",
                    path.display(),
                    read_started.elapsed(),
                    data.len()
                ),
                Err(err) => profile_log!("font '{family}' {}: read failed: {err}", path.display()),
            }
        }

        resolve_ui_font(ui_family);
        profile_log!("font probe finished in {:?}", started.elapsed());
    }

    /// Repeats Slint's font query for `family` against the system collection:
    /// the named family, then sans-serif and system-ui.
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
            profile_log!("UI family '{family}': no system font matched");
            return;
        };
        let name = collection
            .family_name(font.family.0)
            .unwrap_or("<unknown>")
            .to_owned();
        profile_log!(
            "UI family '{family}' resolves to '{name}' index={} bytes={} (resolve {:?})",
            font.index,
            font.blob.data().len(),
            resolve_started.elapsed()
        );
    }
}
