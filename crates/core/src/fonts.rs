use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ttf_parser::{Face, PlatformId, name_id};

use crate::profile::ProfileTimer;
use crate::profile_log;

/// Only log individual font file timings above this threshold. Per-face
/// logging would add eMMC write traffic and distort startup measurements.
const SLOW_FONT_STAGE: Duration = Duration::from_millis(20);

/// Upper bound for a `name` table read; real tables are a few KB.
const MAX_NAME_TABLE: u32 = 1 << 20;

/// Upper bound for faces in one collection file.
const MAX_COLLECTION_FACES: u32 = 256;

/// One face inside a font file. A `.ttc` can contain several families.
#[derive(Clone, Debug)]
struct FontEntry {
    family: String,
    path: PathBuf,
    index: u32,
}

/// A card font choice plus any characters that no configured font covers.
#[derive(Clone, Debug)]
pub struct FontChoice {
    pub family: String,
    pub missing: Vec<char>,
}

/// Configured font families and a lazy cmap cache for the card coverage guard.
///
/// The catalog is built on the UI thread at startup and cloned into card
/// workers. Building reads only each file's table directory and `name` table;
/// cmap parsing is lazy and cached.
#[derive(Clone)]
pub struct FontCatalog {
    primary: String,
    fallbacks: Vec<String>,
    entries: Vec<FontEntry>,
    coverage: Arc<Mutex<HashMap<String, Option<Arc<HashSet<u32>>>>>>,
}

