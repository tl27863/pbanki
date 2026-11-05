//! Renders every card of an `.apkg` with the app's card renderer and logs
//! the same `[card]` lines as the device (notes/test-decks.md).
//!
//! ```sh
//! cargo run --profile host-bench -p pbanki-core --example render_deck -- \
//!     target/test-decks/pbanki-css-bench.apkg --runs 3 2> /tmp/render-deck.log
//! python3 -I scripts/test-decks/analyze-render-log.py /tmp/render-deck.log
//! ```
//!
//! Options: `--runs N` (renders of each card, default 3), `--rgb` (`Rgb8`
//! pages; default `Gray8` as on the PB710), `--area WxH@scale` (default the
//! PB710 card area, `1144x1053@3`), `--font-dir DIR` and `--family NAME` (the
//! card font, default Noto Sans CJK JP), `--only TEXT` (cards whose first
//! field contains TEXT), `--dump DIR` (writes the pages of the first run as
//! PGM or PPM), `--shadows` (`card_shadows = true`; off by default, as in the app), `--no-image-cache`
//! (decode every image again, as before the cache; the cache also keeps
//! images between runs, so later runs of an image card are hits),
//! `--lookahead MS` (after each card, send the next two as lookahead and
//! wait MS, the reading time; the first run of the next card is then a
//! lookahead hit, `ahead=…` in its render line).
//!
//! The package is imported into a new collection in the temp folder. Each
//! card is rendered as `next_card` renders it (`study::card_documents`, then
//! `CardRenderer::render`); then every other page is rendered.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anki::card::Card;
use anki::collection::{Collection, CollectionBuilder};
use anki::import_export::package::ImportAnkiPackageOptions;
use anki::search::SortMode;

use pbanki_core::card_render::{
    CardRenderer, ImageCacheSize, PageBuf, PageFormat, PageReady, RenderArea, RendererConfig, Side,
};
use pbanki_core::fonts::{FontCatalog, card_font_families, card_font_files};
use pbanki_core::study::card_documents;

struct Args {
    apkg: PathBuf,
    runs: u32,
    format: PageFormat,
    area: RenderArea,
    font_dir: String,
    family: String,
    only: Option<String>,
    dump: Option<PathBuf>,
    shadows: bool,
    image_cache: ImageCacheSize,
    lookahead: Option<Duration>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        apkg: PathBuf::new(),
        runs: 3,
        format: PageFormat::Gray8,
        area: RenderArea {
            width: 1144,
            height: 1053,
            scale: 3.0,
        },
        font_dir: "/usr/share/fonts/noto-cjk".into(),
        family: "Noto Sans CJK JP".into(),
        only: None,
        dump: None,
        shadows: false,
        image_cache: ImageCacheSize::DEFAULT,
        lookahead: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--runs" => args.runs = value()?.parse().map_err(|e| format!("--runs: {e}"))?,
            "--rgb" => args.format = PageFormat::Rgb8,
            "--area" => args.area = parse_area(&value()?)?,
            "--font-dir" => args.font_dir = value()?,
            "--family" => args.family = value()?,
            "--only" => args.only = Some(value()?),
            "--dump" => args.dump = Some(PathBuf::from(value()?)),
            "--shadows" => args.shadows = true,
            "--no-image-cache" => args.image_cache = ImageCacheSize::OFF,
            "--lookahead" => {
                let ms = value()?.parse().map_err(|e| format!("--lookahead: {e}"))?;
                args.lookahead = Some(Duration::from_millis(ms));
            }
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}")),
            _ => args.apkg = PathBuf::from(arg),
        }
    }
    if args.apkg.as_os_str().is_empty() {
        return Err("usage: render_deck <file.apkg> [options] (see the file header)".into());
    }
    args.runs = args.runs.max(1);
    Ok(args)
}

fn parse_area(text: &str) -> Result<RenderArea, String> {
    let bad = || format!("--area: expected WxH@scale, got {text}");
    let (size, scale) = text.split_once('@').ok_or_else(bad)?;
    let (width, height) = size.split_once('x').ok_or_else(bad)?;
    Ok(RenderArea {
        width: width.parse().map_err(|_| bad())?,
        height: height.parse().map_err(|_| bad())?,
        scale: scale.parse().map_err(|_| bad())?,
    })
}

