//! The open collection and the study state shared by the UI callbacks.

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use anki::collection::{Collection, CollectionBuilder};
use anki::prelude::I18n;
use anki::scheduler::states::SchedulingStates;
use anki::services::SchedulerService;

use crate::config::Config;
use crate::fonts::FontCatalog;
use crate::sync::SyncManager;

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
    /// `LoadedCard::predicted_next` of the shown card. The next rating logs
    /// whether its card was predicted (`[card] lookahead`).
    pub predicted_next: RefCell<Vec<i64>>,
    pub font_catalog: FontCatalog,
    pub sync_manager: Rc<SyncManager>,
}

pub fn init_session(config: &Config) -> Rc<LearnSession> {
    let collection_path = &config.general.collection_path;
    let col_file = collection_file(config);

    if let Some(parent) = col_file.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            tracing::error!("Failed to create collection directory: {:?}", e);
            panic!(
                "Cannot create collection directory at: {}",
                parent.display()
            );
        }
    }

    let mut col = match collection_builder(config).build() {
        Ok(col) => col,
        Err(e) => {
            tracing::error!("Failed to open collection: {:?}", e);
            panic!("Cannot continue without a valid collection.");
        }
    };

    if let Err(e) = col.upgrade_scheduler() {
        tracing::warn!("Failed to upgrade scheduler: {:?}", e);
    }

    import_pending_packages(&mut col, collection_path);
    suspend_unsupported_cards(&mut col);

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
        predicted_next: RefCell::new(Vec::new()),
        font_catalog,
        sync_manager: Rc::new(SyncManager::new()),
    })
}

pub(crate) fn collection_file(config: &Config) -> PathBuf {
    Path::new(&config.general.collection_path).join("collection.anki2")
}

/// The media folder that `with_desktop_media_paths` sets up.
pub(crate) fn media_folder(config: &Config) -> PathBuf {
    collection_file(config).with_extension("media")
}

/// Builder for the configured collection. `with_desktop_media_paths` creates
/// `collection.media/` and `collection.mdb`, which media sync needs.
fn collection_builder(config: &Config) -> CollectionBuilder {
    let mut builder = CollectionBuilder::new(collection_file(config));
    builder.with_desktop_media_paths();
    builder.set_tr(I18n::new(&[config.general.language.as_str()]));
    builder
}

/// Installs the collection rebuilt by `builder` after a full sync closed it.
pub(crate) fn reopen_collection(
    session: &LearnSession,
    builder: &mut CollectionBuilder,
    config: &Config,
) -> Result<(), String> {
    match builder.build() {
        Ok(collection) => {
            *session.collection.borrow_mut() = Some(collection);
            Ok(())
        }
        Err(err) => {
            tracing::error!("Could not reopen collection after full sync: {err}");
            // Fall back to a plain reopen from disk so a failure here does not
            // leave the app running without a collection.
            match collection_builder(config).build() {
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

/// Imports every `*.apkg` in the collection directory (with scheduling and
/// media) and renames it to `*.apkg.imported`. This is how decks are installed
/// on the device.
fn import_pending_packages(collection: &mut Collection, collection_path: &str) {
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
                tracing::info!(
                    "Imported {} ({} new notes)",
                    path.display(),
                    output.output.new.len()
                );
                let imported_path = path.with_extension("apkg.imported");
                if let Err(err) = fs::rename(&path, &imported_path) {
                    tracing::warn!(
                        "Imported {}, but could not rename it to {}: {err}",
                        path.display(),
                        imported_path.display()
                    );
                }
            }
            Err(err) => {
                tracing::error!("Failed to import {}: {err:?}", path.display());
            }
        }
    }
}

/// Image Occlusion Enhanced cards cannot be shown as text, so they are
/// suspended instead of appearing as unanswerable cards.
fn suspend_unsupported_cards(collection: &mut Collection) {
    let search = "note:\"Image Occlusion Enhanced\"";

    if let Ok(card_ids) = collection.search_cards(search, anki::search::SortMode::NoOrder) {
        let _ = collection.bury_or_suspend_cards(
            &card_ids,
            anki_proto::scheduler::bury_or_suspend_cards_request::Mode::Suspend,
        );
    }
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
