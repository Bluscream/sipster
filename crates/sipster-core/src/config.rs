use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Transport used to reach the registrar. Only UDP is implemented today; the
/// enum exists so the on-disk config format is stable when TCP/TLS land.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Udp,
}

/// A single SIP account: everything needed to register and place calls.
///
/// Field names deliberately mirror the Fritz!Box "IP phone" setup dialog so a
/// user can copy values across without translation.
#[derive(Clone, Serialize, Deserialize)]
pub struct SipAccount {
    /// Friendly label shown in the UI (e.g. "Fritz!Box Office").
    #[serde(default = "default_label")]
    pub label: String,
    /// Registrar host — `fritz.box`, a LAN IP, or a provider domain.
    pub registrar: String,
    /// Registrar port; 5060 for plain UDP.
    #[serde(default = "default_port")]
    pub port: u16,
    /// SIP username / internal number registered on the PBX.
    pub username: String,
    /// Authentication user. Optional — defaults to `username`, which is the
    /// common case on a Fritz!Box.
    #[serde(default)]
    pub auth_user: String,
    /// Account password. Never logged; see `Debug` impl below.
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub transport: Transport,
    /// Re-registration interval requested from the registrar, in seconds.
    #[serde(default = "default_expires")]
    pub expires: u32,
}

fn default_label() -> String {
    "Default Account".into()
}

fn default_port() -> u16 {
    5060
}

fn default_expires() -> u32 {
    600
}

impl Default for SipAccount {
    fn default() -> Self {
        Self {
            label: default_label(),
            registrar: String::new(),
            port: default_port(),
            username: String::new(),
            auth_user: String::new(),
            password: String::new(),
            transport: Transport::default(),
            expires: default_expires(),
        }
    }
}

/// Redacts the password so it never leaks through `{:?}`, tracing spans, or
/// panic messages — the exact leak flagged in the previous implementation.
impl std::fmt::Debug for SipAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SipAccount")
            .field("label", &self.label)
            .field("registrar", &self.registrar)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("auth_user", &self.auth_user)
            .field("password", &"<redacted>")
            .field("transport", &self.transport)
            .field("expires", &self.expires)
            .finish()
    }
}

impl SipAccount {
    /// Validates that the account has the minimum needed to attempt a register.
    pub fn validate(&self) -> Result<()> {
        if self.registrar.trim().is_empty() {
            return Err(Error::Config("registrar host is empty".into()));
        }
        if self.username.trim().is_empty() {
            return Err(Error::Config("username is empty".into()));
        }
        Ok(())
    }

    /// The effective auth user, falling back to `username` when unset.
    pub fn effective_auth_user(&self) -> &str {
        if self.auth_user.is_empty() {
            &self.username
        } else {
            &self.auth_user
        }
    }
}

/// Top-level config. A file holds zero or more accounts; the UI edits this.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub accounts: Vec<SipAccount>,
}

impl Config {
    /// Loads config from a TOML file. Missing file yields an empty config so
    /// first run is not an error.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| Error::Config(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Builds a single-account config from `SIPSTER_*` environment variables.
    ///
    /// This is the credential path used for testing against a real PBX without
    /// writing secrets into the repo or the chat transcript.
    pub fn from_env() -> Result<Self> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let registrar = get("SIPSTER_REGISTRAR")
            .ok_or_else(|| Error::Config("SIPSTER_REGISTRAR not set".into()))?;
        let username = get("SIPSTER_USERNAME")
            .ok_or_else(|| Error::Config("SIPSTER_USERNAME not set".into()))?;
        let account = SipAccount {
            label: get("SIPSTER_LABEL").unwrap_or_else(|| "env".into()),
            registrar,
            port: get("SIPSTER_PORT").and_then(|p| p.parse().ok()).unwrap_or(5060),
            auth_user: get("SIPSTER_AUTH_USER").unwrap_or_default(),
            username,
            password: get("SIPSTER_PASSWORD").unwrap_or_default(),
            transport: Transport::Udp,
            expires: get("SIPSTER_EXPIRES").and_then(|e| e.parse().ok()).unwrap_or(600),
        };
        Ok(Self { accounts: vec![account] })
    }
}
