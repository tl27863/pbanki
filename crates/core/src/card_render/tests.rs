//! Host tests with real Blitz layouts. They need one font file on the host;
//! without one they log and pass.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use super::html::card_document;
use super::paginate::{collect_bands, paginate};
use super::*;

const FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
];

fn host_font() -> Option<PathBuf> {
    let found = FONT_CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|path| path.exists());
    if found.is_none() {
        // ast-grep-ignore: pbanki-no-eprintln -- test-only skip notice; this file is `mod tests`, declared in mod.rs
        eprintln!("no host font found; skipping the Blitz layout test");
    }
    found
}

const AREA: RenderArea = RenderArea {
    width: 600,
    height: 400,
    scale: 2.0,
};

/// Anki's default card CSS: a family that is not installed.
const ANKI_CSS: &str =
    ".card { font-family: arial; font-size: 20px; text-align: center; color: black; }";

fn doc(body: &str, css: &str) -> String {
    card_document(body, css, 0, AREA.height as f32 / AREA.scale, true)
}

fn long_text(paragraphs: usize) -> String {
    (0..paragraphs)
        .map(|i| {
            format!(
                "<p>Paragraph {i}. The Peace of Westphalia ended the Thirty Years' War and \
                 is often cited as the origin of the modern system of sovereign states.</p>"
            )
        })
        .collect()
}

fn spawn(
    font: PathBuf,
    format: PageFormat,
    media: PathBuf,
) -> (CardRenderer, mpsc::Receiver<PageReady>) {
    let (tx, rx) = mpsc::channel();
    let renderer = CardRenderer::spawn(
        RendererConfig {
            format,
            font_files: vec![font],
            font_families: Vec::new(),
            media_folder: media,
            image_cache: ImageCacheSize::DEFAULT,
        },
        Box::new(move |page| {
            let _ = tx.send(page);
        }),
    );
    (renderer, rx)
}

fn card(question: &str, answer: &str) -> CardHtml {
    CardHtml {
        question: doc(question, ANKI_CSS),
        answer: doc(answer, ANKI_CSS),
        font_family: String::new(),
    }
}

fn ink_rows(page: &PageBuf) -> Vec<usize> {
    let row = page.width as usize * page.format.bytes_per_pixel();
    page.data
        .chunks_exact(row)
        .enumerate()
        .filter(|(_, r)| r.iter().any(|&v| v < 128))
        .map(|(y, _)| y)
        .collect()
}

/// Lays out `html` on this thread, like the render thread does.
fn layout(html: &str, font: &PathBuf, scale: f32) -> HtmlDocument {
    let mut fonts = raster::CardFonts::load(std::slice::from_ref(font), &[]);
    let mut doc = HtmlDocument::from_html(
        html,
        DocumentConfig {
            viewport: Some(Viewport::new(
                (AREA.width as f32 / AREA.scale * scale) as u32,
                (AREA.height as f32 / AREA.scale * scale) as u32,
                scale,
                ColorScheme::Light,
            )),
            font_ctx: Some(fonts.context_for("")),
            ..Default::default()
        },
    );
    doc.resolve(0.0);
    doc
}

#[test]
fn line_bands_match_the_painted_ink() {
    // The bands must be where blitz-paint draws the text: every ink row is
    // inside a band, and every band has ink. A unit error (CSS px against
    // device px, or a missing offset) breaks this at scale 2.
    let Some(font) = host_font() else { return };
    let mut doc = layout(&doc(&long_text(2), ANKI_CSS), &font, AREA.scale);
    let bands = collect_bands(&doc, AREA.scale);
    assert!(bands.len() >= 4, "{bands:?}");
    for pair in bands.windows(2) {
        assert!(pair[0].bottom <= pair[1].top + 0.5, "{pair:?}");
    }
    let mut raster = raster::Raster::new(AREA.width, AREA.height);
    let (page, _) = raster.render_page(&mut doc, AREA.scale, 0.0, AREA.height, PageFormat::Gray8);
    let ink = ink_rows(&page);
    assert!(!ink.is_empty());
    for y in &ink {
        let y = *y as f32;
        assert!(
            bands
                .iter()
                .any(|b| y >= b.top.floor() && y < b.bottom.ceil()),
            "ink row {y} outside the bands {bands:?}"
        );
    }
    for band in bands.iter().filter(|b| b.bottom <= AREA.height as f32) {
        assert!(
            ink.iter()
                .any(|&y| (y as f32) >= band.top && (y as f32) < band.bottom),
            "{band:?} has no ink"
        );
    }
}

