//! The pages of the shown card on the UI thread (notes/blitz-card-render.md,
//! "Slint side").
//!
//! `LearnScreen` binds its card image to the pure callback
//! `page-image(answer, page, version)`. A page turn re-evaluates the binding in
//! the same frame. A page that is not here yet is asked from the render
//! thread; the old page stays until it arrives, then `page-version` changes,
//! the binding runs again, and a redraw is requested.
//!
//! Only a window of pages is kept: the shown page and its neighbors, and the
//! first pages of both sides. Long cards then use the same memory as short
//! ones, also with color (`Rgb8`) pages.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use slint::{ComponentHandle, Image, Rgb8Pixel, SharedPixelBuffer};

use crate::MainWindow;
use crate::card_render::{CardRenderer, FirstPages, PageBuf, PageFormat, PageReady, Side};

thread_local! {
    static STORE: RefCell<PageStore> = RefCell::new(PageStore::default());
}

type Key = (Side, u32);

#[derive(Default)]
struct PageStore {
    renderer: Option<CardRenderer>,
    /// The card of these pages; 0 when no card has pages.
    seq: u64,
    question_pages: u32,
    answer_pages: u32,
    answer_start: u32,
    pages: HashMap<Key, PageBuf>,
    /// Pages asked from the render thread (or prefetched by it) and not
    /// evicted since.
    requested: HashSet<Key>,
    shown: Option<Key>,
    /// The image of the last page built, returned again until the next page
    /// is ready.
    image: Option<(Key, Image)>,
    version: i32,
}

pub(super) fn set_renderer(renderer: CardRenderer) {
    STORE.with(|store| store.borrow_mut().renderer = Some(renderer));
}

/// The pages of a new card. Call it before `MainWindow::set_current_card`.
pub(super) fn install(first: FirstPages) {
    STORE.with(|store| store.borrow_mut().install(first));
}

/// No pages: the card area shows a message.
pub(super) fn clear() {
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        let renderer = store.renderer.take();
        *store = PageStore {
            renderer,
            ..PageStore::default()
        };
    });
}

/// The image for `LearnScreen`'s `page-image` callback.
pub(super) fn page_image(answer: bool, page: i32) -> Image {
    STORE.with(|store| store.borrow_mut().page_image(answer, page))
}

/// A page from the render thread.
pub(super) fn deliver(ui: &MainWindow, page: PageReady) {
    let bump = STORE.with(|store| store.borrow_mut().deliver(page));
    if let Some(version) = bump {
        ui.set_page_version(version);
        ui.window().request_redraw();
    }
}

impl PageStore {
    fn count(&self, side: Side) -> u32 {
        match side {
            Side::Question => self.question_pages,
            Side::Answer => self.answer_pages,
        }
    }

    fn install(&mut self, first: FirstPages) {
        let start = first.answer_start;
        self.seq = first.seq;
        self.question_pages = first.question_pages;
        self.answer_pages = first.answer_pages;
        self.answer_start = start;
        self.pages.clear();
        self.requested.clear();
        self.shown = None;
        // The cached image is keyed by side and page only. Kept, it would be
        // returned for the same page of the new card: going Home from
        // question page 1 and entering another deck showed the old card.
        self.image = None;
        self.pages.insert((Side::Question, 0), first.question);
        self.pages.insert((Side::Answer, start), first.answer);
        // The render thread prefetches these by itself.
        if first.question_pages > 1 {
            self.requested.insert((Side::Question, 1));
        }
        if start + 1 < first.answer_pages {
            self.requested.insert((Side::Answer, start + 1));
        }
        if start > 0 {
            self.requested.insert((Side::Answer, start - 1));
        }
    }

    fn page_image(&mut self, answer: bool, page: i32) -> Image {
        let side = if answer { Side::Answer } else { Side::Question };
        let Ok(page) = u32::try_from(page) else {
            return Image::default();
        };
        if self.seq == 0 || page >= self.count(side) {
            return Image::default();
        }
        let key = (side, page);
        self.shown = Some(key);
        self.evict();
        if page > 0 {
            self.request((side, page - 1));
        }
        if page + 1 < self.count(side) {
            self.request((side, page + 1));
        }

        if let Some((cached, image)) = &self.image
            && *cached == key
        {
            return image.clone();
        }
        if let Some(buf) = self.pages.get(&key) {
            let image = to_image(buf);
            self.image = Some((key, image.clone()));
            return image;
        }
        self.request(key);
        tracing::info!("[card] page {page} of the {side:?} is not ready yet");
        self.image
            .as_ref()
            .map(|(_, image)| image.clone())
            .unwrap_or_default()
    }

    /// Stores a delivered page. Returns the new `page-version` if the page is
    /// the one waiting to show.
    fn deliver(&mut self, page: PageReady) -> Option<i32> {
        let key = (page.side, page.page);
        if page.seq != self.seq || !self.keeps(key) {
            return None;
        }
        self.pages.insert(key, page.buf);
        let waiting = self.shown == Some(key) && self.image.as_ref().is_none_or(|(k, _)| *k != key);
        waiting.then(|| {
            self.version = self.version.wrapping_add(1);
            self.version
        })
    }

