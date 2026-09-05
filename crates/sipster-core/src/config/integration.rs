//! Contact and call-history providers, and the settings that configure them.
//!
//! Split from `config` because it is a self-contained group — every provider's
//! credentials, the blocking rules that act on what they return, and the
//! `Debug` impls that keep those credentials out of a log.

use serde::{Deserialize, Serialize};

use super::{enabled, secret};

/// Action taken when an incoming call matches a blocked number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BlockAction {
    /// Send SIP 603 Decline immediately.
    #[default]
    Reject,
    /// Ring silently without playing audio or showing popup alerts.
    Mute,
}

impl BlockAction {
    pub const ALL: [Self; 2] = [Self::Reject, Self::Mute];

    pub fn label(self) -> &'static str {
        match self {
            Self::Reject => "Reject (Instant SIP 603)",
            Self::Mute => "Mute (Silent Ring)",
        }
    }
}

impl std::fmt::Display for BlockAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// A blocked number rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedNumber {
    pub number: String,
    pub name: Option<String>,
    pub action: BlockAction,
    pub added_at: String,
}

/// Google OAuth 2.0 Account configuration for Google Contacts sync.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleAccountConfig {
    pub id: String,
    #[serde(with = "secret")]
    pub email: String,
    #[serde(with = "secret")]
    pub refresh_token: String,
    #[serde(with = "secret::optional")]
    pub client_id: Option<String>,
    #[serde(with = "secret::optional")]
    pub client_secret: Option<String>,
    pub enabled: bool,
}

/// `CardDAV` account configuration for remote address book sync.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardDavAccountConfig {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(with = "secret")]
    pub username: String,
    #[serde(with = "secret")]
    pub password: String,
    pub enabled: bool,
}

/// FRITZ!Box TR-064 integration credentials.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FritzBoxSettings {
    pub host: String,
    pub port: u16,
    /// Fetch over TLS rather than plain HTTP.
    ///
    /// On by default. Everything TR-064 carries — contacts, call lists — would
    /// otherwise cross the LAN in the clear; digest auth keeps the password
    /// off the wire but not the data it protects.
    ///
    /// The router's certificate is self-signed, so it is pinned on first use
    /// rather than verified against a certificate authority. See
    /// [`cert_fingerprint`](Self::cert_fingerprint) and
    /// `sipster_integrations::pinned_tls`.
    ///
    /// Turn it off only for a device that cannot do TLS on 49443.
    #[serde(default = "yes")]
    pub tls: bool,
    /// SHA-256 of the router's certificate, remembered on the first TLS
    /// connection and required on every one after it.
    ///
    /// The certificate is self-signed, so nothing else can vouch for it. Clear
    /// this if the router legitimately gets a new one — a factory reset, say —
    /// and it will be learned again.
    #[serde(default)]
    pub cert_fingerprint: String,
    #[serde(with = "secret")]
    pub username: String,
    #[serde(with = "secret")]
    pub password: String,
    pub enabled: bool,
}

/// The serde default for [`FritzBoxSettings::tls`]. A config written before
/// TLS existed has no key, and should still be upgraded rather than left on
/// plain HTTP.
const fn yes() -> bool {
    true
}

impl Default for FritzBoxSettings {
    fn default() -> Self {
        Self {
            // `fritz.box` is the name AVM routers answer to on any LAN. The
            // previous default was one developer's own address, which every
            // other user would have had their router credentials sent to.
            host: "fritz.box".into(),
            port: 49000,
            tls: true,
            cert_fingerprint: String::new(),
            username: String::new(),
            password: String::new(),
            // Off until the user configures it. Defaulting to on meant a fresh
            // install immediately tried to authenticate against a guessed host
            // with blank credentials on every sync.
            enabled: false,
        }
    }
}

/// Redacts the refresh token and client secret. Both are bearer credentials:
/// anything holding them can read the user's Google contacts. `SipAccount`
/// already learned this lesson; these regressed it.
impl std::fmt::Debug for GoogleAccountConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleAccountConfig")
            .field("id", &self.id)
            .field("email", &self.email)
            .field("refresh_token", &redacted(&self.refresh_token))
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_deref().map(redacted))
            .field("enabled", &self.enabled)
            .finish()
    }
}

/// Redacts the account password.
impl std::fmt::Debug for CardDavAccountConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardDavAccountConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &redacted(&self.password))
            .field("enabled", &self.enabled)
            .finish()
    }
}

/// Redacts the router password, which is also the FRITZ!Box admin password.
impl std::fmt::Debug for FritzBoxSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FritzBoxSettings")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            // Not secret — a certificate fingerprint is meant to be compared
            // out loud — but worth seeing when a pin stops matching.
            .field("cert_fingerprint", &self.cert_fingerprint)
            .field("username", &self.username)
            .field("password", &redacted(&self.password))
            .field("enabled", &self.enabled)
            .finish()
    }
}

/// Hand-written so the secret-bearing members above keep their redaction; a
/// derived `Debug` here would print them through their own fields.
impl std::fmt::Debug for IntegrationSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntegrationSettings")
            .field("local_history_enabled", &self.local_history_enabled)
            .field("fritzbox", &self.fritzbox)
            .field("google_accounts", &self.google_accounts)
            .field("carddav_accounts", &self.carddav_accounts)
            .field("eds_enabled", &self.eds_enabled)
            .field("vdir_enabled", &self.vdir_enabled)
            .field("vdir_path", &self.vdir_path)
            .field("blocked_numbers", &self.blocked_numbers.len())
            .field("default_block_action", &self.default_block_action)
            .finish()
    }
}

/// `<redacted>` for anything set, `<empty>` for anything not — so a missing
/// credential is still diagnosable without revealing one that is present.
fn redacted(secret: &str) -> &'static str {
    if secret.is_empty() {
        "<empty>"
    } else {
        "<redacted>"
    }
}

/// Comprehensive settings for contact and call history providers, local storage, and call blocking.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IntegrationSettings {
    /// Save placed and received calls to local history (~/.local/share/sipster/history.json).
    pub local_history_enabled: bool,
    /// FRITZ!Box router integration.
    pub fritzbox: FritzBoxSettings,
    /// Google Contacts accounts.
    pub google_accounts: Vec<GoogleAccountConfig>,
    /// `CardDAV` / vCard servers.
    pub carddav_accounts: Vec<CardDavAccountConfig>,
    /// Read contacts from Evolution Data Server, the GNOME desktop's own
    /// address book — and so from any account the user added there. Linux
    /// only, and ignored where EDS is not on the session bus.
    #[serde(default = "enabled")]
    pub eds_enabled: bool,
    /// Read contacts from a local directory of `.vcf` files.
    ///
    /// The nearest thing Linux has to a shared contact store: the convention
    /// used by vdirsyncer, khard, Radicale and KDE's directory address books.
    /// `None` for the path means "look in the usual places".
    pub vdir_enabled: bool,
    pub vdir_path: Option<std::path::PathBuf>,
    /// Numbers blocked from calling in.
    pub blocked_numbers: Vec<BlockedNumber>,
    /// Default action applied when blocking a number.
    pub default_block_action: BlockAction,
}

impl Default for IntegrationSettings {
    fn default() -> Self {
        Self {
            local_history_enabled: true,
            fritzbox: FritzBoxSettings::default(),
            google_accounts: Vec::new(),
            carddav_accounts: Vec::new(),
            eds_enabled: true,
            vdir_enabled: true,
            vdir_path: None,
            blocked_numbers: Vec::new(),
            default_block_action: BlockAction::default(),
        }
    }
}

