//! Fonts, media and pixels for the card-render thread.

use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyrender::{ImageRenderer as _, PaintScene as _};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{BaseDocument, FontContext};
use blitz_paint::paint_scene;
use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};
use parley::fontique::{
    Blob, Collection, CollectionOptions, FallbackKey, FamilyId, GenericFamily, Script, SourceCache,
};
use peniko::kurbo::{Affine, Rect};
use peniko::{Color, Fill};

use super::image_cache::{ImageCache, ImageCacheSize, ImageKey, decode_to_pam};
use super::{PageBuf, PageFormat};

/// Scripts whose fallback chain is the card fonts. Without a fallback, text in
/// a family that is not installed (Anki's default `.card { font-family:
/// arial }`) would find no font: there are no system fonts.
const FALLBACK_SCRIPTS: &[[u8; 4]] = &[
    *b"Latn", *b"Grek", *b"Cyrl", *b"Hira", *b"Kana", *b"Hani", *b"Hang", *b"Arab", *b"Hebr",
    *b"Thai", *b"Deva", *b"Zyyy", *b"Zinh", *b"Zzzz",
];

const GENERIC_FAMILIES: &[GenericFamily] = &[
    GenericFamily::SansSerif,
    GenericFamily::Serif,
    GenericFamily::SystemUi,
    GenericFamily::Monospace,
    GenericFamily::UiSansSerif,
    GenericFamily::UiSerif,
];

/// The card fonts in a parley font context of their own (Blitz uses parley
/// 0.12; Slint has fontique 0.11, so the two cannot share a collection).
pub struct CardFonts {
    ctx: FontContext,
    /// Registered families with their names, in configured order.
    families: Vec<(FamilyId, String)>,
}

impl CardFonts {
    /// Memory-maps and registers `files`. `configured` is the family order
    /// of the font catalog. A collection file can bring more families; those
    /// go last.
    pub fn load(files: &[PathBuf], configured: &[String]) -> Self {
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        let mut ids: Vec<FamilyId> = Vec::new();
        for path in files {
            let mapped = File::open(path).and_then(|file| unsafe { memmap2::Mmap::map(&file) });
            let mapped = match mapped {
                Ok(mapped) => mapped,
                Err(err) => {
                    tracing::warn!("[card] font {}: {err}", path.display());
                    continue;
                }
            };
            for (family, _) in collection.register_fonts(Blob::new(Arc::new(mapped)), None) {
                if !ids.contains(&family) {
                    ids.push(family);
                }
            }
        }
        let mut families: Vec<(FamilyId, String)> = ids
            .into_iter()
            .map(|id| {
                let name = collection.family_name(id).unwrap_or_default().to_owned();
                (id, name)
            })
            .collect();
        families.sort_by_key(|(_, name)| {
            configured
                .iter()
                .position(|c| c.eq_ignore_ascii_case(name))
                .unwrap_or(usize::MAX)
        });
        if families.is_empty() {
            tracing::warn!("[card] no card fonts loaded; card text will not show");
        } else {
            tracing::info!(
                "[card] loaded {} font file(s): {:?}",
                files.len(),
                families.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>()
            );
        }
        let mut fonts = Self {
            ctx: FontContext {
                collection,
                source_cache: SourceCache::new_shared(),
            },
            families,
        };
        fonts.prefer(None);
        fonts
    }

    /// A font context whose fallback and generic chains start with `family`
    /// (the font that covers the whole card, from `FontCatalog::choose_font`),
    /// then the other card fonts in configured order.
    pub fn context_for(&mut self, family: &str) -> FontContext {
        self.prefer(Some(family));
        self.ctx.clone()
    }

    fn prefer(&mut self, family: Option<&str>) {
        let mut order: Vec<FamilyId> = Vec::with_capacity(self.families.len());
        if let Some(family) = family {
            order.extend(
                self.families
                    .iter()
                    .filter(|(_, name)| name.eq_ignore_ascii_case(family))
                    .map(|(id, _)| *id),
            );
        }
        for (id, _) in &self.families {
            if !order.contains(id) {
                order.push(*id);
            }
        }
        let collection = &mut self.ctx.collection;
        for script in FALLBACK_SCRIPTS {
            let key = FallbackKey::new(Script::from_bytes(*script), None);
            collection.set_fallbacks(key, order.iter().copied());
        }
        for generic in GENERIC_FAMILIES {
            collection.set_generic_families(*generic, order.iter().copied());
        }
    }
}

