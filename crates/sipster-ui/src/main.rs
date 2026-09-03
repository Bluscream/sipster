//! Sipster desktop UI.
//!
//! This crate is a **skin**: it renders state and forwards user intent to
//! [`sipster_core::SipEngine`]. It contains no SIP, SDP, RTP or audio logic —
//! anything reusable by another frontend belongs in `sipster-core`.

mod app;
mod engine_bridge;
mod view;

use app::SipsterApp;

pub fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sipster_core=debug".into()),
        )
        .init();

    iced::application(SipsterApp::new, SipsterApp::update, SipsterApp::view)
        .title("Sipster")
        .subscription(SipsterApp::subscription)
        .theme(SipsterApp::theme)
        .run()
}
