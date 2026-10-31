//! The settings screen: opening and closing it, and saving sync settings.

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use super::FrontendHooks;
use super::models::deck_tree;
use crate::config::Config;
use crate::session::LearnSession;
use crate::sync::normalize_endpoint;
use crate::{MainWindow, SyncSettings, SyncState};

pub(super) fn wire(
    ui: &MainWindow,
    session: &Rc<LearnSession>,
    config: &Rc<RefCell<Config>>,
    hooks: &FrontendHooks,
) {
    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        let config = config.clone();
        let hooks = hooks.clone();
        ui.on_open_settings(move || {
            // The deck list stays visible (and its gear reachable) while a deck
            // entry job runs, until the loading screen or card appears.
            if session.busy.get() {
                return;
            }
            if let Some(ui) = ui_weak.upgrade() {
                show_settings(&ui, &config.borrow(), &hooks);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        let hooks = hooks.clone();
        ui.on_settings_back(move || {
            if session.busy.get() {
                return;
            }
            if let Some(ui) = ui_weak.upgrade() {
                let was_open = ui.get_settings_open();
                ui.set_settings_open(false);
                ui.set_deck_tree(deck_tree(&session));
                if was_open {
                    hooks.request_full_present(&ui);
                }
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let config = config.clone();
        ui.on_save_settings(move |settings| {
            let result = {
                let mut config = config.borrow_mut();
                save_sync_settings(&mut config, settings)
            };

            if let Some(ui) = ui_weak.upgrade() {
                ui.set_sync_settings(sync_settings_from_config(&config.borrow()));
                ui.set_sync_state(match result {
                    Ok(()) => SyncState::info("Settings saved"),
                    Err(err) => SyncState::error(&err),
                });
            }
        });
    }
}

/// Opens the settings screen with the saved settings. Opening it is a screen
/// transition, so it gets a full present unless it was already open.
pub(super) fn show_settings(ui: &MainWindow, config: &Config, hooks: &FrontendHooks) {
    let was_open = ui.get_settings_open();
    ui.set_sync_settings(sync_settings_from_config(config));
    ui.set_settings_open(true);
    if !was_open {
        hooks.request_full_present(ui);
    }
}

pub(crate) fn sync_settings_from_config(config: &Config) -> SyncSettings {
    SyncSettings {
        endpoint: config.ankiweb.endpoint.clone().unwrap_or_default().into(),
        username: config.ankiweb.username.clone().into(),
        password: config.ankiweb.password.clone().into(),
        auto_sync: config.ankiweb.auto_sync,
        sync_on_exit: config.ankiweb.sync_on_exit,
        sync_media: config.ankiweb.sync_media,
    }
}

fn save_sync_settings(config: &mut Config, settings: SyncSettings) -> Result<(), String> {
    let endpoint = normalize_endpoint(settings.endpoint.as_str())?;
    let endpoint_changed = config
        .ankiweb
        .endpoint
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        != endpoint.as_deref().unwrap_or_default();
    let credentials_changed = endpoint_changed
        || config.ankiweb.username != settings.username.as_str()
        || config.ankiweb.password != settings.password.as_str();

    config.ankiweb.endpoint = endpoint;
    config.ankiweb.username = settings.username.to_string();
    config.ankiweb.password = settings.password.to_string();
    config.ankiweb.auto_sync = settings.auto_sync;
    config.ankiweb.sync_on_exit = settings.sync_on_exit;
    config.ankiweb.sync_media = settings.sync_media;

    if credentials_changed {
        config.ankiweb.token = None;
        config.ankiweb.token_endpoint = None;
    }

    config.save().map_err(|err| err.to_string())
}
