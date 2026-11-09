//! Collection and media sync with AnkiWeb or a custom sync server.
//!
//! `sync_now` and `full_sync` block the calling (UI) thread. Full sync takes
//! the collection out of `LearnSession.collection` and reopens it afterwards.

mod auth;
mod backup;
mod client;
#[cfg(test)]
mod tests;

use std::time::Instant;

use anki::sync::login::SyncAuth;
use tokio::runtime::Runtime;

use crate::config::Config;
use crate::session::{LearnSession, reopen_collection, reset_session_memory};

pub use auth::{normalize_endpoint, parse_endpoint};
pub use client::SyncManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncAction {
    #[default]
    NoChanges,
    NormalSync,
    FullSyncRequired {
        upload_ok: bool,
        download_ok: bool,
    },
}

#[derive(Debug, Clone, Default)]
pub struct SyncResult {
    pub success: bool,
    pub message: String,
    pub server_message: Option<String>,
    pub action: SyncAction,
    pub media_error: Option<String>,
    pub new_endpoint: Option<String>,
}

impl SyncResult {
    fn error(message: String) -> Self {
        Self {
            message,
            ..Default::default()
        }
    }
}

pub fn sync_now(session: &LearnSession, config: &mut Config) -> SyncResult {
    let rt = Runtime::new().unwrap();
    let mut auth = match auth::sync_auth(session, config, &rt) {
        Ok(auth) => auth,
        Err(message) => return SyncResult::error(message),
    };

    let sync_result = {
        let mut col_borrow = session.collection.borrow_mut();
        let Some(col) = col_borrow.as_mut() else {
            return SyncResult::error("Collection is unavailable".into());
        };
        rt.block_on(session.sync_manager.sync_collection(col, &auth))
    };

    let mut result = match sync_result {
        Ok(result) => result,
        Err(err) => return SyncResult::error(format!("Sync failed: {err}")),
    };

    if let Some(endpoint) = result.new_endpoint.take() {
        tracing::info!("[sync] server moved this account to {endpoint}");
        let normalized = normalize_endpoint(&endpoint).ok().flatten();
        // Media sync below must use the new server too.
        if let Ok(Some(url)) = parse_endpoint(Some(&endpoint)) {
            auth.endpoint = Some(url);
        }
        config.ankiweb.endpoint = Some(endpoint);
        if config.ankiweb.token.is_some() {
            config.ankiweb.token_endpoint = normalized;
        }
        let _ = config.save();
    }

    if matches!(result.action, SyncAction::FullSyncRequired { .. }) {
        return result;
    }

    sync_media_if_enabled(session, config, &rt, auth, result)
}

pub fn full_sync(session: &LearnSession, config: &mut Config, upload: bool) -> SyncResult {
    let rt = Runtime::new().unwrap();
    let auth = match auth::sync_auth(session, config, &rt) {
        Ok(auth) => auth,
        Err(message) => return SyncResult::error(message),
    };

    let mut builder = {
        let col_borrow = session.collection.borrow();
        let Some(col) = col_borrow.as_ref() else {
            return SyncResult::error("Collection is unavailable".into());
        };
        col.as_builder()
    };

    if !upload {
        // Close the collection first so SQLite checkpoints the WAL and the files
        // copied below are self-consistent, then reopen it for the download.
        let collection = match session.collection.borrow_mut().take() {
            Some(collection) => collection,
            None => return SyncResult::error("Collection is unavailable".into()),
        };
        if let Err(err) = collection.close(None) {
            return SyncResult::error(format!("Could not close collection before backup: {err}"));
        }
        if let Err(err) = backup::backup_collection(config) {
            let reopen_note = match reopen_collection(session, &mut builder, config) {
                Ok(()) => String::new(),
                Err(reopen_err) => format!("; {reopen_err}"),
            };
            return SyncResult::error(format!(
                "Could not back up local collection: {err}{reopen_note}"
            ));
        }
        tracing::info!("[sync] backed up the local collection before download");
        if let Err(err) = reopen_collection(session, &mut builder, config) {
            return SyncResult::error(format!(
                "Could not reopen collection before download: {err}"
            ));
        }
    }

    let collection = match session.collection.borrow_mut().take() {
        Some(collection) => collection,
        None => return SyncResult::error("Collection is unavailable".into()),
    };

    let sync_result = rt.block_on(
        session
            .sync_manager
            .full_sync(collection, auth.clone(), upload),
    );
    let reopen_result = reopen_collection(session, &mut builder, config);

    match (sync_result, reopen_result) {
        (Ok(()), Ok(())) => {}
        (Err(err), Ok(())) => return SyncResult::error(format!("Full sync failed: {err}")),
        (Ok(()), Err(err)) => {
            return SyncResult::error(format!("Full sync completed, but {err}"));
        }
        (Err(sync_err), Err(reopen_err)) => {
            return SyncResult::error(format!("Full sync failed: {sync_err}; {reopen_err}"));
        }
    }

    reset_session_memory(session);

    let result = SyncResult {
        success: true,
        message: if upload {
            "Collection uploaded successfully".into()
        } else {
            "Collection downloaded successfully".into()
        },
        ..Default::default()
    };

    sync_media_if_enabled(session, config, &rt, auth, result)
}

/// Runs media sync after a successful collection sync. A media failure is a
/// warning on the collection result, not a sync failure.
fn sync_media_if_enabled(
    session: &LearnSession,
    config: &Config,
    rt: &Runtime,
    auth: SyncAuth,
    mut result: SyncResult,
) -> SyncResult {
    if !config.ankiweb.sync_media {
        tracing::info!("[sync] media: off (sync_media = false)");
        return result;
    }

    let started = Instant::now();
    let media_result = {
        let col_borrow = session.collection.borrow();
        let Some(col) = col_borrow.as_ref() else {
            return SyncResult::error("Collection is unavailable".into());
        };
        rt.block_on(session.sync_manager.sync_media(col, auth))
    };

    match media_result {
        Ok(()) => tracing::info!("[sync] media: done in {} ms", started.elapsed().as_millis()),
        // Logged with the collection result by the frontend.
        Err(err) => result.media_error = Some(format!("Media sync failed: {err}")),
    }
    result
}
