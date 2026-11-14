//! The deck list and study screen: deck entry, rating, and returning Home.

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use super::FrontendHooks;
use super::card_jobs::{DECK_LOADING_DELAY, Loading, RATING_LOADING_DELAY, spawn_card_job};
use super::decks;
use super::sync::{SyncTrigger, end_finished_sync, schedule_sync};
use crate::MainWindow;
use crate::card_render::{CardRenderer, RenderArea};
use crate::config::Config;
use crate::keys::{HwKey, Resolved, Side, resolve};
use crate::session::LearnSession;
use crate::study::{RenderContext, next_card, rate_card};

/// What a card job needs to render pages for the current window.
fn render_context(
    ui: &MainWindow,
    session: &LearnSession,
    renderer: &CardRenderer,
    shadows: bool,
) -> RenderContext {
    let scale = ui.window().scale_factor();
    // Round down, so the page never is larger than the area.
    let device = |logical: f32| (logical * scale).floor().max(1.0) as u32;
    RenderContext {
        fonts: session.font_catalog.clone(),
        renderer: renderer.clone(),
        area: RenderArea {
            width: device(ui.get_card_area_width()),
            height: device(ui.get_card_area_height()),
            scale,
        },
        shadows,
    }
}

pub(super) fn wire(
    ui: &MainWindow,
    session: &Rc<LearnSession>,
    config: &Rc<RefCell<Config>>,
    hooks: &FrontendHooks,
    renderer: &CardRenderer,
) {
    // Config file only (no settings UI), so it is read once.
    let shadows = config.borrow().general.card_shadows;
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
                end_finished_sync(&ui);
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
                    SyncTrigger::LeaveStudy,
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
                decks::reload(&ui, &session);
            }
        });
    }

    let rate: Rc<dyn Fn(i32)> = {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        let hooks = hooks.clone();
        let renderer = renderer.clone();
        Rc::new(move |rating| {
            if session.busy.get() {
                return;
            }
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };

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
            let deck = ui.get_current_card().deck;
            let deck_id = deck.id;
            let set_deck = session.current_deck_id.get() != Some(deck_id);
            let render = render_context(&ui, &session, &renderer, shadows);

            spawn_card_job(
                ui_weak.clone(),
                session.clone(),
                hooks.clone(),
                deck,
                Loading::Deferred(RATING_LOADING_DELAY),
                move |col| {
                    rate_card(
                        col, card_id, states, elapsed, rating, deck_id, set_deck, &render,
                    )
                },
            );
        })
    };

    {
        let rate = rate.clone();
        ui.on_rate(move |rating, _deck| rate(rating));
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        ui.on_study_key(move |key| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let card = ui.get_current_card();
            // No keys while a job runs, on the loader, or on a message card
            // without a deck (an error or "no cards").
            if session.busy.get() || ui.get_loading() || card.id == -1 {
                return;
            }
            let answer = ui.get_show_answer();
            let pages = if answer {
                card.answer_pages
            } else {
                card.question_pages
            };
            let resolved = resolve(
                match key {
                    crate::HwKey::Prev => HwKey::Prev,
                    crate::HwKey::Next => HwKey::Next,
                },
                if answer { Side::Answer } else { Side::Question },
                ui.get_study_page(),
                pages.max(1),
                &session.key_config.get(),
            );
            match resolved {
                Resolved::PagePrev => ui.set_study_page(ui.get_study_page() - 1),
                Resolved::PageNext => ui.set_study_page(ui.get_study_page() + 1),
                Resolved::Reveal => {
                    // As `reveal()` in `learn.slint`.
                    ui.set_study_page(card.answer_start);
                    ui.set_show_answer(true);
                }
                Resolved::Rate(rating) => rate(rating),
                Resolved::None => {}
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        let hooks = hooks.clone();
        let renderer = renderer.clone();
        ui.on_deck_clicked(move |deck| {
            if session.busy.get() {
                return;
            }
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };

            let deck_id = deck.id;
            let set_deck = session.current_deck_id.get() != Some(deck_id);
            let render = render_context(&ui, &session, &renderer, shadows);
            tracing::info!("[deck {deck_id}] entering, set_deck={set_deck}");
            end_finished_sync(&ui);

            spawn_card_job(
                ui_weak.clone(),
                session.clone(),
                hooks.clone(),
                deck,
                Loading::Transition(DECK_LOADING_DELAY),
                move |col| next_card(col, deck_id, set_deck, &render),
            );
        });
    }
}