fn main() {
    pbanki_core::logging::init();
    if let Err(err) = run() {
        tracing::error!("[bench] {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let dir = std::env::temp_dir().join(format!("pbanki-render-deck-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let result = render_package(&args, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn render_package(args: &Args, dir: &Path) -> Result<(), String> {
    let mut col = CollectionBuilder::new(dir.join("collection.anki2"))
        .with_desktop_media_paths()
        .build()
        .map_err(|e| format!("collection: {e}"))?;
    col.import_apkg(
        &args.apkg,
        ImportAnkiPackageOptions {
            merge_notetypes: true,
            with_scheduling: true,
            with_deck_configs: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("import {}: {e}", args.apkg.display()))?;

    let fonts = FontCatalog::build(&args.family, &[], std::slice::from_ref(&args.font_dir));
    let font_files = card_font_files(&fonts);
    let (tx, pages) = mpsc::channel();
    let renderer = CardRenderer::spawn(
        RendererConfig {
            format: args.format,
            font_files,
            font_families: card_font_families(&fonts),
            media_folder: dir.join("collection.media"),
            image_cache: args.image_cache,
        },
        Box::new(move |page| {
            let _ = tx.send(page);
        }),
    );

    let cards = cards_in_study_order(&mut col)?;
    tracing::info!(
        "[bench] {} card(s) from {}, {} run(s), {:?} {}x{}@{}",
        cards.len(),
        args.apkg.display(),
        args.runs,
        args.format,
        args.area.width,
        args.area.height,
        args.area.scale
    );
    let mut selected = Vec::new();
    for (index, card) in cards.iter().enumerate() {
        let name = card_name(&col, card);
        if let Some(only) = &args.only
            && !name.contains(only.as_str())
        {
            continue;
        }
        let documents = card_documents(&mut col, card, &fonts, args.area, args.shadows)?;
        selected.push((index, card, name, documents.html));
    }
    for (position, (index, card, name, html)) in selected.iter().enumerate() {
        for run in 0..args.runs {
            tracing::info!("[bench] card={} variant={name} run={run}", card.id().0);
            let first = renderer.render(html.clone(), args.area)?;
            let mut received = vec![
                (Side::Question, 0, first.question),
                (Side::Answer, first.answer_start, first.answer),
            ];
            received.extend(rest_of_card(
                &renderer,
                &pages,
                first.seq,
                [first.question_pages, first.answer_pages],
                first.answer_start,
            )?);
            if run == 0
                && let Some(out) = &args.dump
            {
                for (side, page, buf) in &received {
                    let file = out.join(format!("{index:02}-{name}-{side:?}-{page}"));
                    write_page(&file, buf)?;
                }
            }
        }
        if let Some(reading) = args.lookahead {
            // As `next_card`: the next two cards, then the user reads.
            let next = selected[position + 1..]
                .iter()
                .take(2)
                .map(|(_, _, _, html)| html.clone())
                .collect();
            renderer.lookahead(next, args.area);
            std::thread::sleep(reading);
        }
    }
    Ok(())
}

/// The cards of the package in the order a new-card queue shows them (new
/// cards by position).
fn cards_in_study_order(col: &mut Collection) -> Result<Vec<Card>, String> {
    let ids = col
        .search_cards("", SortMode::Custom("c.due, c.ord, c.id".into()))
        .map_err(|e| format!("search: {e}"))?;
    let mut cards = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(card) = col.storage.get_card(id).map_err(|e| format!("card: {e}"))? {
            cards.push(card);
        }
    }
    Ok(cards)
}

/// The first field of the card's note without HTML (the variant name of the
/// CSS bench deck).
fn card_name(col: &Collection, card: &Card) -> String {
    let note = col.storage.get_note(card.note_id()).ok().flatten();
    let field = note
        .as_ref()
        .and_then(|note| note.fields().first())
        .map(|field| anki::text::strip_html(field).into_owned())
        .unwrap_or_default();
    let name: String = field
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    if name.is_empty() {
        format!("card-{}", card.id().0)
    } else {
        name
    }
}

/// Waits for the pages the renderer prefetches by itself, then asks for the
/// others one by one, so no page is rendered twice.
fn rest_of_card(
    renderer: &CardRenderer,
    pages: &mpsc::Receiver<PageReady>,
    seq: u64,
    counts: [u32; 2],
    answer_start: u32,
) -> Result<Vec<(Side, u32, PageBuf)>, String> {
    let [question_pages, answer_pages] = counts;
    // The same pages as `RenderThread::start_card` queues.
    let mut prefetched = BTreeSet::new();
    if question_pages > 1 {
        prefetched.insert((0, 1));
    }
    if answer_start + 1 < answer_pages {
        prefetched.insert((1, answer_start + 1));
    }
    if answer_start > 0 {
        prefetched.insert((1, answer_start - 1));
    }
    let mut wanted: Vec<(Side, u32)> = (1..question_pages)
        .map(|page| (Side::Question, page))
        .chain(
            (0..answer_pages)
                .filter(|&page| page != answer_start)
                .map(|page| (Side::Answer, page)),
        )
        .filter(|&(side, page)| !prefetched.contains(&(side_index(side), page)))
        .collect();
    wanted.reverse();

    let mut out = Vec::new();
    let mut outstanding = prefetched.len();
    loop {
        if outstanding == 0 {
            let Some((side, page)) = wanted.pop() else {
                return Ok(out);
            };
            renderer.want(seq, side, page);
            outstanding = 1;
        }
        let ready = pages
            .recv_timeout(Duration::from_secs(60))
            .map_err(|e| format!("waiting for a page of seq {seq}: {e}"))?;
        if ready.seq == seq {
            outstanding -= 1;
            out.push((ready.side, ready.page, ready.buf));
        }
    }
}

fn side_index(side: Side) -> u8 {
    match side {
        Side::Question => 0,
        Side::Answer => 1,
    }
}

/// Writes `buf` as binary PGM (`Gray8`) or PPM (`Rgb8`).
fn write_page(path: &Path, buf: &PageBuf) -> Result<(), String> {
    let (magic, ext) = match buf.format {
        PageFormat::Gray8 => ("P5", "pgm"),
        PageFormat::Rgb8 => ("P6", "ppm"),
    };
    let path = path.with_extension(ext);
    let mut data = format!("{magic}\n{} {}\n255\n", buf.width, buf.height).into_bytes();
    data.extend_from_slice(&buf.data);
    std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))
}
