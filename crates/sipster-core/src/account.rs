use serde::{Deserialize, Serialize};

/// Configuration for a SIP Account (e.g. FRITZ!Box IP phone)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SipAccount {
    /// Friendly label (e.g. "Fritz!Box Home")
    pub label: String,
    /// Registrar host or IP (e.g. "fritz.box" or "192.168.178.1")
    pub registrar: String,
    /// Registrar port (default 5060)
    pub port: u16,
    /// SIP username / Extension (e.g. "620")
    pub username: String,
    /// Authentication user (often identical to username)
    pub auth_user: String,
    /// Account password
    pub password: String,
}

impl Default for SipAccount {
    fn default() -> Self {
        Self {
            label: "FRITZ!Box Phone".into(),
            registrar: "fritz.box".into(),
            port: 5060,
            username: "620".into(),
            auth_user: "620".into(),
            password: "".into(),
        }
    }
}
