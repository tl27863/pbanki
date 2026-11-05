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
mod image_cache;
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

pub use image_cache::ImageCacheSize;
use paginate::{PageSpan, collect_bands, element_top, page_of, paginate};
use raster::{
    CardFonts, MediaProvider, MediaStats, PageTimes, Raster, absolute_media_folder, media_base_url,
};

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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// The card was in the lookahead list (`CardRenderer::lookahead`), so
    /// some or all of the work was done before this `render`.
    pub from_lookahead: bool,
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
    /// Memory for decoded images.
    pub image_cache: ImageCacheSize,
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
    Lookahead {
        cards: Vec<CardHtml>,
        area: RenderArea,
    },
}

/// Cards rendered ahead at most (`CardRenderer::lookahead`). Each keeps two
/// documents and two pages: about 5 MB on the PB710.
const MAX_AHEAD: usize = 2;

const STOPPED: &str = "the card renderer stopped";

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
        self.start_render(html, area)?.wait()
    }

    /// `render` without the wait: the caller can work while the render
    /// thread renders, then calls `PendingRender::wait`.
    pub fn start_render(&self, html: CardHtml, area: RenderArea) -> Result<PendingRender, String> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let (reply, result) = mpsc::channel();
        self.tx
            .send(Msg::Render {
                seq,
                html,
                area,
                reply,
            })
            .map_err(|_| STOPPED.to_owned())?;
        Ok(PendingRender { result })
    }

    /// The cards that probably come next, in queue order (at most
    /// `MAX_AHEAD` are used). The render thread lays them out and renders
    /// their first pages when it has nothing else to do. A later `render` of
    /// the same documents and area answers from that work. The list replaces
    /// the previous one: cards in both keep their work, the others are
    /// dropped. An empty list drops all. Does not block.
    pub fn lookahead(&self, cards: Vec<CardHtml>, area: RenderArea) {
        let _ = self.tx.send(Msg::Lookahead { cards, area });
    }

    /// Asks for one page of card `seq`. Pages of an older card and pages out
    /// of range are ignored. Does not block.
    pub fn want(&self, seq: u64, side: Side, page: u32) {
        let _ = self.tx.send(Msg::Want { seq, side, page });
    }
}

/// A `render` in progress (`CardRenderer::start_render`).
pub struct PendingRender {
    result: Receiver<Result<FirstPages, String>>,
}

impl PendingRender {
    pub fn wait(self) -> Result<FirstPages, String> {
        self.result.recv().map_err(|_| STOPPED.to_owned())?
    }
}

/// Both documents of a card, laid out, and their page breaks.
struct Laid {
    area: RenderArea,
    docs: [HtmlDocument; 2],
    pages: [Vec<PageSpan>; 2],
    /// The answer page that shows `#answer`.
    answer_start: u32,
    steps: LayoutTimes,
    /// All of the layout: `steps` and the time between them.
    time: Duration,
}

impl Laid {
    /// The first page of a side: question page 0, the answer start page.
    fn first_page(&self, side: Side) -> u32 {
        match side {
            Side::Question => 0,
            Side::Answer => self.answer_start,
        }
    }
}

/// A rendered page and where its time went.
type Rendered = (PageBuf, PageTimes);

/// The card on screen: its documents stay for the pages after the first.
struct Card {
    seq: u64,
    laid: Laid,
    queue: VecDeque<(Side, u32)>,
}

/// A card rendered ahead (`CardRenderer::lookahead`). The work goes in units
/// (the layout, then each first page), so a message waits at most one unit.
struct Ahead {
    html: CardHtml,
    area: RenderArea,
    laid: Option<Laid>,
    question: Option<Rendered>,
    answer: Option<Rendered>,
}

impl Ahead {
    fn new(html: CardHtml, area: RenderArea) -> Self {
        Self {
            html,
            area,
            laid: None,
            question: None,
            answer: None,
        }
    }

    /// Same documents, font and area: the work would give the same pages.
    fn matches(&self, html: &CardHtml, area: RenderArea) -> bool {
        self.area == area && self.html == *html
    }

    fn is_done(&self) -> bool {
        self.question.is_some() && self.answer.is_some()
    }

    /// The parts done so far, for the logs: `layout+q+a`, `layout`, or `-`.
    fn parts(&self) -> String {
        let parts: Vec<&str> = [
            (self.laid.is_some(), "layout"),
            (self.question.is_some(), "q"),
            (self.answer.is_some(), "a"),
        ]
        .into_iter()
        .filter_map(|(done, name)| done.then_some(name))
        .collect();
        if parts.is_empty() {
            "-".into()
        } else {
            parts.join("+")
        }
    }

    /// The time spent on the parts done so far.
    fn work(&self) -> Duration {
        let page = |r: &Option<Rendered>| r.as_ref().map_or(Duration::ZERO, |(_, t)| t.total());
        self.laid.as_ref().map_or(Duration::ZERO, |l| l.time)
            + page(&self.question)
            + page(&self.answer)
    }
}