/// Reads `<img>` and other sub-resources from the collection media folder.
///
/// It answers synchronously: the bytes are in the document's queue when
/// `fetch` returns, and `BaseDocument::handle_messages` loads them. Only
/// relative paths inside the media folder are read. Images are decoded once
/// and kept in an `ImageCache` (see `image_cache.rs`).
pub struct MediaProvider {
    pub folder: PathBuf,
    images: Mutex<ImageCache>,
    stats: MediaCounters,
}

/// Time spent in `MediaProvider::fetch` since the last `take_stats`.
#[derive(Default)]
struct MediaCounters {
    files: AtomicU32,
    cached: AtomicU32,
    read_us: AtomicU64,
    decode_us: AtomicU64,
}

/// Media work for one document (`MediaProvider::take_stats`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MediaStats {
    pub files: u32,
    /// Images that came from the image cache (no file read, no full decode).
    pub cached: u32,
    /// Reading the files.
    pub read: Duration,
    /// Decoding the images that missed the cache, and `NetHandler::bytes`
    /// (Blitz decodes images inside it, `ImageHandler`).
    pub decode: Duration,
}

impl MediaStats {
    pub fn add(&mut self, other: MediaStats) {
        self.files += other.files;
        self.cached += other.cached;
        self.read += other.read;
        self.decode += other.decode;
    }
}

impl MediaProvider {
    pub fn new(folder: PathBuf, image_cache: ImageCacheSize) -> Self {
        Self {
            folder,
            images: Mutex::new(ImageCache::new(image_cache)),
            stats: MediaCounters::default(),
        }
    }

    /// The media work since the last call. The render thread calls it after
    /// each document, so the numbers belong to that document.
    pub fn take_stats(&self) -> MediaStats {
        MediaStats {
            files: self.stats.files.swap(0, Ordering::Relaxed),
            cached: self.stats.cached.swap(0, Ordering::Relaxed),
            read: Duration::from_micros(self.stats.read_us.swap(0, Ordering::Relaxed)),
            decode: Duration::from_micros(self.stats.decode_us.swap(0, Ordering::Relaxed)),
        }
    }

    fn resolve(&self, request: &Request) -> Result<PathBuf, String> {
        if request.url.scheme() != "file" {
            return Err(format!("not a media file: {}", request.url));
        }
        let path = request
            .url
            .to_file_path()
            .map_err(|()| format!("bad media path: {}", request.url))?;
        let relative = path
            .strip_prefix(&self.folder)
            .map_err(|_| format!("outside the media folder: {}", path.display()))?;
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(format!("bad media path: {}", path.display()));
        }
        Ok(path)
    }
}

impl MediaProvider {
    fn images(&self) -> std::sync::MutexGuard<'_, ImageCache> {
        self.images.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The bytes for Blitz: decoded images as PAM (from the cache or decoded
    /// now), anything else as read. The flag is true for a cache hit.
    fn load(&self, request: &Request) -> Result<(Bytes, bool), String> {
        let micros = |since: Instant| since.elapsed().as_micros() as u64;
        let started = Instant::now();
        let path = self.resolve(request)?;
        let meta = std::fs::metadata(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        let key = ImageKey {
            path,
            len: meta.len(),
            modified: meta.modified().ok(),
        };
        if let Some(pam) = self.images().get(&key) {
            self.stats
                .read_us
                .fetch_add(micros(started), Ordering::Relaxed);
            return Ok((pam, true));
        }
        let bytes =
            std::fs::read(&key.path).map_err(|err| format!("{}: {err}", key.path.display()))?;
        self.stats
            .read_us
            .fetch_add(micros(started), Ordering::Relaxed);

        let started = Instant::now();
        let max_entry = self.images().max_entry();
        let pam = decode_to_pam(&bytes, max_entry);
        self.stats
            .decode_us
            .fetch_add(micros(started), Ordering::Relaxed);
        Ok(match pam {
            Some(pam) => {
                self.images().insert(key, pam.clone());
                (pam, false)
            }
            None => (Bytes::from(bytes), false),
        })
    }
}

impl NetProvider for MediaProvider {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.to_string();
        match self.load(&request) {
            Ok((bytes, cached)) => {
                self.stats.files.fetch_add(1, Ordering::Relaxed);
                if cached {
                    self.stats.cached.fetch_add(1, Ordering::Relaxed);
                }
                let started = Instant::now();
                handler.bytes(url, bytes);
                self.stats
                    .decode_us
                    .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
            }
            Err(err) => {
                tracing::warn!("[card] media: {err}");
                handler.error(url, err);
            }
        }
    }
}

/// The media folder as an absolute path. The configured collection path can
/// be relative (the desktop uses `./pbanki/collection`), and a `file:` URL
/// needs an absolute path.
pub fn absolute_media_folder(folder: &Path) -> PathBuf {
    std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf())
}

