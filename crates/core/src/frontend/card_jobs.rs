//! Card jobs: loading and rating run on a worker thread that owns the
//! collection while the Slint event loop stays responsive.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anki::collection::Collection;
use slint::ComponentHandle;

use super::FrontendHooks;
use super::models::{card_node, error_card};
use crate::profile::ProfileTimer;
use crate::profile_log;
use crate::session::{LearnSession, clear_current_card, reset_session_memory};
use crate::study::LoadedCard;
use crate::{DeckNode, MainWindow};

/// Delay before a deck entry shows its loading screen. Loads that finish
/// sooner (first entries take ~120-190 ms on the PB710, later ones a few ms)
/// go straight to the card with a single full present, instead of presenting
/// the loading screen and then waiting for the panel to finish it.
pub(super) const DECK_LOADING_DELAY: Duration = Duration::from_millis(200);

/// Delay before a rating shows its loading overlay, so the `RatingButton`
/// that received the click is not removed inside its own click callback.
pub(super) const RATING_LOADING_DELAY: Duration = Duration::from_millis(300);

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
pub(super) enum Loading {
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
pub(super) fn spawn_card_job<F>(
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
        profile_log!(
            "[job {job_id}] worker start delay {:?}",
            queued_timer.elapsed()
        );
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
            (card_node(loaded, job.deck.clone()), font)
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
