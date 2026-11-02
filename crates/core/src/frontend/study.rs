//! The deck list and study screen: deck entry, rating, and returning Home.

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use super::FrontendHooks;
use super::card_jobs::{DECK_LOADING_DELAY, Loading, RATING_LOADING_DELAY, spawn_card_job};
use super::models::deck_tree;
use super::sync::schedule_sync;
use crate::MainWindow;
use crate::config::Config;
use crate::session::LearnSession;
use crate::study::{next_card, rate_card};

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
                ui.set_deck_tree(deck_tree(&session));
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
        let hooks = hooks.clone();
        ui.on_deck_clicked(move |deck, chars_per_page| {
            if session.busy.get() {
                return;
            }

            let deck_id = deck.id;
            let set_deck = session.current_deck_id.get() != Some(deck_id);
            let fonts = session.font_catalog.clone();
            tracing::info!("[deck {deck_id}] entering, set_deck={set_deck}");

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
