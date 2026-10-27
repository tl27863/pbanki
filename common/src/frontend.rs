use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::ComponentHandle;

use crate::api::{
    full_sync, init_translations, next_card, rate_card, save_sync_settings, sync_now,
    sync_settings_from_config, sync_state_from_result, sync_state_from_status, update_deck_tree,
};
use crate::{Config, LearnSession, MainWindow, SyncAction, SyncResult, SyncState, SyncStatus};

/// Platform hooks run synchronously around every sync attempt.
///
/// Both callbacks run on the UI thread. `before_sync` must be cheap and
/// idempotent; it may wait for a user-visible platform prompt (for example
/// Wi-Fi setup), but must not block indefinitely on its own. Returning an error
/// aborts the sync and surfaces the message in `SyncState`. `after_sync` runs
/// exactly once iff `before_sync` returned `Ok`, including after a sync error
/// or panic, and must tolerate a partially prepared environment. Desktop leaves
/// both hooks unset.
#[derive(Clone, Default)]
pub struct SyncHooks {
    /// Prepares the environment (for example, Wi-Fi) before the blocking sync.
    ///
    /// Returning `Err` aborts the sync. It may wait for a user-visible prompt,
    /// but must not block indefinitely on its own.
    pub before_sync: Option<Rc<dyn Fn() -> Result<(), String>>>,
    /// Releases resources acquired by `before_sync`.
    ///
    /// Runs exactly once after a successful `before_sync`, even if the sync
    /// itself fails or panics.
    pub after_sync: Option<Rc<dyn Fn()>>,
}

struct SyncGuard {
    hooks: Option<SyncHooks>,
}

impl SyncHooks {
    fn prepare(&self) -> Result<(), String> {
        self.before_sync.as_ref().map_or(Ok(()), |hook| hook())
    }

    fn finish(&self) {
        if let Some(hook) = &self.after_sync {
            hook();
        }
    }

    fn start(self) -> Result<SyncGuard, String> {
        self.prepare()?;
        Ok(SyncGuard { hooks: Some(self) })
    }
}

impl Drop for SyncGuard {
    fn drop(&mut self) {
        if let Some(hooks) = self.hooks.take() {
            hooks.finish();
        }
    }
}

pub fn init_ui(ui: &MainWindow, session: &LearnSession, config: &Config, native_keyboard: bool) {
    ui.set_native_keyboard(native_keyboard);
    ui.set_tr(init_translations(session));
    ui.set_sync_settings(sync_settings_from_config(config));
    ui.set_sync_state(sync_state_from_status(&SyncStatus::Idle));
    ui.set_deck_tree(update_deck_tree(session));
}

pub fn maybe_auto_sync(
    ui: &MainWindow,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: SyncHooks,
) {
    let auto_sync = {
        let config = config.borrow();
        config.ankiweb.auto_sync && config.ankiweb.has_credentials()
    };
    if auto_sync {
        schedule_sync(ui.as_weak(), session, config, hooks);
    }
}

