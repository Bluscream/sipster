pub mod secret;

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

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

/// Which colour theme the UI should use.
///
/// A closed set rather than a free string so an unreadable value cannot end up
/// in the file; the UI maps each to an `iced::Theme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    Dracula,
    Nord,
    SolarizedDark,
    GruvboxDark,
    CatppuccinMocha,
    TokyoNight,
}

impl ThemeChoice {
    /// Every choice, for populating a picker.
    pub const ALL: [Self; 8] = [
        Self::Dark,
        Self::Light,
        Self::Dracula,
        Self::Nord,
        Self::SolarizedDark,
        Self::GruvboxDark,
        Self::CatppuccinMocha,
        Self::TokyoNight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::Dracula => "Dracula",
            Self::Nord => "Nord",
            Self::SolarizedDark => "Solarized Dark",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::TokyoNight => "Tokyo Night",
        }
    }
}

impl std::fmt::Display for ThemeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Presentation and local-feedback preferences. None of this reaches the wire.
///
/// The bool count trips `struct_excessive_bools`, whose usual remedy — folding
/// them into an enum or a state machine — does not apply: these are genuinely
/// independent on/off preferences, and each one is a checkbox in the settings
/// window and a self-describing key in the TOML file. Grouping them would make
/// both worse.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub theme: ThemeChoice,
    /// Ring the speaker while an inbound call is pending.
    pub ringtone: bool,
    /// Raise a desktop notification for an inbound call.
    pub notifications: bool,
    /// Local DTMF beep when a dialpad key is pressed. Not sent to the peer.
    pub dtmf_feedback: bool,
    /// Short chimes when a call starts and ends.
    pub call_chimes: bool,
    /// Show the wordmark above the dialpad.
    pub show_banner: bool,
    /// Register and set as default handler for tel:, sip:, sips:, callto:, and sipster: URI schemes.
    pub register_uri_schemes: bool,
    /// Keep running in the background when the dialer window is closed if a system tray icon is active.
    pub close_to_tray: bool,
    /// Mask names and numbers everywhere they are displayed, leaving only the
    /// first and last character. For screen sharing and recording.
    pub streaming_mode: bool,
    /// Timestamp of the newest missed call the user has already looked at.
    ///
    /// The badge on the History window's Missed filter counts only what is
    /// newer than this, so it reads as an unread marker rather than a running
    /// total. Persisted, because a badge that came back on every restart
    /// would be exactly the nag it is meant not to be.
    #[serde(default)]
    pub missed_seen_until: Option<String>,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            ringtone: true,
            notifications: true,
            dtmf_feedback: true,
            call_chimes: true,
            show_banner: true,
            register_uri_schemes: false,
            close_to_tray: true,
            streaming_mode: false,
            missed_seen_until: None,
        }
    }
}

/// Masks a name or number for [`UiSettings::streaming_mode`].
///
/// Keeps the first and last character so entries stay tellable apart and the
/// layout keeps its shape, and hides everything between:
/// `Alice Smith` becomes `A…h`, `+49301234567` becomes `+…7`.
///
/// One- and two-character values are replaced outright rather than returned
/// as-is, since `A…A` would leak the whole thing.
#[must_use]
pub fn mask_identity(value: &str) -> String {
    let trimmed = value.trim();
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    match chars.next_back() {
        // 3 or more characters: keep both ends.
        Some(last) if trimmed.chars().count() > 2 => format!("{first}…{last}"),
        // 1-2 characters: nothing can be safely revealed.
        _ => "…".to_string(),
    }
}

/// Capture and playback device selection. `None` means "system default".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub input: Option<String>,
    pub output: Option<String>,
}

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
    /// Off by default, and that is a measured retreat rather than an
    /// oversight. The SOAP calls work over TLS, but the phonebook downloads
    /// that follow do not: the router frames them with `Connection: close` and
    /// no `Content-Length`, then closes the socket without a TLS
    /// `close_notify`, which rustls reports as `unexpected_eof`. Every
    /// download fails and the sync reports success with zero contacts.
    /// Encrypting only the SOAP metadata while the contacts themselves still
    /// travel in clear would miss the point, so the default stays on plain
    /// HTTP until that is solved.
    ///
    /// Turning it on is safe to try — the certificate is pinned, see
    /// [`cert_fingerprint`](Self::cert_fingerprint) — and a router or HTTP
    /// stack that closes cleanly will work.
    #[serde(default)]
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

