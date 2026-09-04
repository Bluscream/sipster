//! Single-instance enforcement and command IPC.
//!
//! A softphone must be single-instance: two processes cannot both hold SIP
//! port 5060 or register the same account sensibly. The first process takes a
//! kernel file lock and opens a control channel; later invocations find the
//! lock held, forward their command to the running instance, and exit.
//!
//! The same channel is the remote-control interface, so `sipster --call 611`,
//! a `tel:` link from a browser, and a script all take one path.
//!
//! The channel is a Unix domain socket or a Windows named pipe depending on
//! the platform; see [`transport`].

mod transport;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::cli;
use crate::error::Result;
use crate::instance::{self, Guard};

pub use transport::{serve, Listener};

/// A command sent to the running instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Command {
    /// Place a call to a number, extension or SIP URI.
    Call { target: String },
    /// Fill in the dial box and focus the dialer window without calling immediately.
    Dial { target: String },
    /// Answer the currently ringing inbound call.
    Answer,
    /// Hang up the active call, or decline a ringing one.
    Hangup,
    /// Raise and focus the window.
    Show,
    /// Open the Settings window.
    OpenSettings,
    /// Open the Contacts window.
    OpenContacts,
    /// Open or navigate to Call List / History.
    OpenCallList,
    /// Ask the running instance to quit.
    Quit,
}

impl Command {
    /// Parses a `tel:`, `sip:`, `sips:` or `callto:` URI into a command.
    ///
    /// Handles the shapes a desktop actually delivers: `tel:+49301234`,
    /// `tel://611`, percent-encoding, RFC 3966 visual separators
    /// (`tel:+49-30-12 34`), and a `?call` style query suffix.
    pub fn from_uri(uri: &str) -> Option<Self> {
        let uri = uri.trim();
        let (scheme, rest) = uri.split_once(':')?;
        let scheme_lower = scheme.to_ascii_lowercase();
        if !matches!(
            scheme_lower.as_str(),
            "tel" | "sip" | "sips" | "callto" | "sipster"
        ) {
            return None;
        }

        if scheme_lower == "sipster" {
            let path = rest.trim_start_matches('/');
            let (action, query) = path.split_once(['?', '#']).unwrap_or((path, ""));
            let action_lower = action.to_ascii_lowercase();

            return match action_lower.as_str() {
                "open/settings" | "settings" => Some(Self::OpenSettings),
                "open/contacts" | "contacts" => Some(Self::OpenContacts),
                "open/calllist" | "open/history" | "calllist" | "history" => Some(Self::OpenCallList),
                "hangup" | "end" => Some(Self::Hangup),
                "answer" => Some(Self::Answer),
                "show" | "focus" | "" => Some(Self::Show),
                "quit" => Some(Self::Quit),
                _ if action_lower.starts_with("call/") => {
                    let target = &action[5..];
                    let decoded_target = percent_decode(target);
                    if decoded_target.trim().is_empty() {
                        Some(Self::Show)
                    } else {
                        Some(Self::Call { target: decoded_target.trim().to_string() })
                    }
                }
                _ if action_lower.starts_with("dial/") => {
                    let target = &action[5..];
                    let decoded_target = percent_decode(target);
                    if decoded_target.trim().is_empty() {
                        Some(Self::Show)
                    } else {
                        Some(Self::Dial { target: decoded_target.trim().to_string() })
                    }
                }
                _ => {
                    // Fallback: if action is a dialable number or SIP URI, fill the dial box
                    let candidate = if action.is_empty() { query } else { action };
                    let decoded = percent_decode(candidate);
                    let inner = decoded.trim();
                    if inner.is_empty() {
                        Some(Self::Show)
                    } else {
                        Some(Self::Dial { target: inner.to_string() })
                    }
                }
            };
        }

        // `tel://611` and `tel:611` are both seen in the wild.
        let rest = rest.trim_start_matches("//");
        // Drop any query/fragment: `tel:611?call` -> `611`.
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
        let decoded = percent_decode(rest);

        let target = if scheme_lower == "sip" || scheme_lower == "sips" {
            // Keep SIP URIs intact so they route as entered.
            format!("{scheme_lower}:{decoded}")
        } else {
            // tel:/callto: carry dialable digits; strip RFC 3966 separators.
            let digits: String = decoded
                .chars()
                .filter(|c| c.is_ascii_digit() || matches!(c, '+' | '*' | '#'))
                .collect();
            digits
        };

        // Standard telephony / SIP URIs (tel:, sip:, callto:) fill the dial box without calling immediately
        (!target.is_empty()).then_some(Self::Dial { target })
    }

