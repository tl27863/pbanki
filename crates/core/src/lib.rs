slint::include_modules!();

pub mod app;
pub mod card_render;
pub mod config;
pub mod fonts;
pub mod frontend;
pub mod keys;
pub mod logging;
pub mod profile;
pub mod session;
pub mod stats;
pub mod study;
pub mod sync;

// For `profile_log!`, which expands in the calling crate.
#[doc(hidden)]
pub use tracing;
