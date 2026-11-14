//! Conversions from session data to the Slint view models in `ui/types.slint`.

use std::collections::HashSet;
use std::rc::Rc;

use anki::timestamp::TimestampSecs;
use anki_proto::decks::DeckTreeNode;
use slint::{ModelRc, VecModel};

use crate::session::LearnSession;
use crate::study::LoadedCard;
use crate::sync::{SyncAction, SyncResult};
use crate::{CardNode, DeckNode, SyncState, Translations};

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
        count_new: "New".into(),
        count_learn: "Learn".into(),
        count_due: "Due".into(),
        no_cards_due: i181.studying_no_cards_are_due_yet().as_ref().into(),
        settings: i181.preferences_preferences().as_ref().into(),
        back: "Back".into(),
        save: i181.actions_save().as_ref().into(),
        sync_now: "Sync now".into(),
        server_url: "Server URL".into(),
        email: "Email".into(),
        password: "Password".into(),
        auto_sync: "Auto sync".into(),
        sync_on_exit: "Sync on exit".into(),
        sync_media: "Sync media".into(),
        upload: "Upload".into(),
        download: "Download".into(),
        full_sync_title: "Full sync required".into(),
        full_sync_required: "The collection on this device and the collection on the server are \
                             different. Keep one side. The other side is replaced."
            .into(),
        confirm_full_sync: "I understand that one side is replaced".into(),
        cancel: i181.sync_cancel_button().as_ref().into(),
    }
}

/// All decks in tree order with today's counts. `None` if the collection is
/// unavailable (a job owns it) or the tree cannot be read.
pub(crate) fn load_deck_nodes(session: &LearnSession) -> Option<Vec<DeckNode>> {
    let mut col_borrow = session.collection.borrow_mut();
    let col = col_borrow.as_mut()?;
    let deck_tree = col
        .deck_tree(Some(TimestampSecs::now()))
        .inspect_err(|err| tracing::error!("Could not read the deck tree: {err}"))
        .ok()?;
    let mut nodes = Vec::new();
    flatten_tree_recursive(&deck_tree, -1, &mut nodes);
    Some(nodes)
}

fn flatten_tree_recursive(node: &DeckTreeNode, parent_index: i32, result: &mut Vec<DeckNode>) {
    for child in &node.children {
        let current_index = result.len() as i32;

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

/// The indexes of the decks that the list shows: every deck without a
/// collapsed ancestor. `nodes` is in tree order.
pub(crate) fn visible_decks(nodes: &[DeckNode], collapsed: &HashSet<i64>) -> Vec<usize> {
    let mut visible = Vec::with_capacity(nodes.len());
    // The level of the collapsed deck whose descendants are being skipped.
    let mut hidden_below: Option<i32> = None;
    for (index, node) in nodes.iter().enumerate() {
        if hidden_below.is_some_and(|level| node.level > level) {
            continue;
        }
        hidden_below = collapsed.contains(&node.id).then_some(node.level);
        visible.push(index);
    }
    visible
}

/// One page of `visible`: `(page, page count, rows)`. `page` is brought into
/// range. There is always at least one page.
pub(crate) fn page_slice(
    visible: &[usize],
    page: usize,
    rows_per_page: usize,
) -> (usize, usize, &[usize]) {
    let rows_per_page = rows_per_page.max(1);
    let page_count = visible.len().div_ceil(rows_per_page).max(1);
    let page = page.min(page_count - 1);
    let start = page * rows_per_page;
    let end = (start + rows_per_page).min(visible.len());
    (page, page_count, &visible[start.min(end)..end])
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

    /// The state after a screen change, or `None` to keep it. A finished
    /// result (the done icon with "Sync complete", or an info line such as
    /// "Settings saved") ends there: Home shows the sync icon again, and
    /// Settings does not show a result from another screen. A running sync,
    /// an error, a media warning, and a required full sync stay; the error
    /// icon on Home opens Settings to show them.
    pub(crate) fn after_screen_change(&self) -> Option<Self> {
        let keep = self.is_syncing || self.has_error || self.has_warning || self.full_sync_required;
        (!keep && *self != Self::default()).then(Self::default)
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
            done: result.success && !full_sync_required,
            full_sync_required,
            can_upload,
            can_download,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i64, level: i32) -> DeckNode {
        DeckNode {
            id,
            level,
            ..Default::default()
        }
    }

    /// 1
    /// ├─ 2
    /// │  └─ 3
    /// └─ 4
    /// 5
    fn tree() -> Vec<DeckNode> {
        vec![node(1, 1), node(2, 2), node(3, 3), node(4, 2), node(5, 1)]
    }

    fn visible_ids(collapsed: &[i64]) -> Vec<i64> {
        let nodes = tree();
        visible_decks(&nodes, &collapsed.iter().copied().collect())
            .into_iter()
            .map(|index| nodes[index].id)
            .collect()
    }

    #[test]
    fn visible_decks_hide_all_descendants() {
        assert_eq!(visible_ids(&[]), [1, 2, 3, 4, 5]);
        assert_eq!(visible_ids(&[2]), [1, 2, 4, 5]);
        // The grandchild is hidden too, also when its parent is expanded.
        assert_eq!(visible_ids(&[1]), [1, 5]);
        assert_eq!(visible_ids(&[1, 2]), [1, 5]);
        // A collapsed deck without subdecks hides nothing.
        assert_eq!(visible_ids(&[3, 5]), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn page_slice_splits_and_clamps() {
        let visible: Vec<usize> = (0..7).collect();

        assert_eq!(page_slice(&visible, 0, 3), (0, 3, &visible[0..3]));
        assert_eq!(page_slice(&visible, 2, 3), (2, 3, &visible[6..7]));
        // A page past the end (the list got shorter) goes to the last page.
        assert_eq!(page_slice(&visible, 9, 3), (2, 3, &visible[6..7]));
        assert_eq!(page_slice(&visible, 0, 7), (0, 1, &visible[..]));
        assert_eq!(page_slice(&visible, 0, 0), (0, 7, &visible[0..1]));
        assert_eq!(page_slice(&[], 4, 3), (0, 1, &[][..]));
    }

    #[test]
    fn a_screen_change_ends_only_a_finished_sync() {
        let result = |success: bool, action: SyncAction, media_error: Option<&str>| {
            SyncState::from_result(&SyncResult {
                success,
                message: "Sync complete".into(),
                action,
                media_error: media_error.map(Into::into),
                ..Default::default()
            })
        };

        // The done icon and "Sync complete" end; so does "Settings saved".
        let done = result(true, SyncAction::NormalSync, None);
        assert!(done.done);
        assert_eq!(done.after_screen_change(), Some(SyncState::default()));
        assert_eq!(
            SyncState::info("Settings saved").after_screen_change(),
            Some(SyncState::default())
        );

        // These stay until the next sync.
        let full = SyncAction::FullSyncRequired {
            upload_ok: true,
            download_ok: true,
        };
        for state in [
            SyncState::syncing("Syncing"),
            SyncState::error("Network error"),
            result(false, SyncAction::NoChanges, None),
            result(true, SyncAction::NormalSync, Some("media failed")),
            result(true, full, None),
        ] {
            assert_eq!(state.after_screen_change(), None, "{state:?}");
        }
        assert_eq!(SyncState::default().after_screen_change(), None);
    }
}