    /// Parses command-line arguments into a [`Command`], if any was requested.
    ///
    /// Supports:
    /// - `--call <TARGET>` or `-c <TARGET>`
    /// - `--answer` or `-a`
    /// - `--hangup`
    /// - `--show`
    /// - `--quit` or `-q`
    /// - A bare `tel:`, `sip:`, `sips:` or `callto:` URI
    pub fn from_args<I, T>(args: I) -> Option<Self>
    where
        I: IntoIterator<Item = T>,
        T: AsRef<str>,
    {
        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            let arg_ref = arg.as_ref();
            match arg_ref {
                "--call" | "-c" => {
                    if let Some(target) = iter.next() {
                        let t = target.as_ref().trim();
                        if !t.is_empty() {
                            return Some(Self::Call {
                                target: t.to_string(),
                            });
                        }
                    }
                }
                "--answer" | "-a" => return Some(Self::Answer),
                "--hangup" => return Some(Self::Hangup),
                "--show" => return Some(Self::Show),
                "--quit" | "-q" => return Some(Self::Quit),
                _ if arg_ref.starts_with("--call=") => {
                    let target = arg_ref.trim_start_matches("--call=").trim();
                    if !target.is_empty() {
                        return Some(Self::Call {
                            target: target.to_string(),
                        });
                    }
                }
                _ => {
                    if let Some(cmd) = Self::from_uri(arg_ref) {
                        return Some(cmd);
                    }
                }
            }
        }
        None
    }
}

/// Minimal percent-decoding; URIs here are short and ASCII.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Address of the control channel for this user.
///
/// On Unix a socket under `XDG_RUNTIME_DIR`, which is per-user, `0700` and
/// cleared on logout, so a stale socket cannot outlive the session. On Windows
/// a named pipe. See [`transport`] for the details.
pub fn socket_path() -> PathBuf {
    let args: Vec<String> = std::env::args().skip(1).collect();
    socket_path_from(&args, configured_socket())
}

/// A socket path from the config file, published once at startup.
///
/// The control socket is needed by code that has no access to the config (the
/// forwarding path in a second invocation, and cleanup), so the resolved value
/// is parked here rather than threaded through every call site.
static CONFIGURED: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

/// Publishes the socket path from the config file. Call once, at startup.
pub fn set_configured_socket(path: Option<PathBuf>) {
    let _ = CONFIGURED.set(path);
}

fn configured_socket() -> Option<PathBuf> {
    CONFIGURED.get().cloned().flatten()
}

/// The testable core of [`socket_path`], with argv and the configured value
/// injected. Precedence is explicit flag, then config file, then the default
/// under `XDG_RUNTIME_DIR`.
pub fn socket_path_from<S: AsRef<str>>(args: &[S], configured: Option<PathBuf>) -> PathBuf {
    const SOCKET_FLAGS: [&str; 3] = ["--socket", "--target-socket", "-s"];

    if let Some(path) = cli::flag_value(args, &SOCKET_FLAGS) {
        return PathBuf::from(path);
    }
    if let Some(path) = configured {
        return path;
    }
    // XDG_RUNTIME_DIR is a platform directory convention, not Sipster
    // configuration — it says where per-user runtime state belongs.
    transport::default_path(std::env::var("XDG_RUNTIME_DIR").ok())
}

/// Outcome of trying to become the single running instance.
pub enum Instance {
    /// We are the first instance; owns the kernel file lock, the bound control
    /// channel and any command this invocation asked for.
    Primary {
        lock: Guard,
        listener: Listener,
        initial_command: Option<Command>,
    },
    /// Another instance is already running and has been sent the command.
    Forwarded,
}

/// Attempts to hand `command` to an instance that is already accepting.
///
/// # Errors
///
/// Only if the command could not be encoded or the write failed midway; not
/// finding anyone to talk to is `Ok(None)`.
pub async fn try_forward(command: Option<Command>) -> Result<Option<Instance>> {
    if transport::forward(&socket_path(), command.as_ref()).await? {
        info!("another instance is running; command forwarded");
        return Ok(Some(Instance::Forwarded));
    }
    Ok(None)
}

/// Opens the control channel at the configured address without taking the
/// single-instance lock.
///
/// Only for `--no-single-instance`; normal startup goes through [`acquire`].
///
/// # Errors
///
/// Fails when the address is already claimed or cannot be created.
pub fn bind_control_channel() -> Result<Listener> {
    transport::bind(&socket_path())
}

/// Becomes the primary instance, or forwards `command` to the one already
/// running.
///
/// Mutual exclusion is enforced by [`crate::instance::claim`]; command
/// forwarding rides the platform control channel.
pub async fn acquire(command: Option<Command>) -> Result<Instance> {
    let path = socket_path();

    // The kernel advisory lock is the source of truth for who is primary; the
    // socket is only how commands reach them.
    let Some(lock) = instance::claim()? else {
        return forward_to_primary(command).await;
    };

    Ok(Instance::Primary {
        lock,
        listener: transport::bind(&path)?,
        initial_command: command,
    })
}

