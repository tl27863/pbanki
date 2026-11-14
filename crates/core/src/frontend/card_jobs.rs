//! Collection jobs: card loading, rating, sync, and stats run on a worker
//! thread that owns the collection while the Slint event loop stays
//! responsive. `spawn_collection_job` is the runner; `spawn_card_job` is its
//! caller for cards.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anki::collection::Collection;
use slint::ComponentHandle;

use super::FrontendHooks;
use super::card_pages;
use super::models::{card_node, error_card};
use crate::profile::ProfileTimer;
use crate::profile_log;
use crate::session::{LearnSession, clear_current_card, reset_session_memory};
use crate::study::{CardContent, LoadedCard};
use crate::{DeckNode, MainWindow};

/// Delay before a deck entry shows its loading screen. Loads that finish
/// sooner (first entries take ~120-190 ms on the PB710, later ones a few ms)
/// go straight to the card with a single full present, instead of presenting
/// the loading screen and then waiting for the panel to finish it.
pub(super) const DECK_LOADING_DELAY: Duration = Duration::from_millis(200);

/// Delay before a rating shows the loading icon in the study top bar. A card
/// that arrives sooner costs no extra panel update.
pub(super) const RATING_LOADING_DELAY: Duration = Duration::from_millis(300);

thread_local! {
    /// The collection job whose result the UI thread is waiting for. There is
    /// at most one: `LearnSession.busy` blocks new jobs until it is delivered.
    static PENDING_JOB: RefCell<Option<PendingJob>> = const { RefCell::new(None) };
}

struct PendingJob {
    id: u64,
    /// Takes the result from the worker, puts the collection back, and runs
    /// the job's `finish` closure. Runs on the UI thread.
    deliver: Box<dyn FnOnce()>,
    #[cfg(test)]
    worker: std::thread::JoinHandle<()>,
}

/// Runs `operation` on a worker thread that owns the collection, then
/// `finish` on the UI thread.
///
/// The collection is moved out of the session, so `busy` stays set until the
/// result is delivered. The operation gets the `Option` itself: a full sync
/// closes the collection and opens it again, and the collection can be
/// missing after an earlier failure.
///
/// `finish` gets `None` if the worker stopped without a result (a panic). The
/// collection is then lost. Before `finish` runs, the runner puts the
/// collection back, clears `busy` (session and window), and increments
/// `job_seq`, so the timers of this job do not fire during a later one.
/// `finish` is dropped without a call only if the event loop has stopped.
///
/// Returns the job id, or `None` if another job is still running. Callers
/// check `session.busy` first.
pub(super) fn spawn_collection_job<T, Op, Fin>(
    ui: &slint::Weak<MainWindow>,
    session: &Rc<LearnSession>,
    operation: Op,
    finish: Fin,
) -> Option<u64>
where
    T: Send + 'static,
    Op: FnOnce(&mut Option<Collection>) -> T + Send + 'static,
    Fin: FnOnce(Option<T>) + 'static,
{
    if PENDING_JOB.with(|slot| slot.borrow().is_some()) {
        return None;
    }

    session.busy.set(true);
    if let Some(ui) = ui.upgrade() {
        ui.set_busy(true);
    }
    let job_id = session.job_seq.get().wrapping_add(1);
    session.job_seq.set(job_id);

    let mut collection = session.collection.borrow_mut().take();
    let (tx, rx) = mpsc::channel::<(Option<Collection>, T)>();

    let deliver = {
        let ui = ui.clone();
        let session = session.clone();
        Box::new(move || {
            let delivery_timer = ProfileTimer::start();
            tracing::info!("[job {job_id}] delivering result to UI");
            // A disconnected channel means that the worker panicked.
            let output = rx.try_recv().ok().map(|(collection, output)| {
                *session.collection.borrow_mut() = collection;
                output
            });
            session.busy.set(false);
            session.job_seq.set(session.job_seq.get().wrapping_add(1));
            if let Some(ui) = ui.upgrade() {
                ui.set_busy(false);
            }
            finish(output);
            profile_log!("[job {job_id}] UI delivery: {:?}", delivery_timer.elapsed());
        })
    };

    let queued_timer = ProfileTimer::start();
    let worker = std::thread::spawn(move || {
        // Declared before `tx`, so it is dropped after it: delivery always
        // finds either the result or a disconnected channel, also when
        // `operation` panics.
        let _deliver = DeliverOnDrop(job_id);
        let tx = tx;
        profile_log!(
            "[job {job_id}] worker start delay {:?}",
            queued_timer.elapsed()
        );
        let started = Instant::now();
        let output = operation(&mut collection);
        tracing::info!(
            "[job {job_id}] worker finished in {} ms",
            started.elapsed().as_millis()
        );
        let _ = tx.send((collection, output));
    });
    #[cfg(not(test))]
    drop(worker);

    PENDING_JOB.with(|slot| {
        *slot.borrow_mut() = Some(PendingJob {
            id: job_id,
            deliver,
            #[cfg(test)]
            worker,
        })
    });
    Some(job_id)
}

