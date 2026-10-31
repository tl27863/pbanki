//! Slint callback wiring shared by both frontends.
//!
//! Platform differences come in through `FrontendHooks`. Card loads run on a
//! worker thread (`card_jobs`); sync still blocks the UI thread (`sync`).

mod card_jobs;
mod models;
mod settings;
mod study;
mod sync;

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::config::Config;
use crate::session::LearnSession;
use crate::{MainWindow, SyncState};
use models::{deck_tree, translations};
use settings::sync_settings_from_config;

pub use sync::{SyncHooks, maybe_auto_sync};

/// Platform hooks passed to `wire_callbacks` and `maybe_auto_sync`.
#[derive(Clone, Default)]
pub struct FrontendHooks {
    pub sync: SyncHooks,
    /// Requests a non-flashing, full-frame present of the next rendered frame.
    ///
    /// Called on the UI thread for genuine screen transitions only (deck entry
    /// and its first result, Home, Settings open/back). Desktop leaves this
    /// unset, making transition requests a plain redraw.
    pub full_present: Option<Rc<dyn Fn()>>,
}

impl FrontendHooks {
    fn request_full_present(&self, ui: &MainWindow) {
        ui.window().request_redraw();
        if let Some(hook) = &self.full_present {
            hook();
        }
    }
}

pub fn init_ui(ui: &MainWindow, session: &LearnSession, config: &Config, native_keyboard: bool) {
    ui.set_native_keyboard(native_keyboard);
    ui.set_busy(false);
    ui.set_card_font(session.font_catalog.primary().into());
    ui.set_tr(translations(session));
    ui.set_sync_settings(sync_settings_from_config(config));
    ui.set_sync_state(SyncState::default());
    ui.set_deck_tree(deck_tree(session));
}

pub fn wire_callbacks(
    ui: &MainWindow,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: FrontendHooks,
) {
    settings::wire(ui, &session, &config, &hooks);
    sync::wire(ui, &session, &config, &hooks);
    study::wire(ui, &session, &config, &hooks);
}
