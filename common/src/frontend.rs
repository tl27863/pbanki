use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anki::collection::Collection;
use slint::ComponentHandle;

use crate::api::{
    LoadedCard, clear_current_card, error_card, reset_session_memory, full_sync, init_translations, next_card, rate_card, save_sync_settings,
    sync_now, sync_settings_from_config, sync_state_from_result, sync_state_from_status,
    update_deck_tree,
};
use crate::profile::ProfileTimer;
use crate::profile_log;
use crate::{
    Config, DeckNode, LearnSession, MainWindow, SyncAction, SyncResult, SyncState, SyncStatus,
};

/// Delay before a deck entry shows its loading screen. Loads that finish
/// sooner (first entries take ~120-190 ms on the PB710, later ones a few ms)
/// go straight to the card with a single full present, instead of presenting
/// the loading screen and then waiting for the panel to finish it.
const DECK_LOADING_DELAY: Duration = Duration::from_millis(200);

/// Delay before a rating shows its loading overlay, so the `RatingButton`
/// that received the click is not removed inside its own click callback.
const RATING_LOADING_DELAY: Duration = Duration::from_millis(300);

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

thread_local! {
    /// The card job whose result the UI thread is waiting for. There is at
    /// most one: `LearnSession.busy` blocks new jobs until `finish_job`.
    static PENDING_JOB: RefCell<Option<PendingJob>> = const { RefCell::new(None) };
}

struct PendingJob {
    id: u64,
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    deck: DeckNode,
    receiver: Receiver<(Collection, Result<LoadedCard, String>)>,
    full_transition: bool,
    hooks: FrontendHooks,
}

/// When and how a card job shows its loading state.
#[derive(Clone, Copy)]
enum Loading {
    /// Screen transition (deck entry): the loading screen, if the job is still
    /// running after the delay, and the result each get a full present.
    Transition(Duration),
    /// Content update (rating): the overlay and result use the normal damage
    /// path.
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
    hooks: FrontendHooks,
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

    let full_transition = matches!(loading, Loading::Transition(_));
    let Some(ui_ref) = ui.upgrade() else {
        return;
    };

    session.busy.set(true);
    ui_ref.set_busy(true);
    let job_id = session.job_seq.get().wrapping_add(1);
    session.job_seq.set(job_id);

    let Some(mut collection) = session.collection.borrow_mut().take() else {
        session.busy.set(false);
        reset_session_memory(&session);
        ui_ref.set_card_font(session.font_catalog.primary().into());
        ui_ref.set_current_card(error_card(deck, "collection is unavailable"));
        ui_ref.set_loading(false);
        ui_ref.set_busy(false);
        if full_transition {
            ui_ref.set_learning(true);
            hooks.request_full_present(&ui_ref);
        } else {
            ui_ref.window().request_redraw();
        }
        return;
    };

    let (Loading::Transition(delay) | Loading::Deferred(delay)) = loading;
    {
        let ui = ui.clone();
        let session = session.clone();
        let hooks = hooks.clone();
        let message = loading_message.to_string();
        slint::Timer::single_shot(delay, move || {
            if session.busy.get() && session.job_seq.get() == job_id {
                if let Some(ui) = ui.upgrade() {
                    ui.set_loading_message(message.into());
                    ui.set_loading(true);
                    if full_transition {
                        // Deck entry switches to the study screen only now
                        // (or when the result arrives), so the previous
                        // session's card is never shown.
                        ui.set_learning(true);
                        hooks.request_full_present(&ui);
                    } else {
                        ui.window().request_redraw();
                    }
                }
            }
        });
    }
    ui_ref.window().request_redraw();
    drop(ui_ref);

    let (tx, rx) = mpsc::channel();
    PENDING_JOB.with(|slot| {
        *slot.borrow_mut() = Some(PendingJob {
            id: job_id,
            ui,
            session,
            deck,
            receiver: rx,
            full_transition,
            hooks,
        })
    });

    let queued_timer = ProfileTimer::start();
    std::thread::spawn(move || {
        // Declared before `tx`, so it is dropped after it: delivery always
        // finds either the result or a disconnected channel, also when
        // `operation` panics.
        let _deliver = DeliverOnDrop(job_id);
        let tx = tx;
        profile_log!("[job {job_id}] worker start delay {:?}", queued_timer.elapsed());
        let started = Instant::now();
        let result = operation(&mut collection);
        eprintln!(
            "[job {job_id}] worker finished in {} ms",
            started.elapsed().as_millis()
        );
        let _ = tx.send((collection, result));
    });
}

