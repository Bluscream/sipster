//! Fetch contacts and calls from the configured router over TLS, to check that
//! the pinned connector tolerates the router's unclean close.
//!
//! Run with: cargo run -p sipster-integrations --example `tls_probe`

fn main() {


    let cfg = sipster_core::config::Config::load(sipster_core::config::Config::path())
        .expect("load config");
    let fb = &cfg.integration.fritzbox;

    let client = sipster_integrations::fritzbox::FritzBoxClient::new(
        sipster_integrations::fritzbox::FritzConfig {
            host: fb.host.clone(),
            port: fb.port,
            username: fb.username.clone(),
            password: fb.password.clone(),
            tls: fb.tls,
            cert_fingerprint: fb.cert_fingerprint.clone(),
        },
    );

    match client.fetch_contacts() {
        Ok(c) => println!("contacts: {}", c.len()),
        Err(e) => println!("contacts FAILED: {e}"),
    }
    match client.fetch_calls() {
        Ok(c) => println!("calls: {}", c.len()),
        Err(e) => println!("calls FAILED: {e}"),
    }
    if let Some(fp) = sipster_integrations::fritzbox::take_learned_fingerprint() {
        println!("learned fingerprint: {fp}");
    }
}
