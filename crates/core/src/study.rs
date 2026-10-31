//! Card loading and rating. These functions only touch the collection, so
//! they run on the card-job worker thread (see `frontend::card_jobs`).

use anki::collection::Collection;
use anki::scheduler::answering::{CardAnswer, Rating};
use anki::scheduler::states::SchedulingStates;
use anki::timestamp::TimestampMillis;

use crate::fonts::FontCatalog;
use crate::profile::ProfileTimer;
use crate::profile_log;
use crate::text::{paginate_text, strip_html_remove_brackets_and_paginate};

/// A card loaded from the collection. This type contains no Slint models so it
/// can be produced on a worker thread and turned into a `CardNode` on the UI
/// thread.
pub struct LoadedCard {
    pub id: i64,
    pub new_count: i32,
    pub learn_count: i32,
    pub due_count: i32,
    pub question: Vec<slint::SharedString>,
    pub answer: Vec<slint::SharedString>,
    pub durations: Vec<slint::SharedString>,
    pub states: Option<SchedulingStates>,
    pub font: String,
}

impl LoadedCard {
    fn no_cards(new_count: i32, learn_count: i32, due_count: i32, font: &str) -> Self {
        Self {
            id: -1,
            new_count,
            learn_count,
            due_count,
            question: vec!["No more cards due!".into()],
            answer: Vec::new(),
            durations: Vec::new(),
            states: None,
            font: font.to_owned(),
        }
    }
}

/// Fetches and renders the next card. Runs on a worker thread; `set_deck`
/// should only be true when the collection's current deck differs from
/// `deck_id`, so that Anki's card queue is not invalidated on every card.
pub fn next_card(
    col: &mut Collection,
    deck_id: i64,
    chars_per_page: i32,
    set_deck: bool,
    fonts: &FontCatalog,
) -> Result<LoadedCard, String> {
    let total_timer = ProfileTimer::start();
    let set_deck_timer = ProfileTimer::start();
    if set_deck {
        col.set_current_deck(anki::decks::DeckId(deck_id))
            .map_err(|err| format!("could not open deck: {err}"))?;
    }
    let set_deck_elapsed = set_deck_timer.elapsed();

    let queue_timer = ProfileTimer::start();
    let queued_cards = col
        .get_queued_cards(1, false)
        .map_err(|err| format!("could not load the card queue: {err}"))?;
    let queue_elapsed = queue_timer.elapsed();
    let new_count = queued_cards.new_count as i32;
    let learn_count = queued_cards.learning_count as i32;
    let due_count = queued_cards.review_count as i32;
    let Some(card) = queued_cards.cards.first() else {
        profile_log!(
            "next card empty: set_deck={set_deck_elapsed:?} queue={queue_elapsed:?} total={:?}",
            total_timer.elapsed()
        );
        return Ok(LoadedCard::no_cards(
            new_count,
            learn_count,
            due_count,
            fonts.primary(),
        ));
    };

    let states_timer = ProfileTimer::start();
    let durations = col
        .describe_next_states(&card.states)
        .unwrap_or_else(|_| vec![String::new(); 4]);
    let states_elapsed = states_timer.elapsed();

    let render_timer = ProfileTimer::start();
    let rendered = col
        .render_existing_card(card.card.id(), false, false)
        .map_err(|err| format!("could not render card: {err}"))?;
    let render_elapsed = render_timer.elapsed();

    let text_timer = ProfileTimer::start();
    let mut answer = anki::text::strip_html(rendered.answer().as_ref()).into_owned();

    // Handle special case for type in cards
    if answer.contains("[[type:") {
        let note_opt = col
            .storage
            .get_note(card.card.note_id())
            .map_err(|err| format!("could not load note: {err}"))?;

        if let Some(note) = note_opt {
            let fields = note.fields();
            let notetype_id = note.notetype_id;
            let notetype = col
                .get_notetype(notetype_id)
                .map_err(|err| format!("could not load notetype: {err}"))?;

            if let Some(notetype) = notetype {
                for (index, field) in notetype.fields.iter().enumerate() {
                    if field.name == "Back" {
                        if let Some(value) = fields.get(index) {
                            answer = value.to_owned();
                        }
                    }
                }
            }
        }
    }

    let question_html = rendered.question().into_owned();
    let question_text = anki::text::strip_html(&question_html);
    let combined = format!("{question_text}{answer}");
    let text_elapsed = text_timer.elapsed();

    let font_timer = ProfileTimer::start();
    let font_choice = fonts.choose_font(&combined);
    let font_elapsed = font_timer.elapsed();
    if !font_choice.missing.is_empty() {
        eprintln!(
            "[font] card {}: {} character(s) not covered by '{}': {:?}",
            card.card.id().0,
            font_choice.missing.len(),
            font_choice.family,
            font_choice.missing
        );
    }
    let pages_timer = ProfileTimer::start();
    let answer_text = fonts.sanitize(&answer, &font_choice.family);

    let chars_per_page = chars_per_page.max(1) as usize;
    // Sanitize after stripping: replacing a missing character inside HTML
    // markup would corrupt tags, and coverage was checked on the visible text.
    let question = strip_html_remove_brackets_and_paginate(&question_html, true, chars_per_page)
        .into_iter()
        .map(|page| slint::SharedString::from(fonts.sanitize(&page, &font_choice.family)))
        .collect();
    let answer = paginate_text(&answer_text, chars_per_page);
    let pages_elapsed = pages_timer.elapsed();
    let total_elapsed = total_timer.elapsed();

    profile_log!(
        "card {} load: set_deck={set_deck_elapsed:?} queue={queue_elapsed:?} \
         states={states_elapsed:?} render={render_elapsed:?} text={text_elapsed:?} \
         font={font_elapsed:?} pages={pages_elapsed:?} total={total_elapsed:?}",
        card.card.id().0
    );

    Ok(LoadedCard {
        id: card.card.id().0,
        new_count,
        learn_count,
        due_count,
        question,
        answer,
        durations: durations.into_iter().map(Into::into).collect(),
        states: Some(card.states.clone()),
        font: font_choice.family,
    })
}

