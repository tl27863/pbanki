use super::should_disconnect;

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
