//! Sync scheduling from the UI and the platform hooks around every sync.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::ComponentHandle;

use anki::collection::Collection;

use super::FrontendHooks;
use super::card_jobs::spawn_collection_job;
use super::decks;
use super::settings::show_settings;
use crate::config::Config;
use crate::session::{LearnSession, reset_session_memory};
use crate::sync::{SyncAction, SyncManager, SyncOutcome, SyncResult, full_sync, sync_now};
use crate::{MainWindow, SyncState};

/// Platform hooks run around every sync attempt. The sync itself runs on a
/// worker thread between them.
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
    /// Prepares the environment (for example, Wi-Fi) before the sync.
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
    /// The sync button on Home, or Sync now in Settings.
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
    start_sync(
        ui,
        session,
        config,
        hooks,
        trigger.name(),
        "Syncing...",
        sync_now,
    );
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
    start_sync(
        ui,
        session,
        config,
        hooks,
        kind,
        message,
        move |collection, config, client| full_sync(collection, config, client, upload),
    );
}

/// Marks the sync as running. After a short delay (so the syncing state can
/// render), it runs `before_sync` on the UI thread and then `operation` as a
/// collection job on a worker thread.
fn start_sync<F>(
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: FrontendHooks,
    kind: &'static str,
    message: &str,
    operation: F,
) where
    F: FnOnce(&mut Option<Collection>, &Config, &SyncManager) -> SyncOutcome + Send + 'static,
{
    if session.busy.get() {
        tracing::info!("[sync] {kind}: ignored, collection busy");
        return;
    }
    if let Some(ui) = ui.upgrade() {
        if ui.get_sync_state().is_syncing {
            tracing::info!("[sync] {kind}: ignored, sync already running");
            return;
        }
        ui.set_sync_state(SyncState::syncing(message));
        ui.set_busy(true);
    }
    session.busy.set(true);

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
        tracing::info!(
            "[sync] {kind}: started (preparation {} ms)",
            started.elapsed().as_millis()
        );
        run_sync_job(ui, session, config, hooks, guard, kind, operation);
    });
}

/// Runs `operation` on the worker thread. `guard` stays on the UI thread and
/// is dropped after the result is applied, or with the job if the event loop
/// stops first. Thus `after_sync` runs exactly once, also when the worker
/// panics.
fn run_sync_job<F>(
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: FrontendHooks,
    guard: SyncGuard,
    kind: &'static str,
    operation: F,
) where
    F: FnOnce(&mut Option<Collection>, &Config, &SyncManager) -> SyncOutcome + Send + 'static,
{
    // The worker gets a copy of the config. The UI thread applies and saves
    // what the sync changed.
    let snapshot = config.borrow().clone();
    let client = session.sync_manager.clone();
    let finish_session = session.clone();
    let finish_ui = ui.clone();
    let job = spawn_collection_job(
        &ui,
        &session,
        move |collection| {
            let started = Instant::now();
            let outcome = operation(collection, &snapshot, &client);
            (outcome, started.elapsed().as_millis())
        },
        move |output| {
            let session = finish_session;
            let result = match output {
                Some((outcome, sync_ms)) => {
                    outcome.apply_config(&mut config.borrow_mut());
                    log_result(kind, &outcome.result, sync_ms);
                    outcome.result
                }
                None => {
                    tracing::error!("[sync] {kind}: the sync worker stopped unexpectedly");
                    SyncResult {
                        message: "Sync stopped unexpectedly; restart pbAnki".into(),
                        ..Default::default()
                    }
                }
            };
            // Any sync, even a failed one, may have changed the collection's
            // current deck. Syncs only start outside a study session, so the
            // card state is not in use.
            reset_session_memory(&session);
            apply_sync_result(finish_ui, &session, &config, &hooks, &result);
            drop(guard);
        },
    );
    if job.is_none() {
        // Not reachable: `busy` was set for this sync, so no other job runs.
        // The dropped closure has already released the guard.
        tracing::error!("[sync] {kind}: not started, another job is running");
        session.busy.set(false);
        if let Some(ui) = ui.upgrade() {
            ui.set_sync_state(SyncState::error("Sync could not start"));
            ui.set_busy(false);
            ui.window().request_redraw();
        }
    }
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

/// Call on every screen change: a finished sync result ends there
/// (`SyncState::after_screen_change`). The new screen is presented anyway, so
/// this adds no panel update.
pub(super) fn end_finished_sync(ui: &MainWindow) {
    if let Some(state) = ui.get_sync_state().after_screen_change() {
        ui.set_sync_state(state);
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
        if matches!(result.action, SyncAction::FullSyncRequired { .. }) {
            show_settings(&ui, &config.borrow(), hooks);
        } else if result.success {
            decks::reload(&ui, session);
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

    /// A session with an empty collection in a temp dir.
    fn test_session(name: &str) -> (Rc<LearnSession>, Rc<RefCell<Config>>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "pbanki-sync-job-test-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = Config {
            path: dir.join("config.toml"),
            ..Config::default()
        };
        config.general.collection_path = dir.join("col").to_string_lossy().into_owned();
        config.general.card_fallback_fonts.clear();
        config.general.font_dirs.clear();
        let session = crate::session::init_session(&config);
        (session, Rc::new(RefCell::new(config)), dir)
    }

    #[test]
    fn guard_finishes_when_worker_panics() {
        let (hooks, finished) = hooks(|| Ok(()));
        let (session, config, dir) = test_session("panic");
        let guard = hooks.start().unwrap();
        session.busy.set(true);

        run_sync_job(
            slint::Weak::default(),
            session.clone(),
            config,
            FrontendHooks::default(),
            guard,
            "test",
            |_, _, _| panic!("sync worker failed"),
        );
        assert_eq!(finished.get(), 0);
        assert!(session.busy.get());

        super::super::card_jobs::deliver_pending_for_test();

        assert_eq!(finished.get(), 1);
        assert!(!session.busy.get());
        // The worker owned the collection when it panicked.
        assert!(session.collection.borrow().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn worker_result_returns_the_collection() {
        let (hooks, finished) = hooks(|| Ok(()));
        let (session, config, dir) = test_session("ok");
        let guard = hooks.start().unwrap();
        session.busy.set(true);

        run_sync_job(
            slint::Weak::default(),
            session.clone(),
            config,
            FrontendHooks::default(),
            guard,
            "test",
            |collection, _, _| {
                assert!(collection.is_some());
                SyncOutcome::default()
            },
        );
        super::super::card_jobs::deliver_pending_for_test();

        assert_eq!(finished.get(), 1);
        assert!(!session.busy.get());
        assert!(session.collection.borrow().is_some());
        drop(session.collection.borrow_mut().take());
        let _ = std::fs::remove_dir_all(dir);
    }
}