impl FontCatalog {
    pub fn build(primary: &str, fallbacks: &[String], font_dirs: &[String]) -> Self {
        let build_timer = ProfileTimer::start();
        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        for dir in font_dirs {
            let Ok(read_dir) = fs::read_dir(dir) else {
                continue;
            };
            for entry in read_dir.flatten() {
                let path = entry.path();
                let extension = path
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if !matches!(extension.as_str(), "ttf" | "otf" | "ttc") {
                    continue;
                }

                let file_timer = ProfileTimer::start();
                let families = match face_families(&path) {
                    Ok(families) => families,
                    Err(err) => {
                        tracing::warn!("[font] skipping {}: {err}", path.display());
                        continue;
                    }
                };
                let face_count = families.len();
                for (index, family) in families {
                    let Some(family) = family else {
                        continue;
                    };

                    let key = (family.clone(), path.clone(), index);
                    if seen.insert(key) {
                        entries.push(FontEntry {
                            family,
                            path: path.clone(),
                            index,
                        });
                    }
                }
                let file_elapsed = file_timer.elapsed();
                if file_elapsed >= SLOW_FONT_STAGE {
                    profile_log!(
                        "font build {}: names={file_elapsed:?} faces={face_count}",
                        path.display()
                    );
                }
            }
        }

        profile_log!(
            "font catalog build: {:?} ({} faces)",
            build_timer.elapsed(),
            entries.len()
        );

        let primary = primary.to_owned();
        let fallbacks = fallbacks.to_vec();
        if entries.is_empty() {
            tracing::warn!(
                "[font] no fonts found in configured font_dirs; coverage guard disabled"
            );
        } else {
            let missing: Vec<&str> = std::iter::once(primary.as_str())
                .chain(fallbacks.iter().map(String::as_str))
                .filter(|family| {
                    !entries
                        .iter()
                        .any(|entry| entry.family.eq_ignore_ascii_case(family))
                })
                .collect();
            if !missing.is_empty() {
                tracing::warn!("[font] configured families not found: {missing:?}");
            }
            tracing::info!(
                "[font] catalog: {} faces from {}",
                entries.len(),
                font_dirs.join(", ")
            );
        }

        Self {
            primary,
            fallbacks,
            entries,
            coverage: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn primary(&self) -> &str {
        &self.primary
    }

    /// Returns the first configured family that covers every character in
    /// `text`, or the primary family and its missing characters if none do.
    pub fn choose_font(&self, text: &str) -> FontChoice {
        let mut candidates = Vec::with_capacity(self.fallbacks.len() + 1);
        candidates.push(self.primary.as_str());
        candidates.extend(self.fallbacks.iter().map(String::as_str));

        for family in candidates {
            let Some(coverage) = self.coverage_for(family) else {
                continue;
            };
            let missing = missing_chars(text, &coverage);
            if missing.is_empty() {
                return FontChoice {
                    family: family.to_owned(),
                    missing: Vec::new(),
                };
            }
        }

        let missing = self
            .coverage_for(&self.primary)
            .map(|coverage| missing_chars(text, &coverage))
            .unwrap_or_default();
        FontChoice {
            family: self.primary.clone(),
            missing,
        }
    }

    /// Font files for the primary and fallback families, one per family.
    pub fn files_to_register(&self) -> Vec<PathBuf> {
        let mut result: Vec<PathBuf> = Vec::new();
        for (_, path, _) in self.registered_faces() {
            if !result.contains(&path) {
                result.push(path);
            }
        }
        result
    }

    /// `(family, file, face index)` for the primary and each fallback family
    /// found in the catalog, in priority order.
    pub fn registered_faces(&self) -> Vec<(String, PathBuf, u32)> {
        let mut families = Vec::with_capacity(self.fallbacks.len() + 1);
        families.push(self.primary.as_str());
        families.extend(self.fallbacks.iter().map(String::as_str));

        families
            .into_iter()
            .filter_map(|family| {
                self.entries
                    .iter()
                    .find(|entry| self.same_family(&entry.family, family))
                    .map(|entry| (entry.family.clone(), entry.path.clone(), entry.index))
            })
            .collect()
    }

    fn coverage_for(&self, family: &str) -> Option<Arc<HashSet<u32>>> {
        if let Some(cached) = self.coverage.lock().unwrap().get(family).cloned() {
            return cached;
        }

        let load_timer = ProfileTimer::start();
        let mut read_elapsed = Duration::ZERO;
        let mut files_read = 0usize;
        let mut coverage = HashSet::new();
        let mut found = false;
        for entry in self
            .entries
            .iter()
            .filter(|entry| self.same_family(&entry.family, family))
        {
            let read_timer = ProfileTimer::start();
            let data = fs::read(&entry.path);
            read_elapsed += read_timer.elapsed();
            let Ok(data) = data else {
                continue;
            };
            files_read += 1;
            let Ok(face) = Face::parse(&data, entry.index) else {
                continue;
            };
            found = true;
            if let Some(cmap) = face.tables().cmap {
                for subtable in cmap.subtables {
                    if subtable.is_unicode() {
                        subtable.codepoints(|codepoint| {
                            coverage.insert(codepoint);
                        });
                    }
                }
            }
        }

        let result = found.then(|| Arc::new(coverage));
        self.coverage
            .lock()
            .unwrap()
            .insert(family.to_owned(), result.clone());
        let total_elapsed = load_timer.elapsed();
        profile_log!(
            "font coverage '{family}': files={files_read} read={read_elapsed:?} \
             parse={:?} total={total_elapsed:?}",
            total_elapsed.saturating_sub(read_elapsed)
        );
        result
    }

    fn same_family(&self, left: &str, right: &str) -> bool {
        left.eq_ignore_ascii_case(right)
    }
}

/// Scripts whose fallback chain starts with the registered card fonts.
const CJK_SCRIPTS: [[u8; 4]; 4] = [*b"Hira", *b"Kana", *b"Hani", *b"Hang"];

/// Register the configured primary/fallback font files with Slint.
///
/// The files are memory-mapped and added to Slint's shared font collection,
/// so card text finds them by family name. They are also put first in the
/// fallback chain for Japanese, Chinese, and Korean text, so UI text in those
/// scripts does not fall back to a system font without the glyphs
/// (fireflysung lacks 両). Generic families are left alone: UI text keeps the
/// system's sans-serif (Roboto on the PB710). `SLINT_FONT_PATH` and
/// `SLINT_DEFAULT_FONT` would instead make these fonts the sans-serif chain.
///
/// Both frontends call this after the Slint platform is set up and before
/// creating `MainWindow`, so card fonts from `font_dirs` are available to Slint
/// as well as to the cmap coverage check.
pub fn register_catalog_fonts(catalog: &FontCatalog) {
    use slint::fontique_011::fontique::{Blob, FallbackKey, Script};

    let timer = ProfileTimer::start();
    let files = card_font_files(catalog);

    let mut collection = slint::fontique_011::shared_collection();
    let mut families = Vec::new();
    for path in &files {
        let mapped = File::open(path).and_then(|file| unsafe { memmap2::Mmap::map(&file) });
        let mapped = match mapped {
            Ok(mapped) => mapped,
            Err(err) => {
                tracing::warn!("[font] {}: {err}", path.display());
                continue;
            }
        };
        let fonts = collection.register_fonts(Blob::new(Arc::new(mapped)), None);
        if fonts.is_empty() {
            tracing::warn!("[font] {}: no fonts found", path.display());
        }
        for (family, _) in fonts {
            if !families.contains(&family) {
                families.push(family);
            }
        }
    }

    // A collection file registers all of its families (Noto Sans CJK: TC, SC,
    // JP, ...); put the configured ones first, in configured order, so
    // Japanese text does not pick up Chinese glyph forms.
    let configured: Vec<String> = catalog
        .registered_faces()
        .into_iter()
        .map(|(family, _, _)| family)
        .collect();
    families.sort_by_key(|&family| {
        collection
            .family_name(family)
            .and_then(|name| configured.iter().position(|c| c.eq_ignore_ascii_case(name)))
            .unwrap_or(usize::MAX)
    });

    for script in CJK_SCRIPTS {
        let key = FallbackKey::new(Script::from_bytes(script), None);
        let system: Vec<_> = collection
            .fallback_families(key)
            .filter(|family| !families.contains(family))
            .collect();
        collection.set_fallbacks(key, families.iter().copied().chain(system));
    }

    tracing::info!(
        "[font] registered {} font file(s) ({} families) with Slint",
        files.len(),
        families.len()
    );
    profile_log!("font registration: {:?}", timer.elapsed());
}

/// The card font files: the configured families, or the built-in PocketBook
/// font when none is found. Slint and the card renderer load the same files.
pub fn card_font_files(catalog: &FontCatalog) -> Vec<PathBuf> {
    let mut files = catalog.files_to_register();
    if files.is_empty() {
        tracing::warn!("[font] no configured font files found; trying built-in paths");
        files.extend(default_font_file());
    }
    files
}

/// The configured card families found in the catalog, in priority order.
pub fn card_font_families(catalog: &FontCatalog) -> Vec<String> {
    catalog
        .registered_faces()
        .into_iter()
        .map(|(family, _, _)| family)
        .collect()
}

/// The PocketBook Japanese font, used when no configured font file exists.
fn default_font_file() -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "/mnt/ext1/system/adobe/fonts/RyoGothicPlusN-Medium.otf",
        "/mnt/ext1/system/fonts/RyoGothicPlusN-Medium.otf",
        "/system/adobe/fonts/RyoGothicPlusN-Medium.otf",
        "/adobefonts/RyoGothicPlusN-Medium.otf",
        "/fonts/RyoGothicPlusN-Medium.otf",
        "/ebrmain/fonts/RyoGothicPlusN-Medium.otf",
    ];
    const DIRS: &[&str] = &[
        "/mnt/ext1/system/adobe/fonts",
        "/system/adobe/fonts",
        "/adobefonts",
        "/fonts",
        "/mnt/ext1/system/fonts",
        "/ebrmain/fonts",
    ];