/// The `file://` base URL of the media folder (with a trailing slash, so
/// relative `src` values resolve inside it).
///
/// Never `None`: without a base URL Blitz keeps a `data:` URL, and resolving
/// a relative `<img src>` against it panics (`BaseDocument::resolve_url`).
/// If the folder has no URL form, `file:///` still resolves; the media
/// provider then refuses the files, as they are outside the folder.
pub fn media_base_url(folder: &Path) -> String {
    blitz_traits::net::Url::from_directory_path(folder)
        .map(String::from)
        .unwrap_or_else(|()| "file:///".to_owned())
}

/// Rasterizes pages of one size.
pub struct Raster {
    renderer: VelloCpuImageRenderer,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl Raster {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            renderer: VelloCpuImageRenderer::new(width, height),
            rgba: vec![0; width as usize * height as usize * 4],
            width,
            height,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Draws the document scrolled to `top` (device px) and keeps the first
    /// `keep` rows. The rows below are white: they show the start of the
    /// next page's first line.
    pub fn render_page(
        &mut self,
        doc: &mut BaseDocument,
        scale: f32,
        top: f32,
        keep: u32,
        format: PageFormat,
    ) -> (PageBuf, PageTimes) {
        let (w, h) = (self.width, self.height);
        let started = Instant::now();
        let mut record = Duration::ZERO;
        doc.set_viewport_scroll(blitz_dom::Point {
            x: 0.0,
            y: f64::from(top) / f64::from(scale),
        });
        self.renderer.reset();
        self.renderer.render(
            |scene| {
                let started = Instant::now();
                scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    Color::WHITE,
                    None,
                    &Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
                );
                paint_scene(scene, doc, f64::from(scale), w, h, 0, 0);
                record = started.elapsed();
            },
            &mut self.rgba,
        );
        let raster = started.elapsed() - record;
        let started = Instant::now();
        let keep = keep.min(h) as usize;
        let mut page = convert(&self.rgba, w, h, format);
        let row = w as usize * format.bytes_per_pixel();
        page.data[keep * row..].fill(255);
        let times = PageTimes {
            record,
            raster,
            convert: started.elapsed(),
        };
        (page, times)
    }
}

/// Where the time of one page went.
#[derive(Clone, Copy, Debug, Default)]
pub struct PageTimes {
    /// `paint_scene`: walking the document and recording draw commands.
    pub record: Duration,
    /// vello_cpu rasterization (blur of `box-shadow` is here).
    pub raster: Duration,
    /// RGBA to the page format, and the mask below the cut.
    pub convert: Duration,
}

impl PageTimes {
    pub fn total(&self) -> Duration {
        self.record + self.raster + self.convert
    }
}

impl std::fmt::Display for PageTimes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ms = super::Ms;
        write!(
            f,
            "{} ms (record {}, raster {}, convert {})",
            ms(self.total()),
            ms(self.record),
            ms(self.raster),
            ms(self.convert)
        )
    }
}