pub fn wire_callbacks(
    ui: &MainWindow,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: SyncHooks,
) {
    {
        let ui_weak = ui.as_weak();
        let config = config.clone();
        ui.on_open_settings(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_sync_settings(sync_settings_from_config(&config.borrow()));
                ui.set_settings_open(true);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        ui.on_settings_back(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_settings_open(false);
                ui.set_deck_tree(update_deck_tree(&session));
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
                let state = match result {
                    Ok(()) => SyncState {
                        is_syncing: false,
                        status_message: "Settings saved".into(),
                        has_error: false,
                        has_warning: false,
                        full_sync_required: false,
                        can_upload: false,
                        can_download: false,
                    },
                    Err(err) => SyncState {
                        is_syncing: false,
                        status_message: err.into(),
                        has_error: true,
                        has_warning: false,
                        full_sync_required: false,
                        can_upload: false,
                        can_download: false,
                    },
                };
                ui.set_sync_state(state);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        let config = config.clone();
        let hooks = hooks.clone();
        ui.on_sync_now(move || {
            schedule_sync(
                ui_weak.clone(),
                session.clone(),
                config.clone(),
                hooks.clone(),
            );
        });
    }

    {
        let hooks = hooks.clone();
        let ui_weak = ui.as_weak();
        let session = session.clone();
        let config = config.clone();
        ui.on_full_sync(move |upload| {
            schedule_full_sync(
                ui_weak.clone(),
                session.clone(),
                config.clone(),
                hooks.clone(),
                upload,
            );
        });
    }

    {
        let hooks = hooks.clone();
        let ui_weak = ui.as_weak();
        let session = session.clone();
        let config = config.clone();
        ui.on_session_ended(move || {
            let sync_on_exit = {
                let config = config.borrow();
                config.ankiweb.sync_on_exit && config.ankiweb.has_credentials()
            };
            if sync_on_exit {
                schedule_sync(
                    ui_weak.clone(),
                    session.clone(),
                    config.clone(),
                    hooks.clone(),
                );
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        ui.on_update_deck_tree(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_deck_tree(update_deck_tree(&session));
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        ui.on_rate(move |rating, deck, chars_per_page| {
            let next = rate_card(&session, rating, deck, chars_per_page);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_current_card(next);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        ui.on_deck_clicked(move |deck, chars_per_page| {
            let next = next_card(&session, deck, chars_per_page);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_current_card(next);
            }
        });
    }
}

fn schedule_sync(
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: SyncHooks,
) {
    if !begin_sync(&ui, "Syncing...") {
        return;
    }

    run_sync_after_delay(ui, session, config, hooks, sync_now);
}

fn schedule_full_sync(
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: SyncHooks,
    upload: bool,
) {
    let message = if upload {
        "Uploading collection..."
    } else {
        "Downloading collection..."
    };

    if !begin_sync(&ui, message) {
        return;
    }

    run_sync_after_delay(ui, session, config, hooks, move |session, config| {
        full_sync(session, config, upload)
    });
}

fn begin_sync(ui: &slint::Weak<MainWindow>, message: &str) -> bool {
    if let Some(ui) = ui.upgrade() {
        if ui.get_sync_state().is_syncing {
            return false;
        }
        ui.set_sync_state(syncing_state(message));
    }
    true
}

fn run_sync_after_delay<F>(
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: SyncHooks,
    operation: F,
) where
    F: FnOnce(&LearnSession, &mut Config) -> SyncResult + 'static,
{
    slint::Timer::single_shot(Duration::from_millis(10), move || {
        let guard = match hooks.start() {
            Ok(guard) => guard,
            Err(err) => {
                if let Some(ui) = ui.upgrade() {
                    ui.set_sync_state(error_state(&err));
                }
                return;
            }
        };

        let result = {
            let mut cfg = config.borrow_mut();
            operation(&session, &mut cfg)
        };

        apply_sync_result(ui, &session, &config, &result);
        drop(guard);
    });
}

fn apply_sync_result(
    ui: slint::Weak<MainWindow>,
    session: &LearnSession,
    config: &Rc<RefCell<Config>>,
    result: &SyncResult,
) {
    if let Some(ui) = ui.upgrade() {
        ui.set_sync_state(sync_state_from_result(result));
        if matches!(result.action, SyncAction::FullSyncRequired { .. }) {
            ui.set_sync_settings(sync_settings_from_config(&config.borrow()));
            ui.set_settings_open(true);
        } else if result.success {
            ui.set_deck_tree(update_deck_tree(session));
        }
    }
}

fn syncing_state(message: &str) -> SyncState {
    SyncState {
        is_syncing: true,
        status_message: message.into(),
        has_error: false,
        has_warning: false,
        full_sync_required: false,
        can_upload: false,
        can_download: false,
    }
}

fn error_state(message: &str) -> SyncState {
    SyncState {
        is_syncing: false,
        status_message: message.into(),
        has_error: true,
        has_warning: false,
        full_sync_required: false,
        can_upload: false,
        can_download: false,
    }
}

#[cfg(test)]
#[path = "../tests/unit/sync_hooks.rs"]
mod tests;