/// Applies an answer and returns the next card. Runs on a worker thread.
#[allow(clippy::too_many_arguments)]
pub fn rate_card(
    col: &mut Collection,
    card_id: i64,
    states: SchedulingStates,
    elapsed: u32,
    rating: i32,
    deck_id: i64,
    chars_per_page: i32,
    set_deck: bool,
    fonts: &FontCatalog,
) -> Result<LoadedCard, String> {
    let (new_state, rating_enum) = match rating {
        0 => (states.again.clone(), Rating::Again),
        1 => (states.hard.clone(), Rating::Hard),
        2 => (states.good.clone(), Rating::Good),
        3 => (states.easy.clone(), Rating::Easy),
        _ => return Err("invalid rating".into()),
    };

    let mut answer = CardAnswer {
        card_id: anki::card::CardId(card_id),
        current_state: states.current.clone(),
        new_state,
        rating: rating_enum,
        answered_at: TimestampMillis::now(),
        milliseconds_taken: elapsed,
        custom_data: None,
        from_queue: true,
    };

    let answer_timer = ProfileTimer::start();
    let answer_result = col.answer_card(&mut answer);
    profile_log!(
        "card {card_id} answer rating={rating}: {:?}",
        answer_timer.elapsed()
    );
    if let Err(err) = answer_result {
        // Loading the next card anyway would show the unchanged queue head as
        // if it were new, silently dropping the rating.
        eprintln!("Failed to answer card {card_id}: {err:?}");
        return Err(format!("could not save the rating: {err}"));
    }

    next_card(col, deck_id, chars_per_page, set_deck, fonts)
}

#[allow(dead_code)]
fn assert_worker_types_are_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Collection>();
    assert_send::<SchedulingStates>();
    assert_send::<LoadedCard>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    use anki::collection::CollectionBuilder;
    use anki::scheduler::states::{CardState, NewState};

    fn temp_collection(name: &str) -> (PathBuf, Collection) {
        let dir =
            std::env::temp_dir().join(format!("pbanki-api-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let col = CollectionBuilder::new(dir.join("collection.anki2"))
            .build()
            .unwrap();
        (dir, col)
    }

    fn new_states() -> SchedulingStates {
        let new: CardState = NewState { position: 0 }.into();
        SchedulingStates {
            current: new,
            again: new,
            hard: new,
            good: new,
            easy: new,
        }
    }

    /// A rating that cannot be saved must surface as an error instead of loading
    /// the unchanged queue head as if it were the next card.
    #[test]
    fn rate_card_reports_failed_answer() {
        let (dir, mut col) = temp_collection("rate");
        let fonts = FontCatalog::build("none", &[], &[]);

        let result = rate_card(&mut col, 424242, new_states(), 0, 2, 1, 100, true, &fonts);

        let err = result.err().expect("answering a missing card must fail");
        assert!(err.starts_with("could not save the rating"), "{err}");
        let reviews: u32 = col
            .storage
            .db()
            .query_row("select count() from revlog", [], |row| row.get(0))
            .unwrap();
        assert_eq!(reviews, 0);

        drop(col);
        let _ = fs::remove_dir_all(&dir);
    }
}