    if let Some(path) = CANDIDATES.iter().map(Path::new).find(|path| path.is_file()) {
        return Some(path.to_owned());
    }

    for dir in DIRS {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.contains("ryogothic")
                && (name.ends_with(".otf") || name.ends_with(".ttf") || name.ends_with(".ttc"))
            {
                return Some(entry.path());
            }
        }
    }

    tracing::warn!("[font] Ryo Gothic PlusN not found; using system fonts only");
    None
}

/// Family name of every face in a font file, read from the table directory and
/// `name` table only. A `.ttc` yields one entry per face.
fn face_families(path: &Path) -> io::Result<Vec<(u32, Option<String>)>> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 12];
    file.read_exact(&mut header)?;

    let face_offsets = if &header[0..4] == b"ttcf" {
        let count = be_u32(&header[8..12]);
        if count == 0 || count > MAX_COLLECTION_FACES {
            return Err(invalid("bad collection face count"));
        }
        let mut offsets = vec![0u8; count as usize * 4];
        file.read_exact(&mut offsets)?;
        offsets
            .as_chunks::<4>()
            .0
            .iter()
            .map(|o| u32::from_be_bytes(*o))
            .collect()
    } else {
        vec![0]
    };

    let mut result = Vec::with_capacity(face_offsets.len());
    for (index, offset) in face_offsets.into_iter().enumerate() {
        let family = read_name_table(&mut file, offset)?
            .as_deref()
            .and_then(ttf_parser::name::Table::parse)
            .and_then(|table| family_name(table.names));
        result.push((index as u32, family));
    }
    Ok(result)
}

