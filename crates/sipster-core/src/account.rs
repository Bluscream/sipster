use serde::{Deserialize, Serialize};

/// Configuration for a SIP Account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SipAccount {
    /// Friendly label (e.g. "Home Office", "SIP Provider")
    pub label: String,
    /// Registrar host or IP
    pub registrar: String,
    /// Registrar port (default 5060)
    pub port: u16,
    /// SIP username / Extension
    pub username: String,
    /// Authentication user (often identical to username)
    pub auth_user: String,
    /// Account password
    pub password: String,
}

impl Default for SipAccount {
    fn default() -> Self {
        Self {
            label: "Default Account".into(),
            registrar: "".into(),
            port: 5060,
            username: "".into(),
            auth_user: "".into(),
            password: "".into(),
        }
    }
}
