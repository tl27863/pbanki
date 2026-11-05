use std::os::raw::c_int;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use inkview::bindings::{
    Inkview, NET_ABORTED, NET_CONNECTED, NET_EAUTH, NET_EBADCONF, NET_EDISABLED, NET_EFLIGHTMODE,
    NET_EHARDWARE, NET_EINIT, NET_ENOTCONF, NET_ENOTFOUND, NET_ETIMEOUT, NET_EWRONGKEY, NET_OK,
    NET_WIFIREADY,
};

static CONNECTED_BY_APP: AtomicBool = AtomicBool::new(false);
/// The radio was off before the sync and pbAnki powered it on.
static POWERED_BY_APP: AtomicBool = AtomicBool::new(false);
static OWNED_SSID: Mutex<Option<String>> = Mutex::new(None);

// Log lines say whether the network changed but never name it (SSIDs are
// personal data and the logs get shared).

pub fn ensure_connected(iv: &'static Inkview) -> Result<(), String> {
    if is_connected(iv) {
        tracing::info!("[wifi] already connected; leaving it on after sync");
        clear_ownership();
        return Ok(());
    }

    let flags = query_network(iv);
    let radio_on = flags & NET_WIFIREADY != 0;
    tracing::info!(
        "[wifi] not connected; radio {} (flags {flags:#x}); opening the connect dialog",
        if radio_on { "on" } else { "off, powering it" }
    );
    let started = Instant::now();
    if !radio_on {
        POWERED_BY_APP.store(true, Ordering::SeqCst);
        unsafe {
            iv.WiFiPower(1);
        }
    }

    // NetConnectSilent can block for minutes on a cold radio, and can abort
    // when called off-thread. The system dialog is the reliable path used by
    // KOReader and lets the user enable Wi-Fi without leaving the app.
    let status = unsafe { iv.NetConnect(std::ptr::null()) };
    // The dialog drew over our frame, and InkView does not always send a
    // Foreground event afterwards. The request is consumed after the sync
    // callback returns, so the present shows the sync result.
    inkview_slint::request_full_present();

    let elapsed_ms = started.elapsed().as_millis();
    if status == NET_OK as c_int || is_connected(iv) {
        tracing::info!("[wifi] connected by pbAnki in {elapsed_ms} ms (status {status})");
        CONNECTED_BY_APP.store(true, Ordering::SeqCst);
        set_owned_ssid(current_ssid(iv));
        Ok(())
    } else {
        tracing::warn!("[wifi] connection failed after {elapsed_ms} ms (status {status})");
        if POWERED_BY_APP.load(Ordering::SeqCst) {
            power_off(iv);
        }
        clear_ownership();
        Err(connection_error(status))
    }
}

