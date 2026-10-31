//! Thin async wrapper over anki's sync client with pbAnki's HTTP timeouts.

use anki::collection::Collection;
use anki::sync::collection::normal::SyncActionRequired;
use anki::sync::login::{SyncAuth, sync_login};
use anki::sync::media::progress::MediaSyncProgress;
use std::time::Duration;

use super::auth::parse_endpoint;
use super::{SyncAction, SyncResult};

pub struct SyncManager {
    http_client: reqwest::Client,
    full_http_client: reqwest::Client,
}

impl SyncManager {
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap(),
            full_http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(600))
                .build()
                .unwrap(),
        }
    }

    pub async fn login(
        &self,
        username: &str,
        password: &str,
        endpoint: Option<&str>,
    ) -> Result<SyncAuth, Box<dyn std::error::Error>> {
        let endpoint_url = parse_endpoint(endpoint)?;
        let auth = sync_login(
            username,
            password,
            endpoint.map(str::to_owned),
            self.http_client.clone(),
        )
        .await?;

        Ok(SyncAuth {
            hkey: auth.hkey,
            endpoint: endpoint_url,
            io_timeout_secs: Some(60),
        })
    }

    pub async fn sync_collection(
        &self,
        collection: &mut Collection,
        auth: &SyncAuth,
    ) -> Result<SyncResult, Box<dyn std::error::Error>> {
        let output = collection
            .normal_sync(auth.clone(), self.http_client.clone())
            .await?;
        let action = match output.required {
            SyncActionRequired::NoChanges => SyncAction::NoChanges,
            SyncActionRequired::NormalSyncRequired => SyncAction::NormalSync,
            SyncActionRequired::FullSyncRequired {
                upload_ok,
                download_ok,
            } => SyncAction::FullSyncRequired {
                upload_ok,
                download_ok,
            },
        };
        let message = match action {
            SyncAction::NoChanges => "Sync complete",
            SyncAction::NormalSync => "Collection synced successfully",
            SyncAction::FullSyncRequired { .. } => "Full sync required",
        };

        Ok(SyncResult {
            success: true,
            message: message.to_string(),
            server_message: (!output.server_message.is_empty()).then_some(output.server_message),
            action,
            media_error: None,
            new_endpoint: output.new_endpoint,
        })
    }

    pub async fn full_sync(
        &self,
        collection: Collection,
        auth: SyncAuth,
        upload: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if upload {
            collection
                .full_upload(auth, self.full_http_client.clone())
                .await?;
        } else {
            collection
                .full_download(auth, self.full_http_client.clone())
                .await?;
        }
        Ok(())
    }

    pub async fn sync_media(
        &self,
        collection: &Collection,
        auth: SyncAuth,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let media = collection.media()?;
        let progress = collection.new_progress_handler::<MediaSyncProgress>();
        media
            .sync_media(progress, auth, self.full_http_client.clone(), None)
            .await?;
        Ok(())
    }
}

impl Default for SyncManager {
    fn default() -> Self {
        Self::new()
    }
}
