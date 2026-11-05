//! Sync scheduling from the UI and the platform hooks around every sync.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::ComponentHandle;

use super::FrontendHooks;
use super::models::deck_tree;
use super::settings::show_settings;
use crate::config::Config;
use crate::session::{LearnSession, reset_session_memory};
use crate::sync::{SyncAction, SyncResult, full_sync, sync_now};
use crate::{MainWindow, SyncState};

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

/// What started a normal sync; only used in log lines.
#[derive(Clone, Copy, Debug)]
pub(super) enum SyncTrigger {
    /// The reload icon on Home.
    Manual,
    /// `auto_sync` at startup.
    Startup,
    /// `sync_on_exit`, after leaving a study session.
    LeaveStudy,
}

impl SyncTrigger {
    fn name(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Startup => "startup",
            Self::LeaveStudy => "after study",
        }
    }
}

pub fn maybe_auto_sync(
    ui: &MainWindow,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: FrontendHooks,
) {
    let auto_sync = {
        let config = config.borrow();
        config.ankiweb.auto_sync && config.ankiweb.has_credentials()
    };
    if auto_sync {
        schedule_sync(ui.as_weak(), session, config, hooks, SyncTrigger::Startup);
    }
}

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
        ui.on_sync_now(move || {
            schedule_sync(
                ui_weak.clone(),
                session.clone(),
                config.clone(),
                hooks.clone(),
                SyncTrigger::Manual,
            );
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        let config = config.clone();
        let hooks = hooks.clone();
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
}

pub(super) fn schedule_sync(
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: FrontendHooks,
    trigger: SyncTrigger,
) {
    let kind = trigger.name();
    if session.busy.get() {
        tracing::info!("[sync] {kind}: ignored, collection busy");
        return;
    }
    if !begin_sync(&ui, "Syncing...") {
        tracing::info!("[sync] {kind}: ignored, sync already running");
        return;
    }

    session.busy.set(true);
    set_busy(&ui, true);
    run_sync_after_delay(ui, session, config, hooks, kind, sync_now);
}

fn schedule_full_sync(
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: FrontendHooks,
    upload: bool,
) {
    let (kind, message) = if upload {
        ("full upload", "Uploading collection...")
    } else {
        ("full download", "Downloading collection...")
    };

    if session.busy.get() {
        tracing::info!("[sync] {kind}: ignored, collection busy");
        return;
    }
    if !begin_sync(&ui, message) {
        tracing::info!("[sync] {kind}: ignored, sync already running");
        return;
    }

    session.busy.set(true);
    set_busy(&ui, true);
    run_sync_after_delay(ui, session, config, hooks, kind, move |session, config| {
        full_sync(session, config, upload)
    });
}

fn begin_sync(ui: &slint::Weak<MainWindow>, message: &str) -> bool {
    if let Some(ui) = ui.upgrade() {
        if ui.get_sync_state().is_syncing {
            return false;
        }
        ui.set_sync_state(SyncState::syncing(message));
    }
    true
}

fn set_busy(ui: &slint::Weak<MainWindow>, busy: bool) {
    if let Some(ui) = ui.upgrade() {
        ui.set_busy(busy);
    }
}

fn run_sync_after_delay<F>(
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: FrontendHooks,
    kind: &'static str,
    operation: F,
) where
    F: FnOnce(&LearnSession, &mut Config) -> SyncResult + 'static,
{
    slint::Timer::single_shot(Duration::from_millis(10), move || {
        let started = Instant::now();
        let guard = match hooks.sync.clone().start() {
            Ok(guard) => guard,
            Err(err) => {
                tracing::warn!("[sync] {kind}: not started: {err}");
                session.busy.set(false);
                if let Some(ui) = ui.upgrade() {
                    ui.set_sync_state(SyncState::error(&err));
                    ui.set_busy(false);
                    // See notes/card-loading.md, "Explicit redraws".
                    ui.window().request_redraw();
                }
                return;
            }
        };

        let prepared_ms = started.elapsed().as_millis();
        tracing::info!("[sync] {kind}: started (preparation {prepared_ms} ms)");
        let result = {
            let mut cfg = config.borrow_mut();
            operation(&session, &mut cfg)
        };
        log_result(kind, &result, started.elapsed().as_millis() - prepared_ms);
        // Any sync, even a failed one, may have changed the collection's
        // current deck. Syncs only start outside a study session, so the card
        // state is not in use.
        reset_session_memory(&session);

        session.busy.set(false);
        apply_sync_result(ui, &session, &config, &hooks, &result);
        drop(guard);
    });
}

fn log_result(kind: &str, result: &SyncResult, sync_ms: u128) {
    let server = result
        .server_message
        .as_deref()
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(|message| format!(", server: {message}"))
        .unwrap_or_default();
    if matches!(result.action, SyncAction::FullSyncRequired { .. }) {
        tracing::warn!(
            "[sync] {kind}: full sync required after {sync_ms} ms, {:?}: {}{server}",
            result.action,
            result.message
        );
    } else if result.success {
        tracing::info!(
            "[sync] {kind}: done in {sync_ms} ms, {:?}: {}{server}",
            result.action,
            result.message
        );
    } else {
        tracing::error!(
            "[sync] {kind}: failed after {sync_ms} ms: {}{server}",
            result.message
        );
    }
    if let Some(media_error) = &result.media_error {
        tracing::warn!("[sync] {kind}: {media_error}");
    }
}

fn apply_sync_result(
    ui: slint::Weak<MainWindow>,
    session: &LearnSession,
    config: &Rc<RefCell<Config>>,
    hooks: &FrontendHooks,
    result: &SyncResult,
) {
    if let Some(ui) = ui.upgrade() {
        ui.set_sync_state(SyncState::from_result(result));
        ui.set_busy(false);
        if matches!(result.action, SyncAction::FullSyncRequired { .. }) {
            show_settings(&ui, &config.borrow(), hooks);
        } else if result.success {
            ui.set_deck_tree(deck_tree(session));
        }
        // Property changes alone sometimes do not render on the PB710 until
        // the next touch. See notes/card-loading.md, "Explicit redraws".
        ui.window().request_redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn hooks(before: impl Fn() -> Result<(), String> + 'static) -> (SyncHooks, Rc<Cell<u32>>) {
        let finished = Rc::new(Cell::new(0));
        let after = finished.clone();
        let hooks = SyncHooks {
            before_sync: Some(Rc::new(before)),
            after_sync: Some(Rc::new(move || after.set(after.get() + 1))),
        };
        (hooks, finished)
    }

    #[test]
    fn guard_finishes_after_successful_prepare() {
        let (hooks, finished) = hooks(|| Ok(()));

        {
            let _guard = hooks.start().unwrap();
            assert_eq!(finished.get(), 0);
        }

        assert_eq!(finished.get(), 1);
    }

    #[test]
    fn guard_does_not_finish_when_prepare_fails() {
        let (hooks, finished) = hooks(|| Err("offline".to_string()));

        assert!(hooks.start().is_err());
        assert_eq!(finished.get(), 0);
    }

    #[test]
    fn guard_finishes_when_sync_panics() {
        let (hooks, finished) = hooks(|| Ok(()));

        let result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = hooks.start().unwrap();
            panic!("sync failed");
        }));

        assert!(result.is_err());
        assert_eq!(finished.get(), 1);
    }
}
