//! Decoded media images for the card-render thread (notes/blitz-card-render.md,
//! "Media").
//!
//! Blitz decodes every image it gets in `NetHandler::bytes`, and it has no
//! hook for decoded images. Each card is two documents (question and answer),
//! and a deck often shows one picture on many cards, so the same file was
//! decoded again and again: a 400×600 WebP costs about 93 ms on the PB710.
//!
//! The media provider decodes an image once and keeps it as PAM: uncompressed
//! 8-bit RGBA with a short text header. Blitz decodes PAM almost with a copy
//! (host: 0.04 ms against 0.3 ms for BMP and 4 ms for WebP, for 400×600).
//! The pixels are the ones Blitz would decode itself (`decode`, then
//! `into_rgba8`), and the size is the same, so the layout does not change.

use std::collections::VecDeque;
use std::io::Cursor;
use std::path::PathBuf;
use std::time::SystemTime;

use blitz_traits::net::Bytes;

/// Memory for the decoded images (`[general] image_cache_mb` and
/// `image_cache_max_image_mb` in the config). A 400×600 picture is 0.9 MB of
/// RGBA, a 1920×1080 screenshot 8 MB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageCacheSize {
    /// All images together.
    pub total: usize,
    /// One image. A larger image goes to Blitz unchanged (`decode_to_pam`).
    pub max_image: usize,
}

impl ImageCacheSize {
    /// 32 MB, of which one image can take half (16 MB, about 4 megapixels),
    /// so one image does not push out all the others.
    pub const DEFAULT: Self = Self::from_mb(32, 16);
    /// No cache: every document decodes its images, as before the cache.
    pub const OFF: Self = Self::from_mb(0, 0);

    /// Saturates where `usize` is too small (32-bit ARM on the PocketBook:
    /// 4096 MB and more), so a large value does not wrap to a small cache.
    pub const fn from_mb(total: u32, max_image: u32) -> Self {
        Self {
            total: (total as usize).saturating_mul(1 << 20),
            max_image: (max_image as usize).saturating_mul(1 << 20),
        }
    }
}

/// A media file as it is on disk now. A file that sync replaces gets a new
/// length or modification time, so its old entry is not used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageKey {
    pub path: PathBuf,
    pub len: u64,
    pub modified: Option<SystemTime>,
}

/// Most recently used images, as PAM bytes, within a byte budget.
pub struct ImageCache {
    budget: usize,
    max_entry: usize,
    used: usize,
    /// Most recently used first. A card shows a few images, so a linear
    /// search is enough.
    entries: VecDeque<(ImageKey, Bytes)>,
}

impl ImageCache {
    pub fn new(size: ImageCacheSize) -> Self {
        Self {
            budget: size.total,
            max_entry: size.max_image.min(size.total),
            used: 0,
            entries: VecDeque::new(),
        }
    }

    pub fn get(&mut self, key: &ImageKey) -> Option<Bytes> {
        let index = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(index)?;
        let bytes = entry.1.clone();
        self.entries.push_front(entry);
        Some(bytes)
    }

    /// The largest image the cache keeps (bytes of RGBA). 0 when the cache
    /// is off.
    pub fn max_entry(&self) -> usize {
        self.max_entry
    }

