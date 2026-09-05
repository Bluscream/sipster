//! The SIP account: who we register as, and how we reach the registrar.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

use super::{enabled, secret};

/// Transport used to reach the registrar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Udp,
    Tcp,
    /// TLS, which addresses the registrar as `sips:`.
    Tls,
}

impl Transport {
    pub const ALL: [Self; 3] = [Self::Udp, Self::Tcp, Self::Tls];

    /// The default SIP port for this transport. TLS is 5061; the other two
    /// share 5060.
    #[must_use]
    pub fn default_port(self) -> u16 {
        match self {
            Self::Tls => 5061,
            Self::Udp | Self::Tcp => 5060,
        }
    }

    /// The URI scheme a registrar is addressed with.
    #[must_use]
    pub fn scheme(self) -> &'static str {
        match self {
            Self::Tls => "sips",
            Self::Udp | Self::Tcp => "sip",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Udp => "UDP",
            Self::Tcp => "TCP",
            Self::Tls => "TLS",
        }
    }
}

impl std::fmt::Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// A single SIP account: everything needed to register and place calls.
///
/// Field names deliberately mirror the Fritz!Box "IP phone" setup dialog so a
/// user can copy values across without translation.
#[derive(Clone, Serialize, Deserialize)]
pub struct SipAccount {
    /// Registrar host — `fritz.box`, a LAN IP, or a provider domain.
    pub registrar: String,
    /// Registrar port; 5060 for plain UDP.
    #[serde(default = "default_port")]
    pub port: u16,
    /// SIP username / internal number registered on the PBX.
    #[serde(with = "secret")]
    pub username: String,
    /// Authentication user. Optional — defaults to `username`, which is the
    /// common case on a Fritz!Box.
    #[serde(default, with = "secret")]
    pub auth_user: String,
    /// Account password. Never logged; see `Debug` impl below, and stored
    /// encrypted — see [`secret`].
    #[serde(default, with = "secret")]
    pub password: String,
    #[serde(default)]
    pub transport: Transport,
    /// Whether to register this account at all. Lets an account be kept in
    /// the config without being used.
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// Re-registration interval requested from the registrar, in seconds.
    #[serde(default = "default_expires")]
    pub expires: u32,
    /// Local UDP port to bind SIP to. 5060 by convention; if it is already in
    /// use an ephemeral port is chosen instead.
    #[serde(default = "default_port")]
    pub local_port: u16,
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
            registrar: String::new(),
            port: default_port(),
            username: String::new(),
            auth_user: String::new(),
            password: String::new(),
            transport: Transport::default(),
            enabled: true,
            expires: default_expires(),
            local_port: default_port(),
        }
    }
}

/// Redacts the password so it never leaks through `{:?}`, tracing spans, or
/// panic messages — the exact leak flagged in the previous implementation.
impl std::fmt::Debug for SipAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SipAccount")
            .field("registrar", &self.registrar)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("auth_user", &self.auth_user)
            .field("password", &"<redacted>")
            .field("transport", &self.transport)
            .field("enabled", &self.enabled)
            .field("expires", &self.expires)
            .field("local_port", &self.local_port)
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

    /// How the account is named in the UI.
    ///
    /// Derived rather than stored: a hand-typed label drifts out of date the
    /// moment the account is edited, and every account already has a unique
    /// natural name in the values that define it.
    ///
    /// `auth_user` is preferred where it is set, because that is the name the
    /// registrar actually authenticates.
    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "{}://{}@{}:{}",
            self.transport.label().to_lowercase(),
            self.effective_auth_user(),
            self.registrar,
            self.port
        )
    }

    /// Builds the registrar as a SIP URI, which is what the engine requires.
    ///
    /// Users type what the Fritz!Box shows them — `fritz.box` or `192.168.2.1`
    /// — so accept a bare host, `host:port`, or an already-complete
    /// `sip:`/`sips:` URI, and add the scheme and port when missing.
    pub fn registrar_uri(&self) -> String {
        let raw = self.registrar.trim();
        // An explicit scheme in the host field wins — someone who typed
        // `sips:` meant it — otherwise the transport decides.
        let (scheme, rest) = if let Some(rest) = raw.strip_prefix("sips:") {
            ("sips", rest)
        } else if let Some(rest) = raw.strip_prefix("sip:") {
            ("sip", rest)
        } else {
            (self.transport.scheme(), raw)
        };
        let rest = rest.trim_start_matches("//");

        if has_port(rest) {
            format!("{scheme}:{rest}")
        } else {
            format!("{scheme}:{rest}:{}", self.port)
        }
    }
}

/// Whether `host` already carries an explicit `:port`, accounting for the
/// bracketed IPv6 form (`[::1]:5060`) where colons are part of the address.
fn has_port(host: &str) -> bool {
    host.rfind(']').map_or_else(
        || host.matches(':').count() == 1,
        |bracket| host[bracket..].contains(':'),
    )
}
