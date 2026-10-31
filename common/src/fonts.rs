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
                        eprintln!("[font] skipping {}: {err}", path.display());
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
            eprintln!("[font] no fonts found in configured font_dirs; coverage guard disabled");
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
                eprintln!("[font] configured families not found: {missing:?}");
            }
            eprintln!(
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

    /// Replaces characters not present in `family` with `?`.
    pub fn sanitize(&self, text: &str, family: &str) -> String {
        let Some(coverage) = self.coverage_for(family) else {
            return text.to_owned();
        };
        text.chars()
            .map(|character| {
                if character.is_whitespace() || coverage.contains(&(character as u32)) {
                    character
                } else {
                    '?'
                }
            })
            .collect()
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

/// Register the configured primary/fallback font files with Slint.
///
/// `SLINT_DEFAULT_FONT` accepts a single file or a directory. We build a
/// temporary directory of symlinks so all configured families are available to
/// the Slint font collection without copying multi-megabyte files.
///
/// Both frontends call this before creating `MainWindow`, so card fonts from
/// `font_dirs` are available to Slint as well as to the cmap coverage check.
pub fn register_catalog_fonts(catalog: &FontCatalog) {
    let timer = ProfileTimer::start();
    let files = catalog.files_to_register();
    if files.is_empty() {
        eprintln!("[font] no configured font files found; trying built-in paths");
        register_default_font();
        profile_log!("font registration fallback: {:?}", timer.elapsed());
        return;
    }

    let dir = Path::new("/tmp/pbanki-fonts");
    let _ = fs::remove_dir_all(dir);
    if fs::create_dir_all(dir).is_err() {
        register_default_font();
        profile_log!("font registration failed after {:?}", timer.elapsed());
        return;
    }

    let mut registered = 0;
    for (index, path) in files.iter().enumerate() {
        let Some(name) = path.file_name() else {
            continue;
        };
        let destination = dir.join(format!("{index}-{}", name.to_string_lossy()));
        if std::os::unix::fs::symlink(path, &destination).is_ok() {
            registered += 1;
        }
    }

    if registered == 0 {
        eprintln!("[font] could not link configured fonts; trying built-in paths");
        register_default_font();
        profile_log!("font registration failed after {:?}", timer.elapsed());
        return;
    }

    unsafe {
        std::env::set_var("SLINT_DEFAULT_FONT", dir);
    }
    eprintln!(
        "[font] SLINT_DEFAULT_FONT={} ({} configured font file(s))",
        dir.display(),
        registered
    );
    profile_log!("font registration: {:?}", timer.elapsed());
}

/// Register the PocketBook Japanese font directly with Slint.
///
/// fontconfig does not necessarily expose /adobefonts to the Slint process, so
/// selecting "Ryo Gothic PlusN" alone can still fall back to fireflysung (which
/// has no 両 glyph) and stall the UI thread.
fn register_default_font() {
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

    for path in CANDIDATES {
        if std::path::Path::new(path).is_file() {
            unsafe {
                std::env::set_var("SLINT_DEFAULT_FONT", path);
            }
            eprintln!("[font] SLINT_DEFAULT_FONT={path}");
            return;
        }
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
                let path = entry.path();
                unsafe {
                    std::env::set_var("SLINT_DEFAULT_FONT", &path);
                }
                eprintln!("[font] SLINT_DEFAULT_FONT={}", path.display());
                return;
            }
        }
    }

    eprintln!("[font] Ryo Gothic PlusN not found; using system fallback");
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
        offsets.as_chunks::<4>().0.iter().map(|o| u32::from_be_bytes(*o)).collect()
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
    let Some(record) = records.as_chunks::<16>().0.iter().find(|record| &record[0..4] == b"name") else {
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
#[path = "../tests/unit/fonts.rs"]
mod tests;
