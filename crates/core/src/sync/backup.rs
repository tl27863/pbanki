use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::session::collection_file;

/// Copies the collection and its SQLite side files to `*.backup-<unix time>`
/// before a full download replaces them.
pub(super) fn backup_collection(config: &Config) -> Result<(), String> {
    let path = collection_file(config);
    if !path.exists() {
        return Ok(());
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| err.to_string())?
        .as_secs();

    for suffix in ["", "-wal", "-shm"] {
        let source = if suffix.is_empty() {
            path.clone()
        } else {
            Path::new(&format!("{}{suffix}", path.display())).to_path_buf()
        };
        if source.exists() {
            let file_name = source
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("collection.anki2");
            let backup = source.with_file_name(format!("{file_name}.backup-{timestamp}"));
            fs::copy(&source, &backup).map_err(|err| err.to_string())?;
        }
    }

    Ok(())
}
