//! Sipster desktop UI.
//!
//! This crate is a **skin**: it renders state and forwards user intent to
//! [`sipster_core::SipEngine`]. It contains no SIP, SDP, RTP or audio logic —
//! anything reusable by another frontend belongs in `sipster-core`.

mod app;
mod engine_bridge;
mod tray;
mod view;

use std::sync::OnceLock;

use app::SipsterApp;
use sipster_core::ipc::{self, Command, Instance};
use tokio::sync::mpsc::UnboundedReceiver;

/// IPC command receiver — produced before Iced starts, consumed once by
/// [`SipsterApp::new`] via [`engine_bridge::run`].
static IPC_RX: OnceLock<std::sync::Mutex<Option<UnboundedReceiver<Command>>>> =
    OnceLock::new();

/// Tray handle — produced before Iced starts, consumed once by
/// [`SipsterApp::new`].
static TRAY: OnceLock<std::sync::Mutex<Option<tray::Handle>>> = OnceLock::new();

/// Entry point for Sipster.
///
/// # Panics
///
/// Panics only if the local tokio runtime fails to initialize.
pub fn main() -> iced::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // No args → forward a Show command so the running window gets focus.
    let command = Command::from_args(&args).or(Some(Command::Show));

    // Always check single-instance before starting Iced.
    let ipc_rx = {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime");

        match rt.block_on(ipc::acquire(command)) {
            Ok(Instance::Primary(rx)) => rx,
            Ok(Instance::Forwarded) => return Ok(()),
            Err(e) => {
                eprintln!("sipster: IPC socket error: {e}");
                // Start without IPC rather than refusing to open.
                tokio::sync::mpsc::unbounded_channel::<Command>().1
            }
        }
    };

    IPC_RX
        .set(std::sync::Mutex::new(Some(ipc_rx)))
        .ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sipster_core=debug".into()),
        )
        .init();

    // Spawn the tray icon inside a small tokio runtime. The handle is picked
    // up by SipsterApp::new() below. Not having a tray is fine.
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime for tray");
        let handle = rt.block_on(tray::spawn());
        TRAY.set(std::sync::Mutex::new(handle)).ok();
    }

    let window_icon = tray::window_icon();

    let mut app = iced::application(SipsterApp::new, SipsterApp::update, SipsterApp::view)
        .title("Sipster")
        .subscription(SipsterApp::subscription)
        .theme(SipsterApp::theme);

    if let Some(icon) = window_icon {
        app = app.window(iced::window::Settings {
            icon: Some(icon),
            ..iced::window::Settings::default()
        });
    }

    app.run()
}

/// Called by [`engine_bridge::run`] to take the IPC receiver exactly once.
pub(crate) fn take_ipc_rx() -> Option<UnboundedReceiver<Command>> {
    IPC_RX.get()?.lock().ok()?.take()
}

/// Called by [`SipsterApp::new`] to take the tray handle exactly once.
pub(crate) fn take_tray() -> Option<tray::Handle> {
    TRAY.get()?.lock().ok()?.take()
}