    fn request(&mut self, key: Key) {
        if self.pages.contains_key(&key) || !self.requested.insert(key) {
            return;
        }
        if let Some(renderer) = &self.renderer {
            renderer.want(self.seq, key.0, key.1);
        }
    }

    /// The shown page and its neighbors, and the first pages of both sides
    /// with their neighbors (what the render thread prefetches).
    fn keeps(&self, (side, page): Key) -> bool {
        let near = |a: u32, b: u32| a.abs_diff(b) <= 1;
        let home = match side {
            Side::Question => 0,
            Side::Answer => self.answer_start,
        };
        near(page, home) || self.shown.is_some_and(|(s, p)| s == side && near(page, p))
    }

    fn evict(&mut self) {
        let keep: Vec<Key> = self
            .pages
            .keys()
            .chain(self.requested.iter())
            .copied()
            .filter(|key| !self.keeps(*key))
            .collect();
        for key in keep {
            self.pages.remove(&key);
            self.requested.remove(&key);
        }
    }
}

/// A Slint image of the page. Slint has no gray format: gray pages are
/// expanded to RGB, and the backend converts them back for a gray panel.
fn to_image(buf: &PageBuf) -> Image {
    let mut pixels = SharedPixelBuffer::<Rgb8Pixel>::new(buf.width, buf.height);
    let out = pixels.make_mut_bytes();
    match buf.format {
        PageFormat::Rgb8 => out.copy_from_slice(&buf.data),
        PageFormat::Gray8 => {
            for (rgb, &gray) in out.as_chunks_mut::<3>().0.iter_mut().zip(&buf.data) {
                *rgb = [gray; 3];
            }
        }
    }
    Image::from_rgb8(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf() -> PageBuf {
        PageBuf {
            width: 2,
            height: 1,
            format: PageFormat::Gray8,
            data: vec![0, 255],
        }
    }

    fn store(question: u32, answer: u32, start: u32) -> PageStore {
        let mut store = PageStore {
            seq: 7,
            question_pages: question,
            answer_pages: answer,
            answer_start: start,
            ..Default::default()
        };
        store.pages.insert((Side::Question, 0), buf());
        store.pages.insert((Side::Answer, start), buf());
        store
    }

    fn ready(seq: u64, side: Side, page: u32) -> PageReady {
        PageReady {
            seq,
            side,
            page,
            buf: buf(),
        }
    }

    fn first(seq: u64, value: u8) -> FirstPages {
        let page = || PageBuf {
            width: 1,
            height: 1,
            format: PageFormat::Rgb8,
            data: vec![value; 3],
        };
        FirstPages {
            seq,
            question_pages: 1,
            answer_pages: 1,
            answer_start: 0,
            question: page(),
            answer: page(),
        }
    }

    fn pixel(image: &Image) -> u8 {
        image.to_rgb8().unwrap().as_bytes()[0]
    }

    #[test]
    fn a_new_card_does_not_show_the_old_card_image() {
        // Home from question page 1, then another deck: the new card asks for
        // the same page (question, 0) as the image still cached.
        let mut store = PageStore::default();
        store.install(first(1, 10));
        assert_eq!(pixel(&store.page_image(false, 0)), 10);
        store.install(first(2, 200));
        assert_eq!(pixel(&store.page_image(false, 0)), 200);
    }

    #[test]
    fn first_pages_show_at_once() {
        let mut store = store(3, 4, 2);
        assert_eq!(store.page_image(false, 0).size().width, 2);
        assert_eq!(store.page_image(true, 2).size().width, 2);
    }

    #[test]
    fn a_missing_page_keeps_the_old_image_until_delivered() {
        let mut store = store(5, 1, 0);
        let first = store.page_image(false, 0);
        // Page 2 is not here: the old image stays, and it is requested.
        let shown = store.page_image(false, 2);
        assert_eq!(shown, first);
        assert!(store.requested.contains(&(Side::Question, 2)));
        // A stale card's page and an unrelated page do not bump the version.
        assert_eq!(store.deliver(ready(6, Side::Question, 2)), None);
        assert_eq!(store.deliver(ready(7, Side::Question, 3)), None);
        // The waiting page bumps it once.
        assert_eq!(store.deliver(ready(7, Side::Question, 2)), Some(1));
        assert!(store.pages.contains_key(&(Side::Question, 2)));
    }

    #[test]
    fn far_pages_are_evicted() {
        let mut store = store(10, 1, 0);
        for page in 1..10 {
            store.pages.insert((Side::Question, page), buf());
        }
        store.page_image(false, 6);
        let mut kept: Vec<u32> = store
            .pages
            .keys()
            .filter(|(side, _)| *side == Side::Question)
            .map(|(_, page)| *page)
            .collect();
        kept.sort_unstable();
        assert_eq!(kept, vec![0, 1, 5, 6, 7]);
    }

    #[test]
    fn out_of_range_pages_are_empty() {
        let mut store = store(1, 1, 0);
        assert_eq!(store.page_image(false, 3).size().width, 0);
        assert_eq!(store.page_image(false, -1).size().width, 0);
    }
}