/// Hands `command` to the instance that holds the lock.
///
/// There is a window where the primary holds the lock but has not bound its
/// socket yet, so a single failed connect is retried before giving up. Failing
/// to deliver is still not an error for us: the other copy is running, which is
/// the outcome single-instance mode exists to produce.
async fn forward_to_primary(command: Option<Command>) -> Result<Instance> {
    if let Some(instance) = try_forward(command.clone()).await? {
        return Ok(instance);
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    if let Some(instance) = try_forward(command).await? {
        return Ok(instance);
    }
    info!("another instance holds the lock but is not accepting commands yet; exiting");
    Ok(Instance::Forwarded)
}

/// Removes the control channel's filesystem entry. Call on clean shutdown.
pub fn cleanup() {
    transport::cleanup(&socket_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_tel_uri() {
        assert_eq!(
            Command::from_uri("tel:611"),
            Some(Command::Dial { target: "611".into() })
        );
    }

    #[test]
    fn parses_tel_uri_with_authority_and_query() {
        // Browsers and desktop handlers produce both of these shapes.
        assert_eq!(
            Command::from_uri("tel://611?call"),
            Some(Command::Dial { target: "611".into() })
        );
    }

    #[test]
    fn strips_rfc3966_visual_separators() {
        assert_eq!(
            Command::from_uri("tel:+49-30-12 34"),
            Some(Command::Dial { target: "+493012 34".replace(' ', "") })
        );
    }

    #[test]
    fn decodes_percent_encoding() {
        // %2B is '+', as delivered by some browsers.
        assert_eq!(
            Command::from_uri("tel:%2B4930123"),
            Some(Command::Dial { target: "+4930123".into() })
        );
    }

    #[test]
    fn parses_sipster_app_scheme() {
        assert_eq!(Command::from_uri("sipster://open/settings"), Some(Command::OpenSettings));
        assert_eq!(Command::from_uri("sipster://settings"), Some(Command::OpenSettings));
        assert_eq!(Command::from_uri("sipster://open/contacts"), Some(Command::OpenContacts));
        assert_eq!(Command::from_uri("sipster://open/calllist"), Some(Command::OpenCallList));
        assert_eq!(Command::from_uri("sipster://hangup"), Some(Command::Hangup));
        assert_eq!(Command::from_uri("sipster://answer"), Some(Command::Answer));
        assert_eq!(Command::from_uri("sipster://show"), Some(Command::Show));
        assert_eq!(Command::from_uri("sipster://call/611"), Some(Command::Call { target: "611".into() }));
        assert_eq!(Command::from_uri("sipster://dial/611"), Some(Command::Dial { target: "611".into() }));
        assert_eq!(Command::from_uri("sipster:611"), Some(Command::Dial { target: "611".into() }));
    }

    #[test]
    fn keeps_sip_uris_intact() {
        assert_eq!(
            Command::from_uri("sip:bob@example.com"),
            Some(Command::Dial { target: "sip:bob@example.com".into() })
        );
    }

    #[test]
    fn rejects_unrelated_schemes() {
        assert_eq!(Command::from_uri("https://example.com"), None);
        assert_eq!(Command::from_uri("611"), None);
    }

    #[test]
    fn rejects_empty_target() {
        assert_eq!(Command::from_uri("tel:"), None);
        assert_eq!(Command::from_uri("tel:---"), None);
    }

    #[test]
    fn commands_round_trip_as_json() {
        for command in [
            Command::Call { target: "611".into() },
            Command::Dial { target: "611".into() },
            Command::Answer,
            Command::Hangup,
            Command::Show,
            Command::OpenSettings,
            Command::OpenContacts,
            Command::OpenCallList,
            Command::Quit,
        ] {
            let json = serde_json::to_string(&command).unwrap();
            assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), command);
        }
    }

    #[test]
    fn the_socket_flag_wins() {
        assert_eq!(
            socket_path_from(&["--socket", "/from/flag.sock"], Some("/from/config.sock".into())),
            PathBuf::from("/from/flag.sock")
        );
    }

    #[test]
    fn the_configured_socket_is_used_when_there_is_no_flag() {
        assert_eq!(
            socket_path_from::<&str>(&[], Some("/from/config.sock".into())),
            PathBuf::from("/from/config.sock")
        );
    }

    /// Without a flag or a configured path the socket must still land
    /// somewhere absolute and per-user, not in the working directory.
    #[test]
    fn falls_back_to_a_runtime_directory() {
        let path = socket_path_from::<&str>(&[], None);
        assert!(path.is_absolute(), "socket path must be absolute: {}", path.display());
        assert_eq!(path.file_name().unwrap(), "sipster.sock");
    }

    #[test]
    fn parses_cli_arguments() {
        assert_eq!(
            Command::from_args(["--call", "611"]),
            Some(Command::Call { target: "611".into() })
        );
        assert_eq!(
            Command::from_args(["--call=**611"]),
            Some(Command::Call { target: "**611".into() })
        );
        assert_eq!(
            Command::from_args(["-c", "123"]),
            Some(Command::Call { target: "123".into() })
        );
        assert_eq!(Command::from_args(["--answer"]), Some(Command::Answer));
        assert_eq!(Command::from_args(["--hangup"]), Some(Command::Hangup));
        assert_eq!(Command::from_args(["--show"]), Some(Command::Show));
        assert_eq!(Command::from_args(["--quit"]), Some(Command::Quit));
        assert_eq!(
            Command::from_args(["tel:611"]),
            Some(Command::Dial { target: "611".into() })
        );
        assert_eq!(Command::from_args(["--unknown"]), None);
    }
}
