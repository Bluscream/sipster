//! Single-instance enforcement and command IPC.
//!
//! A softphone must be single-instance: two processes cannot both hold SIP
//! port 5060 or register the same account sensibly. The first process binds a
//! Unix socket; later invocations find it already bound, forward their command
//! to the running instance, and exit.
//!
//! The same channel is the remote-control interface, so `sipster --call 611`,
//! a `tel:` link from a browser, and a script all take one path.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::error::{Error, Result};

/// A command sent to the running instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Command {
    /// Place a call to a number, extension or SIP URI.
    Call { target: String },
    /// Answer the currently ringing inbound call.
    Answer,
    /// Hang up the active call, or decline a ringing one.
    Hangup,
    /// Raise and focus the window.
    Show,
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
        if !matches!(
            scheme.to_ascii_lowercase().as_str(),
            "tel" | "sip" | "sips" | "callto"
        ) {
            return None;
        }

        // `tel://611` and `tel:611` are both seen in the wild.
        let rest = rest.trim_start_matches("//");
        // Drop any query/fragment: `tel:611?call` -> `611`.
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
        let decoded = percent_decode(rest);

        let target = if scheme.eq_ignore_ascii_case("sip") || scheme.eq_ignore_ascii_case("sips") {
            // Keep SIP URIs intact so they route as entered.
            format!("{}:{}", scheme.to_ascii_lowercase(), decoded)
        } else {
            // tel:/callto: carry dialable digits; strip RFC 3966 separators.
            let digits: String = decoded
                .chars()
                .filter(|c| c.is_ascii_digit() || matches!(c, '+' | '*' | '#'))
                .collect();
            digits
        };

        (!target.is_empty()).then_some(Self::Call { target })
    }

    /// Parses command-line arguments into a [`Command`], if any was requested.
    ///
    /// Supports:
    /// - `--call <TARGET>` or `-c <TARGET>`
    /// - `--answer` or `-a`
    /// - `--hangup` or `-h`
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

/// Path of the control socket for this user.
///
/// Lives in `XDG_RUNTIME_DIR`, which is per-user, `0700`, and cleared on
/// logout — so a stale socket cannot outlive the session.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .map_or_else(|_| std::env::temp_dir(), PathBuf::from);
    dir.join("sipster.sock")
}

/// Outcome of trying to become the single running instance.
pub enum Instance {
    /// We are the first instance; owns the bound listener and optional initial command.
    Primary {
        listener: UnixListener,
        initial_command: Option<Command>,
    },
    /// Another instance is already running and has been sent the command.
    Forwarded,
}

/// Becomes the primary instance, or forwards `command` to the one already
/// running.
///
/// Attempts to connect to an existing instance and forward the command.
async fn try_forward(path: &std::path::Path, command: Option<Command>) -> Result<Option<Instance>> {
    if let Ok(mut stream) = UnixStream::connect(path).await {
        if let Some(command) = command {
            let mut line = serde_json::to_string(&command)
                .map_err(|e| Error::Config(format!("encode command: {e}")))?;
            line.push('\n');
            stream.write_all(line.as_bytes()).await?;
            stream.flush().await?;
        }
        info!("another instance is running; command forwarded");
        return Ok(Some(Instance::Forwarded));
    }
    Ok(None)
}

/// Removes any leftover socket file from an earlier crashed instance.
fn remove_stale_socket(path: &std::path::Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => debug!("removed stale control socket"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::Io(e)),
    }
    Ok(())
}

/// Becomes the primary instance, or forwards `command` to the one already
/// running.
///
/// `command` is what this invocation was asked to do — e.g. the `tel:` URI it
/// was launched with.
pub async fn acquire(command: Option<Command>) -> Result<Instance> {
    let path = socket_path();

    if let Some(instance) = try_forward(&path, command.clone()).await? {
        return Ok(instance);
    }

    remove_stale_socket(&path)?;

    let listener = UnixListener::bind(&path)?;
    info!(socket = %path.display(), "listening for control commands");

    Ok(Instance::Primary {
        listener,
        initial_command: command,
    })
}

/// Accepts control connections and forwards decoded commands to `tx`.
pub async fn serve(listener: UnixListener, tx: mpsc::UnboundedSender<Command>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            warn!("control socket closed");
            return;
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<Command>(&line) {
                    Ok(command) => {
                        info!(?command, "control command received");
                        if tx.send(command).is_err() {
                            return;
                        }
                    }
                    Err(e) => warn!(error = %e, "ignoring malformed control command"),
                }
            }
        });
    }
}

/// Removes the control socket. Call on clean shutdown.
pub fn cleanup() {
    let _ = std::fs::remove_file(socket_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_tel_uri() {
        assert_eq!(
            Command::from_uri("tel:611"),
            Some(Command::Call { target: "611".into() })
        );
    }

    #[test]
    fn parses_tel_uri_with_authority_and_query() {
        // Browsers and desktop handlers produce both of these shapes.
        assert_eq!(
            Command::from_uri("tel://611?call"),
            Some(Command::Call { target: "611".into() })
        );
    }

    #[test]
    fn strips_rfc3966_visual_separators() {
        assert_eq!(
            Command::from_uri("tel:+49-30-12 34"),
            Some(Command::Call { target: "+493012 34".replace(' ', "") })
        );
    }

    #[test]
    fn decodes_percent_encoding() {
        // %2B is '+', as delivered by some browsers.
        assert_eq!(
            Command::from_uri("tel:%2B4930123"),
            Some(Command::Call { target: "+4930123".into() })
        );
    }

    #[test]
    fn keeps_sip_uris_intact() {
        assert_eq!(
            Command::from_uri("sip:bob@example.com"),
            Some(Command::Call { target: "sip:bob@example.com".into() })
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
            Command::Answer,
            Command::Hangup,
            Command::Show,
            Command::Quit,
        ] {
            let json = serde_json::to_string(&command).unwrap();
            assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), command);
        }
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
            Some(Command::Call { target: "611".into() })
        );
        assert_eq!(Command::from_args(["--unknown"]), None);
    }
}
