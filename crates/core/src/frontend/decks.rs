//! The paged deck list of Home. Nothing scrolls: Rust builds one page of
//! rows from the deck tree, the collapsed set, and the rows that fit.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use super::models::{load_deck_nodes, page_slice, visible_decks};
use crate::session::LearnSession;
use crate::{DeckNode, MainWindow};

pub(super) fn wire(ui: &MainWindow, session: &Rc<LearnSession>) {
    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        ui.on_deck_toggle(move |deck| {
            {
                let mut list = session.deck_list.borrow_mut();
                if !list.collapsed.remove(&deck.id) {
                    list.collapsed.insert(deck.id);
                }
            }
            if let Some(ui) = ui_weak.upgrade() {
                show_page(&ui, &session);
            }
        });
    }

    {
        let ui_weak = ui.as_weak();
        let session = session.clone();
        // -1 and 1 turn the page. 0 comes from Slint when the rows per page
        // change (the window got its size).
        ui.on_deck_page_changed(move |delta| {
            {
                let mut list = session.deck_list.borrow_mut();
                list.page = list.page.saturating_add_signed(delta as isize);
            }
            if let Some(ui) = ui_weak.upgrade() {
                show_page(&ui, &session);
            }
        });
    }
}

/// Reads the deck tree from the collection and shows the page again. It
/// keeps the last tree if a job owns the collection.
pub(super) fn reload(ui: &MainWindow, session: &LearnSession) {
    if let Some(nodes) = load_deck_nodes(session) {
        let mut list = session.deck_list.borrow_mut();
        let list = &mut *list;
        for node in &nodes {
            if list.known.insert(node.id) && node.collapsed {
                list.collapsed.insert(node.id);
            }
        }
        list.known
            .retain(|id| nodes.iter().any(|node| node.id == *id));
        list.collapsed.retain(|id| list.known.contains(id));
        list.nodes = nodes;
    }
    show_page(ui, session);
}

/// Sets the page properties of the window from the deck list state.
pub(super) fn show_page(ui: &MainWindow, session: &LearnSession) {
    let mut list = session.deck_list.borrow_mut();
    let rows_per_page = ui.get_deck_rows_per_page().max(1) as usize;
    let visible = visible_decks(&list.nodes, &list.collapsed);
    let (page, page_count, rows) = page_slice(&visible, list.page, rows_per_page);
    let rows: Vec<DeckNode> = rows
        .iter()
        .map(|&index| {
            let node = &list.nodes[index];
            DeckNode {
                collapsed: list.collapsed.contains(&node.id),
                ..node.clone()
            }
        })
        .collect();
    list.page = page;
    ui.set_deck_page(page as i32);
    ui.set_deck_page_count(page_count as i32);
    ui.set_deck_page_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
}
