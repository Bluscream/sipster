use thiserror::Error;

/// Every fallible operation in `sipster-core` returns this error.
///
/// Variants carry enough context to be shown to a user without extra lookups —
/// the UI renders these directly in the status line.
#[derive(Debug, Error)]
pub enum Error {
    #[error("network I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SIP engine error: {0}")]
    Sip(String),

    #[error("registrar {registrar} rejected registration: {status} {reason}")]
    RegistrationRejected {
        registrar: String,
        status: u16,
        reason: String,
    },

    #[error("authentication failed for user {user}: check username, auth user and password")]
    AuthFailed { user: String },

    #[error("could not resolve SIP host {host}")]
    Resolve { host: String },

    #[error("call rejected: {status} {reason}")]
    CallRejected { status: u16, reason: String },

    #[error("no call with id {0}")]
    UnknownCall(crate::call::CallId),

    #[error("audio error: {0}")]
    Audio(String),

    #[error("configuration error: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, Error>;

