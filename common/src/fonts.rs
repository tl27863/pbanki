use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ttf_parser::{Face, PlatformId, name_id};

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
/// The catalog is built on the UI thread at startup (cheap: family names only)
/// and cloned into card workers. Cmap parsing is lazy and cached.
#[derive(Clone)]
pub struct FontCatalog {
    primary: String,
    fallbacks: Vec<String>,
    entries: Vec<FontEntry>,
    coverage: Arc<Mutex<HashMap<String, Option<Arc<HashSet<u32>>>>>>,
}

impl FontCatalog {
    pub fn build(primary: &str, fallbacks: &[String], font_dirs: &[String]) -> Self {
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

                let Ok(data) = fs::read(&path) else {
                    continue;
                };
                let face_count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
                for index in 0..face_count {
                    let Ok(face) = Face::parse(&data, index) else {
                        continue;
                    };
                    let Some(family) = family_name(&face) else {
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
            }
        }

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
        let mut result = Vec::new();
        let mut families = Vec::with_capacity(self.fallbacks.len() + 1);
        families.push(self.primary.as_str());
        families.extend(self.fallbacks.iter().map(String::as_str));

        for family in families {
            if let Some(entry) = self
                .entries
                .iter()
                .find(|entry| self.same_family(&entry.family, family))
            {
                if !result.contains(&entry.path) {
                    result.push(entry.path.clone());
                }
            }
        }

        result
    }

    fn coverage_for(&self, family: &str) -> Option<Arc<HashSet<u32>>> {
        if let Some(cached) = self.coverage.lock().unwrap().get(family).cloned() {
            return cached;
        }

        let mut coverage = HashSet::new();
        let mut found = false;
        for entry in self
            .entries
            .iter()
            .filter(|entry| self.same_family(&entry.family, family))
        {
            let Ok(data) = fs::read(&entry.path) else {
                continue;
            };
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
    let files = catalog.files_to_register();
    if files.is_empty() {
        eprintln!("[font] no configured font files found; trying built-in paths");
        register_default_font();
        return;
    }

    let dir = std::path::Path::new("/tmp/pbanki-fonts");
    let _ = fs::remove_dir_all(dir);
    if fs::create_dir_all(dir).is_err() {
        register_default_font();
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

fn family_name(face: &Face<'_>) -> Option<String> {
    let pick = |family_id| {
        face.names()
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