/// Hands a finished card job to the UI thread through the Slint event loop.
struct DeliverOnDrop(u64);

impl Drop for DeliverOnDrop {
    fn drop(&mut self) {
        let job_id = self.0;
        // Fails only once the event loop has exited, when nobody is waiting.
        let _ = slint::invoke_from_event_loop(move || deliver_job(job_id));
    }
}

fn deliver_job(job_id: u64) {
    let job = PENDING_JOB.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|job| job.id == job_id) {
            slot.take()
        } else {
            None
        }
    });
    let Some(job) = job else {
        return;
    };
    match job.receiver.try_recv() {
        Ok((collection, result)) => finish_job(&job, Some(collection), result),
        Err(_) => finish_job(
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
    let delivery_timer = ProfileTimer::start();
    eprintln!("[job {}] delivering result to UI", job.id);

    if let Some(collection) = collection {
        *job.session.collection.borrow_mut() = Some(collection);
    }

    let (card, font) = match result {
        Ok(loaded) => {
            job.session.current_deck_id.set(Some(job.deck.id));
            let font = loaded.font.clone();
            if loaded.id == -1 {
                clear_current_card(&job.session);
            } else {
                *job.session.current_card.borrow_mut() = Some(loaded.id);
                *job.session.states.borrow_mut() = loaded.states.clone();
                *job.session.start_time.borrow_mut() = Some(Instant::now());
            }
            (loaded.into_card_node(job.deck.clone()), font)
        }
        Err(err) => {
            eprintln!("Failed to load card: {err}");
            // The deck selection may not have completed; select it again on
            // the next entry.
            reset_session_memory(&job.session);
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
        if job.full_transition {
            ui.set_learning(true);
            job.hooks.request_full_present(&ui);
        } else {
            ui.window().request_redraw();
        }
    }

    profile_log!(
        "[job {}] UI delivery: {:?}",
        job.id,
        delivery_timer.elapsed()
    );
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
    hooks: FrontendHooks,
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
    hooks: FrontendHooks,
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
                let was_open = ui.get_settings_open();
                ui.set_sync_settings(sync_settings_from_config(&config.borrow()));
                ui.set_settings_open(true);
                if !was_open {
                    hooks.request_full_present(&ui);
                }
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
                ui.set_deck_tree(update_deck_tree(&session));
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
            if let Some(ui) = ui_weak.upgrade() {
                hooks.request_full_present(&ui);
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
        let hooks = hooks.clone();
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
                hooks.clone(),
                deck,
                "Loading next card...",
                Loading::Deferred(RATING_LOADING_DELAY),
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
            eprintln!("[deck {deck_id}] entering, set_deck={set_deck}");

            spawn_card_job(
                ui_weak.clone(),
                session.clone(),
                hooks.clone(),
                deck,
                "Loading deck...",
                Loading::Transition(DECK_LOADING_DELAY),
                move |col| next_card(col, deck_id, chars_per_page, set_deck, &fonts),
            );
        });
    }
}

fn schedule_sync(
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: FrontendHooks,
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
    hooks: FrontendHooks,
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
    hooks: FrontendHooks,
    operation: F,
) where
    F: FnOnce(&LearnSession, &mut Config) -> SyncResult + 'static,
{
    slint::Timer::single_shot(Duration::from_millis(10), move || {
        let guard = match hooks.sync.clone().start() {
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
        // Any sync, even a failed one, may have changed the collection's
        // current deck. Syncs only start outside a study session, so the card
        // state is not in use.
        reset_session_memory(&session);

        session.busy.set(false);
        apply_sync_result(ui, &session, &config, &hooks, &result);
        drop(guard);
    });
}

fn apply_sync_result(
    ui: slint::Weak<MainWindow>,
    session: &LearnSession,
    config: &Rc<RefCell<Config>>,
    hooks: &FrontendHooks,
    result: &SyncResult,
) {
    if let Some(ui) = ui.upgrade() {
        ui.set_sync_state(sync_state_from_result(result));
        ui.set_busy(false);
        if matches!(result.action, SyncAction::FullSyncRequired { .. }) {
            let was_open = ui.get_settings_open();
            ui.set_sync_settings(sync_settings_from_config(&config.borrow()));
            ui.set_settings_open(true);
            if !was_open {
                hooks.request_full_present(&ui);
            }
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