#[test]
fn long_card_pages_cut_no_line() {
    let Some(font) = host_font() else { return };
    let doc = layout(&doc(&long_text(12), ANKI_CSS), &font, AREA.scale);
    let bands = collect_bands(&doc, AREA.scale);
    let pages = paginate(&bands, AREA.height);
    assert!(pages.len() >= 3, "{pages:?}");
    for band in &bands {
        let page = pages
            .iter()
            .find(|p| band.top >= p.top && band.top < p.top + p.keep as f32)
            .unwrap_or_else(|| panic!("{band:?} on no page"));
        assert!(
            band.bottom <= page.top + page.keep as f32 + 0.5,
            "{band:?} cut by {page:?}"
        );
    }
}

#[test]
fn renders_first_pages_and_wanted_pages() {
    let Some(font) = host_font() else { return };
    let (renderer, pages_rx) = spawn(font, PageFormat::Gray8, std::env::temp_dir());
    let question = long_text(12);
    let answer = format!("{question}<hr id=answer><p>The answer.</p>");
    let first = renderer.render(card(&question, &answer), AREA).unwrap();

    assert!(first.question_pages >= 3);
    assert_eq!(
        first.question.data.len(),
        (AREA.width * AREA.height) as usize
    );
    assert!(
        !ink_rows(&first.question).is_empty(),
        "no text: fallback font not used"
    );
    // The answer opens on the page with `#answer`, past the repeated question.
    assert!(first.answer_start > 0);
    assert_eq!(first.answer_start + 1, first.answer_pages);

    // Prefetch: question page 1 and answer page `answer_start - 1`.
    let mut prefetched: Vec<(Side, u32)> = (0..2)
        .map(|_| {
            let page = pages_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            assert_eq!(page.seq, first.seq);
            (page.side, page.page)
        })
        .collect();
    prefetched.sort_by_key(|(side, page)| (side.index(), *page));
    assert_eq!(
        prefetched,
        vec![(Side::Question, 1), (Side::Answer, first.answer_start - 1)]
    );

    renderer.want(first.seq, Side::Question, 2);
    renderer.want(first.seq, Side::Question, 999);
    renderer.want(first.seq + 100, Side::Question, 0);
    let page = pages_rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!((page.side, page.page), (Side::Question, 2));
    assert!(pages_rx.recv_timeout(Duration::from_millis(500)).is_err());
}

#[test]
fn a_new_card_stops_the_prefetch() {
    let Some(font) = host_font() else { return };
    let (renderer, pages_rx) = spawn(font, PageFormat::Gray8, std::env::temp_dir());
    let long = long_text(12);
    let first = renderer.render(card(&long, &long), AREA).unwrap();
    let second = renderer
        .render(card("<p>Short</p>", "<p>Short</p>"), AREA)
        .unwrap();
    assert!(second.seq > first.seq);
    assert_eq!(second.question_pages, 1);
    // At most one page of the first card was in progress.
    let late: Vec<_> =
        std::iter::from_fn(|| pages_rx.recv_timeout(Duration::from_millis(300)).ok()).collect();
    assert!(late.len() <= 1, "{late:?}");
    assert!(late.iter().all(|p| p.seq == first.seq));
}

#[test]
fn short_card_is_one_centered_page() {
    let Some(font) = host_font() else { return };
    let (renderer, _rx) = spawn(font, PageFormat::Gray8, std::env::temp_dir());
    let first = renderer
        .render(
            card("<p>Hello</p>", "<p>Hello</p><hr id=answer>World"),
            AREA,
        )
        .unwrap();
    assert_eq!(
        (first.question_pages, first.answer_pages, first.answer_start),
        (1, 1, 0)
    );
    let rows = ink_rows(&first.question);
    let (top, bottom) = (rows[0], *rows.last().unwrap());
    let above = top;
    let below = AREA.height as usize - 1 - bottom;
    assert!(
        above.abs_diff(below) <= 12,
        "ink {top}..{bottom} not centered"
    );
}