/// RGBA (opaque: the page is filled white first) to the page format.
pub fn convert(rgba: &[u8], width: u32, height: u32, format: PageFormat) -> PageBuf {
    let pixels = width as usize * height as usize;
    let rgba = &rgba[..pixels * 4];
    let data = match format {
        PageFormat::Gray8 => {
            let mut gray = vec![0u8; pixels];
            // The integer Rec. 709 luma of inkview's `RGB24::to_bb8`, as in
            // the bench. Each pixel is read as one u32, so LLVM vectorizes it.
            for (out, px) in gray.iter_mut().zip(rgba.as_chunks::<4>().0) {
                let p = u32::from_le_bytes(*px);
                let luma = 54 * (p & 0xff) + 183 * ((p >> 8) & 0xff) + 19 * ((p >> 16) & 0xff);
                *out = (luma >> 8) as u8;
            }
            gray
        }
        PageFormat::Rgb8 => {
            let mut rgb = vec![0u8; pixels * 3];
            for (out, px) in rgb
                .as_chunks_mut::<3>()
                .0
                .iter_mut()
                .zip(rgba.as_chunks::<4>().0)
            {
                *out = [px[0], px[1], px[2]];
            }
            rgb
        }
    };
    PageBuf {
        width,
        height,
        format,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_conversion_matches_inkview_luma() {
        let rgba = [
            255, 255, 255, 255, 0, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255,
        ];
        let page = convert(&rgba, 4, 1, PageFormat::Gray8);
        assert_eq!(page.data, vec![255, 0, 53, 18]);
    }

    #[test]
    fn rgb_conversion_drops_alpha() {
        let rgba = [1, 2, 3, 255, 4, 5, 6, 255];
        let page = convert(&rgba, 2, 1, PageFormat::Rgb8);
        assert_eq!(page.data, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn media_provider_decodes_an_image_once() {
        use std::sync::mpsc;

        struct Capture(mpsc::Sender<Bytes>);
        impl NetHandler for Capture {
            fn bytes(self: Box<Self>, _url: String, bytes: Bytes) {
                let _ = self.0.send(bytes);
            }
        }

        let folder =
            std::env::temp_dir().join(format!("pbanki-image-cache-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let folder = absolute_media_folder(&folder);
        image::RgbImage::from_pixel(8, 6, image::Rgb([10, 0, 0]))
            .save(folder.join("poster.png"))
            .unwrap();
        let provider = MediaProvider::new(folder.clone(), ImageCacheSize::DEFAULT);
        let url = blitz_traits::net::Url::from_file_path(folder.join("poster.png")).unwrap();
        let (tx, rx) = mpsc::channel();
        let fetch = || {
            provider.fetch(0, Request::get(url.clone()), Box::new(Capture(tx.clone())));
            rx.try_recv().expect("the provider answers synchronously")
        };

        // Question and answer document of one card: one decode.
        let first = fetch();
        let second = fetch();
        assert!(first.starts_with(b"P7\n"));
        assert_eq!(first, second);
        let stats = provider.take_stats();
        assert_eq!((stats.files, stats.cached), (2, 1));

        // A file that sync replaced is decoded again.
        std::thread::sleep(Duration::from_millis(20));
        image::RgbImage::from_pixel(9, 6, image::Rgb([200, 0, 0]))
            .save(folder.join("poster.png"))
            .unwrap();
        let third = fetch();
        assert_ne!(third, first);
        assert_eq!(provider.take_stats().cached, 0);

        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn relative_media_folders_get_a_file_base_url() {
        let folder = absolute_media_folder(Path::new("./pbanki/collection/collection.media"));
        assert!(folder.is_absolute());
        let base = media_base_url(&folder);
        assert!(base.starts_with("file:///"), "{base}");
        assert!(base.ends_with("/collection.media/"), "{base}");
        assert_eq!(media_base_url(Path::new("relative")), "file:///");
    }

    #[test]
    fn media_stats_count_files_and_reset() {
        struct Sink;
        impl NetHandler for Sink {
            fn bytes(self: Box<Self>, _: String, _: Bytes) {}
        }
        let dir = std::env::temp_dir().join(format!("pbanki-media-stats-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        let provider = MediaProvider::new(dir.clone(), ImageCacheSize::OFF);
        let url = blitz_traits::net::Url::from_file_path(dir.join("a.png")).unwrap();
        provider.fetch(0, Request::get(url.clone()), Box::new(Sink));
        provider.fetch(0, Request::get(url), Box::new(Sink));
        provider.fetch(
            0,
            Request::get(blitz_traits::net::Url::parse("file:///etc/passwd").unwrap()),
            Box::new(Sink),
        );
        assert_eq!(provider.take_stats().files, 2);
        assert_eq!(provider.take_stats(), MediaStats::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn media_provider_stays_in_the_folder() {
        let provider =
            MediaProvider::new(PathBuf::from("/col/collection.media"), ImageCacheSize::OFF);
        let request = |url: &str| {
            blitz_traits::net::Request::get(blitz_traits::net::Url::parse(url).unwrap())
        };
        assert!(
            provider
                .resolve(&request("file:///col/collection.media/a%20b.png"))
                .is_ok()
        );
        assert!(provider.resolve(&request("file:///etc/passwd")).is_err());
        assert!(
            provider
                .resolve(&request("https://example.com/a.png"))
                .is_err()
        );
    }
}
