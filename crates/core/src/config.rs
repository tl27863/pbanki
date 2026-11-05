use serde::{Deserialize, Serialize};

use crate::card_render::ImageCacheSize;
use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

const DEFAULT_CONFIG_PATH: &str = "/mnt/ext1/applications/pbanki/config.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub general: GeneralConfig,
    pub ankiweb: AnkiWebConfig,
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    pub language: String,
    pub collection_path: String,
    #[serde(default = "default_card_font")]
    pub card_font: String,
    #[serde(default = "default_card_fallback_fonts")]
    pub card_fallback_fonts: Vec<String>,
    #[serde(default = "default_font_dirs")]
    pub font_dirs: Vec<String>,
    /// Show the PocketBook status bar (clock, battery, Wi-Fi; tapping it
    /// opens the system tray) above the app. Takes effect after a restart.
    #[serde(default = "default_system_panel")]
    pub system_panel: bool,
    /// Draw `box-shadow` on cards. False (the default) removes all box
    /// shadows (`card_render::html`, `NO_SHADOW_CSS`): a blurred shadow costs
    /// about 170 ms per page on the PB710. Config file only, no settings UI.
    #[serde(default)]
    pub card_shadows: bool,
    /// Decoded image cache of the card renderer, all images together (MB).
    /// 0 turns it off. Config file only.
    #[serde(default = "default_image_cache_mb")]
    pub image_cache_mb: u32,
    /// The largest single image the cache keeps (MB of RGBA). Config file only.
    #[serde(default = "default_image_cache_max_image_mb")]
    pub image_cache_max_image_mb: u32,
}

/// Comment written above `system_panel` in the template and when the key is
/// added to an existing config.
const SYSTEM_PANEL_COMMENT: &str = "\n# Show the PocketBook status bar (clock, battery, Wi-Fi) above pbAnki; tapping\n# it opens the system tray. Restart pbAnki after changing this.\n";

fn default_system_panel() -> bool {
    true
}

/// Comment written above `card_shadows` in the template and when the key is
/// added to an existing config.
const CARD_SHADOWS_COMMENT: &str = "\n# Draw the box shadows of card templates. Blurred shadows are slow on e-ink\n# (about 0.2 s per page on a PB710), so they are off by default. Restart\n# pbAnki after changing this.\n";

/// Comment written above `image_cache_mb` when the key is added.
const IMAGE_CACHE_COMMENT: &str = "\n# Memory for decoded card images, in MB: all images together, and the largest\n# single image kept (a 1920x1080 picture is 8 MB). A cached image is not\n# decoded again on the next card. 0 turns the cache off. Restart pbAnki after\n# changing this.\n";

fn default_image_cache_mb() -> u32 {
    (ImageCacheSize::DEFAULT.total >> 20) as u32
}

fn default_image_cache_max_image_mb() -> u32 {
    (ImageCacheSize::DEFAULT.max_image >> 20) as u32
}

fn default_card_font() -> String {
    "Ryo Gothic PlusN".into()
}

fn default_card_fallback_fonts() -> Vec<String> {
    vec![
        "Adobe Ming Std".into(),
        "Adobe Heiti Std".into(),
        "Noto Sans CJK JP".into(),
    ]
}