    /// Keeps `bytes` unless they are over `max_entry`.
    pub fn insert(&mut self, key: ImageKey, bytes: Bytes) {
        if bytes.len() > self.max_entry() {
            return;
        }
        if let Some(index) = self.entries.iter().position(|(k, _)| *k == key)
            && let Some((_, old)) = self.entries.remove(index)
        {
            self.used -= old.len();
        }
        self.used += bytes.len();
        self.entries.push_front((key, bytes));
        while self.used > self.budget {
            let Some((_, old)) = self.entries.pop_back() else {
                break;
            };
            self.used -= old.len();
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Decodes a raster image (any format the `image` crate here can read) and
/// returns it as PAM bytes. `None` for anything else (SVG, fonts, style
/// sheets), for images that do not decode, and for images whose RGBA would
/// be over `max_bytes` (the header gives the size before the decode): those
/// go to Blitz unchanged, as without the cache. Converting an image that the
/// cache does not keep would only add copies: about 280 ms for a 1600×2400
/// JPEG on the PB710 (memory bandwidth is about 330 MB/s).
pub fn decode_to_pam(bytes: &[u8], max_bytes: usize) -> Option<Bytes> {
    // The cache is off: no image is kept, so do not even read the header.
    if max_bytes == 0 {
        return None;
    }
    let reader = || {
        image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()
            .filter(|reader| reader.format().is_some())
    };
    let (width, height) = reader()?.into_dimensions().ok()?;
    if u64::from(width) * u64::from(height) * 4 > max_bytes as u64 {
        return None;
    }
    let rgba = reader()?.decode().ok()?.into_rgba8();
    let (width, height) = rgba.dimensions();
    let header = format!(
        "P7\nWIDTH {width}\nHEIGHT {height}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
    );
    let mut out = Vec::with_capacity(header.len() + rgba.as_raw().len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(rgba.as_raw());
    Some(Bytes::from(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(total: usize, max_image: usize) -> ImageCacheSize {
        ImageCacheSize { total, max_image }
    }

    #[test]
    fn the_image_limit_is_at_most_the_total() {
        assert_eq!(ImageCache::new(bytes(10, 20)).max_entry(), 10);
        assert_eq!(ImageCacheSize::DEFAULT, bytes(32 << 20, 16 << 20));
    }

    #[test]
    fn large_sizes_do_not_wrap() {
        // 4096 MB is 1 << 32 bytes: 0 after a plain shift in a 32-bit usize.
        let size = ImageCacheSize::from_mb(4096, u32::MAX);
        assert_eq!(size.total as u64, (4096u64 << 20).min(usize::MAX as u64));
        assert!(size.max_image >= size.total);
    }

    fn key(name: &str) -> ImageKey {
        ImageKey {
            path: PathBuf::from(name),
            len: 1,
            modified: None,
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([x as u8, y as u8, 7, if x == 0 { 0 } else { 255 }])
        });
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    /// What Blitz does with the bytes (`ImageHandler::parse`).
    fn blitz_decode(bytes: &[u8]) -> image::RgbaImage {
        image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap()
            .into_rgba8()
    }

    #[test]
    fn pam_decodes_to_the_same_pixels() {
        let source = png(5, 3);
        let pam = decode_to_pam(&source, 1 << 20).expect("a PNG is an image");
        assert!(pam.starts_with(b"P7\n"));
        assert_eq!(blitz_decode(&pam), blitz_decode(&source));
    }

    #[test]
    fn non_images_are_not_converted() {
        let max = 1 << 20;
        assert!(decode_to_pam(b"<svg xmlns='http://www.w3.org/2000/svg'/>", max).is_none());
        assert!(decode_to_pam(b"\0\x01\0\0 a font", max).is_none());
        assert!(decode_to_pam(b"\x89PNG\r\n\x1a\n broken", max).is_none());
    }

    #[test]
    fn images_the_cache_would_not_keep_are_not_converted() {
        let source = png(5, 3); // 60 bytes of RGBA
        assert!(decode_to_pam(&source, 60).is_some());
        assert!(decode_to_pam(&source, 59).is_none());
        assert!(decode_to_pam(&source, 0).is_none());
    }

    #[test]
    fn least_recently_used_images_leave_first() {
        let mut cache = ImageCache::new(bytes(40, 20));
        assert_eq!(cache.max_entry(), 20);
        cache.insert(key("a"), Bytes::from(vec![0; 10]));
        cache.insert(key("b"), Bytes::from(vec![0; 10]));
        cache.insert(key("c"), Bytes::from(vec![0; 10]));
        assert!(cache.get(&key("a")).is_some()); // a is now the newest
        cache.insert(key("d"), Bytes::from(vec![0; 10]));
        cache.insert(key("e"), Bytes::from(vec![0; 10]));
        assert!(cache.get(&key("b")).is_none());
        assert!(cache.get(&key("a")).is_some());
        assert_eq!(cache.len(), 4);
    }

    #[test]
    fn a_changed_file_misses() {
        let mut cache = ImageCache::new(bytes(100, 50));
        cache.insert(key("a"), Bytes::from(vec![0; 10]));
        let mut changed = key("a");
        changed.len = 2;
        assert!(cache.get(&changed).is_none());
    }

    #[test]
    fn large_images_and_a_zero_budget_are_not_kept() {
        let mut cache = ImageCache::new(bytes(40, 20));
        cache.insert(key("big"), Bytes::from(vec![0; 21]));
        assert_eq!(cache.len(), 0);
        let mut off = ImageCache::new(ImageCacheSize::OFF);
        off.insert(key("a"), Bytes::from(vec![0; 1]));
        assert_eq!(off.len(), 0);
    }
}
