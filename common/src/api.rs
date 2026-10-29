use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anki::collection::{Collection, CollectionBuilder};
use anki::prelude::I18n;
use anki::scheduler::answering::CardAnswer;
use anki::scheduler::states::SchedulingStates;
use anki::services::SchedulerService;
use anki::timestamp::{TimestampMillis, TimestampSecs};

use crate::fonts::FontCatalog;
use crate::profile::ProfileTimer;
use crate::profile_log;
use crate::{
    CardNode, DeckNode, DeckTree, SyncAction, SyncManager, SyncResult, SyncSettings, SyncState,
    SyncStatus, Translations,
};

use slint::ModelRc;

pub struct LearnSession {
    pub collection: Rc<RefCell<Option<Collection>>>,
    pub current_card: RefCell<Option<i64>>,
    pub states: RefCell<Option<SchedulingStates>>,
    pub start_time: RefCell<Option<Instant>>,
    pub current_deck_id: Cell<Option<i64>>,
    /// Incremented for every background card job, used to ignore stale UI timers.
    pub job_seq: Cell<u64>,
    /// True while a card is being loaded on a worker thread or a sync is running.
    pub busy: Cell<bool>,
    pub font_catalog: FontCatalog,
    pub sync_manager: Rc<SyncManager>,
}

pub fn init_session(config: &crate::config::Config) -> Rc<LearnSession> {
    let collection_path = &config.general.collection_path;
    let language = &config.general.language;

    let col_file = format!("{}/collection.anki2", collection_path);

    if let Some(parent) = Path::new(&col_file).parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("Failed to create collection directory: {:?}", e);
            panic!(
                "Cannot create collection directory at: {}",
                parent.display()
            );
        }
    }

    let mut builder = CollectionBuilder::new(&col_file);
    builder.with_desktop_media_paths();
    builder.set_tr(I18n::new(&[language.as_str()]));

    let mut col = match builder.build() {
        Ok(col) => col,
        Err(e) => {
            eprintln!("Failed to open collection: {:?}", e);
            panic!("Cannot continue without a valid collection.");
        }
    };

    if let Err(e) = col.upgrade_scheduler() {
        eprintln!("Failed to upgrade scheduler: {:?}", e);
    }

    import_pending_packages(&mut col, collection_path);

    let search = "note:\"Image Occlusion Enhanced\"";

    if let Ok(card_ids) = col.search_cards(search, anki::search::SortMode::NoOrder) {
        let _ = col.bury_or_suspend_cards(
            &card_ids,
            anki_proto::scheduler::bury_or_suspend_cards_request::Mode::Suspend,
        );
    }

    let font_catalog = FontCatalog::build(
        &config.general.card_font,
        &config.general.card_fallback_fonts,
        &config.general.font_dirs,
    );

    Rc::new(LearnSession {
        collection: Rc::new(RefCell::new(Some(col))),
        current_card: RefCell::new(None),
        states: RefCell::new(None),
        start_time: RefCell::new(None),
        current_deck_id: Cell::new(None),
        job_seq: Cell::new(0),
        busy: Cell::new(false),
        font_catalog,
        sync_manager: Rc::new(SyncManager::new()),
    })
}

fn import_pending_packages(collection: &mut anki::collection::Collection, collection_path: &str) {
    let Ok(entries) = fs::read_dir(collection_path) else {
        return;
    };

    let mut packages: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("apkg"))
        })
        .collect();
    packages.sort();

    for path in packages {
        let options = anki::import_export::package::ImportAnkiPackageOptions {
            merge_notetypes: true,
            with_scheduling: true,
            with_deck_configs: true,
            ..Default::default()
        };

        match collection.import_apkg(&path, options) {
            Ok(output) => {
                eprintln!(
                    "Imported {} ({} new notes)",
                    path.display(),
                    output.output.new.len()
                );
                let imported_path = path.with_extension("apkg.imported");
                if let Err(err) = fs::rename(&path, &imported_path) {
                    eprintln!(
                        "Imported {}, but could not rename it to {}: {err}",
                        path.display(),
                        imported_path.display()
                    );
                }
            }
            Err(err) => {
                eprintln!("Failed to import {}: {err:?}", path.display());
            }
        }
    }
}