#[test]
fn color_pages_keep_colors() {
    let Some(font) = host_font() else { return };
    let (renderer, _rx) = spawn(font, PageFormat::Rgb8, std::env::temp_dir());
    let css = format!("{ANKI_CSS} .card {{ color: rgb(255, 0, 0); font-size: 60px; }}");
    let html = CardHtml {
        question: doc("<p>RED</p>", &css),
        answer: doc("<p>RED</p>", &css),
        font_family: String::new(),
    };
    let first = renderer.render(html, AREA).unwrap();
    assert_eq!(first.question.format, PageFormat::Rgb8);
    let red = first
        .question
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .any(|px| px[0] > 200 && px[1] < 60 && px[2] < 60);
    assert!(red, "no red pixel on a color page");
}

#[test]
fn images_load_from_the_media_folder() {
    let Some(font) = host_font() else { return };
    let media = std::env::temp_dir().join(format!("pbanki-media-{}", std::process::id()));
    std::fs::create_dir_all(&media).unwrap();
    image::RgbImage::from_pixel(40, 30, image::Rgb([0, 0, 0]))
        .save(media.join("black box.png"))
        .unwrap();
    let (renderer, _rx) = spawn(font, PageFormat::Gray8, media.clone());
    let html = card(
        "<img src=\"black%20box.png\" style=\"width: 100px; height: 75px\">",
        "<img src=\"../escape.png\">",
    );
    let first = renderer.render(html, AREA).unwrap();
    let rows = ink_rows(&first.question);
    // 75 CSS px at scale 2.
    assert!(rows.len() >= 140, "image rows: {}", rows.len());
    let _ = std::fs::remove_dir_all(&media);
}

#[test]
fn relative_media_folder_does_not_panic() {
    // The desktop config uses a relative collection path. A relative folder
    // once left Blitz without a `file:` base URL, and a relative `<img src>`
    // panicked the render thread.
    let Some(font) = host_font() else { return };
    let (renderer, _rx) = spawn(
        font,
        PageFormat::Gray8,
        PathBuf::from("./no-such-dir/collection.media"),
    );
    let html = card("<img src=\"missing.png\">", "<img src=\"a b.png\">");
    let first = renderer.render(html, AREA).unwrap();
    assert_eq!(first.question_pages, 1);
    // The thread is still alive.
    renderer.render(card("<p>x</p>", "<p>y</p>"), AREA).unwrap();
}

#[test]
fn fade_in_animations_show_their_end_state() {
    // The "Yap" decks fade the answer in from `opacity: 0`. Resolved at
    // `t = 0` without the override, the answer was laid out but blank.
    let Some(font) = host_font() else { return };
    let (renderer, _rx) = spawn(font, PageFormat::Gray8, std::env::temp_dir());
    let body = "<style>.answer { animation: fade 1s ease-out; } \
                @keyframes fade { from { opacity: 0; } }</style>\
                <div class=answer><p>ANSWER</p></div>";
    let first = renderer.render(card(body, body), AREA).unwrap();
    assert!(
        !ink_rows(&first.answer).is_empty(),
        "animated answer is blank"
    );
}

#[test]
fn ruby_text_sits_above_its_base() {
    // Anki's `{{furigana:}}` filter emits `<ruby><rb>分</rb><rt>わ</rt></ruby>`.
    // Blitz has no ruby layout: without the base CSS the reading is inline
    // text on the same row (one band of ink). With it, the reading is a
    // separate, smaller band above the base.
    let Some(font) = host_font() else { return };
    let (renderer, _rx) = spawn(font, PageFormat::Gray8, std::env::temp_dir());
    let body = "<p style=\"font-size: 60px\"><ruby><rb>AB</rb><rt>xy</rt></ruby></p>";
    let first = renderer.render(card(body, body), AREA).unwrap();
    let rows = ink_rows(&first.question);
    let runs: Vec<(usize, usize)> = rows.iter().fold(Vec::new(), |mut runs, &y| {
        match runs.last_mut() {
            Some((_, end)) if *end + 1 == y => *end = y,
            _ => runs.push((y, y)),
        }
        runs
    });
    assert_eq!(runs.len(), 2, "reading not above the base: {runs:?}");
    let (reading, base) = (runs[0], runs[1]);
    assert!(reading.1 - reading.0 < base.1 - base.0, "{runs:?}");
}