impl Default for FritzBoxSettings {
    fn default() -> Self {
        Self {
            // `fritz.box` is the name AVM routers answer to on any LAN. The
            // previous default was one developer's own address, which every
            // other user would have had their router credentials sent to.
            host: "fritz.box".into(),
            port: 49000,
            tls: false,
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

/// A `serde` default for settings that should stay on for configs written
/// before the field existed.
fn enabled() -> bool {
    true
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

/// Control-channel settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IpcSettings {
    /// Control socket (Unix) or named pipe (Windows). `None` uses the
    /// per-user default under `XDG_RUNTIME_DIR`.
    pub socket: Option<std::path::PathBuf>,
}

/// Top-level config. A file holds zero or more accounts; the UI edits this.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub accounts: Vec<SipAccount>,
    #[serde(default)]
    pub ui: UiSettings,
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub ipc: IpcSettings,
    #[serde(default)]
    pub integration: IntegrationSettings,
}

impl Config {
    /// `$XDG_CONFIG_HOME/sipster/sipster.toml`, falling back to `$HOME/.config`
    /// and finally to the working directory.
    pub fn default_path() -> std::path::PathBuf {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            return Path::new(&xdg).join("sipster/sipster.toml");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return Path::new(&home).join(".config/sipster/sipster.toml");
        }
        std::path::PathBuf::from("sipster.toml")
    }

    /// The config file this process should use: `--config-file <PATH>`, else
    /// [`default_path`](Self::default_path).
    pub fn path() -> std::path::PathBuf {
        let args: Vec<String> = std::env::args().skip(1).collect();
        Self::path_from(&args)
    }

    /// The testable core of [`path`](Self::path), with argv injected.
    pub fn path_from<S: AsRef<str>>(args: &[S]) -> std::path::PathBuf {
        const FLAGS: [&str; 2] = ["--config-file", "--config"];

        crate::cli::flag_value(args, &FLAGS)
            .map_or_else(Self::default_path, std::path::PathBuf::from)
    }

    /// Whether this looks like a first run: no usable account configured.
    ///
    /// The UI opens the settings window when this is true — without an account
    /// the app cannot do anything at all, and there is now no environment
    /// variable that could be supplying one behind the scenes.
    pub fn needs_setup(&self) -> bool {
        self.accounts
            .first()
            .is_none_or(|account| account.validate().is_err())
    }

    /// Writes the config as TOML, creating parent directories as needed.
    ///
    /// The write is atomic (temp file plus rename) so an interrupted save
    /// cannot truncate a working config, and the file is `0600` because it
    /// holds an account password.
    ///
    /// # Errors
    ///
    /// Fails if the directory cannot be created or the file cannot be written.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        secret::use_key_beside(path);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }

        let text = toml::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("could not encode config: {e}")))?;

        // Created 0600 rather than written and then chmod'ed: the old order
        // left a window where the file existed with whatever the umask
        // allowed. The contents are encrypted now, so that window leaked only
        // ciphertext, but there is no reason to leave it open.
        let temp = path.with_extension("toml.tmp");
        write_private(&temp, text.as_bytes())?;
        std::fs::rename(&temp, path)?;
        Ok(())
    }
}

/// Makes the config readable only by its owner. No-op off Unix.
#[cfg(unix)]
/// Writes `bytes` to `path`, readable only by this user from the moment it
/// exists.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
        Ok(())
    }
}


