//! Card loading and rating. These functions only touch the collection, so
//! they run on the card-job worker thread (see `frontend::card_jobs`).

use anki::card::Card;
use anki::collection::Collection;
use anki::scheduler::answering::{CardAnswer, Rating};
use anki::scheduler::states::SchedulingStates;
use anki::timestamp::TimestampMillis;

use anki::card_rendering::extract_av_tags;
use anki::template::RenderedNode;
use anki_proto::card_rendering::AvTag;

use crate::card_render::html::{card_document, replace_av_placeholders, replace_type_tags};
use crate::card_render::{CardHtml, CardRenderer, FirstPages, PendingRender, RenderArea};
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
    /// Audio and TTS tags of each side, in placeholder order (`audio`,
    /// notes/audio.md). The answer's are only its own, not the question's.
    pub question_av: Vec<AvTag>,
    pub answer_av: Vec<AvTag>,
    /// The audio options of the card's deck.
    pub audio: AudioOptions,
    /// The queue kind of this card (`New`, `Review`, `Learning`), for logs.
    /// (rslib does not export the enum.)
    pub queue_kind: String,
    /// The cards after this one in the queue when it was loaded: the cards a
    /// render-ahead cache would prepare (notes/deep-research/card-page-cache.md).
    pub predicted_next: Vec<i64>,
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
            audio: AudioOptions::default(),
            queue_kind: String::new(),
            predicted_next: Vec::new(),
        }
    }
}

/// The audio options of a deck preset (Anki's deck options, "Audio").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioOptions {
    /// Play a side's sounds when it shows ("Don't play audio automatically"
    /// off).
    pub autoplay: bool,
    /// Replay on the answer side plays the question's sounds first ("Skip
    /// question when replaying answer" off).
    pub replay_question: bool,
}

impl Default for AudioOptions {
    fn default() -> Self {
        Self {
            autoplay: true,
            replay_question: true,
        }
    }
}

/// The audio options of the card's home deck (the original deck of a card in
/// a filtered deck). Defaults if the deck or preset is missing.
fn audio_options(col: &mut Collection, card: &Card) -> AudioOptions {
    let proto: anki_proto::cards::Card = card.clone().into();
    let home = if proto.original_deck_id != 0 {
        proto.original_deck_id
    } else {
        proto.deck_id
    };
    let config = col
        .get_deck(anki::decks::DeckId(home))
        .ok()
        .flatten()
        .and_then(|deck| deck.config_id())
        .and_then(|id| col.get_deck_config(id, true).ok().flatten());
    match config {
        Some(config) => AudioOptions {
            autoplay: !config.inner.disable_autoplay,
            replay_question: !config.inner.skip_question_when_replaying_answer,
        },
        None => AudioOptions::default(),
    }
}

/// Cards fetched from the queue: the current one and the next two, which
/// `LoadedCard::predicted_next` reports.
const QUEUE_FETCH: usize = 3;

/// What a card job needs to render pages.
#[derive(Clone)]
pub struct RenderContext {
    pub fonts: FontCatalog,
    pub renderer: CardRenderer,
    pub area: RenderArea,
    /// `[general] card_shadows` of the config.
    pub shadows: bool,
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
        .get_queued_cards(QUEUE_FETCH, false)
        .map_err(|err| format!("could not load the card queue: {err}"))?;
    let queue_elapsed = queue_timer.elapsed();
    let new_count = queued_cards.new_count as i32;
    let learn_count = queued_cards.learning_count as i32;
    let due_count = queued_cards.review_count as i32;
    if set_deck {
        // A deck entry after a sync also comes here (`reset_session_memory`
        // clears the deck): pages rendered ahead before it could show media
        // that the sync replaced.
        render.renderer.lookahead(Vec::new(), render.area);
    }
    let Some(card) = queued_cards.cards.first() else {
        render.renderer.lookahead(Vec::new(), render.area);
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

    let html_timer = ProfileTimer::start();
    let documents = card_documents(col, &card.card, &render.fonts, render.area, render.shadows)?;
    let html_elapsed = html_timer.elapsed();

    let pages_timer = ProfileTimer::start();
    let pending = render.renderer.start_render(documents.html, render.area);
    // While the render thread renders this card, build the documents of the
    // next cards in the queue, so it can render them ahead
    // (notes/deep-research/card-page-cache.md, B).
    let ahead_timer = ProfileTimer::start();
    let ahead = queued_cards.cards[1..]
        .iter()
        .filter_map(|next| {
            card_documents(col, &next.card, &render.fonts, render.area, render.shadows)
                .map_err(|err| tracing::warn!("[card] lookahead card {}: {err}", next.card.id().0))
                .ok()
                .map(|documents| documents.html)
        })
        .collect();
    render.renderer.lookahead(ahead, render.area);
    let ahead_elapsed = ahead_timer.elapsed();
    let content = match pending.and_then(PendingRender::wait) {
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
         states={states_elapsed:?} html={html_elapsed:?} pages={pages_elapsed:?} \
         (lookahead html {ahead_elapsed:?}) \
         total={total_elapsed:?}",
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
        question_av: documents.question_av,
        answer_av: documents.answer_av,
        audio: audio_options(col, &card.card),
        queue_kind: format!("{:?}", card.kind),
        predicted_next: queued_cards.cards[1..]
            .iter()
            .map(|next| next.card.id().0)
            .collect(),
    })
}

