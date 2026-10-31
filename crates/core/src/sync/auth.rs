//! Sync credentials: the saved host key, login, and server URL handling.

use anki::sync::login::SyncAuth;
use reqwest::Url;

use crate::config::Config;
use crate::session::LearnSession;

/// Returns the saved host key if it was issued for the current endpoint, and
/// otherwise logs in and saves the new key.
pub(super) fn sync_auth(
    session: &LearnSession,
    config: &mut Config,
    rt: &tokio::runtime::Runtime,
) -> Result<SyncAuth, String> {
    let endpoint = config.ankiweb.effective_endpoint();
    let endpoint_url = parse_endpoint(endpoint).map_err(|err| err.to_string())?;

    let normalized_endpoint = endpoint_url.as_ref().map(ToString::to_string);
    let token_endpoint_matches = match config.ankiweb.token_endpoint.as_deref() {
        None => true,
        Some(recorded) => Some(recorded) == normalized_endpoint.as_deref(),
    };

    if let Some(token) = config
        .ankiweb
        .token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .filter(|_| token_endpoint_matches)
    {
        return Ok(SyncAuth {
            hkey: token.to_string(),
            endpoint: endpoint_url,
            io_timeout_secs: Some(60),
        });
    }

    let auth = rt
        .block_on(session.sync_manager.login(
            &config.ankiweb.username,
            &config.ankiweb.password,
            endpoint,
        ))
        .map_err(|err| format!("Login failed: {err}"))?;

    config.ankiweb.token = Some(auth.hkey.clone());
    config.ankiweb.token_endpoint = normalized_endpoint;
    let _ = config.save();

    Ok(auth)
}

/// Parses a custom sync server URL. Blank means AnkiWeb (`None`).
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
