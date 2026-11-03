//! Conversions from session data to the Slint view models in `ui/types.slint`.

use std::rc::Rc;

use anki::timestamp::TimestampSecs;
use anki_proto::decks::DeckTreeNode;
use slint::{Model, ModelRc, VecModel};

use crate::session::LearnSession;
use crate::study::LoadedCard;
use crate::sync::{SyncAction, SyncResult};
use crate::{CardNode, DeckNode, DeckTree, SyncState, Translations};

pub(crate) fn translations(session: &LearnSession) -> Translations {
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

pub(crate) fn deck_tree(session: &LearnSession) -> DeckTree {
    let mut col_borrow = session.collection.borrow_mut();
    let Some(col) = col_borrow.as_mut() else {
        return DeckTree {
            due_total: 0,
            deck_nodes: ModelRc::default(),
        };
    };

    let deck_tree = col.deck_tree(Some(TimestampSecs::now())).unwrap();

    let deck_nodes = flatten_tree(&deck_tree);

    DeckTree {
        due_total: deck_tree.review_count as i32,
        deck_nodes: deck_nodes.into(),
    }
}

fn flatten_tree(node: &DeckTreeNode) -> Rc<VecModel<DeckNode>> {
    let result = Rc::new(VecModel::<DeckNode>::default());
    flatten_tree_recursive(node, -1, &result);
    result
}

fn flatten_tree_recursive(node: &DeckTreeNode, parent_index: i32, result: &Rc<VecModel<DeckNode>>) {
    for child in &node.children {
        let current_index = result.row_count() as i32;

        result.push(DeckNode {
            id: child.deck_id,
            name: child.name.clone().into(),
            level: child.level as i32,
            collapsed: child.collapsed,
            new: child.new_count as i32,
            learn: child.learn_count as i32,
            due: child.review_count as i32,
            has_children: !child.children.is_empty(),
            parent_index,
        });
        flatten_tree_recursive(child, current_index, result);
    }
}

/// Builds the displayed card, with the deck's counts taken from the queue the
/// card was loaded from. `pages` is `(question pages, answer pages, answer
/// start)`; all 0 when the card area shows `message`.
pub(crate) fn card_node(
    card: LoadedCard,
    deck: DeckNode,
    pages: (u32, u32, u32),
    message: String,
) -> CardNode {
    let deck = DeckNode {
        new: card.new_count,
        learn: card.learn_count,
        due: card.due_count,
        ..deck
    };
    CardNode {
        id: card.id,
        deck,
        question_pages: pages.0 as i32,
        answer_pages: pages.1 as i32,
        answer_start: pages.2 as i32,
        message: message.into(),
        durations: Rc::new(VecModel::from(card.durations)).into(),
    }
}

pub(crate) fn error_card(deck: DeckNode, message: &str) -> CardNode {
    CardNode {
        id: -1,
        deck,
        question_pages: 0,
        answer_pages: 0,
        answer_start: 0,
        message: format!("Could not load card: {message}").into(),
        durations: ModelRc::new(VecModel::default()),
    }
}

impl SyncState {
    pub(crate) fn syncing(message: &str) -> Self {
        Self {
            is_syncing: true,
            status_message: message.into(),
            ..Default::default()
        }
    }

    pub(crate) fn info(message: &str) -> Self {
        Self {
            status_message: message.into(),
            ..Default::default()
        }
    }

    pub(crate) fn error(message: &str) -> Self {
        Self {
            status_message: message.into(),
            has_error: true,
            ..Default::default()
        }
    }

    pub(crate) fn from_result(result: &SyncResult) -> Self {
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

        Self {
            is_syncing: false,
            status_message: message.into(),
            has_error: !result.success,
            has_warning: result.success && result.media_error.is_some(),
            full_sync_required,
            can_upload,
            can_download,
        }
    }
}
