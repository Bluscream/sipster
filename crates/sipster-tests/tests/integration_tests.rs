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

/// The engine rejects a registrar without a `sip:` scheme, which is exactly
/// how the first real Fritz!Box attempt failed.
#[test]
fn registrar_uri_gains_scheme_and_port() {
    let account = SipAccount {
        registrar: "192.168.2.1".into(),
        port: 5060,
        ..SipAccount::default()
    };
    assert_eq!(account.registrar_uri(), "sip:192.168.2.1:5060");
}

#[test]
fn registrar_uri_accepts_the_forms_users_type() {
    let with = |registrar: &str, port: u16| {
        SipAccount { registrar: registrar.into(), port, ..SipAccount::default() }.registrar_uri()
    };

    // Bare hostname, as shown in the Fritz!Box UI.
    assert_eq!(with("fritz.box", 5060), "sip:fritz.box:5060");
    // Explicit port must be preserved, not doubled.
    assert_eq!(with("fritz.box:5070", 5060), "sip:fritz.box:5070");
    // Already a full URI.
    assert_eq!(with("sip:fritz.box:5060", 5060), "sip:fritz.box:5060");
    assert_eq!(with("sip:fritz.box", 5060), "sip:fritz.box:5060");
    // TLS scheme is preserved rather than downgraded.
    assert_eq!(with("sips:secure.example", 5061), "sips:secure.example:5061");
    // Surrounding whitespace from copy/paste.
    assert_eq!(with("  fritz.box  ", 5060), "sip:fritz.box:5060");
}

#[test]
fn registrar_uri_handles_ipv6() {
    let with = |registrar: &str| {
        SipAccount { registrar: registrar.into(), port: 5060, ..SipAccount::default() }
            .registrar_uri()
    };
    // Bare IPv6 has many colons but no port; it must not be mistaken for one.
    assert_eq!(with("[2001:db8::1]"), "sip:[2001:db8::1]:5060");
    assert_eq!(with("[2001:db8::1]:5070"), "sip:[2001:db8::1]:5070");
}

/// The config file is the only source of configuration; the environment
/// variables that used to supply an account are gone, so a file with no
/// account must read as "needs setup" rather than silently picking one up.
#[test]
fn a_config_without_an_account_needs_setup() {
    let config: Config = toml::from_str("").expect("an empty config is valid");
    assert!(config.accounts.is_empty());
    assert!(config.needs_setup());
}

#[test]
fn a_config_with_a_usable_account_does_not_need_setup() {
    let toml = r#"
        [[accounts]]
        registrar = "fritz.box"
        username = "620"
    "#;
    let config: Config = toml::from_str(toml).expect("valid config");
    assert!(!config.needs_setup());
}

/// An account that could never register must still count as unconfigured, or
/// the settings window would not open and the user would be stuck.
#[test]
fn an_unusable_account_still_needs_setup() {
    let toml = r#"
        [[accounts]]
        registrar = ""
        username = ""
    "#;
    let config: Config = toml::from_str(toml).expect("valid config");
    assert!(config.needs_setup());
}

/// Binding SIP to loopback makes every send to a LAN registrar fail with
/// EINVAL; the bind address must be on the interface that routes to the peer.
#[test]
fn bind_address_is_not_loopback_for_a_routable_peer() {
    let peer: std::net::SocketAddr = "1.1.1.1:5060".parse().unwrap();
    let Ok(bind) = sipster_core::net::bind_address(peer, 0) else {
        return; // no route in a sandboxed environment; nothing to assert
    };
    assert!(!bind.ip().is_loopback(), "bound loopback for a routable peer: {bind}");
    assert!(bind.is_ipv4(), "address family must match the peer: {bind}");
}

#[test]
fn resolve_prefers_ipv4() {
    let addr = sipster_core::net::resolve("127.0.0.1", 5060).expect("literal resolves");
    assert_eq!(addr.port(), 5060);
    assert!(addr.is_ipv4());
}

#[test]
fn resolve_reports_the_host_it_could_not_resolve() {
    let err = sipster_core::net::resolve("no-such-host.invalid", 5060).unwrap_err();
    assert!(format!("{err}").contains("no-such-host.invalid"));
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
