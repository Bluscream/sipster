//! Platform transport for the control channel.
//!
//! The protocol above this layer is identical everywhere — one JSON-encoded
//! [`Command`](super::Command) per line — so only the plumbing differs:
//!
//! - **Unix:** a Unix domain socket in `XDG_RUNTIME_DIR`, which is per-user,
//!   `0700` and cleared on logout.
//! - **Windows:** a named pipe under `\\.\pipe\`, the equivalent local-only IPC
//!   primitive.
//!
//! The two differ in *when* they can be created, and the API reflects that.
//! A Unix listener is bound eagerly, before Iced starts, so that binding
//! failures are reported while we can still exit cleanly. A Windows named pipe
//! server must be created inside a tokio reactor, so [`Listener`] there only
//! remembers the name and creates the first pipe instance in [`Listener::accept`].

use std::path::{Path, PathBuf};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::Command;
use crate::error::Result;

/// Default control-channel address for the current user.
#[cfg(unix)]
pub(super) fn default_path(runtime_dir: Option<String>) -> PathBuf {
    let dir = runtime_dir.map_or_else(std::env::temp_dir, PathBuf::from);
    dir.join("sipster.sock")
}

/// Named pipes live in a flat global namespace, so the user name is part of the
/// address — otherwise two users on one machine would collide.
#[cfg(windows)]
pub(super) fn default_path(_runtime_dir: Option<String>) -> PathBuf {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".into());
    PathBuf::from(format!(r"\\.\pipe\sipster-{user}"))
}

// ── unix ─────────────────────────────────────────────────────────────────────

#[cfg(unix)]
mod imp {
    use super::{Path, Result};

    /// A bound Unix listener, created before the GUI runtime starts.
    #[derive(Debug)]
    pub struct Listener(pub(super) std::os::unix::net::UnixListener);

    impl Listener {
        pub(super) fn bind_impl(path: &Path) -> Result<Self> {
            // Any socket file here is left over from an unclean shutdown: we
            // only get this far while holding the single-instance lock, so
            // nothing can be listening on it.
            match std::fs::remove_file(path) {
                Ok(()) => tracing::debug!("removed stale control socket"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(crate::error::Error::Io(e)),
            }
            let listener = std::os::unix::net::UnixListener::bind(path)?;
            listener.set_nonblocking(true)?;
            Ok(Self(listener))
        }
    }

    pub(super) async fn connect(path: &Path) -> Option<tokio::net::UnixStream> {
        tokio::net::UnixStream::connect(path).await.ok()
    }

    /// Removes the socket file. Named pipes have no filesystem entry to clean.
    pub(super) fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}

// ── windows ──────────────────────────────────────────────────────────────────

#[cfg(windows)]
mod imp {
    use super::{Path, PathBuf, Result};
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

    /// The pipe name. Unlike Unix, nothing is created until `accept` runs
    /// inside the tokio runtime.
    #[derive(Debug)]
    pub struct Listener(pub(super) PathBuf);

    impl Listener {
        // Infallible here, but the signature must match the Unix one, where
        // binding a socket genuinely can fail.
        #[allow(clippy::unnecessary_wraps)]
        pub(super) fn bind_impl(path: &Path) -> Result<Self> {
            Ok(Self(path.to_path_buf()))
        }
    }

    pub(super) async fn connect(path: &Path) -> Option<tokio::net::windows::named_pipe::NamedPipeClient> {
        ClientOptions::new().open(path).ok()
    }

    pub(super) fn cleanup(_path: &Path) {}

    impl Listener {
        /// Creates the next pipe instance and waits for a client.
        ///
        /// Named pipes are one server object per connection, so a fresh
        /// instance is created for each accept — the loop in
        /// [`super::serve`] does exactly what the Unix accept loop does.
        pub(super) async fn accept_one(
            &self,
        ) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
            let server = ServerOptions::new().create(&self.0)?;
            server.connect().await?;
            Ok(server)
        }
    }
}

pub use imp::Listener;

// ── shared API ───────────────────────────────────────────────────────────────

/// Claims the control channel at `path`.
///
/// # Errors
///
/// Fails if the address is already in use or cannot be created.
pub fn bind(path: &Path) -> Result<Listener> {
    let listener = Listener::bind_impl(path)?;
    info!(address = %path.display(), "listening for control commands");
    Ok(listener)
}

/// Sends `command` to a running instance, if one is accepting connections.
///
/// Returns `true` when it was delivered.
pub(super) async fn forward(path: &Path, command: Option<&Command>) -> Result<bool> {
    let Some(mut stream) = imp::connect(path).await else {
        return Ok(false);
    };
    if let Some(command) = command {
        let mut line = serde_json::to_string(command)
            .map_err(|e| crate::error::Error::Config(format!("encode command: {e}")))?;
        line.push('\n');
        stream.write_all(line.as_bytes()).await?;
        stream.flush().await?;
    }
    Ok(true)
}

/// Removes any filesystem entry belonging to the control channel.
pub(super) fn cleanup(path: &Path) {
    imp::cleanup(path);
}

/// Reads newline-delimited JSON commands from one client until it disconnects.
async fn read_commands<S>(stream: S, tx: &mpsc::UnboundedSender<Command>)
where
    S: tokio::io::AsyncRead + Unpin,
{
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
}

/// Accepts control connections forever, forwarding decoded commands to `tx`.
#[cfg(unix)]
pub async fn serve(listener: Listener, tx: mpsc::UnboundedSender<Command>) {
    let listener = match tokio::net::UnixListener::from_std(listener.0) {
        Ok(listener) => listener,
        Err(e) => {
            warn!(error = %e, "could not adopt the control socket into the runtime");
            return;
        }
    };
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            warn!("control socket closed");
            return;
        };
        let tx = tx.clone();
        tokio::spawn(async move { read_commands(stream, &tx).await });
    }
}

/// Accepts control connections forever, forwarding decoded commands to `tx`.
#[cfg(windows)]
pub async fn serve(listener: Listener, tx: mpsc::UnboundedSender<Command>) {
    loop {
        let server = match listener.accept_one().await {
            Ok(server) => server,
            Err(e) => {
                warn!(error = %e, "control pipe closed");
                return;
            }
        };
        let tx = tx.clone();
        tokio::spawn(async move { read_commands(server, &tx).await });
    }
}
