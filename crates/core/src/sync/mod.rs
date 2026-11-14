//! Collection and media sync with AnkiWeb or a custom sync server.
//!
//! `sync_now` and `full_sync` block the calling thread: the frontend runs them
//! as a collection job on a worker thread (`frontend/sync.rs`). They get the
//! collection, a copy of the config, and the sync client, and touch nothing
//! else. Config changes (a new token, a moved endpoint) come back in
//! `SyncOutcome` for the UI thread to apply and save. A full sync closes the
//! collection and opens it again.

mod auth;
mod backup;
mod client;
#[cfg(test)]
mod tests;

use std::time::Instant;

use anki::collection::Collection;
use anki::sync::login::SyncAuth;
use tokio::runtime::Runtime;

use crate::config::{AnkiWebConfig, Config};
use crate::session::reopen_collection;

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

/// The sync account state that a sync can change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountChanges {
    pub token: Option<String>,
    pub token_endpoint: Option<String>,
    pub endpoint: Option<String>,
}

/// The result of a sync and the config changes that it made.
#[derive(Debug, Clone, Default)]
pub struct SyncOutcome {
    pub result: SyncResult,
    /// `Some` if the sync logged in or the server moved the account.
    pub changes: Option<AccountChanges>,
}

impl SyncOutcome {
    fn new(result: SyncResult, before: &AnkiWebConfig, after: AnkiWebConfig) -> Self {
        let changed = before.token != after.token
            || before.token_endpoint != after.token_endpoint
            || before.endpoint != after.endpoint;
        Self {
            result,
            changes: changed.then_some(AccountChanges {
                token: after.token,
                token_endpoint: after.token_endpoint,
                endpoint: after.endpoint,
            }),
        }
    }

    /// Writes the changes to `config` and saves it. Call on the thread that
    /// owns the config.
    pub fn apply_config(&self, config: &mut Config) {
        if let Some(changes) = &self.changes {
            config.ankiweb.token = changes.token.clone();
            config.ankiweb.token_endpoint = changes.token_endpoint.clone();
            config.ankiweb.endpoint = changes.endpoint.clone();
            if let Err(err) = config.save() {
                tracing::warn!("[sync] could not save the config: {err}");
            }
        }
    }
}

/// Normal sync, then media sync. `config` is a snapshot: it is not saved here.
pub fn sync_now(
    collection: &mut Option<Collection>,
    config: &Config,
    client: &SyncManager,
) -> SyncOutcome {
    let mut ankiweb = config.ankiweb.clone();
    let result = sync_now_inner(collection, &mut ankiweb, client);
    SyncOutcome::new(result, &config.ankiweb, ankiweb)
}

fn sync_now_inner(
    collection: &mut Option<Collection>,
    ankiweb: &mut AnkiWebConfig,
    client: &SyncManager,
) -> SyncResult {
    let rt = Runtime::new().unwrap();
    let mut auth = match auth::sync_auth(client, ankiweb, &rt) {
        Ok(auth) => auth,
        Err(message) => return SyncResult::error(message),
    };

    let Some(col) = collection.as_mut() else {
        return SyncResult::error("Collection is unavailable".into());
    };
    let mut result = match rt.block_on(client.sync_collection(col, &auth)) {
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
        ankiweb.endpoint = Some(endpoint);
        if ankiweb.token.is_some() {
            ankiweb.token_endpoint = normalized;
        }
    }

    if matches!(result.action, SyncAction::FullSyncRequired { .. }) {
        return result;
    }

    sync_media_if_enabled(collection, ankiweb, client, &rt, auth, result)
}

/// Full upload or download, then media sync. `config` is a snapshot: it is
/// not saved here. The collection is closed and opened again; it is `None`
/// afterwards only if it could not be reopened.
pub fn full_sync(
    collection: &mut Option<Collection>,
    config: &Config,
    client: &SyncManager,
    upload: bool,
) -> SyncOutcome {
    let mut ankiweb = config.ankiweb.clone();
    let result = full_sync_inner(collection, config, &mut ankiweb, client, upload);
    SyncOutcome::new(result, &config.ankiweb, ankiweb)
}

fn full_sync_inner(
    collection: &mut Option<Collection>,
    config: &Config,
    ankiweb: &mut AnkiWebConfig,
    client: &SyncManager,
    upload: bool,
) -> SyncResult {
    let rt = Runtime::new().unwrap();
    let auth = match auth::sync_auth(client, ankiweb, &rt) {
        Ok(auth) => auth,
        Err(message) => return SyncResult::error(message),
    };

    let Some(mut builder) = collection.as_ref().map(Collection::as_builder) else {
        return SyncResult::error("Collection is unavailable".into());
    };

    if !upload {
        // Close the collection first so SQLite checkpoints the WAL and the files
        // copied below are self-consistent, then reopen it for the download.
        let Some(open) = collection.take() else {
            return SyncResult::error("Collection is unavailable".into());
        };
        if let Err(err) = open.close(None) {
            return SyncResult::error(format!("Could not close collection before backup: {err}"));
        }
        let backup = backup::backup_collection(config);
        let reopened = reopen_collection(&mut builder, config);
        let reopen_err = match reopened {
            Ok(reopened) => {
                *collection = Some(reopened);
                None
            }
            Err(err) => Some(err),
        };
        match (backup, reopen_err) {
            (Ok(()), None) => {
                tracing::info!("[sync] backed up the local collection before download");
            }
            (Ok(()), Some(reopen_err)) => {
                return SyncResult::error(format!(
                    "Could not reopen collection before download: {reopen_err}"
                ));
            }
            (Err(err), reopen_err) => {
                let reopen_note = reopen_err.map(|err| format!("; {err}")).unwrap_or_default();
                return SyncResult::error(format!(
                    "Could not back up local collection: {err}{reopen_note}"
                ));
            }
        }
    }

    let Some(open) = collection.take() else {
        return SyncResult::error("Collection is unavailable".into());
    };

    let sync_result = rt.block_on(client.full_sync(open, auth.clone(), upload));
    let reopen_result = reopen_collection(&mut builder, config).map(|reopened| {
        *collection = Some(reopened);
    });

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

    let result = SyncResult {
        success: true,
        message: if upload {
            "Collection uploaded successfully".into()
        } else {
            "Collection downloaded successfully".into()
        },
        ..Default::default()
    };

    sync_media_if_enabled(collection, ankiweb, client, &rt, auth, result)
}

/// Runs media sync after a successful collection sync. A media failure is a
/// warning on the collection result, not a sync failure.
fn sync_media_if_enabled(
    collection: &Option<Collection>,
    ankiweb: &AnkiWebConfig,
    client: &SyncManager,
    rt: &Runtime,
    auth: SyncAuth,
    mut result: SyncResult,
) -> SyncResult {
    if !ankiweb.sync_media {
        tracing::info!("[sync] media: off (sync_media = false)");
        return result;
    }

    let started = Instant::now();
    let Some(col) = collection.as_ref() else {
        return SyncResult::error("Collection is unavailable".into());
    };

    match rt.block_on(client.sync_media(col, auth)) {
        Ok(()) => tracing::info!("[sync] media: done in {} ms", started.elapsed().as_millis()),
        // Logged with the collection result by the frontend.
        Err(err) => result.media_error = Some(format!("Media sync failed: {err}")),
    }
    result
}
