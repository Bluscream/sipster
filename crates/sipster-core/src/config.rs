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
    /// Local UDP port to bind SIP to. 5060 by convention; if it is already in
    /// use an ephemeral port is chosen instead.
    #[serde(default = "default_port")]
    pub local_port: u16,
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
            local_port: default_port(),
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

    /// Builds the registrar as a SIP URI, which is what the engine requires.
    ///
    /// Users type what the Fritz!Box shows them — `fritz.box` or `192.168.2.1`
    /// — so accept a bare host, `host:port`, or an already-complete
    /// `sip:`/`sips:` URI, and add the scheme and port when missing.
    pub fn registrar_uri(&self) -> String {
        let raw = self.registrar.trim();
        let (scheme, rest) = if let Some(rest) = raw.strip_prefix("sips:") {
            ("sips", rest)
        } else if let Some(rest) = raw.strip_prefix("sip:") {
            ("sip", rest)
        } else {
            ("sip", raw)
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
        }
    }
}

/// Capture and playback device selection. `None` means "system default".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub input: Option<String>,
    pub output: Option<String>,
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

    /// Loads the config file, falling back to the environment for the account.
    ///
    /// The file wins: once the settings window has written one, it is the
    /// source of truth. `SIPSTER_*`/`SIP_*` variables only supply the account
    /// when the file has none, which is what makes first run work with nothing
    /// but environment variables.
    pub fn load_or_env(path: impl AsRef<Path>) -> Result<Self> {
        let mut config = Self::load(path)?;
        if config.accounts.is_empty() {
            if let Ok(from_env) = Self::from_env() {
                config.accounts = from_env.accounts;
            }
        }
        Ok(config)
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
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }

        let text = toml::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("could not encode config: {e}")))?;

        let temp = path.with_extension("toml.tmp");
        std::fs::write(&temp, text)?;
        restrict_permissions(&temp)?;
        std::fs::rename(&temp, path)?;
        Ok(())
    }
}

/// Makes the config readable only by its owner. No-op off Unix.
#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
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

    /// Builds a single-account config from environment variables.
    ///
    /// Accepts either the `SIPSTER_` prefix or the shorter `SIP_` prefix
    /// (`SIP_REGISTRAR`, `SIP_USERNAME`, `SIP_AUTH_USER`, `SIP_PASSWORD`), so
    /// credentials can live in something like
    /// `~/.config/environment.d/95-sip.conf` instead of being typed each run.
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// The testable core of [`Config::from_env`], with the environment
    /// injected so tests do not mutate global process state.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |name: &str| {
            lookup(&format!("SIPSTER_{name}"))
                .or_else(|| lookup(&format!("SIP_{name}")))
                .filter(|value| !value.is_empty())
        };

        let registrar = get("REGISTRAR")
            .ok_or_else(|| Error::Config("SIP_REGISTRAR (or SIPSTER_REGISTRAR) not set".into()))?;
        let username = get("USERNAME")
            .ok_or_else(|| Error::Config("SIP_USERNAME (or SIPSTER_USERNAME) not set".into()))?;

        let account = SipAccount {
            label: get("LABEL").unwrap_or_else(|| "env".into()),
            registrar,
            port: get("PORT").and_then(|p| p.parse().ok()).unwrap_or_else(default_port),
            auth_user: get("AUTH_USER").unwrap_or_default(),
            username,
            password: get("PASSWORD").unwrap_or_default(),
            transport: Transport::Udp,
            expires: get("EXPIRES").and_then(|e| e.parse().ok()).unwrap_or_else(default_expires),
            local_port: get("LOCAL_PORT").and_then(|p| p.parse().ok()).unwrap_or_else(default_port),
        };
        Ok(Self { accounts: vec![account], ..Self::default() })
    }
}
