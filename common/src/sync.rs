use anki::collection::Collection;
use anki::sync::collection::normal::SyncActionRequired;
use anki::sync::login::{SyncAuth, sync_login};
use anki::sync::media::progress::MediaSyncProgress;
use reqwest::Url;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum SyncStatus {
    Idle,
    Authenticating,
    CheckingStatus,
    SyncingCollection {
        progress: String,
    },
    SyncingMedia {
        checked: String,
        added: String,
        removed: String,
    },
    Complete {
        message: String,
    },
    Error {
        message: String,
    },
}

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

pub struct SyncManager {
    http_client: reqwest::Client,
    full_http_client: reqwest::Client,
    sync_status: Arc<Mutex<SyncStatus>>,
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
            sync_status: Arc::new(Mutex::new(SyncStatus::Idle)),
        }
    }

    pub fn get_status(&self) -> SyncStatus {
        self.sync_status.lock().unwrap().clone()
    }

    fn set_status(&self, status: SyncStatus) {
        *self.sync_status.lock().unwrap() = status;
    }

    pub async fn login(
        &self,
        username: &str,
        password: &str,
        endpoint: Option<&str>,
    ) -> Result<SyncAuth, Box<dyn std::error::Error>> {
        self.set_status(SyncStatus::Authenticating);

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
        self.set_status(SyncStatus::CheckingStatus);
        self.set_status(SyncStatus::SyncingCollection {
            progress: "Syncing collection...".into(),
        });

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
        }
        .to_string();

        self.set_status(SyncStatus::Complete {
            message: message.clone(),
        });

        Ok(SyncResult {
            success: true,
            message,
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
        self.set_status(SyncStatus::SyncingCollection {
            progress: if upload {
                "Uploading collection...".into()
            } else {
                "Downloading collection...".into()
            },
        });

        let result = if upload {
            collection
                .full_upload(auth, self.full_http_client.clone())
                .await
        } else {
            collection
                .full_download(auth, self.full_http_client.clone())
                .await
        };

        match result {
            Ok(()) => {
                self.set_status(SyncStatus::Complete {
                    message: if upload {
                        "Collection uploaded".into()
                    } else {
                        "Collection downloaded".into()
                    },
                });
                Ok(())
            }
            Err(err) => {
                self.set_status(SyncStatus::Error {
                    message: err.to_string(),
                });
                Err(Box::new(err))
            }
        }
    }

    pub async fn sync_media(
        &self,
        collection: &Collection,
        auth: SyncAuth,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.set_status(SyncStatus::SyncingMedia {
            checked: String::new(),
            added: String::new(),
            removed: String::new(),
        });

        let media = collection.media()?;
        let progress = collection.new_progress_handler::<MediaSyncProgress>();
        media
            .sync_media(progress, auth, self.full_http_client.clone(), None)
            .await?;

        self.set_status(SyncStatus::Complete {
            message: "Media synced successfully".into(),
        });
        Ok(())
    }
}

impl Default for SyncManager {
    fn default() -> Self {
        Self::new()
    }
}

pub fn parse_endpoint(endpoint: Option<&str>) -> Result<Option<Url>, Box<dyn std::error::Error>> {
    let Some(endpoint) = endpoint.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    let url = Url::parse(endpoint).map_err(|err| format!("Invalid sync server URL: {err}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Sync server URL must start with http:// or https://".into());
    }
    let url = url
        .join("./")
        .map_err(|err| format!("Invalid sync server URL: {err}"))?;
    Ok(Some(url))
}

pub fn normalize_endpoint(endpoint: &str) -> Result<Option<String>, String> {
    parse_endpoint(Some(endpoint))
        .map(|url| url.map(|url| url.to_string()))
        .map_err(|err| err.to_string())
}