pub fn init_translations(session: &LearnSession) -> Translations {
    let col_borrow = session.collection.borrow();
    let col = col_borrow.as_ref().expect("collection is unavailable");
    let i181 = col.tr();

    Translations {
        show_answer: i181.studying_show_answer().as_ref().into(),
        again: i181.studying_again().as_ref().into(),
        hard: i181.studying_hard().as_ref().into(),
        good: i181.studying_good().as_ref().into(),
        easy: i181.studying_easy().as_ref().into(),
        cards_due_suffix: {
            let full = i181.statistics_cards_due(0);
            full.as_ref().replace("0", "").into()
        },
        no_cards_due: i181.studying_no_cards_are_due_yet().as_ref().into(),
        settings: i181.preferences_preferences().as_ref().into(),
        back: "Back".into(),
        save: i181.actions_save().as_ref().into(),
        sync_now: "Sync now".into(),
        server_url: i181.preferences_custom_sync_url().as_ref().into(),
        email: i181.sync_ankiweb_id_label().as_ref().into(),
        password: i181.sync_password_label().as_ref().into(),
        auto_sync: i181
            .preferences_automatically_sync_on_profile_openclose()
            .as_ref()
            .into(),
        sync_on_exit: "Sync after study session".into(),
        sync_media: i181
            .preferences_synchronize_audio_and_images_too()
            .as_ref()
            .into(),
        upload: "Upload local collection".into(),
        download: "Download server collection".into(),
        full_sync_required: "A full sync is required. Choose one direction.".into(),
        confirm_full_sync: "I understand this replaces data".into(),
        cancel: i181.sync_cancel_button().as_ref().into(),
    }
}

pub fn update_deck_tree(session: &LearnSession) -> DeckTree {
    let mut col_borrow = session.collection.borrow_mut();
    let Some(col) = col_borrow.as_mut() else {
        return DeckTree {
            due_total: 0,
            deck_nodes: ModelRc::default(),
        };
    };

    let deck_tree = col.deck_tree(Some(TimestampSecs::now())).unwrap();

    let deck_nodes = crate::utils::flatten_tree(&deck_tree);

    DeckTree {
        due_total: deck_tree.review_count as i32,
        deck_nodes: deck_nodes.into(),
    }
}

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

    pub fn into_card_node(self, deck: DeckNode) -> CardNode {
        let deck = DeckNode {
            new: self.new_count,
            learn: self.learn_count,
            due: self.due_count,
            ..deck
        };
        CardNode {
            id: self.id,
            deck,
            question: Rc::new(slint::VecModel::from(self.question)).into(),
            answer: Rc::new(slint::VecModel::from(self.answer)).into(),
            durations: Rc::new(slint::VecModel::from(self.durations)).into(),
        }
    }
}

