//! Probe: what does the router say this line's own numbers are?
//!
//! Run with: cargo run -p sipster-integrations --example `number_probe`

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

    println!("country code: {:?}", client.fetch_country_code());
    for entry in client.fetch_account_numbers().expect("read numbers") {
        println!(
            "{:<12} {:<10} internal {:<5} external {}",
            entry.username, entry.phone_name, entry.internal, entry.external
        );
    }
}
