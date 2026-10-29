use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anki::collection::Collection;
use slint::ComponentHandle;

use crate::api::{
    LoadedCard, error_card, full_sync, init_translations, next_card, rate_card, save_sync_settings,
    sync_now, sync_settings_from_config, sync_state_from_result, sync_state_from_status,
    update_deck_tree,
};
use crate::{
    Config, DeckNode, LearnSession, MainWindow, SyncAction, SyncResult, SyncState, SyncStatus,
};

/// How often the UI thread checks whether a background card load has finished.
const JOB_POLL_INTERVAL: Duration = Duration::from_millis(25);

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

struct PendingJob {
    id: u64,
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    deck: DeckNode,
    receiver: Receiver<(Collection, Result<LoadedCard, String>)>,
}

#[derive(Clone, Copy)]
enum Loading {
    Immediate,
    Deferred(Duration),
}

/// Runs a card-loading operation on a worker thread.
///
/// The collection is temporarily moved out of the session, so `busy` must stay
/// set until `finish_job()` puts it back. This keeps the Slint event loop free
/// while Anki builds its queues and renders the card.
fn spawn_card_job<F>(
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    deck: DeckNode,
    loading_message: &str,
    loading: Loading,
    operation: F,
) where
    F: FnOnce(&mut Collection) -> Result<LoadedCard, String> + Send + 'static,
{
    if session.busy.get() {
        return;
    }

    let Some(ui_ref) = ui.upgrade() else {
        return;
    };

    session.busy.set(true);
    ui_ref.set_busy(true);
    let job_id = session.job_seq.get().wrapping_add(1);
    session.job_seq.set(job_id);

    let Some(mut collection) = session.collection.borrow_mut().take() else {
        session.busy.set(false);
        *session.current_card.borrow_mut() = None;
        *session.states.borrow_mut() = None;
        *session.start_time.borrow_mut() = None;
        ui_ref.set_card_font(session.font_catalog.primary().into());
        ui_ref.set_current_card(error_card(deck, "collection is unavailable"));
        ui_ref.set_loading(false);
        ui_ref.set_busy(false);
        ui_ref.window().request_redraw();
        return;
    };

    match loading {
        Loading::Immediate => {
            ui_ref.set_loading_message(loading_message.into());
            ui_ref.set_loading(true);
        }
        Loading::Deferred(delay) => {
            let ui = ui.clone();
            let session = session.clone();
            let message = loading_message.to_string();
            slint::Timer::single_shot(delay, move || {
                if session.busy.get() && session.job_seq.get() == job_id {
                    if let Some(ui) = ui.upgrade() {
                        ui.set_loading_message(message.into());
                        ui.set_loading(true);
                        ui.window().request_redraw();
                    }
                }
            });
        }
    }
    ui_ref.window().request_redraw();
    drop(ui_ref);

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let started = Instant::now();
        let result = operation(&mut collection);
        eprintln!(
            "[job {job_id}] worker finished in {} ms",
            started.elapsed().as_millis()
        );
        let _ = tx.send((collection, result));
    });

    schedule_job_poll(Rc::new(PendingJob {
        id: job_id,
        ui,
        session,
        deck,
        receiver: rx,
    }));
}

fn schedule_job_poll(job: Rc<PendingJob>) {
    slint::Timer::single_shot(JOB_POLL_INTERVAL, move || poll_job(job));
}

fn poll_job(job: Rc<PendingJob>) {
    match job.receiver.try_recv() {
        Ok((collection, result)) => finish_job(&job, Some(collection), result),
        Err(TryRecvError::Empty) => schedule_job_poll(job),
        Err(TryRecvError::Disconnected) => finish_job(
            &job,
            None,
            Err("card loader stopped unexpectedly; restart pbAnki".into()),
        ),
    }
}