fn default_font_dirs() -> Vec<String> {
    vec![
        "/mnt/ext1/system/adobe/fonts".into(),
        "/mnt/ext1/system/fonts".into(),
        "/system/adobe/fonts".into(),
        "/adobefonts".into(),
        "/fonts".into(),
        "/ebrmain/fonts".into(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnkiWebConfig {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub token_endpoint: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub auto_sync: bool,
    #[serde(default)]
    pub sync_on_exit: bool,
    #[serde(default = "default_sync_media")]
    pub sync_media: bool,
}

fn default_sync_media() -> bool {
    true
}

impl AnkiWebConfig {
    pub fn effective_endpoint(&self) -> Option<&str> {
        self.endpoint
            .as_deref()
            .map(str::trim)
            .filter(|endpoint| !endpoint.is_empty())
    }

    pub fn has_credentials(&self) -> bool {
        self.token
            .as_deref()
            .map(str::trim)
            .is_some_and(|token| !token.is_empty())
            || (!self.username.trim().is_empty() && !self.password.is_empty())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig {
                language: "en-GB".into(),
                collection_path: "/mnt/ext1/applications/pbanki/collection".into(),
                card_font: default_card_font(),
                card_fallback_fonts: default_card_fallback_fonts(),
                font_dirs: default_font_dirs(),
                system_panel: default_system_panel(),
                card_shadows: false,
                image_cache_mb: default_image_cache_mb(),
                image_cache_max_image_mb: default_image_cache_max_image_mb(),
            },
            ankiweb: AnkiWebConfig {
                username: String::new(),
                password: String::new(),
                token: None,
                token_endpoint: None,
                endpoint: None,
                auto_sync: false,
                sync_on_exit: false,
                sync_media: true,
            },
            path: PathBuf::from(DEFAULT_CONFIG_PATH),
        }
    }
}

const DEFAULT_CONFIG_WITH_COMMENTS: &str = r#"# pbAnki Configuration File
# Generated automatically - edit with care

[general]
# Language code for Anki i18n (e.g., "en-GB", "de", "fr", "ja", "es", "pt", "ru", "zh", "ko")
language = "en-GB"

# Collection path (relative to app directory or absolute)
collection_path = "/mnt/ext1/applications/pbanki/collection"

# Font family for card text. PocketBook ships Ryo Gothic PlusN in the Adobe
# fonts folder (/mnt/ext1/system/adobe/fonts on-device, /system/adobe/fonts over USB).
card_font = "Ryo Gothic PlusN"

# Fallback families used when the card contains characters outside card_font.
card_fallback_fonts = ["Adobe Ming Std", "Adobe Heiti Std", "Noto Sans CJK JP"]

# Directories scanned for the fonts above. USB view /system/... maps to /mnt/ext1/system/... on-device.
font_dirs = [
    "/mnt/ext1/system/adobe/fonts",
    "/mnt/ext1/system/fonts",
    "/system/adobe/fonts",
    "/adobefonts",
    "/fonts",
    "/ebrmain/fonts",
]

# Show the PocketBook status bar (clock, battery, Wi-Fi) above pbAnki; tapping
# it opens the system tray. Restart pbAnki after changing this.
system_panel = true

# Draw the box shadows of card templates. Blurred shadows are slow on e-ink
# (about 0.2 s per page on a PB710), so they are off by default. Restart
# pbAnki after changing this.
card_shadows = false

# Memory for decoded card images, in MB: all images together, and the largest
# single image kept (a 1920x1080 picture is 8 MB). A cached image is not
# decoded again on the next card. 0 turns the cache off. Restart pbAnki after
# changing this.
image_cache_mb = 32
image_cache_max_image_mb = 16

[ankiweb]
# AnkiWeb synchronization settings
# Leave empty to disable sync

# AnkiWeb username (email)
username = ""

# Password (plain text - will be encrypted in future versions)
# WARNING: Do not share this file if password is filled
password = ""

# Session token (populated after successful login)
# This avoids storing password long-term
token = ""

# Endpoint that issued the session token above (managed automatically)
# token_endpoint = ""

# Optional self-hosted sync server URL. Leave empty to use AnkiWeb.
# Example: endpoint = "http://127.0.0.1:27701/"
endpoint = ""

# Sync automatically on app start
auto_sync = false

# Sync automatically after a study session ends
sync_on_exit = false

# Synchronize media files (audio/images) as well as the collection
sync_media = true
"#;

impl Config {
    pub fn load_or_create(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let path = Path::new(path);

        if path.exists() {
            let contents = fs::read_to_string(path)?;
            let mut config: Config = toml::from_str(&contents)?;
            config.path = path.to_path_buf();

            // Existing configs predate the font and panel settings. Add any
            // missing defaults without touching values the user already set.
            let needs_save = [
                "card_font",
                "card_fallback_fonts",
                "font_dirs",
                "system_panel",
                "card_shadows",
                "image_cache_mb",
                "image_cache_max_image_mb",
            ]
            .iter()
            .any(|key| !general_has_key(&contents, key));
            if needs_save {
                if let Err(err) = config.save() {
                    tracing::warn!("Could not add default settings to config: {err}");
                }
            }

            Ok(config)
        } else {
            let mut config = Config::default();
            config.path = path.to_path_buf();
            config.save_with_comments(path.to_str().unwrap())?;
            Ok(config)
        }
    }

    /// Like `load_or_create`, but logs an unreadable config and continues with
    /// the defaults, saved to `path` on the next change.
    pub fn load_or_default(path: &str) -> Self {
        Self::load_or_create(path).unwrap_or_else(|err| {
            tracing::warn!("Config error: {err:?}, using defaults");
            Config {
                path: path.into(),
                ..Config::default()
            }
        })
    }

    // Keep these assignments in sync with the fields in `Config`.
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = if self.path.as_os_str().is_empty() {
            Path::new(DEFAULT_CONFIG_PATH)
        } else {
            self.path.as_path()
        };

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Merge into the existing document so comments and user edits survive.
        let mut document = match fs::read_to_string(path) {
            Ok(contents) => contents
                .parse::<toml_edit::DocumentMut>()
                .unwrap_or_else(|_| default_config_document()),
            Err(_) => default_config_document(),
        };

        let general = table_mut(&mut document, "general")?;
        general["language"] = value(&self.general.language);
        general["collection_path"] = value(&self.general.collection_path);
        general["card_font"] = value(&self.general.card_font);
        general["card_fallback_fonts"] =
            Item::Value(string_array(&self.general.card_fallback_fonts));
        general["font_dirs"] = Item::Value(string_array(&self.general.font_dirs));
        set_with_comment(
            general,
            "system_panel",
            value(self.general.system_panel),
            SYSTEM_PANEL_COMMENT,
        );
        set_with_comment(
            general,
            "card_shadows",
            value(self.general.card_shadows),
            CARD_SHADOWS_COMMENT,
        );
        set_with_comment(
            general,
            "image_cache_mb",
            value(i64::from(self.general.image_cache_mb)),
            IMAGE_CACHE_COMMENT,
        );
        general["image_cache_max_image_mb"] =
            value(i64::from(self.general.image_cache_max_image_mb));

        let ankiweb = table_mut(&mut document, "ankiweb")?;
        ankiweb["username"] = value(&self.ankiweb.username);
        ankiweb["password"] = value(&self.ankiweb.password);
        ankiweb["token"] = value(self.ankiweb.token.as_deref().unwrap_or_default());
        ankiweb["endpoint"] = value(self.ankiweb.endpoint.as_deref().unwrap_or_default());
        ankiweb["auto_sync"] = value(self.ankiweb.auto_sync);
        ankiweb["sync_on_exit"] = value(self.ankiweb.sync_on_exit);
        ankiweb["sync_media"] = value(self.ankiweb.sync_media);

        match self.ankiweb.token_endpoint.as_deref() {
            Some(token_endpoint) => ankiweb["token_endpoint"] = value(token_endpoint),
            None => {
                ankiweb.remove("token_endpoint");
            }
        }

        fs::write(path, document.to_string())?;
        Ok(())
    }

    fn save_with_comments(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(path, DEFAULT_CONFIG_WITH_COMMENTS)?;
        Ok(())
    }

    pub fn update_and_save(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.save()
    }
}

/// Sets `key`. A key that is new in this file gets `comment` above it.
fn set_with_comment(table: &mut Table, key: &str, item: Item, comment: &str) {
    let new_key = !table.contains_key(key);
    table[key] = item;
    if new_key && let Some(mut key) = table.key_mut(key) {
        key.leaf_decor_mut().set_prefix(comment);
    }
}

fn default_config_document() -> DocumentMut {
    DEFAULT_CONFIG_WITH_COMMENTS
        .parse()
        .expect("default config template must be valid TOML")
}

fn string_array(values: &[String]) -> Value {
    let mut array = Array::new();
    for value in values {
        array.push(value.as_str());
    }
    Value::Array(array)
}

fn general_has_key(contents: &str, key: &str) -> bool {
    contents
        .parse::<DocumentMut>()
        .ok()
        .and_then(|document| {
            document
                .get("general")
                .and_then(Item::as_table)
                .map(|general| general.contains_key(key))
        })
        .unwrap_or(false)
}

fn table_mut<'a>(
    document: &'a mut DocumentMut,
    key: &str,
) -> Result<&'a mut Table, Box<dyn std::error::Error>> {
    if !document.as_table().contains_key(key) {
        document[key] = toml_edit::Item::Table(Table::new());
    }
    document[key]
        .as_table_mut()
        .ok_or_else(|| format!("config key `{key}` is not a table").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config_path(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "pbanki-{name}-{}-{unique}.toml",
            std::process::id()
        ))
    }

    #[test]
    fn default_template_parses() {
        let config: Config = toml::from_str(DEFAULT_CONFIG_WITH_COMMENTS).unwrap();
        assert_eq!(config.general.card_font, "Ryo Gothic PlusN");
        assert!(!config.general.card_fallback_fonts.is_empty());
        assert!(!config.general.font_dirs.is_empty());
    }

    #[test]
    fn new_config_file_is_written_with_font_settings() {
        let path = temp_config_path("new");
        let config = Config::load_or_create(path.to_str().unwrap()).unwrap();
        assert_eq!(config.general.card_font, "Ryo Gothic PlusN");
        assert_eq!(
            config.general.card_fallback_fonts,
            vec![
                "Adobe Ming Std".to_string(),
                "Adobe Heiti Std".to_string(),
                "Noto Sans CJK JP".to_string(),
            ]
        );
        assert!(
            config
                .general
                .font_dirs
                .iter()
                .any(|dir| dir == "/mnt/ext1/system/adobe/fonts")
        );

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains(r#"card_font = "Ryo Gothic PlusN""#));
        assert!(contents.contains("card_fallback_fonts"));
        assert!(contents.contains(r#""/mnt/ext1/system/adobe/fonts""#));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn existing_config_gets_missing_font_defaults() {
        let path = temp_config_path("migrate");
        fs::write(
            &path,
            r#"[general]
language = "en-GB"
collection_path = "/tmp/collection"

[ankiweb]
username = ""
password = ""
"#,
        )
        .unwrap();

        let config = Config::load_or_create(path.to_str().unwrap()).unwrap();
        assert_eq!(config.general.card_font, "Ryo Gothic PlusN");
        assert!(!config.general.card_fallback_fonts.is_empty());
        assert!(!config.general.font_dirs.is_empty());

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("card_font"));
        assert!(contents.contains("card_fallback_fonts"));
        assert!(contents.contains("font_dirs"));
        assert!(config.general.system_panel);
        assert!(contents.contains("# Show the PocketBook status bar"));
        assert!(contents.contains("system_panel = true"));
        assert!(!config.general.card_shadows);
        assert!(contents.contains("# Draw the box shadows of card templates"));
        assert!(contents.contains("card_shadows = false"));
        assert_eq!(
            (
                config.general.image_cache_mb,
                config.general.image_cache_max_image_mb
            ),
            (32, 16)
        );
        assert!(contents.contains("# Memory for decoded card images"));
        assert!(contents.contains("image_cache_mb = 32"));
        assert!(contents.contains("image_cache_max_image_mb = 16"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn card_settings_are_read_and_preserved() {
        let path = temp_config_path("shadows");
        fs::write(
            &path,
            r#"[general]
language = "en-GB"
collection_path = "/tmp/collection"
card_font = "Ryo Gothic PlusN"
card_fallback_fonts = []
font_dirs = []
system_panel = true
card_shadows = true
image_cache_mb = 8
image_cache_max_image_mb = 2

[ankiweb]
username = ""
password = ""
"#,
        )
        .unwrap();

        let mut config = Config::load_or_create(path.to_str().unwrap()).unwrap();
        assert!(config.general.card_shadows);
        assert_eq!(
            (
                config.general.image_cache_mb,
                config.general.image_cache_max_image_mb
            ),
            (8, 2)
        );
        config.ankiweb.username = "user".into();
        config.save().unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("card_shadows = true"));
        assert!(contents.contains("image_cache_mb = 8"));
        assert!(contents.contains("image_cache_max_image_mb = 2"));
        // Present already, so no comments are added.
        assert!(!contents.contains("# Draw the box shadows"));
        assert!(!contents.contains("# Memory for decoded card images"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn system_panel_is_read_and_preserved() {
        let path = temp_config_path("panel");
        fs::write(
            &path,
            r#"[general]
language = "en-GB"
collection_path = "/tmp/collection"
card_font = "Ryo Gothic PlusN"
card_fallback_fonts = []
font_dirs = []
system_panel = false

[ankiweb]
username = ""
password = ""
"#,
        )
        .unwrap();

        let mut config = Config::load_or_create(path.to_str().unwrap()).unwrap();
        assert!(!config.general.system_panel);
        config.ankiweb.username = "user".into();
        config.save().unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("system_panel = false"));
        // Present already, so no comment is added.
        assert!(!contents.contains("# Show the PocketBook status bar"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn existing_font_settings_are_preserved() {
        let path = temp_config_path("preserve");
        fs::write(
            &path,
            r#"[general]
language = "en-GB"
collection_path = "/tmp/collection"
card_font = "Custom Font"
card_fallback_fonts = ["Custom Fallback"]
font_dirs = ["/custom/fonts"]

[ankiweb]
username = ""
password = ""
"#,
        )
        .unwrap();

        let config = Config::load_or_create(path.to_str().unwrap()).unwrap();
        assert_eq!(config.general.card_font, "Custom Font");
        assert_eq!(
            config.general.card_fallback_fonts,
            vec!["Custom Fallback".to_string()]
        );
        assert_eq!(config.general.font_dirs, vec!["/custom/fonts".to_string()]);

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains(r#"card_font = "Custom Font""#));
        assert!(contents.contains(r#"card_fallback_fonts = ["Custom Fallback"]"#));

        let _ = fs::remove_file(path);
    }
}
