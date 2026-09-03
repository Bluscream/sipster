use thiserror::Error;

#[derive(Error, Debug)]
pub enum SipsterError {
    #[error("Network I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SIP protocol error: {0}")]
    Sip(String),

    #[error("SDP negotiation error: {0}")]
    Sdp(String),

    #[error("Audio device error: {0}")]
    Audio(String),

    #[error("Authentication failed for user {0}")]
    AuthFailed(String),

    #[error("Call not found: {0}")]
    CallNotFound(String),
}

pub type Result<T> = std::result::Result<T, SipsterError>;