#[test]
fn a_card_rendered_ahead_gives_the_same_pages() {
    let Some(font) = host_font() else { return };
    let (renderer, _rx) = spawn(font.clone(), PageFormat::Gray8, std::env::temp_dir());
    let next = card("<p>Next</p>", "<p>Next</p><hr id=answer><p>Its answer.</p>");
    renderer
        .render(card("<p>Now</p>", "<p>Now</p>"), AREA)
        .unwrap();
    renderer.lookahead(vec![next.clone()], AREA);
    let ahead = renderer.render(next.clone(), AREA).unwrap();
    assert!(ahead.from_lookahead);

    let (fresh_renderer, _rx) = spawn(font, PageFormat::Gray8, std::env::temp_dir());
    let fresh = fresh_renderer.render(next, AREA).unwrap();
    assert!(!fresh.from_lookahead);
    assert_eq!(
        (ahead.question_pages, ahead.answer_pages, ahead.answer_start),
        (fresh.question_pages, fresh.answer_pages, fresh.answer_start)
    );
    assert!(ahead.question.data == fresh.question.data);
    assert!(ahead.answer.data == fresh.answer.data);
}

#[test]
fn a_changed_card_or_area_is_rendered_again() {
    let Some(font) = host_font() else { return };
    let (renderer, _rx) = spawn(font, PageFormat::Gray8, std::env::temp_dir());
    let next = card("<p>Next</p>", "<p>Next</p>");
    renderer.lookahead(vec![next.clone()], AREA);
    // The note was edited: another string.
    let edited = card("<p>Next!</p>", "<p>Next</p>");
    assert!(!renderer.render(edited, AREA).unwrap().from_lookahead);

    renderer.lookahead(vec![next.clone()], AREA);
    let smaller = RenderArea {
        height: AREA.height - 10,
        ..AREA
    };
    assert!(!renderer.render(next, smaller).unwrap().from_lookahead);
}

#[test]
fn a_new_lookahead_list_keeps_only_its_cards() {
    let Some(font) = host_font() else { return };
    let (renderer, _rx) = spawn(font, PageFormat::Gray8, std::env::temp_dir());
    let [b, c, d] = ["B", "C", "D"].map(|text| card(&format!("<p>{text}</p>"), "<p>x</p>"));
    renderer.lookahead(vec![b.clone(), c.clone()], AREA);
    assert!(renderer.render(b.clone(), AREA).unwrap().from_lookahead);
    // The next rating: C is still ahead, D is new.
    renderer.lookahead(vec![c.clone(), d.clone()], AREA);
    assert!(renderer.render(c, AREA).unwrap().from_lookahead);
    // A deck entry clears the list.
    renderer.lookahead(Vec::new(), AREA);
    assert!(!renderer.render(d, AREA).unwrap().from_lookahead);
    // A card that was served is not served twice.
    assert!(!renderer.render(b, AREA).unwrap().from_lookahead);
}

#[test]
fn wanted_pages_still_arrive_during_lookahead_work() {
    let Some(font) = host_font() else { return };
    let (renderer, pages_rx) = spawn(font, PageFormat::Gray8, std::env::temp_dir());
    let long = long_text(12);
    let first = renderer.render(card(&long, &long), AREA).unwrap();
    renderer.lookahead(
        vec![
            card(&long_text(30), &long_text(30)),
            card(&long_text(31), &long_text(31)),
        ],
        AREA,
    );
    renderer.want(first.seq, Side::Question, 2);
    // The two prefetched pages (no `#answer`: answer page 1 too) and the
    // wanted page arrive. Pages of the card on screen go before lookahead
    // work (`RenderThread::work_unit`).
    let mut got: Vec<(Side, u32)> = (0..3)
        .map(|_| {
            let page = pages_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            assert_eq!(page.seq, first.seq);
            (page.side, page.page)
        })
        .collect();
    got.sort_by_key(|(side, page)| (side.index(), *page));
    assert_eq!(
        got,
        vec![(Side::Question, 1), (Side::Question, 2), (Side::Answer, 1)]
    );
}