struct RenderThread {
    config: RendererConfig,
    deliver: Box<dyn Fn(PageReady) + Send>,
    fonts: Option<CardFonts>,
    raster: Option<Raster>,
    media: Arc<MediaProvider>,
    card: Option<Card>,
    /// Cards rendered ahead, in queue order.
    ahead: Vec<Ahead>,
}

impl RenderThread {
    fn new(config: RendererConfig, deliver: Box<dyn Fn(PageReady) + Send>) -> Self {
        let mut config = config;
        config.media_folder = absolute_media_folder(&config.media_folder);
        let media = Arc::new(MediaProvider::new(
            config.media_folder.clone(),
            config.image_cache,
        ));
        Self {
            config,
            deliver,
            fonts: None,
            raster: None,
            media,
            card: None,
            ahead: Vec::new(),
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            // Between work units, take messages without waiting: a `Render`
            // or a `Want` goes before the rest of the work.
            let msg = if self.has_work() {
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
                    let _ = reply.send(self.render_card(seq, html, area));
                }
                Some(Msg::Want { seq, side, page }) => {
                    if let Some(card) = self.card.as_mut()
                        && card.seq == seq
                        && (page as usize) < card.laid.pages[side.index()].len()
                        && !card.queue.contains(&(side, page))
                    {
                        card.queue.push_back((side, page));
                    }
                }
                Some(Msg::Lookahead { cards, area }) => self.set_lookahead(cards, area),
                None => self.work_unit(),
            }
        }
    }

    fn has_work(&self) -> bool {
        self.card
            .as_ref()
            .is_some_and(|card| !card.queue.is_empty())
            || self.ahead.iter().any(|ahead| !ahead.is_done())
    }

    /// One unit: a page of the card on screen first, then lookahead work.
    fn work_unit(&mut self) {
        if self
            .card
            .as_ref()
            .is_some_and(|card| !card.queue.is_empty())
        {
            self.render_queued();
        } else {
            self.ahead_unit();
        }
    }

    fn render_card(
        &mut self,
        seq: u64,
        html: CardHtml,
        area: RenderArea,
    ) -> Result<FirstPages, String> {
        if area.width == 0 || area.height == 0 || area.scale <= 0.0 {
            return Err(format!("invalid card area {area:?}"));
        }
        let started = Instant::now();
        // Drop the old documents first: the card on screen and the lookahead
        // cards are the only ones in memory.
        self.card = None;
        let ahead = self
            .ahead
            .iter()
            .position(|ahead| ahead.matches(&html, area))
            .map(|index| self.ahead.remove(index));
        let ahead_parts = ahead.as_ref().map(Ahead::parts);
        let (laid, question, answer) = match ahead {
            Some(ahead) => (ahead.laid, ahead.question, ahead.answer),
            None => (None, None, None),
        };
        let mut laid = match laid {
            Some(laid) => laid,
            None => self.layout(&html, area),
        };
        let (question, question_times) = match question {
            Some(rendered) => rendered,
            None => self.render_first(&mut laid, Side::Question),
        };
        let (answer, answer_times) = match answer {
            Some(rendered) => rendered,
            None => self.render_first(&mut laid, Side::Answer),
        };

        // `layout` covers both documents: parse (with the media, which Blitz
        // decodes while parsing), style and layout, and the page breaks.
        // `total` is the work, also when it was done ahead; `wait` is the
        // time of this call.
        let from_lookahead = ahead_parts.is_some();
        let ahead_note = ahead_parts.map_or(String::new(), |parts| format!(" ahead={parts}"));
        tracing::info!(
            "[card] render seq={seq} {}x{}@{} pages q={} a={} a_start={} \
             layout={} ms ({}) q={question_times} a={answer_times} total={} ms wait={} ms{ahead_note}",
            area.width,
            area.height,
            area.scale,
            laid.pages[0].len(),
            laid.pages[1].len(),
            laid.answer_start,
            Ms(laid.time),
            laid.steps,
            Ms(laid.time + question_times.total() + answer_times.total()),
            Ms(started.elapsed())
        );

        // Prefetch the pages next to the first ones. The UI asks for the
        // rest as the user turns pages (`want`).
        let answer_start = laid.answer_start;
        let mut queue = VecDeque::new();
        if laid.pages[0].len() > 1 {
            queue.push_back((Side::Question, 1));
        }
        if (answer_start as usize) + 1 < laid.pages[1].len() {
            queue.push_back((Side::Answer, answer_start + 1));
        }
        if answer_start > 0 {
            queue.push_back((Side::Answer, answer_start - 1));
        }

        let first = FirstPages {
            seq,
            question_pages: laid.pages[0].len() as u32,
            answer_pages: laid.pages[1].len() as u32,
            answer_start,
            question,
            answer,
            from_lookahead,
        };
        self.card = Some(Card { seq, laid, queue });
        Ok(first)
    }

    /// Parses, lays out and paginates both documents of a card.
    fn layout(&mut self, html: &CardHtml, area: RenderArea) -> Laid {
        let started = Instant::now();
        let fonts = self.fonts.get_or_insert_with(|| {
            let timer = Instant::now();
            let fonts = CardFonts::load(&self.config.font_files, &self.config.font_families);
            tracing::info!("[card] fonts loaded in {} ms", timer.elapsed().as_millis());
            fonts
        });
        let font_ctx = fonts.context_for(&html.font_family);
        let base_url = Some(media_base_url(&self.config.media_folder));
        let mut steps = LayoutTimes::default();
        let mut make_doc = |source: &str| {
            let timer = Instant::now();
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
            steps.parse += timer.elapsed();
            steps.media.add(self.media.take_stats());
            let timer = Instant::now();
            doc.resolve(0.0);
            steps.resolve += timer.elapsed();
            doc
        };
        let docs = [make_doc(&html.question), make_doc(&html.answer)];
        let timer = Instant::now();
        let pages = [
            paginate(&collect_bands(&docs[0], area.scale), area.height),
            paginate(&collect_bands(&docs[1], area.scale), area.height),
        ];
        let answer_start = element_top(&docs[1], "answer", area.scale)
            .map(|y| page_of(&pages[1], y))
            .unwrap_or(0) as u32;
        steps.breaks = timer.elapsed();
        Laid {
            area,
            docs,
            pages,
            answer_start,
            steps,
            time: started.elapsed(),
        }
    }

    /// Renders the first page of one side.
    fn render_first(&mut self, laid: &mut Laid, side: Side) -> Rendered {
        let page = laid.first_page(side);
        self.render_page(laid, side, page)
    }

    fn render_page(&mut self, laid: &mut Laid, side: Side, page: u32) -> Rendered {
        let area = laid.area;
        let raster = match self.raster.take() {
            Some(raster) if raster.size() == (area.width, area.height) => raster,
            _ => Raster::new(area.width, area.height),
        };
        let raster = self.raster.insert(raster);
        let span = laid.pages[side.index()][page as usize];
        raster.render_page(
            &mut laid.docs[side.index()],
            area.scale,
            span.top,
            span.keep,
            self.config.format,
        )
    }

    fn render_queued(&mut self) {
        let Some(mut card) = self.card.take() else {
            return;
        };
        if let Some((side, page)) = card.queue.pop_front() {
            let (buf, times) = self.render_page(&mut card.laid, side, page);
            tracing::info!("[card] page seq={} {side:?} {page} in {times}", card.seq);
            (self.deliver)(PageReady {
                seq: card.seq,
                side,
                page,
                buf,
            });
        }
        self.card = Some(card);
    }

    /// Replaces the lookahead list. Cards in the old and the new list keep
    /// their work.
    fn set_lookahead(&mut self, cards: Vec<CardHtml>, area: RenderArea) {
        let mut old = std::mem::take(&mut self.ahead);
        if area.width > 0 && area.height > 0 && area.scale > 0.0 {
            for html in cards.into_iter().take(MAX_AHEAD) {
                match old.iter().position(|ahead| ahead.matches(&html, area)) {
                    Some(index) => self.ahead.push(old.remove(index)),
                    None => self.ahead.push(Ahead::new(html, area)),
                }
            }
        }
        for unused in old.into_iter().filter(|ahead| ahead.laid.is_some()) {
            tracing::info!(
                "[card] ahead unused: {} done in {} ms",
                unused.parts(),
                Ms(unused.work())
            );
        }
    }

    /// One unit of lookahead work: the first card that is not done gets its
    /// layout or its next first page.
    fn ahead_unit(&mut self) {
        let Some(index) = self.ahead.iter().position(|ahead| !ahead.is_done()) else {
            return;
        };
        let mut ahead = self.ahead.remove(index);
        match ahead.laid.take() {
            None => ahead.laid = Some(self.layout(&ahead.html, ahead.area)),
            Some(mut laid) => {
                let side = if ahead.question.is_none() {
                    Side::Question
                } else {
                    Side::Answer
                };
                let rendered = self.render_first(&mut laid, side);
                match side {
                    Side::Question => ahead.question = Some(rendered),
                    Side::Answer => ahead.answer = Some(rendered),
                }
                ahead.laid = Some(laid);
            }
        }
        self.ahead.insert(index, ahead);
    }
}

/// Where the layout time of a card went (both documents together).
#[derive(Default)]
struct LayoutTimes {
    /// `HtmlDocument::from_html` and `handle_messages`, media included.
    parse: Duration,
    media: MediaStats,
    resolve: Duration,
    breaks: Duration,
}

/// A duration in the `[card]` log lines: milliseconds with one decimal, so
/// host renders (often below 1 ms) can be compared too.
struct Ms(Duration);

impl std::fmt::Display for Ms {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.1}", self.0.as_secs_f64() * 1000.0)
    }
}

impl std::fmt::Display for LayoutTimes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ms = Ms;
        write!(
            f,
            "parse {} incl. media {} file(s) read {} decode {} ({} cached), resolve {}, breaks {}",
            ms(self.parse),
            self.media.files,
            ms(self.media.read),
            ms(self.media.decode),
            self.media.cached,
            ms(self.resolve),
            ms(self.breaks)
        )
    }
}

#[cfg(test)]
mod tests;
