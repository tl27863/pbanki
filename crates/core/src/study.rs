//! Card loading and rating. These functions only touch the collection, so
//! they run on the card-job worker thread (see `frontend::card_jobs`).

use anki::collection::Collection;
use anki::scheduler::answering::{CardAnswer, Rating};
use anki::scheduler::states::SchedulingStates;
use anki::timestamp::TimestampMillis;

use anki::card_rendering::extract_av_tags;
use anki_proto::card_rendering::AvTag;

use crate::card_render::html::{card_document, replace_av_placeholders, replace_type_tags};
use crate::card_render::{CardHtml, CardRenderer, FirstPages, RenderArea};
use crate::fonts::FontCatalog;
use crate::profile::ProfileTimer;
use crate::profile_log;

/// What the card area shows.
pub enum CardContent {
    /// Rendered pages (notes/blitz-card-render.md).
    Pages(FirstPages),
    /// A text instead of pages: no card due, or the render failed.
    Message(String),
}

/// A card loaded from the collection. This type contains no Slint models so it
/// can be produced on a worker thread and turned into a `CardNode` on the UI
/// thread.
pub struct LoadedCard {
    pub id: i64,
    pub new_count: i32,
    pub learn_count: i32,
    pub due_count: i32,
    pub content: CardContent,
    pub durations: Vec<slint::SharedString>,
    pub states: Option<SchedulingStates>,
    /// Audio and TTS tags of each side, in placeholder order. Not played yet:
    /// kept for audio support (notes/blitz-card-render.md, "Card HTML").
    pub question_av: Vec<AvTag>,
    pub answer_av: Vec<AvTag>,
}

impl LoadedCard {
    fn no_cards(new_count: i32, learn_count: i32, due_count: i32) -> Self {
        Self {
            id: -1,
            new_count,
            learn_count,
            due_count,
            content: CardContent::Message("No more cards due!".into()),
            durations: Vec::new(),
            states: None,
            question_av: Vec::new(),
            answer_av: Vec::new(),
        }
    }
}

/// What a card job needs to render pages.
#[derive(Clone)]
pub struct RenderContext {
    pub fonts: FontCatalog,
    pub renderer: CardRenderer,
    pub area: RenderArea,
}

/// Fetches and renders the next card. Runs on a worker thread; `set_deck`
/// should only be true when the collection's current deck differs from
/// `deck_id`, so that Anki's card queue is not invalidated on every card.
pub fn next_card(
    col: &mut Collection,
    deck_id: i64,
    set_deck: bool,
    render: &RenderContext,
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
        return Ok(LoadedCard::no_cards(new_count, learn_count, due_count));
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
    let tr = col.tr().clone();
    let (question_html, question_av) = extract_av_tags(rendered.question().into_owned(), true, &tr);
    let (answer_html, answer_av) = extract_av_tags(rendered.answer().into_owned(), false, &tr);
    let type_fields = if answer_html.contains("[[type:") {
        type_in_fields(col, card.card.note_id())?
    } else {
        Vec::new()
    };
    let question_html = replace_av_placeholders(&replace_type_tags(&question_html, |_| None));
    let answer_html = replace_av_placeholders(&replace_type_tags(&answer_html, |name| {
        type_fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
    }));
    let combined = format!(
        "{}{}",
        anki::text::strip_html(&question_html),
        anki::text::strip_html(&answer_html)
    );
    let text_elapsed = text_timer.elapsed();

    let font_timer = ProfileTimer::start();
    let font_choice = render.fonts.choose_font(&combined);
    let font_elapsed = font_timer.elapsed();
    if !font_choice.missing.is_empty() {
        tracing::warn!(
            "[font] card {}: {} character(s) not covered by '{}': {:?}",
            card.card.id().0,
            font_choice.missing.len(),
            font_choice.family,
            font_choice.missing
        );
    }

    let pages_timer = ProfileTimer::start();
    let area = render.area;
    let page_height_css = area.height as f32 / area.scale;
    let ord = card.card.template_idx();
    let html = CardHtml {
        question: card_document(&question_html, &rendered.css, ord, page_height_css),
        answer: card_document(&answer_html, &rendered.css, ord, page_height_css),
        font_family: font_choice.family,
    };
    let content = match render.renderer.render(html, area) {
        Ok(pages) => CardContent::Pages(pages),
        Err(err) => {
            tracing::error!("[card] card {} render failed: {err}", card.card.id().0);
            CardContent::Message(format!("Could not render this card: {err}"))
        }
    };
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
        content,
        durations: durations.into_iter().map(Into::into).collect(),
        states: Some(card.states.clone()),
        question_av,
        answer_av,
    })
}

/// `(field name, field HTML)` of the card's note, for `[[type:Field]]` tags.
fn type_in_fields(
    col: &mut Collection,
    note_id: anki::notes::NoteId,
) -> Result<Vec<(String, String)>, String> {
    let note = col
        .storage
        .get_note(note_id)
        .map_err(|err| format!("could not load note: {err}"))?;
    let Some(note) = note else {
        return Ok(Vec::new());
    };
    let notetype = col
        .get_notetype(note.notetype_id)
        .map_err(|err| format!("could not load notetype: {err}"))?;
    let Some(notetype) = notetype else {
        return Ok(Vec::new());
    };
    Ok(notetype
        .fields
        .iter()
        .zip(note.fields())
        .map(|(field, value)| (field.name.clone(), value.clone()))
        .collect())
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
    set_deck: bool,
    render: &RenderContext,
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
        tracing::error!("Failed to answer card {card_id}: {err:?}");
        return Err(format!("could not save the rating: {err}"));
    }

    next_card(col, deck_id, set_deck, render)
}

#[allow(dead_code)]
fn assert_worker_types_are_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Collection>();
    assert_send::<SchedulingStates>();
    assert_send::<LoadedCard>();
    assert_send::<RenderContext>();
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
        let render = RenderContext {
            fonts: FontCatalog::build("none", &[], &[]),
            renderer: CardRenderer::spawn(
                crate::card_render::RendererConfig {
                    format: crate::card_render::PageFormat::Gray8,
                    font_files: Vec::new(),
                    font_families: Vec::new(),
                    media_folder: dir.clone(),
                },
                Box::new(|_| {}),
            ),
            area: RenderArea {
                width: 100,
                height: 100,
                scale: 1.0,
            },
        };

        let result = rate_card(&mut col, 424242, new_states(), 0, 2, 1, true, &render);

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
