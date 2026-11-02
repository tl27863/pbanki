//! Logging setup shared by both frontends.
//!
//! pbAnki logs through `tracing`. anki logs through `tracing` too, and
//! inkview-rs through `log`, which the subscriber bridges. Everything goes to
//! stderr, which the PocketBook launcher writes to the session log together
//! with panics and InkView's own output.
//!
//! The filter comes from `PBANKI_LOG` (`EnvFilter` syntax, for example
//! `PBANKI_LOG=info,anki=debug`); the default is `info`.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;

/// Environment variable with the log filter.
pub const LOG_ENV: &str = "PBANKI_LOG";

const DEFAULT_FILTER: &str = "info";

/// Installs the global subscriber. Call it first in `main`; events logged
/// earlier are dropped.
pub fn init() {
    let (filter, bad_filter) = match std::env::var(LOG_ENV) {
        Ok(spec) => match EnvFilter::try_new(&spec) {
            Ok(filter) => (filter, None),
            Err(err) => (EnvFilter::new(DEFAULT_FILTER), Some((spec, err))),
        },
        Err(_) => (EnvFilter::new(DEFAULT_FILTER), None),
    };
    let installed = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(false)
        .with_timer(LocalTime)
        .try_init();
    if let Err(err) = installed {
        // No subscriber to log through; stderr still reaches the session log.
        // ast-grep-ignore: pbanki-no-eprintln
        eprintln!("[log] subscriber not installed: {err}");
    }
    if let Some((spec, err)) = bad_filter {
        tracing::warn!("[log] ignoring {LOG_ENV}='{spec}': {err}; using '{DEFAULT_FILTER}'");
    }
}

/// Local wall-clock time with milliseconds, matching the launcher's
/// `date '+%Y-%m-%d %H:%M:%S'` lines in the same log.
struct LocalTime;

impl FormatTime for LocalTime {
    fn format_time(&self, w: &mut Writer<'_>) -> fmt::Result {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let millis = now.subsec_millis();
        let secs = now.as_secs() as libc::time_t;
        // SAFETY: `tm` is plain data; `localtime_r` writes it or returns null.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
            return write!(w, "{}.{millis:03}", now.as_secs());
        }
        write!(
            w,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{millis:03}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}