fn finish_job(
    job: &PendingJob,
    collection: Option<Collection>,
    result: Result<LoadedCard, String>,
) {
    eprintln!("[job {}] delivering result to UI", job.id);

    if let Some(collection) = collection {
        *job.session.collection.borrow_mut() = Some(collection);
    }

    let (card, font) = match result {
        Ok(loaded) => {
            job.session.current_deck_id.set(Some(job.deck.id));
            let font = loaded.font.clone();
            if loaded.id == -1 {
                *job.session.current_card.borrow_mut() = None;
                *job.session.states.borrow_mut() = None;
                *job.session.start_time.borrow_mut() = None;
            } else {
                *job.session.current_card.borrow_mut() = Some(loaded.id);
                *job.session.states.borrow_mut() = loaded.states.clone();
                *job.session.start_time.borrow_mut() = Some(Instant::now());
            }
            (loaded.into_card_node(job.deck.clone()), font)
        }
        Err(err) => {
            eprintln!("Failed to load card: {err}");
            *job.session.current_card.borrow_mut() = None;
            *job.session.states.borrow_mut() = None;
            *job.session.start_time.borrow_mut() = None;
            (
                error_card(job.deck.clone(), &err),
                job.session.font_catalog.primary().to_owned(),
            )
        }
    };

    job.session.busy.set(false);
    // Invalidate any deferred loading overlay this job may still have pending.
    job.session
        .job_seq
        .set(job.session.job_seq.get().wrapping_add(1));

    if let Some(ui) = job.ui.upgrade() {
        ui.set_card_font(font.into());
        ui.set_current_card(card);
        ui.set_loading(false);
        ui.set_busy(false);
        ui.window().request_redraw();
    }
}

pub fn init_ui(ui: &MainWindow, session: &LearnSession, config: &Config, native_keyboard: bool) {
    ui.set_native_keyboard(native_keyboard);
    ui.set_busy(false);
    ui.set_card_font(session.font_catalog.primary().into());
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
            if session.busy.get() {
                return;
            }
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
            if session.busy.get() {
                return;
            }
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
            if session.busy.get() {
                return;
            }
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_deck_tree(update_deck_tree(&session));
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        ui.on_rate(move |rating, deck, chars_per_page| {
            if session.busy.get() {
                return;
            }

            let card_id = session.current_card.borrow().unwrap_or(-1);
            let Some(states) = session.states.borrow().clone() else {
                return;
            };
            if card_id == -1 {
                return;
            }

            let elapsed = session
                .start_time
                .borrow()
                .map(|t| t.elapsed().as_millis() as u32)
                .unwrap_or(0);
            let deck_id = deck.id;
            let set_deck = session.current_deck_id.get() != Some(deck_id);
            let fonts = session.font_catalog.clone();

            spawn_card_job(
                ui_weak.clone(),
                session.clone(),
                deck,
                "Loading next card...",
                Loading::Deferred(Duration::from_millis(300)),
                move |col| {
                    rate_card(
                        col,
                        card_id,
                        states,
                        elapsed,
                        rating,
                        deck_id,
                        chars_per_page,
                        set_deck,
                        &fonts,
                    )
                },
            );
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        ui.on_deck_clicked(move |deck, chars_per_page| {
            if session.busy.get() {
                return;
            }

            let deck_id = deck.id;
            let set_deck = session.current_deck_id.get() != Some(deck_id);
            let fonts = session.font_catalog.clone();

            spawn_card_job(
                ui_weak.clone(),
                session.clone(),
                deck,
                "Loading deck...",
                Loading::Immediate,
                move |col| next_card(col, deck_id, chars_per_page, set_deck, &fonts),
            );
        });
    }
}

fn schedule_sync(
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: SyncHooks,
) {
    if session.busy.get() {
        return;
    }
    if !begin_sync(&ui, "Syncing...") {
        return;
    }

    session.busy.set(true);
    set_busy(&ui, true);
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

    if session.busy.get() {
        return;
    }
    if !begin_sync(&ui, message) {
        return;
    }

    session.busy.set(true);
    set_busy(&ui, true);
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

fn set_busy(ui: &slint::Weak<MainWindow>, busy: bool) {
    if let Some(ui) = ui.upgrade() {
        ui.set_busy(busy);
    }
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
                session.busy.set(false);
                if let Some(ui) = ui.upgrade() {
                    ui.set_sync_state(error_state(&err));
                    ui.set_busy(false);
                }
                return;
            }
        };

        let result = {
            let mut cfg = config.borrow_mut();
            operation(&session, &mut cfg)
        };

        session.busy.set(false);
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
        ui.set_busy(false);
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
