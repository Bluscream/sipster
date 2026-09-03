//! Sipster core engine.
//!
//! Everything reusable by any UI skin lives here: SIP account configuration,
//! the calling engine (a thin, safe wrapper over rvoip's softphone `Endpoint`),
//! call/registration state, and — as they land — contacts and the call list.
//! `sipster-ui` is presentation only; it must not contain telephony logic.

pub mod audio;
pub mod call;
pub mod config;
pub mod engine;
pub mod error;

pub use call::{CallDirection, CallEvent, CallId, CallState, RegistrationState};
pub use config::{Config, SipAccount, Transport};
pub use engine::SipEngine;
pub use error::{Error, Result};

/// Builds an rvoip `Endpoint` from a validated [`SipAccount`].
///
/// This is the single seam between our config and the rvoip stack; the full
/// engine (registration loop, event translation, call handles) builds on top.
pub(crate) async fn build_endpoint(account: &SipAccount) -> Result<rvoip_sip::Endpoint> {
    account.validate()?;
    // Boxed: constructing the rvoip endpoint produces a very large future.
    Box::pin(
        rvoip_sip::Endpoint::builder()
            .account(&account.username)
            .auth_username(account.effective_auth_user())
            .password(&account.password)
            .registrar(format!("{}:{}", account.registrar, account.port))
            .expires(account.expires)
            .build(),
    )
    .await
    .map_err(|e| Error::Config(format!("endpoint build failed: {e}")))
}