pub fn error_card(deck: DeckNode, message: &str) -> CardNode {
    CardNode {
        id: -1,
        deck,
        question: Rc::new(slint::VecModel::from(vec![slint::SharedString::from(
            format!("Could not load card: {message}"),
        )]))
        .into(),
        answer: ModelRc::new(slint::VecModel::default()),
        durations: ModelRc::new(slint::VecModel::default()),
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
    let question =
        crate::utils::strip_html_remove_brackets_and_paginate(&question_html, true, chars_per_page)
            .into_iter()
            .map(|page| slint::SharedString::from(fonts.sanitize(&page, &font_choice.family)))
            .collect();
    let answer = crate::utils::paginate_text(&answer_text, chars_per_page);
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
        0 => (
            states.again.clone(),
            anki::scheduler::answering::Rating::Again,
        ),
        1 => (
            states.hard.clone(),
            anki::scheduler::answering::Rating::Hard,
        ),
        2 => (
            states.good.clone(),
            anki::scheduler::answering::Rating::Good,
        ),
        3 => (
            states.easy.clone(),
            anki::scheduler::answering::Rating::Easy,
        ),
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

pub fn sync_now(session: &LearnSession, config: &mut crate::config::Config) -> SyncResult {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let auth = match sync_auth(session, config, &rt) {
        Ok(auth) => auth,
        Err(message) => return error_result(message),
    };

    let sync_result = {
        let mut col_borrow = session.collection.borrow_mut();
        let Some(col) = col_borrow.as_mut() else {
            return error_result("Collection is unavailable".into());
        };
        rt.block_on(session.sync_manager.sync_collection(col, &auth))
    };

    let mut result = match sync_result {
        Ok(result) => result,
        Err(err) => return error_result(format!("Sync failed: {err}")),
    };

    if let Some(endpoint) = result.new_endpoint.take() {
        let normalized = crate::sync::normalize_endpoint(&endpoint).ok().flatten();
        config.ankiweb.endpoint = Some(endpoint);
        if config.ankiweb.token.is_some() {
            config.ankiweb.token_endpoint = normalized;
        }
        let _ = config.save();
    }

    if matches!(result.action, SyncAction::FullSyncRequired { .. }) {
        return result;
    }

    if config.ankiweb.sync_media {
        let media_result = {
            let col_borrow = session.collection.borrow();
            let Some(col) = col_borrow.as_ref() else {
                return error_result("Collection is unavailable".into());
            };
            rt.block_on(session.sync_manager.sync_media(col, auth))
        };

        if let Err(err) = media_result {
            result.media_error = Some(format!("Media sync failed: {err}"));
        }
    }

    result
}

pub fn full_sync(
    session: &LearnSession,
    config: &mut crate::config::Config,
    upload: bool,
) -> SyncResult {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let auth = match sync_auth(session, config, &rt) {
        Ok(auth) => auth,
        Err(message) => return error_result(message),
    };

    let mut builder = {
        let col_borrow = session.collection.borrow();
        let Some(col) = col_borrow.as_ref() else {
            return error_result("Collection is unavailable".into());
        };
        col.as_builder()
    };

    if !upload {
        // Close the collection first so SQLite checkpoints the WAL and the files
        // copied below are self-consistent, then reopen it for the download.
        let collection = match session.collection.borrow_mut().take() {
            Some(collection) => collection,
            None => return error_result("Collection is unavailable".into()),
        };
        if let Err(err) = collection.close(None) {
            return error_result(format!("Could not close collection before backup: {err}"));
        }
        if let Err(err) = backup_collection(config) {
            let reopen_note = match reopen_collection(session, &mut builder, config) {
                Ok(()) => String::new(),
                Err(reopen_err) => format!("; {reopen_err}"),
            };
            return error_result(format!(
                "Could not back up local collection: {err}{reopen_note}"
            ));
        }
        if let Err(err) = reopen_collection(session, &mut builder, config) {
            return error_result(format!(
                "Could not reopen collection before download: {err}"
            ));
        }
    }

    let collection = match session.collection.borrow_mut().take() {
        Some(collection) => collection,
        None => return error_result("Collection is unavailable".into()),
    };

    let sync_result = rt.block_on(
        session
            .sync_manager
            .full_sync(collection, auth.clone(), upload),
    );
    let reopen_result = reopen_collection(session, &mut builder, config);

    match (sync_result, reopen_result) {
        (Ok(()), Ok(())) => {}
        (Err(err), Ok(())) => return error_result(format!("Full sync failed: {err}")),
        (Ok(()), Err(err)) => {
            return error_result(format!("Full sync completed, but {err}"));
        }
        (Err(sync_err), Err(reopen_err)) => {
            return error_result(format!("Full sync failed: {sync_err}; {reopen_err}"));
        }
    }

    reset_session_memory(session);

    let mut result = SyncResult {
        success: true,
        message: if upload {
            "Collection uploaded successfully".into()
        } else {
            "Collection downloaded successfully".into()
        },
        server_message: None,
        action: SyncAction::NoChanges,
        media_error: None,
        new_endpoint: None,
    };

    if config.ankiweb.sync_media {
        let media_result = {
            let col_borrow = session.collection.borrow();
            let Some(col) = col_borrow.as_ref() else {
                return error_result("Collection is unavailable".into());
            };
            rt.block_on(session.sync_manager.sync_media(col, auth))
        };

        if let Err(err) = media_result {
            result.media_error = Some(format!("Media sync failed: {err}"));
        }
    }

    result
}

fn sync_auth(
    session: &LearnSession,
    config: &mut crate::config::Config,
    rt: &tokio::runtime::Runtime,
) -> Result<anki::sync::login::SyncAuth, String> {
    let endpoint = config.ankiweb.effective_endpoint();
    let endpoint_url = crate::sync::parse_endpoint(endpoint).map_err(|err| err.to_string())?;

    let normalized_endpoint = endpoint_url.as_ref().map(ToString::to_string);
    let token_endpoint_matches = match config.ankiweb.token_endpoint.as_deref() {
        None => true,
        Some(recorded) => Some(recorded) == normalized_endpoint.as_deref(),
    };

    if let Some(token) = config
        .ankiweb
        .token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .filter(|_| token_endpoint_matches)
    {
        return Ok(anki::sync::login::SyncAuth {
            hkey: token.to_string(),
            endpoint: endpoint_url,
            io_timeout_secs: Some(60),
        });
    }

    let auth = rt
        .block_on(session.sync_manager.login(
            &config.ankiweb.username,
            &config.ankiweb.password,
            endpoint,
        ))
        .map_err(|err| format!("Login failed: {err}"))?;

    config.ankiweb.token = Some(auth.hkey.clone());
    config.ankiweb.token_endpoint = normalized_endpoint;
    let _ = config.save();

    Ok(auth)
}

fn reopen_collection(
    session: &LearnSession,
    builder: &mut CollectionBuilder,
    config: &crate::config::Config,
) -> Result<(), String> {
    match builder.build() {
        Ok(collection) => {
            *session.collection.borrow_mut() = Some(collection);
            Ok(())
        }
        Err(err) => {
            eprintln!("Could not reopen collection after full sync: {err}");
            // Fall back to a plain reopen from disk so a failure here does not
            // leave the app running without a collection.
            let col_file = Path::new(&config.general.collection_path).join("collection.anki2");
            let mut fallback = CollectionBuilder::new(col_file);
            fallback.with_desktop_media_paths();
            match fallback.build() {
                Ok(collection) => {
                    *session.collection.borrow_mut() = Some(collection);
                    Ok(())
                }
                Err(fallback_err) => Err(format!(
                    "the collection could not be reopened ({err}); fallback reopen also failed \
                     ({fallback_err}). Restart pbAnki."
                )),
            }
        }
    }
}

fn backup_collection(config: &crate::config::Config) -> Result<(), String> {
    let path = Path::new(&config.general.collection_path).join("collection.anki2");
    if !path.exists() {
        return Ok(());
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| err.to_string())?
        .as_secs();

    for suffix in ["", "-wal", "-shm"] {
        let source = if suffix.is_empty() {
            path.clone()
        } else {
            Path::new(&format!("{}{suffix}", path.display())).to_path_buf()
        };
        if source.exists() {
            let file_name = source
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("collection.anki2");
            let backup = source.with_file_name(format!("{file_name}.backup-{timestamp}"));
            fs::copy(&source, &backup).map_err(|err| err.to_string())?;
        }
    }

    Ok(())
}

/// Forgets the displayed card: its id, scheduling states, and display time.
pub(crate) fn clear_current_card(session: &LearnSession) {
    *session.current_card.borrow_mut() = None;
    *session.states.borrow_mut() = None;
    *session.start_time.borrow_mut() = None;
}

/// Forgets the displayed card and the cached current deck, so the next deck
/// entry calls `set_current_deck` again.
///
/// Call after anything that can change the collection's current deck behind
/// the cache: every sync (a merge can pull another device's `curDeck` or
/// delete the deck) and a failed card load.
pub(crate) fn reset_session_memory(session: &LearnSession) {
    clear_current_card(session);
    session.current_deck_id.set(None);
}

#[allow(dead_code)]
fn assert_worker_types_are_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Collection>();
    assert_send::<SchedulingStates>();
    assert_send::<LoadedCard>();
}

fn error_result(message: String) -> SyncResult {
    SyncResult {
        success: false,
        message,
        server_message: None,
        action: SyncAction::NoChanges,
        media_error: None,
        new_endpoint: None,
    }
}

pub fn sync_settings_from_config(config: &crate::config::Config) -> SyncSettings {
    SyncSettings {
        endpoint: config.ankiweb.endpoint.clone().unwrap_or_default().into(),
        username: config.ankiweb.username.clone().into(),
        password: config.ankiweb.password.clone().into(),
        auto_sync: config.ankiweb.auto_sync,
        sync_on_exit: config.ankiweb.sync_on_exit,
        sync_media: config.ankiweb.sync_media,
    }
}

pub fn save_sync_settings(
    config: &mut crate::config::Config,
    settings: SyncSettings,
) -> Result<(), String> {
    let endpoint = crate::sync::normalize_endpoint(settings.endpoint.as_str())?;
    let endpoint_changed = config
        .ankiweb
        .endpoint
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        != endpoint.as_deref().unwrap_or_default();
    let credentials_changed = endpoint_changed
        || config.ankiweb.username != settings.username.as_str()
        || config.ankiweb.password != settings.password.as_str();

    config.ankiweb.endpoint = endpoint;
    config.ankiweb.username = settings.username.to_string();
    config.ankiweb.password = settings.password.to_string();
    config.ankiweb.auto_sync = settings.auto_sync;
    config.ankiweb.sync_on_exit = settings.sync_on_exit;
    config.ankiweb.sync_media = settings.sync_media;

    if credentials_changed {
        config.ankiweb.token = None;
        config.ankiweb.token_endpoint = None;
    }

    config.save().map_err(|err| err.to_string())
}

pub fn sync_state_from_result(result: &SyncResult) -> SyncState {
    let (full_sync_required, can_upload, can_download) = match result.action {
        SyncAction::FullSyncRequired {
            upload_ok,
            download_ok,
        } => (true, upload_ok, download_ok),
        _ => (false, false, false),
    };

    let mut message = result.message.clone();
    if let Some(server_message) = result
        .server_message
        .as_deref()
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        if message.is_empty() {
            message = server_message.to_string();
        } else if server_message != message {
            message = format!("{message}: {server_message}");
        }
    }
    if let Some(media_error) = &result.media_error {
        message = format!("{message}; {media_error}");
    }

    SyncState {
        is_syncing: false,
        status_message: message.into(),
        has_error: !result.success,
        has_warning: result.success && result.media_error.is_some(),
        full_sync_required,
        can_upload,
        can_download,
    }
}

pub fn sync_state_from_status(status: &SyncStatus) -> SyncState {
    let (message, is_syncing, has_error) = match status {
        SyncStatus::Idle => (String::new(), false, false),
        SyncStatus::Authenticating => ("Authenticating...".to_string(), true, false),
        SyncStatus::CheckingStatus => ("Checking sync status...".to_string(), true, false),
        SyncStatus::SyncingCollection { progress } => (progress.clone(), true, false),
        SyncStatus::SyncingMedia { .. } => ("Syncing media...".to_string(), true, false),
        SyncStatus::Complete { message } => (message.clone(), false, false),
        SyncStatus::Error { message } => (message.clone(), false, true),
    };

    SyncState {
        is_syncing,
        status_message: message.into(),
        has_error,
        has_warning: false,
        full_sync_required: false,
        can_upload: false,
        can_download: false,
    }
}

pub fn get_sync_status(session: &LearnSession) -> SyncStatus {
    session.sync_manager.get_status()
}

#[cfg(test)]
#[path = "../tests/unit/api.rs"]
mod tests;