impl Config {
    /// Loads config from a TOML file. Missing file yields an empty config so
    /// first run is not an error.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        secret::use_key_beside(path);
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| Error::Config(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    /// The label is derived so it can never disagree with the account it
    /// names. `auth_user` wins because that is what the registrar checks.
    #[test]
    fn the_label_is_built_from_the_account() {
        let mut account = super::SipAccount {
            registrar: "fritz.box".into(),
            port: 5060,
            username: "bluscream".into(),
            ..super::SipAccount::default()
        };
        assert_eq!(account.label(), "udp://bluscream@fritz.box:5060");

        account.auth_user = "bluscream2".into();
        assert_eq!(account.label(), "udp://bluscream2@fritz.box:5060");

        account.transport = super::Transport::Tls;
        account.port = 5061;
        assert_eq!(account.label(), "tls://bluscream2@fritz.box:5061");
    }

    /// TLS is addressed as `sips:` and defaults to 5061; the other two share
    /// `sip:` and 5060. Getting this wrong means the registrar is contacted
    /// on the wrong port with the wrong scheme.
    #[test]
    fn each_transport_has_its_own_scheme_and_default_port() {
        use super::Transport;
        assert_eq!(Transport::Udp.scheme(), "sip");
        assert_eq!(Transport::Tcp.scheme(), "sip");
        assert_eq!(Transport::Tls.scheme(), "sips");
        assert_eq!(Transport::Udp.default_port(), 5060);
        assert_eq!(Transport::Tcp.default_port(), 5060);
        assert_eq!(Transport::Tls.default_port(), 5061);
    }

    #[test]
    fn a_tls_account_builds_a_sips_registrar_uri() {
        let account = super::SipAccount {
            registrar: "fritz.box".into(),
            port: 5061,
            transport: super::Transport::Tls,
            ..super::SipAccount::default()
        };
        assert_eq!(account.registrar_uri(), "sips:fritz.box:5061");
    }

    /// A scheme typed into the host field is a deliberate choice and must not
    /// be overwritten by the transport setting.
    #[test]
    fn an_explicit_scheme_survives_the_transport_default() {
        let account = super::SipAccount {
            registrar: "sip:fritz.box".into(),
            port: 5060,
            transport: super::Transport::Tls,
            ..super::SipAccount::default()
        };
        assert_eq!(account.registrar_uri(), "sip:fritz.box:5060");
    }

    /// The on-disk format has to keep round-tripping; a config written before
    /// TCP and TLS existed still says `udp`.
    #[test]
    fn transports_round_trip_through_the_config_format() {
        for transport in super::Transport::ALL {
            let account = super::SipAccount { transport, ..super::SipAccount::default() };
            let text = toml::to_string(&account).expect("serialize");
            let back: super::SipAccount = toml::from_str(&text).expect("deserialize");
            assert_eq!(back.transport, transport);
        }
        assert!(toml::to_string(&super::SipAccount::default())
            .expect("serialize")
            .contains("transport = \"udp\""));
    }

    use super::Config;

    #[test]
    fn the_config_flag_selects_the_file() {
        assert_eq!(
            Config::path_from(&["--config-file", "/from/flag.toml"]),
            std::path::PathBuf::from("/from/flag.toml")
        );
        assert_eq!(
            Config::path_from(&["--config=/short.toml"]),
            std::path::PathBuf::from("/short.toml")
        );
    }

    #[test]
    fn defaults_when_no_flag_is_given() {
        assert_eq!(Config::path_from::<&str>(&[]), Config::default_path());
        // A blank value must fall through rather than yield an empty path.
        assert_eq!(Config::path_from(&["--config-file", "  "]), Config::default_path());
    }

    /// First run drives the settings window opening on its own, so "is
    /// anything usable configured" has to be answered precisely: an account
    /// that cannot register is as good as no account.
    #[test]
    fn needs_setup_until_a_usable_account_exists() {
        let mut config = Config::default();
        assert!(config.needs_setup(), "no accounts at all");

        config.accounts.push(super::SipAccount::default());
        assert!(config.needs_setup(), "default account has no registrar");

        config.accounts[0].registrar = "fritz.box".into();
        assert!(config.needs_setup(), "still no username");

        config.accounts[0].username = "bob".into();
        assert!(!config.needs_setup(), "registrar and username are enough");
    }

    #[test]
    fn missing_file_loads_as_an_empty_config() {
        let config = Config::load("/nonexistent/sipster.toml").expect("not an error");
        assert!(config.accounts.is_empty());
        assert!(config.needs_setup());
    }

    /// Saving must round-trip every table, or a settings change would quietly
    /// drop the account or the preferences.
    #[test]
    fn saved_config_round_trips() {
        let dir = std::env::temp_dir().join(format!("sipster-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nested/sipster.toml");

        let mut config = Config::default();
        config.accounts.push(super::SipAccount {
            registrar: "fritz.box".into(),
            username: "bob".into(),
            password: "pw".into(),
            ..super::SipAccount::default()
        });
        config.ui.theme = super::ThemeChoice::Nord;
        config.ui.ringtone = false;
        config.audio.output = Some("hw:1".into());

        config.save(&path).expect("save creates parent directories");
        let back = Config::load(&path).expect("reload");

        assert_eq!(back.accounts.len(), 1);
        assert_eq!(back.accounts[0].password, "pw");
        assert_eq!(back.ui.theme, super::ThemeChoice::Nord);
        assert!(!back.ui.ringtone);
        assert_eq!(back.audio.output.as_deref(), Some("hw:1"));
        assert!(!back.needs_setup());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The file is now the only place a password can live, so it must not be
    /// readable by other users on the machine.
    #[cfg(unix)]
    #[test]
    fn saved_config_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("sipster-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sipster.toml");

        Config::default().save(&path).expect("save");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config holds a password; mode was {mode:o}");

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod mask_tests {
    use super::mask_identity;

    #[test]
    fn keeps_only_the_outer_characters() {
        assert_eq!(mask_identity("Alice Smith"), "A…h");
        assert_eq!(mask_identity("+49301234567"), "+…7");
        assert_eq!(mask_identity("**610"), "*…0");
    }

    /// Short values cannot keep both ends without revealing everything.
    #[test]
    fn very_short_values_reveal_nothing() {
        assert_eq!(mask_identity("ab"), "…");
        assert_eq!(mask_identity("a"), "…");
        assert_eq!(mask_identity(""), "");
    }

    /// Slicing by byte would panic or split a character in half.
    #[test]
    fn handles_multi_byte_characters() {
        assert_eq!(mask_identity("Müller"), "M…r");
        assert_eq!(mask_identity("日本語です"), "日…す");
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(mask_identity("  Alice  "), "A…e");
    }
}