/// Hands a finished job to the UI thread through the Slint event loop.
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
    if let Some(job) = job {
        (job.deliver)();
    }
}

/// Waits for the worker of the pending job and delivers its result, as the
/// event loop does. Tests have no event loop.
#[cfg(test)]
pub(super) fn deliver_pending_for_test() {
    let Some(job) = PENDING_JOB.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    let _ = job.worker.join();
    (job.deliver)();
}

/// When and how a card job shows its loading state.
#[derive(Clone, Copy)]
pub(super) enum Loading {
    /// Screen transition (deck entry): the loading screen, if the job is still
    /// running after the delay, and the result each get a full present.
    Transition(Duration),
    /// Content update (rating): the loading icon in the top bar
    /// (`MainWindow.card-loading`) and the result use the normal damage path.
    /// The answered card stays visible.
    Deferred(Duration),
}

/// What `finish_job` needs on the UI thread.
struct CardJob {
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    deck: DeckNode,
    full_transition: bool,
    hooks: FrontendHooks,
}

/// Runs a card-loading operation on a worker thread
/// (`spawn_collection_job`). This keeps the Slint event loop free while Anki
/// builds its queues and renders the card.
pub(super) fn spawn_card_job<F>(
    ui: slint::Weak<MainWindow>,
    session: Rc<LearnSession>,
    hooks: FrontendHooks,
    deck: DeckNode,
    loading: Loading,
    operation: F,
) where
    F: FnOnce(&mut Collection) -> Result<LoadedCard, String> + Send + 'static,
{
    if session.busy.get() {
        return;
    }

    let full_transition = matches!(loading, Loading::Transition(_));
    let job = CardJob {
        ui: ui.clone(),
        session: session.clone(),
        deck,
        full_transition,
        hooks: hooks.clone(),
    };
    let Some(job_id) = spawn_collection_job(
        &ui,
        &session,
        move |collection| match collection {
            Some(collection) => operation(collection),
            None => Err("collection is unavailable".into()),
        },
        move |result| {
            let result = result
                .unwrap_or_else(|| Err("card loader stopped unexpectedly; restart pbAnki".into()));
            finish_job(&job, result);
        },
    ) else {
        return;
    };

    let (Loading::Transition(delay) | Loading::Deferred(delay)) = loading;
    {
        let ui = ui.clone();
        slint::Timer::single_shot(delay, move || {
            if session.busy.get() && session.job_seq.get() == job_id {
                if let Some(ui) = ui.upgrade() {
                    if full_transition {
                        // Deck entry switches to the study screen only now
                        // (or when the result arrives), so the previous
                        // session's card is never shown.
                        ui.set_loading(true);
                        ui.set_learning(true);
                        hooks.request_full_present(&ui);
                    } else {
                        ui.set_card_loading(true);
                        ui.window().request_redraw();
                    }
                }
            }
        });
    }
    if let Some(ui) = ui.upgrade() {
        ui.window().request_redraw();
    }
}