/// Reads the `name` table of the face whose table directory starts at
/// `offset`, or `None` if the face has no `name` table.
fn read_name_table(file: &mut File, offset: u32) -> io::Result<Option<Vec<u8>>> {
    let mut directory = [0u8; 12];
    file.seek(SeekFrom::Start(offset as u64))?;
    file.read_exact(&mut directory)?;
    let table_count = u16::from_be_bytes([directory[4], directory[5]]) as usize;

    let mut records = vec![0u8; table_count * 16];
    file.read_exact(&mut records)?;
    let Some(record) = records
        .as_chunks::<16>()
        .0
        .iter()
        .find(|record| &record[0..4] == b"name")
    else {
        return Ok(None);
    };

    let table_offset = be_u32(&record[8..12]);
    let table_length = be_u32(&record[12..16]);
    if table_length > MAX_NAME_TABLE {
        return Err(invalid("name table too large"));
    }
    let mut table = vec![0u8; table_length as usize];
    file.seek(SeekFrom::Start(table_offset as u64))?;
    file.read_exact(&mut table)?;
    Ok(Some(table))
}

fn be_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn family_name(names: ttf_parser::name::Names<'_>) -> Option<String> {
    let pick = |family_id| {
        names
            .into_iter()
            .filter(|name| name.name_id == family_id && name.is_unicode())
            .min_by_key(|name| match (name.platform_id, name.language_id) {
                (PlatformId::Windows, 0x0409) => 0u8,
                (PlatformId::Unicode, 0) => 1,
                (PlatformId::Windows, _) => 2,
                (PlatformId::Unicode, _) => 3,
                _ => 4,
            })
            .and_then(|name| name.to_string())
    };

    // Some CJK fonts list localized family names before the English one, so
    // prefer the en-US record instead of the first match.
    pick(name_id::TYPOGRAPHIC_FAMILY).or_else(|| pick(name_id::FAMILY))
}

fn missing_chars(text: &str, coverage: &HashSet<u32>) -> Vec<char> {
    let mut missing = Vec::new();
    for character in text.chars() {
        if !character.is_whitespace()
            && !coverage.contains(&(character as u32))
            && !missing.contains(&character)
        {
            missing.push(character);
        }
    }
    missing
}

#[allow(dead_code)]
fn _assert_font_catalog_send() {
    fn assert_send<T: Send>() {}
    assert_send::<FontCatalog>();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every font file under the host's system font directory.
    fn host_fonts() -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|ext| {
                        matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc")
                    })
                {
                    out.push(path);
                }
            }
        }
        let mut fonts = Vec::new();
        walk(Path::new("/usr/share/fonts"), &mut fonts);
        fonts
    }

    /// The header-only `name` reader must agree with a full ttf-parser parse.
    #[test]
    fn face_families_match_full_parse() {
        let fonts = host_fonts();
        if fonts.is_empty() {
            eprintln!("no host fonts under /usr/share/fonts; skipping");
            return;
        }

        let mut checked = 0;
        for path in fonts {
            let Ok(data) = fs::read(&path) else {
                continue;
            };
            let face_count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
            let expected: Vec<(u32, Option<String>)> = (0..face_count)
                .filter_map(|index| {
                    let face = Face::parse(&data, index).ok()?;
                    Some((index, family_name(face.names())))
                })
                .collect();
            if expected.is_empty() {
                continue;
            }

            let actual =
                face_families(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            let actual: Vec<_> = actual
                .into_iter()
                .filter(|(index, _)| expected.iter().any(|(e, _)| e == index))
                .collect();
            assert_eq!(actual, expected, "{}", path.display());
            checked += 1;
        }
        eprintln!("checked {checked} font files");
    }

    #[test]
    fn face_families_rejects_non_fonts() {
        let dir = std::env::temp_dir().join(format!("pbanki-font-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let short = dir.join("short.ttf");
        fs::write(&short, b"abc").unwrap();
        assert!(face_families(&short).is_err());

        let bogus = dir.join("bogus.ttc");
        let mut data = b"ttcf".to_vec();
        data.extend_from_slice(&[0, 1, 0, 0]);
        data.extend_from_slice(&u32::MAX.to_be_bytes());
        fs::write(&bogus, &data).unwrap();
        assert!(face_families(&bogus).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