pub fn disconnect_after_sync(iv: &'static Inkview) {
    let owned = CONNECTED_BY_APP.swap(false, Ordering::SeqCst);
    let powered = POWERED_BY_APP.swap(false, Ordering::SeqCst);
    if !owned {
        return;
    }

    let expected = OWNED_SSID.lock().ok().and_then(|mut owned| owned.take());
    let current = current_ssid(iv);
    let same_network = should_disconnect(expected.as_deref(), current.as_deref());

    match after_sync_action(owned, powered, same_network) {
        AfterSync::Keep => {
            tracing::info!("[wifi] network changed during sync; leaving it connected");
        }
        AfterSync::Disconnect => {
            tracing::info!("[wifi] disconnecting the connection pbAnki made");
            unsafe {
                iv.NetDisconnect();
            }
        }
        AfterSync::DisconnectAndPowerOff => {
            tracing::info!("[wifi] disconnecting the connection pbAnki made");
            unsafe {
                iv.NetDisconnect();
            }
            power_off(iv);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AfterSync {
    Keep,
    Disconnect,
    DisconnectAndPowerOff,
}

/// What to do after a sync. `owned`: pbAnki made the connection. `powered`:
/// pbAnki turned the radio on. `same_network`: the connection is still the
/// one pbAnki made (the user did not switch networks during the sync).
fn after_sync_action(owned: bool, powered: bool, same_network: bool) -> AfterSync {
    match (owned, same_network, powered) {
        (false, _, _) | (true, false, _) => AfterSync::Keep,
        (true, true, false) => AfterSync::Disconnect,
        (true, true, true) => AfterSync::DisconnectAndPowerOff,
    }
}

fn power_off(iv: &Inkview) {
    tracing::info!("[wifi] powering the radio off (pbAnki turned it on)");
    unsafe {
        iv.WiFiPower(0);
    }
}

fn query_network(iv: &Inkview) -> u32 {
    unsafe { iv.QueryNetwork() as u32 }
}

fn is_connected(iv: &Inkview) -> bool {
    query_network(iv) & NET_CONNECTED != 0
}

fn clear_ownership() {
    CONNECTED_BY_APP.store(false, Ordering::SeqCst);
    POWERED_BY_APP.store(false, Ordering::SeqCst);
    if let Ok(mut owned) = OWNED_SSID.lock() {
        *owned = None;
    }
}

fn set_owned_ssid(ssid: Option<String>) {
    if let Ok(mut owned) = OWNED_SSID.lock() {
        *owned = ssid;
    }
}

fn current_ssid(iv: &Inkview) -> Option<String> {
    unsafe {
        let info = iv.NetInfo();
        if info.is_null() {
            return None;
        }

        let name = &(*info).name;
        let len = name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(name.len());
        let bytes: Vec<u8> = name[..len].iter().map(|&byte| byte as u8).collect();
        let ssid = String::from_utf8_lossy(&bytes).into_owned();

        (!ssid.is_empty()).then_some(ssid)
    }
}

fn should_disconnect(expected: Option<&str>, current: Option<&str>) -> bool {
    match (expected, current) {
        (Some(expected), Some(current)) => expected == current,
        _ => true,
    }
}

fn connection_error(status: c_int) -> String {
    match status {
        NET_EAUTH | NET_EWRONGKEY => {
            "Wi-Fi authentication failed. Re-enter the network password in PocketBook Settings > Wi-Fi, then tap sync again.".to_string()
        }
        NET_ABORTED => {
            "Wi-Fi connection was canceled or interrupted. Tap sync again to retry.".to_string()
        }
        NET_EINIT => {
            "Wi-Fi is still starting. Wait a moment and tap sync again.".to_string()
        }
        NET_EDISABLED => {
            "Wi-Fi is off. Turn it on from the PocketBook quick settings, then tap sync again.".to_string()
        }
        NET_EFLIGHTMODE => {
            "Airplane mode is on. Turn it off in PocketBook settings, then tap sync again."
                .to_string()
        }
        NET_EHARDWARE => "Wi-Fi hardware is unavailable on this device.".to_string(),
        NET_ENOTCONF | NET_ENOTFOUND => {
            "No saved Wi-Fi network. Open PocketBook Settings > Wi-Fi, join a network, then tap sync again.".to_string()
        }
        NET_ETIMEOUT => {
            "Wi-Fi connection timed out. Check the network, then tap sync again.".to_string()
        }
        NET_EBADCONF => {
            "Saved Wi-Fi settings are invalid. Reconfigure the network in PocketBook Settings > Wi-Fi.".to_string()
        }
        _ => format!("Wi-Fi connection failed (error {status}). Tap sync to retry."),
    }
}

#[cfg(test)]
mod tests {
    use super::{AfterSync, after_sync_action, should_disconnect};

    #[test]
    fn powers_off_a_radio_pbanki_turned_on() {
        assert_eq!(
            after_sync_action(true, true, true),
            AfterSync::DisconnectAndPowerOff
        );
    }

    #[test]
    fn leaves_a_radio_that_was_already_on() {
        assert_eq!(after_sync_action(true, false, true), AfterSync::Disconnect);
    }

    #[test]
    fn keeps_radio_and_connection_when_user_switched_networks() {
        assert_eq!(after_sync_action(true, true, false), AfterSync::Keep);
        assert_eq!(after_sync_action(true, false, false), AfterSync::Keep);
    }

    #[test]
    fn leaves_connections_pbanki_did_not_make() {
        assert_eq!(after_sync_action(false, false, true), AfterSync::Keep);
    }

    #[test]
    fn disconnects_when_owned_ssid_is_still_active() {
        assert!(should_disconnect(Some("Home"), Some("Home")));
    }

    #[test]
    fn keeps_connection_when_user_switched_networks() {
        assert!(!should_disconnect(Some("Home"), Some("Cafe")));
    }

    #[test]
    fn falls_back_to_disconnect_when_ssid_is_unknown() {
        assert!(should_disconnect(None, Some("Home")));
        assert!(should_disconnect(Some("Home"), None));
        assert!(should_disconnect(None, None));
    }
}