fn finish_job(job: &CardJob, result: Result<LoadedCard, String>) {
    let flash = full_refresh_due(&job.session, job.full_transition, result.is_ok());
    let card = match result {
        Ok(mut loaded) => {
            job.session.current_deck_id.set(Some(job.deck.id));
            log_lookahead(job, &loaded);
            let content =
                std::mem::replace(&mut loaded.content, CardContent::Message(String::new()));
            let (pages, message) = match content {
                CardContent::Pages(first) => {
                    let pages = (first.question_pages, first.answer_pages, first.answer_start);
                    card_pages::install(first);
                    (pages, String::new())
                }
                CardContent::Message(message) => {
                    card_pages::clear();
                    ((0, 0, 0), message)
                }
            };
            if loaded.id == -1 {
                clear_current_card(&job.session);
            } else {
                *job.session.current_card.borrow_mut() = Some(loaded.id);
                *job.session.states.borrow_mut() = loaded.states.clone();
                *job.session.start_time.borrow_mut() = Some(Instant::now());
            }
            card_node(loaded, job.deck.clone(), pages, message)
        }
        Err(err) => {
            tracing::error!("Failed to load card: {err}");
            job.session.predicted_next.borrow_mut().clear();
            // The deck selection may not have completed; select it again on
            // the next entry.
            reset_session_memory(&job.session);
            card_pages::clear();
            error_card(job.deck.clone(), &err)
        }
    };

    if let Some(ui) = job.ui.upgrade() {
        show_new_card(&ui, card);
        ui.set_loading(false);
        ui.set_card_loading(false);
        if job.full_transition {
            ui.set_learning(true);
            job.hooks.request_full_present(&ui);
        } else if flash {
            job.hooks.request_full_refresh(&ui);
        } else {
            ui.window().request_redraw();
        }
    }
}

/// Counts a rated card. True if the present of the next card must be the
/// flashing full refresh (`full_refresh_every`, Settings > Device). A deck
/// entry is a full present already, so it starts the count again.
fn full_refresh_due(session: &LearnSession, deck_entry: bool, loaded: bool) -> bool {
    if deck_entry {
        session.rated_since_refresh.set(0);
        return false;
    }
    if !loaded {
        return false;
    }
    let every = session.full_refresh_every.get();
    let rated = session.rated_since_refresh.get().saturating_add(1);
    if every > 0 && rated >= every {
        session.rated_since_refresh.set(0);
        true
    } else {
        session.rated_since_refresh.set(rated);
        false
    }
}

/// Logs whether a rating led to a card that the queue had predicted when the
/// previous card was loaded (`[card] lookahead`). This measures the hit rate
/// of a render-ahead cache before it exists
/// (notes/deep-research/card-page-cache.md). Deck entries only record the
/// prediction: the previous card belonged to another session.
fn log_lookahead(job: &CardJob, loaded: &LoadedCard) {
    let predicted = std::mem::replace(
        &mut *job.session.predicted_next.borrow_mut(),
        loaded.predicted_next.clone(),
    );
    if job.full_transition || loaded.id == -1 {
        return;
    }
    match predicted.iter().position(|&id| id == loaded.id) {
        Some(index) => tracing::info!(
            "[card] lookahead hit pos={} card={} kind={}",
            index + 1,
            loaded.id,
            loaded.queue_kind
        ),
        None => tracing::info!(
            "[card] lookahead miss predicted={predicted:?} card={} kind={}",
            loaded.id,
            loaded.queue_kind
        ),
    }
}

/// Shows a new card at question page 1. The page state is reset before the
/// card changes, so the first frame of the card already shows that page (a
/// `changed card` handler in Slint would run after it).
fn show_new_card(ui: &MainWindow, card: crate::CardNode) {
    ui.set_show_answer(false);
    ui.set_study_page(0);
    ui.set_current_card(card);
}
