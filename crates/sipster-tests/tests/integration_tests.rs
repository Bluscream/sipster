//! Offline tests for `sipster-core`.
//!
//! These exercise configuration, redaction, and type invariants without a live
//! registrar. Call-path tests that need a PBX live behind a feature/env guard
//! (added when the Fritz!Box test harness lands) so `cargo test` stays hermetic.

use sipster_core::{CallEvent, CallId, CallState, Config, RegistrationState, SipAccount};

#[test]
fn default_account_uses_standard_sip_port() {
    let account = SipAccount::default();
    assert_eq!(account.port, 5060);
    assert!(account.registrar.is_empty());
}

#[test]
fn empty_registrar_fails_validation() {
    let account = SipAccount::default();
    assert!(account.validate().is_err(), "empty registrar must be rejected");
}

#[test]
fn valid_account_passes_validation() {
    let account = SipAccount {
        registrar: "fritz.box".into(),
        username: "620".into(),
        ..SipAccount::default()
    };
    assert!(account.validate().is_ok());
}

#[test]
fn auth_user_falls_back_to_username() {
    let account = SipAccount {
        username: "620".into(),
        auth_user: String::new(),
        ..SipAccount::default()
    };
    assert_eq!(account.effective_auth_user(), "620");

    let explicit = SipAccount {
        username: "620".into(),
        auth_user: "sip620".into(),
        ..SipAccount::default()
    };
    assert_eq!(explicit.effective_auth_user(), "sip620");
}

#[test]
fn password_is_never_shown_in_debug() {
    let account = SipAccount {
        password: "s3cr3t-pw".into(),
        ..SipAccount::default()
    };
    let rendered = format!("{account:?}");
    assert!(!rendered.contains("s3cr3t-pw"), "password leaked via Debug: {rendered}");
    assert!(rendered.contains("<redacted>"));
}

#[test]
fn config_parses_from_toml() {
    let toml = r#"
        [[accounts]]
        label = "Home"
        registrar = "fritz.box"
        port = 5060
        username = "620"
        password = "pw"
    "#;
    let config: Config = toml::from_str(toml).expect("valid config");
    assert_eq!(config.accounts.len(), 1);
    assert_eq!(config.accounts[0].registrar, "fritz.box");
    assert_eq!(config.accounts[0].expires, 600, "expires should default");
}

#[test]
fn missing_config_file_is_not_an_error() {
    let config = Config::load("/nonexistent/sipster.toml").expect("missing file -> empty config");
    assert!(config.accounts.is_empty());
}

#[test]
fn call_ids_are_unique() {
    assert_ne!(CallId::new(), CallId::new());
}

#[test]
fn call_events_round_trip_json() {
    let id = CallId::new();
    let event = CallEvent::StateChanged { id, state: CallState::Active };
    let json = serde_json::to_string(&event).unwrap();
    let back: CallEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(event, back);

    let reg = CallEvent::Registration(RegistrationState::Registered);
    let json = serde_json::to_string(&reg).unwrap();
    assert_eq!(reg, serde_json::from_str::<CallEvent>(&json).unwrap());
}