/// The documents of one card for `CardRenderer::render`, and its audio tags.
pub struct CardDocuments {
    pub html: CardHtml,
    pub question_av: Vec<AvTag>,
    pub answer_av: Vec<AvTag>,
}

/// Renders the card templates and builds both documents. This is Anki's
/// template render only, no layout. The host render harness
/// (`examples/render_deck.rs`) uses it too.
pub fn card_documents(
    col: &mut Collection,
    card: &Card,
    fonts: &FontCatalog,
    area: RenderArea,
    shadows: bool,
) -> Result<CardDocuments, String> {
    // Partial render, as Anki desktop: `{{FrontSide}}` is filled here with
    // the question after its AV tags became `[anki:play:q:N]` references, so
    // the answer's tags are only its own and Show answer does not play the
    // question's sounds again.
    let rendered = col
        .render_existing_card(card.id(), false, true)
        .map_err(|err| format!("could not render card: {err}"))?;

    let tr = col.tr().clone();
    let (question_html, question_av) =
        extract_av_tags(rendered_text(&rendered.qnodes, ""), true, &tr);
    let (answer_html, answer_av) =
        extract_av_tags(rendered_text(&rendered.anodes, &question_html), false, &tr);
    let type_fields = if answer_html.contains("[[type:") {
        type_in_fields(col, card.note_id())?
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

    let font_choice = fonts.choose_font(&combined);
    if !font_choice.missing.is_empty() {
        tracing::warn!(
            "[font] card {}: {} character(s) not covered by '{}': {:?}",
            card.id().0,
            font_choice.missing.len(),
            font_choice.family,
            font_choice.missing
        );
    }

    let page_height_css = area.height as f32 / area.scale;
    let ord = card.template_idx();
    let html = CardHtml {
        question: card_document(&question_html, &rendered.css, ord, page_height_css, shadows),
        answer: card_document(&answer_html, &rendered.css, ord, page_height_css, shadows),
        font_family: font_choice.family,
    };
    Ok(CardDocuments {
        html,
        question_av,
        answer_av,
    })
}

/// The text of partially rendered template nodes, as Anki's `template.py`
/// builds it without add-on filters: `{{FrontSide}}` becomes `frontside`,
/// any other field left for Python keeps the text rslib gave it.
fn rendered_text(nodes: &[RenderedNode], frontside: &str) -> String {
    let mut text = String::new();
    for node in nodes {
        match node {
            RenderedNode::Text { text: part } => text.push_str(part),
            RenderedNode::Replacement {
                field_name,
                current_text,
                ..
            } => {
                if field_name == "FrontSide" {
                    text.push_str(frontside);
                } else {
                    text.push_str(current_text);
                }
            }
        }
    }
    text
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
                    image_cache: crate::card_render::ImageCacheSize::OFF,
                },
                Box::new(|_| {}),
            ),
            area: RenderArea {
                width: 100,
                height: 100,
                scale: 1.0,
            },
            shadows: true,
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

    fn sound_names(tags: &[AvTag]) -> Vec<String> {
        use anki_proto::card_rendering::av_tag::Value;
        tags.iter()
            .filter_map(|tag| match &tag.value {
                Some(Value::SoundOrVideo(name)) => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    /// The answer of a `{{FrontSide}}` template holds only its own sounds, as
    /// in Anki desktop; the question's sounds stay references in its text.
    #[test]
    fn answer_audio_excludes_the_front_side() {
        let (dir, mut col) = temp_collection("frontside");
        let notetype = col.get_notetype_by_name("Basic").unwrap().unwrap();
        let mut note = notetype.new_note();
        note.set_field(0, "word[sound:q.mp3]").unwrap();
        note.set_field(1, "meaning[sound:a1.mp3][sound:a2.ogg]")
            .unwrap();
        col.add_note(&mut note, anki::decks::DeckId(1)).unwrap();
        let card = col.storage.all_cards_of_note(note.id).unwrap().remove(0);

        let fonts = FontCatalog::build("none", &[], &[]);
        let area = RenderArea {
            width: 100,
            height: 100,
            scale: 1.0,
        };
        let docs = card_documents(&mut col, &card, &fonts, area, false).unwrap();

        assert_eq!(sound_names(&docs.question_av), vec!["q.mp3"]);
        assert_eq!(sound_names(&docs.answer_av), vec!["a1.mp3", "a2.ogg"]);
        // The default preset plays automatically and replays the question.
        assert_eq!(audio_options(&mut col, &card), AudioOptions::default());
        // The answer still shows the question text.
        assert!(docs.html.answer.contains("word"));
        assert!(docs.html.answer.contains("meaning"));

        drop(col);
        let _ = fs::remove_dir_all(&dir);
    }
}
