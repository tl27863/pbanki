//! Card pages rendered with Blitz (notes/blitz-card-render.md).
//!
//! One thread owns the Blitz documents, the card fonts and the rasterizer.
//! Blitz documents are not `Send` (Stylo keeps thread-local state), and the
//! card-job worker is a new thread for each job, so the documents of the
//! current card live here. That lets the UI ask for more pages later.
//!
//! - `CardRenderer::render` (called on the card-job worker) lays out both
//!   sides, finds the page breaks, renders question page 1 and the answer
//!   start page, and returns them. Show answer then never waits.
//! - `CardRenderer::want` asks for one more page of the current card. The
//!   page arrives through the `deliver` callback given to `spawn`.

pub mod html;
mod paginate;
mod raster;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};

use paginate::{PageSpan, collect_bands, element_top, page_of, paginate};
use raster::{CardFonts, MediaProvider, Raster, absolute_media_folder, media_base_url};

/// Pixel format of the pages. It follows the panel, not the card: a gray
/// panel (`BB8` framebuffer) gets `Gray8`, a color panel (`RGB24`) and the
/// desktop get `Rgb8`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageFormat {
    Gray8,
    #[default]
    Rgb8,
}

impl PageFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Gray8 => 1,
            Self::Rgb8 => 3,
        }
    }
}

/// One rendered page.
pub struct PageBuf {
    pub width: u32,
    pub height: u32,
    pub format: PageFormat,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for PageBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PageBuf({}x{} {:?})",
            self.width, self.height, self.format
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Question,
    Answer,
}

impl Side {
    fn index(self) -> usize {
        match self {
            Self::Question => 0,
            Self::Answer => 1,
        }
    }
}

/// The card area in device pixels, and the CSS scale (the Slint scale factor,
/// so CSS `20px` is as large as Slint's `20px`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderArea {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

/// The HTML of both sides of one card.
#[derive(Clone, Debug)]
pub struct CardHtml {
    /// Complete documents (see `html::card_document`).
    pub question: String,
    pub answer: String,
    /// The family that covers the card text (`FontCatalog::choose_font`).
    /// It goes first in the font fallback chain.
    pub font_family: String,
}

/// The first pages of a card, from `CardRenderer::render`.
#[derive(Debug)]
pub struct FirstPages {
    /// Identifies the card in later `want` calls and `PageReady` deliveries.
    pub seq: u64,
    pub question_pages: u32,
    pub answer_pages: u32,
    /// The answer page that shows `#answer` (Anki's `<hr id=answer>`).
    pub answer_start: u32,
    /// Question page 0.
    pub question: PageBuf,
    /// Answer page `answer_start`.
    pub answer: PageBuf,
}

/// A page rendered after `FirstPages`, for `CardRenderer::want`.
#[derive(Debug)]
pub struct PageReady {
    pub seq: u64,
    pub side: Side,
    pub page: u32,
    pub buf: PageBuf,
}

/// Fixed settings of the render thread.
pub struct RendererConfig {
    pub format: PageFormat,
    /// Card font files, in configured order.
    pub font_files: Vec<PathBuf>,
    /// Card font families, in configured order.
    pub font_families: Vec<String>,
    /// The collection media folder, for `<img src>`.
    pub media_folder: PathBuf,
}

enum Msg {
    Render {
        seq: u64,
        html: CardHtml,
        area: RenderArea,
        reply: Sender<Result<FirstPages, String>>,
    },
    Want {
        seq: u64,
        side: Side,
        page: u32,
    },
}

/// Handle to the card-render thread. Cheap to clone and `Send`.
#[derive(Clone)]
pub struct CardRenderer {
    tx: Sender<Msg>,
    next_seq: Arc<AtomicU64>,
}

impl CardRenderer {
    /// Starts the thread. Fonts are loaded on the first `render`, not here,
    /// so startup does not wait for them.
    pub fn spawn(config: RendererConfig, deliver: Box<dyn Fn(PageReady) + Send>) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("pbanki-card-render".into())
            // Stylo and Taffy recurse over the tree.
            .stack_size(8 * 1024 * 1024)
            .spawn(move || RenderThread::new(config, deliver).run(rx))
            .expect("failed to spawn the card-render thread");
        Self {
            tx,
            next_seq: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Lays out and paginates both sides and renders their first pages.
    /// Blocks until done; call it off the UI thread. A new `render` stops
    /// the work for the previous card.
    pub fn render(&self, html: CardHtml, area: RenderArea) -> Result<FirstPages, String> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let (reply, result) = mpsc::channel();
        self.tx
            .send(Msg::Render {
                seq,
                html,
                area,
                reply,
            })
            .map_err(|_| "the card renderer stopped".to_owned())?;
        result
            .recv()
            .map_err(|_| "the card renderer stopped".to_owned())?
    }

    /// Asks for one page of card `seq`. Pages of an older card and pages out
    /// of range are ignored. Does not block.
    pub fn want(&self, seq: u64, side: Side, page: u32) {
        let _ = self.tx.send(Msg::Want { seq, side, page });
    }
}

struct Card {
    seq: u64,
    area: RenderArea,
    docs: [HtmlDocument; 2],
    pages: [Vec<PageSpan>; 2],
    queue: VecDeque<(Side, u32)>,
}

struct RenderThread {
    config: RendererConfig,
    deliver: Box<dyn Fn(PageReady) + Send>,
    fonts: Option<CardFonts>,
    raster: Option<Raster>,
    media: Arc<MediaProvider>,
    card: Option<Card>,
}

