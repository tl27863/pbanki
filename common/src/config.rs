use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Table, value};

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
            Ok(config)
        } else {
            let mut config = Config::default();
            config.path = path.to_path_buf();
            config.save_with_comments(path.to_str().unwrap())?;
            Ok(config)
        }
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

fn default_config_document() -> DocumentMut {
    DEFAULT_CONFIG_WITH_COMMENTS
        .parse()
        .expect("default config template must be valid TOML")
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
