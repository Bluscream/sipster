//! Sipster core engine.
//!
//! Everything reusable by any UI skin lives here: SIP account configuration,
//! the calling engine (a thin, safe wrapper over rvoip's softphone `Endpoint`),
//! call/registration state, and — as they land — contacts and the call list.
//! `sipster-ui` is presentation only; it must not contain telephony logic.

pub mod audio;
pub mod call;
pub mod cli;
pub mod config;
pub mod engine;
pub mod error;
pub mod instance;
pub mod ipc;
pub mod net;

pub use call::{CallDirection, CallEvent, CallId, CallState, RegistrationState};
pub use config::{
    mask_identity, AudioSettings, BlockAction, BlockedNumber, CardDavAccountConfig, Config,
    default_log_filter, FritzBoxSettings, GoogleAccountConfig, IntegrationSettings, IpcSettings,
    LanguageChoice, LogSettings, SipAccount, ThemeChoice, Transport, UiSettings,
};
pub use engine::SipEngine;
pub use error::{Error, Result};
pub use instance::Guard;
pub use ipc::{Command, Instance};

/// Builds an rvoip `Endpoint` from a validated [`SipAccount`].
///
/// This is the single seam between our config and the rvoip stack; the full
/// engine (registration loop, event translation, call handles) builds on top.
pub(crate) async fn build_endpoint(account: &SipAccount) -> Result<rvoip_sip::Endpoint> {
    account.validate()?;

    // The SIP stack defaults to binding 127.0.0.1, which makes every send to a
    // LAN registrar fail with EINVAL. Bind to the interface that actually
    // routes to the registrar, and advertise that same address.
    let peer = net::resolve(&account.registrar, account.port)?;
    let bind = net::bind_address(peer, account.local_port)?;
    tracing::debug!(%peer, %bind, "binding SIP transport");

    // Boxed: constructing the rvoip endpoint produces a very large future.
    Box::pin(
        rvoip_sip::Endpoint::builder()
            .account(&account.username)
            .auth_username(account.effective_auth_user())
            .password(&account.password)
            .registrar(account.registrar_uri())
            .expires(account.expires)
            .bind_addr(bind)
            .transport(endpoint_transport(account.transport))
            .build(),
    )
    .await
    .map_err(|e| Error::Config(format!("endpoint build failed: {e}")))
}

/// Maps our transport setting onto rvoip's.
fn endpoint_transport(transport: Transport) -> rvoip_sip::EndpointTransport {
    match transport {
        Transport::Udp => rvoip_sip::EndpointTransport::Udp,
        Transport::Tcp => rvoip_sip::EndpointTransport::Tcp,
        Transport::Tls => rvoip_sip::EndpointTransport::Tls,
    }
}
