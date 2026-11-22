//! The settings screen: opening and closing it, and saving the sync and
//! device settings.

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use super::FrontendHooks;
use super::audio;
use super::decks;
use super::sync::end_finished_sync;
use crate::config::Config;
use crate::keys::KeyAction;
use crate::session::LearnSession;
use crate::sync::normalize_endpoint;
use crate::{DeviceSettings, KeyHints, MainWindow, SyncSettings, SyncState};

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
                audio::stop();
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
                end_finished_sync(&ui);
                ui.set_settings_open(false);
                decks::reload(&ui, &session);
                if was_open {
                    hooks.request_full_present(&ui);
                }
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let config = config.clone();
        let session = session.clone();
        ui.on_save_settings(move |settings| {
            // A running sync works on a copy of the config and writes its
            // token back when it ends.
            if session.busy.get() {
                return;
            }
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

    {
        let ui_weak = ui.as_weak();
        let config = config.clone();
        let session = session.clone();
        // The Device tab has no Save button: each change is saved at once.
        ui.on_save_device_settings(move |settings| {
            let result = {
                let mut config = config.borrow_mut();
                save_device_settings(&mut config, &settings)
            };
            if let Err(err) = result {
                tracing::error!("Could not save the device settings: {err}");
            }
            let config = config.borrow();
            apply_device_settings(&session, &config);
            audio::apply_config(&config);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_device_settings(device_settings_from_config(&config));
                ui.set_key_hints(key_hints_from_config(&config));
            }
        });
    }
}

/// Opens the settings screen with the saved settings, on the Sync tab. Opening it is a screen
/// transition, so it gets a full present unless it was already open.
pub(super) fn show_settings(ui: &MainWindow, config: &Config, hooks: &FrontendHooks) {
    let was_open = ui.get_settings_open();
    if !was_open {
        // A required full sync stays: it is why the sync result opens this.
        end_finished_sync(ui);
    }
    ui.set_sync_settings(sync_settings_from_config(config));
    ui.set_device_settings(device_settings_from_config(config));
    ui.set_settings_tab(0);
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

fn ui_action(action: KeyAction) -> crate::KeyAction {
    match action {
        KeyAction::Nothing => crate::KeyAction::Nothing,
        KeyAction::ShowAnswer => crate::KeyAction::ShowAnswer,
        KeyAction::Again => crate::KeyAction::Again,
        KeyAction::Hard => crate::KeyAction::Hard,
        KeyAction::Good => crate::KeyAction::Good,
        KeyAction::Easy => crate::KeyAction::Easy,
    }
}

fn config_action(action: crate::KeyAction) -> KeyAction {
    match action {
        crate::KeyAction::Nothing => KeyAction::Nothing,
        crate::KeyAction::ShowAnswer => KeyAction::ShowAnswer,
        crate::KeyAction::Again => KeyAction::Again,
        crate::KeyAction::Hard => KeyAction::Hard,
        crate::KeyAction::Good => KeyAction::Good,
        crate::KeyAction::Easy => KeyAction::Easy,
    }
}

pub(crate) fn device_settings_from_config(config: &Config) -> DeviceSettings {
    let general = &config.general;
    DeviceSettings {
        full_refresh_every: general.full_refresh_every as i32,
        key_prev_question: ui_action(general.key_prev_question),
        key_next_question: ui_action(general.key_next_question),
        key_prev_answer: ui_action(general.key_prev_answer),
        key_next_answer: ui_action(general.key_next_answer),
        audio_enabled: general.audio_enabled,
        audio_volume: general.audio_volume as i32,
    }
}

/// The configured key actions, for the key glyphs of the study screen.
pub(crate) fn key_hints_from_config(config: &Config) -> KeyHints {
    let general = &config.general;
    KeyHints {
        prev_question: ui_action(general.key_prev_question),
        next_question: ui_action(general.key_next_question),
        prev_answer: ui_action(general.key_prev_answer),
        next_answer: ui_action(general.key_next_answer),
    }
}

/// Copies the device settings to the session, where the study code reads
/// them.
fn apply_device_settings(session: &LearnSession, config: &Config) {
    session.key_config.set(config.general.key_config());
    session
        .full_refresh_every
        .set(config.general.full_refresh_every);
}

fn save_device_settings(config: &mut Config, settings: &DeviceSettings) -> Result<(), String> {
    let general = &mut config.general;
    general.full_refresh_every =
        crate::config::clamp_full_refresh_every(i64::from(settings.full_refresh_every));
    general.key_prev_question = config_action(settings.key_prev_question);
    general.key_next_question = config_action(settings.key_next_question);
    general.key_prev_answer = config_action(settings.key_prev_answer);
    general.key_next_answer = config_action(settings.key_next_answer);
    general.audio_enabled = settings.audio_enabled;
    general.audio_volume = settings
        .audio_volume
        .clamp(0, crate::config::AUDIO_VOLUME_MAX as i32) as u32;
    config.save().map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_settings_round_trip_through_the_config() {
        let dir =
            std::env::temp_dir().join(format!("pbanki-device-settings-{}", std::process::id()));
        let mut config = Config {
            path: dir.join("config.toml"),
            ..Config::default()
        };
        let mut settings = device_settings_from_config(&config);
        assert_eq!(settings.full_refresh_every, 0);
        assert_eq!(settings.key_next_answer, crate::KeyAction::Good);

        settings.full_refresh_every = 10;
        settings.key_prev_answer = crate::KeyAction::Hard;
        settings.key_next_question = crate::KeyAction::Nothing;
        settings.audio_enabled = false;
        settings.audio_volume = 9;
        save_device_settings(&mut config, &settings).unwrap();

        let saved = Config::load_or_create(config.path.to_str().unwrap()).unwrap();
        assert_eq!(saved.general.full_refresh_every, 10);
        assert_eq!(saved.general.key_prev_answer, KeyAction::Hard);
        assert_eq!(saved.general.key_next_question, KeyAction::Nothing);
        assert!(!saved.general.audio_enabled);
        assert_eq!(saved.general.audio_volume, 9);
        assert_eq!(device_settings_from_config(&saved), settings);
        assert_eq!(
            key_hints_from_config(&saved).prev_answer,
            crate::KeyAction::Hard
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
