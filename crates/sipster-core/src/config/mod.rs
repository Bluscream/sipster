pub mod account;
pub mod appearance;
pub mod integration;
pub mod secret;

pub use account::{SipAccount, Transport};
pub use appearance::{mask_identity, LanguageChoice, ThemeChoice, UiSettings};
pub use integration::{
    BlockAction, BlockedNumber, CardDavAccountConfig, FritzBoxSettings, GoogleAccountConfig,
    IntegrationSettings,
};

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Capture and playback device selection. `None` means "system default".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub input: Option<String>,
    pub output: Option<String>,
}

fn default_google_edit_cmd() -> String {
    "xdg-open https://contacts.google.com/person/{short_id}".to_string()
}

fn default_fritzbox_edit_cmd() -> String {
    "xdg-open http://{registrar}/?lp=pBook&BookId={phonebook_id}&edit={short_id}".to_string()
}

fn default_carddav_edit_cmd() -> String {
    "xdg-open https://{account}".to_string()
}

fn default_local_edit_cmd() -> String {
    "xdg-open {path}".to_string()
}

fn default_eds_edit_cmd() -> String {
    "gnome-contacts".to_string()
}

fn default_fallback_edit_cmd() -> String {
    "xdg-open {target}".to_string()
}

/// A `serde` default for settings that should stay on for configs written
/// before the field existed.
fn enabled() -> bool {
    true
}

/// Custom command hooks for app lifecycle, call events, and contact editing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandsSettings {
    pub on_app_start: Option<String>,
    pub on_app_exit: Option<String>,

    pub on_contacts_synced: Option<String>,
    pub on_history_synced: Option<String>,

    pub on_sip_registered: Option<String>,
    pub on_sip_unregistered: Option<String>,
    pub on_sip_registration_failed: Option<String>,

    pub on_call_incoming: Option<String>,
    pub on_call_outgoing: Option<String>,
    pub on_call_connected: Option<String>,
    pub on_call_held: Option<String>,
    pub on_call_unheld: Option<String>,
    pub on_call_transferred: Option<String>,
    pub on_call_ended: Option<String>,

    #[serde(default = "default_google_edit_cmd")]
    pub edit_google: String,

    #[serde(default = "default_fritzbox_edit_cmd")]
    pub edit_fritzbox: String,

    #[serde(default = "default_carddav_edit_cmd")]
    pub edit_carddav: String,

    #[serde(default = "default_local_edit_cmd")]
    pub edit_local: String,

    #[serde(default = "default_eds_edit_cmd")]
    pub edit_eds: String,

    #[serde(default = "default_fallback_edit_cmd")]
    pub edit_default: String,
}

impl Default for CommandsSettings {
    fn default() -> Self {
        Self {
            on_app_start: None,
            on_app_exit: None,
            on_contacts_synced: None,
            on_history_synced: None,
            on_sip_registered: None,
            on_sip_unregistered: None,
            on_sip_registration_failed: None,
            on_call_incoming: None,
            on_call_outgoing: None,
            on_call_connected: None,
            on_call_held: None,
            on_call_unheld: None,
            on_call_transferred: None,
            on_call_ended: None,
            edit_google: default_google_edit_cmd(),
            edit_fritzbox: default_fritzbox_edit_cmd(),
            edit_carddav: default_carddav_edit_cmd(),
            edit_local: default_local_edit_cmd(),
            edit_eds: default_eds_edit_cmd(),
            edit_default: default_fallback_edit_cmd(),
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

/// Where and how much Sipster logs.
///
/// In the config rather than on the command line or in the environment: the
/// config file is the only source of configuration, so a log setting survives
/// however Sipster was started — from a desktop icon, a URI handler, or a
/// terminal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSettings {
    /// Append logs to this file as well as the console. `None` for console
    /// only.
    ///
    /// Both, never either: writing only to a file means anyone running Sipster
    /// from a terminal sees nothing, including the messages explaining why
    /// something did not work.
    #[serde(default)]
    pub file: Option<std::path::PathBuf>,
    /// A `tracing` filter, in the same syntax `RUST_LOG` used to take, for
    /// example `info,sipster_core=trace`.
    #[serde(default = "default_log_filter")]
    pub filter: String,
}

/// The default log filter.
///
/// Two upstream crates are muted to `warn`, and neither is a matter of taste:
///
/// - `iced_winit` logs the window attributes at INFO, and those attributes
///   embed the 256x256 window icon, which the pretty-printer renders as one
///   line per byte — a 5 MB, 262,000-line log on every single startup.
/// - `rvoip_media_core` logs four INFO lines per RTP packet, so a call emitted
///   ~255 lines per second, roughly 2 MB for thirty seconds of talking.
///
/// Both drowned the SIP signalling that a bug report actually needs. `warn`
/// still surfaces genuine problems from either.
pub fn default_log_filter() -> String {
    "info,sipster_core=debug,\
     iced_winit=warn,iced_wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,\
     rvoip_media_core=warn"
        .to_string()
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            file: None,
            filter: default_log_filter(),
        }
    }
}

/// Top-level config. One account per file; the UI edits this.
///
/// One account rather than a list: a softphone registers one line, and every
/// piece of state around a call — which engine, which registration, which
/// number — was previously a parallel vector indexed by the same integer.
/// Running a second line means running a second copy with `--config`, which
/// also gives it its own window, tray icon and call state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub account: SipAccount,
    #[serde(default)]
    pub ui: UiSettings,
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub ipc: IpcSettings,
    #[serde(default)]
    pub log: LogSettings,
    #[serde(default)]
    pub integration: IntegrationSettings,
    #[serde(default)]
    pub commands: CommandsSettings,
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
        self.account.validate().is_err()
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

/// Writes `bytes` to `path`, readable only by this user from the moment it
/// exists.
///
/// The permission bits are Unix-only; elsewhere this is an ordinary write.
/// The `#[cfg(unix)]` that used to sit on the function itself compiled the
/// whole thing out on Windows, where `save` still called it — caught only by
/// the Windows cross-build, since `check` builds for this host alone.
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
            Ok(text) => {
                toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
            }
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
        assert!(config.needs_setup(), "a default account has no registrar");

        config.account.registrar = "fritz.box".into();
        assert!(config.needs_setup(), "still no username");

        config.account.username = "bob".into();
        assert!(!config.needs_setup(), "registrar and username are enough");
    }

    #[test]
    fn missing_file_loads_as_an_empty_config() {
        let config = Config::load("/nonexistent/sipster.toml").expect("not an error");
        assert!(config.needs_setup());
    }

    /// Saving must round-trip every table, or a settings change would quietly
    /// drop the account or the preferences.
    #[test]
    fn saved_config_round_trips() {
        let dir = std::env::temp_dir().join(format!("sipster-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nested/sipster.toml");

        let mut config = Config {
            account: super::SipAccount {
                registrar: "fritz.box".into(),
                username: "bob".into(),
                password: "pw".into(),
                ..super::SipAccount::default()
            },
            ..Config::default()
        };
        config.ui.theme = super::ThemeChoice::Nord;
        config.ui.ringtone = false;
        config.audio.output = Some("hw:1".into());

        config.save(&path).expect("save creates parent directories");
        let back = Config::load(&path).expect("reload");

        assert_eq!(back.account.password, "pw");
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