impl RenderThread {
    fn new(config: RendererConfig, deliver: Box<dyn Fn(PageReady) + Send>) -> Self {
        let mut config = config;
        config.media_folder = absolute_media_folder(&config.media_folder);
        let media = Arc::new(MediaProvider {
            folder: config.media_folder.clone(),
        });
        Self {
            config,
            deliver,
            fonts: None,
            raster: None,
            media,
            card: None,
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let pending = self
                .card
                .as_ref()
                .is_some_and(|card| !card.queue.is_empty());
            let msg = if pending {
                match rx.try_recv() {
                    Ok(msg) => Some(msg),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                }
            } else {
                match rx.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => return,
                }
            };
            match msg {
                Some(Msg::Render {
                    seq,
                    html,
                    area,
                    reply,
                }) => {
                    // Drop the old documents first: only one card in memory.
                    self.card = None;
                    let _ = reply.send(self.start_card(seq, html, area));
                }
                Some(Msg::Want { seq, side, page }) => {
                    if let Some(card) = self.card.as_mut()
                        && card.seq == seq
                        && (page as usize) < card.pages[side.index()].len()
                        && !card.queue.contains(&(side, page))
                    {
                        card.queue.push_back((side, page));
                    }
                }
                None => self.render_queued(),
            }
        }
    }

    fn start_card(
        &mut self,
        seq: u64,
        html: CardHtml,
        area: RenderArea,
    ) -> Result<FirstPages, String> {
        if area.width == 0 || area.height == 0 || area.scale <= 0.0 {
            return Err(format!("invalid card area {area:?}"));
        }
        let started = Instant::now();
        let fonts = self.fonts.get_or_insert_with(|| {
            let timer = Instant::now();
            let fonts = CardFonts::load(&self.config.font_files, &self.config.font_families);
            tracing::info!("[card] fonts loaded in {} ms", timer.elapsed().as_millis());
            fonts
        });
        let font_ctx = fonts.context_for(&html.font_family);
        let base_url = Some(media_base_url(&self.config.media_folder));
        let make_doc = |source: &str| {
            let mut doc = HtmlDocument::from_html(
                source,
                DocumentConfig {
                    viewport: Some(Viewport::new(
                        area.width,
                        area.height,
                        area.scale,
                        ColorScheme::Light,
                    )),
                    base_url: base_url.clone(),
                    net_provider: Some(self.media.clone()),
                    font_ctx: Some(font_ctx.clone()),
                    ..Default::default()
                },
            );
            // The media provider answers synchronously: load the images
            // before layout, so their size is known.
            doc.handle_messages();
            doc.resolve(0.0);
            doc
        };
        let mut docs = [make_doc(&html.question), make_doc(&html.answer)];
        let pages = [
            paginate(&collect_bands(&docs[0], area.scale), area.height),
            paginate(&collect_bands(&docs[1], area.scale), area.height),
        ];
        let answer_start = element_top(&docs[1], "answer", area.scale)
            .map(|y| page_of(&pages[1], y))
            .unwrap_or(0) as u32;
        let layout = started.elapsed();

        let raster = match self.raster.take() {
            Some(raster) if raster.size() == (area.width, area.height) => raster,
            _ => Raster::new(area.width, area.height),
        };
        let raster = self.raster.insert(raster);
        let format = self.config.format;
        let render = |raster: &mut Raster, doc: &mut HtmlDocument, span: PageSpan| {
            raster.render_page(doc, area.scale, span.top, span.keep, format)
        };
        let timer = Instant::now();
        let question = render(raster, &mut docs[0], pages[0][0]);
        let question_time = timer.elapsed();
        let timer = Instant::now();
        let answer = render(raster, &mut docs[1], pages[1][answer_start as usize]);
        let answer_time = timer.elapsed();

        tracing::info!(
            "[card] render seq={seq} {}x{}@{} pages q={} a={} a_start={answer_start} \
             layout={} ms q={} ms a={} ms total={} ms",
            area.width,
            area.height,
            area.scale,
            pages[0].len(),
            pages[1].len(),
            layout.as_millis(),
            question_time.as_millis(),
            answer_time.as_millis(),
            started.elapsed().as_millis()
        );

        // Prefetch the pages next to the first ones. The UI asks for the
        // rest as the user turns pages (`want`).
        let mut queue = VecDeque::new();
        if pages[0].len() > 1 {
            queue.push_back((Side::Question, 1));
        }
        if (answer_start as usize) + 1 < pages[1].len() {
            queue.push_back((Side::Answer, answer_start + 1));
        }
        if answer_start > 0 {
            queue.push_back((Side::Answer, answer_start - 1));
        }

        let first = FirstPages {
            seq,
            question_pages: pages[0].len() as u32,
            answer_pages: pages[1].len() as u32,
            answer_start,
            question,
            answer,
        };
        self.card = Some(Card {
            seq,
            area,
            docs,
            pages,
            queue,
        });
        Ok(first)
    }

    fn render_queued(&mut self) {
        let (Some(card), Some(raster)) = (self.card.as_mut(), self.raster.as_mut()) else {
            return;
        };
        let Some((side, page)) = card.queue.pop_front() else {
            return;
        };
        let started = Instant::now();
        let span = card.pages[side.index()][page as usize];
        let buf = raster.render_page(
            &mut card.docs[side.index()],
            card.area.scale,
            span.top,
            span.keep,
            self.config.format,
        );
        log_page(card.seq, side, page, started.elapsed());
        (self.deliver)(PageReady {
            seq: card.seq,
            side,
            page,
            buf,
        });
    }
}

fn log_page(seq: u64, side: Side, page: u32, elapsed: Duration) {
    tracing::info!(
        "[card] page seq={seq} {side:?} {page} in {} ms",
        elapsed.as_millis()
    );
}

#[cfg(test)]
mod tests;
