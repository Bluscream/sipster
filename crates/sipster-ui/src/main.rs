//! Sipster desktop UI.
//!
//! This crate is a **skin**: it renders state and forwards user intent to
//! [`sipster_core::SipEngine`]. It contains no SIP, SDP, RTP or audio logic —
//! anything reusable by another frontend belongs in `sipster-core`.

mod app;
mod engine_bridge;
mod sound;
mod tray;
mod view;

use std::sync::OnceLock;

use app::SipsterApp;
use sipster_core::cli;
use sipster_core::ipc::{self, Command, Instance, Listener};

/// Primary instance state, parked here between the single-instance check (which
/// runs before Iced starts) and the engine subscription that consumes it.
pub(crate) struct PrimaryState {
    pub(crate) listener: Listener,
    pub(crate) initial_command: Option<Command>,
}

/// The single-instance lock. Held for the lifetime of the process — releasing
/// it early would let a second copy start and fight over SIP port 5060.
static PRIMARY_LOCK: OnceLock<sipster_core::instance::Guard> = OnceLock::new();
static PRIMARY_STATE: OnceLock<std::sync::Mutex<Option<PrimaryState>>> = OnceLock::new();

/// Tray handle — produced before Iced starts, consumed once by
/// [`SipsterApp::new`].
static TRAY: OnceLock<std::sync::Mutex<Option<tray::Handle>>> = OnceLock::new();

/// Default log filter, used when `RUST_LOG` is unset.
///
/// Two upstream crates are muted to `warn`, and neither is a matter of taste:
///
/// - `iced_winit` logs the window attributes at INFO, and those attributes
///   embed the 256×256 window icon, which the pretty-printer renders as one
///   line per byte — a 5 MB, 262,000-line log on every single startup.
/// - `rvoip_media_core` logs four INFO lines per RTP packet, so a call emitted
///   ~255 lines per second, roughly 2 MB for thirty seconds of talking.
///
/// Both drowned the SIP signalling that a bug report actually needs. `warn`
/// still surfaces genuine problems from either. `RUST_LOG` overrides all of
/// this, e.g. `RUST_LOG=rvoip_media_core=debug` when chasing an audio fault.
const DEFAULT_LOG_FILTER: &str = "info,sipster_core=debug,\
     iced_winit=warn,iced_wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,\
     rvoip_media_core=warn";

/// Desktop application id. Must stay in sync with `packaging/sipster.desktop`
/// — both its filename and its `StartupWMClass` — for the icon to resolve.
///
/// Linux only: Windows takes its window icon from the embedded pixels instead.
#[cfg(target_os = "linux")]
const APP_ID: &str = "sipster";

const HELP: &str = "\
Sipster — a desktop SIP softphone

Usage:
  sipster [OPTIONS] [tel:|sip:|sips:|callto: URI]

Only one copy of Sipster runs at a time. Starting it again forwards the
requested action to the running copy and exits, so these flags double as a
remote control:

Actions:
  -c, --call <TARGET>     Dial a number, extension or SIP URI
  -a, --answer            Answer the ringing call
      --hangup            Hang up the active call, or decline a ringing one
      --show              Raise and focus the window
  -q, --quit              Ask the running instance to quit

Options:
      --log-file <PATH>   Append logs to PATH instead of stderr
  -s, --socket <PATH>     Control socket to use
                          (default: $XDG_RUNTIME_DIR/sipster.sock)
      --no-single-instance
                          Start even if another copy is running. Intended for
                          development; both copies will contend for SIP ports.
  -h, --help              Show this help
  -V, --version           Show the version

Configuration is read from SIPSTER_* (or SIP_*) environment variables, then
from $XDG_CONFIG_HOME/sipster/sipster.toml. See the README for the full list.

Log verbosity follows RUST_LOG, e.g. RUST_LOG=sipster_core=trace.
";

/// Entry point for Sipster.
///
/// # Panics
///
/// Panics only if the local tokio runtime fails to initialize.
pub fn main() -> iced::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if cli::has_flag(&args, &["-h", "--help"]) {
        print!("{HELP}");
        return Ok(());
    }
    if cli::has_flag(&args, &["-V", "--version"]) {
        println!("sipster {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    init_logging(cli::flag_value(&args, &["--log-file"]));

    if let Some(early_exit) = claim_instance(&args) {
        return early_exit;
    }

    // Spawn the tray icon. The handle is picked up by SipsterApp::new() below.
    // Not having a tray is fine — plenty of desktops have none.
    TRAY.set(std::sync::Mutex::new(tray::spawn())).ok();

    #[allow(unused_mut)] // only Linux mutates the platform-specific settings
    let mut window = iced::window::Settings {
        size: iced::Size::new(320.0, 480.0),
        min_size: Some(iced::Size::new(280.0, 440.0)),
        icon: tray::window_icon(),
        ..iced::window::Settings::default()
    };

    // Wayland ignores the pixel icon above and resolves the window icon from
    // the .desktop file matching this app id. It must equal the desktop file's
    // basename and its StartupWMClass, or the window shows a generic
    // placeholder — which is exactly what happened before this was set, since
    // winit defaults the id to the crate name (`sipster-ui`) while the AppImage
    // installs the binary and desktop entry as `sipster`.
    //
    // `application_id` exists only in iced's Linux PlatformSpecific.
    #[cfg(target_os = "linux")]
    {
        APP_ID.clone_into(&mut window.platform_specific.application_id);
    }

    iced::application(SipsterApp::new, SipsterApp::update, SipsterApp::view)
        .title("Sipster")
        .subscription(SipsterApp::subscription)
        .theme(SipsterApp::theme)
        .window(window)
        .run()
}

/// Becomes the primary instance, or forwards this invocation's command to the
/// copy already running.
///
/// `Some(..)` means the GUI must not start — the command was forwarded to the
/// running copy, or the claim failed. Both are ordinary outcomes, not errors.
fn claim_instance(args: &[String]) -> Option<iced::Result> {
    if cli::has_flag(args, &["--no-single-instance"]) {
        // Development mode: still open the control channel so the instance can
        // be driven over IPC, but do not take the single-instance lock. If the
        // channel is already claimed by a real primary, carry on without one.
        match ipc::bind_control_channel() {
            Ok(listener) => store_primary_state(PrimaryState {
                listener,
                initial_command: Command::from_args(args),
            }),
            Err(e) => eprintln!("sipster: no control channel in this instance: {e}"),
        }
        return None;
    }

    // No action requested → ask the running copy (if any) to show itself.
    let command = Command::from_args(args).or(Some(Command::Show));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to create tokio runtime");

    match runtime.block_on(ipc::acquire(command)) {
        Ok(Instance::Primary { lock, listener, initial_command }) => {
            let _ = PRIMARY_LOCK.set(lock);
            store_primary_state(PrimaryState { listener, initial_command });
            None
        }
        Ok(Instance::Forwarded) => Some(Ok(())),
        Err(e) => {
            eprintln!("sipster: could not claim the single-instance lock: {e}");
            Some(Ok(()))
        }
    }
}

fn store_primary_state(state: PrimaryState) {
    PRIMARY_STATE.set(std::sync::Mutex::new(Some(state))).ok();
}

/// Installs the tracing subscriber, writing to `path` when one was given.
///
/// A log file that cannot be opened falls back to stderr rather than starting
/// the app with logging silently switched off.
fn init_logging(path: Option<&str>) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| DEFAULT_LOG_FILTER.into());

    let file = path.and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| eprintln!("sipster: cannot write to {path}: {e}; logging to stderr"))
            .ok()
    });

    match file {
        Some(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(file)
            .with_ansi(false)
            .init(),
        None => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

/// Called by [`engine_bridge::run`] to take the primary state exactly once.
pub(crate) fn take_primary_state() -> Option<PrimaryState> {
    PRIMARY_STATE.get()?.lock().ok()?.take()
}

/// Called by [`SipsterApp::new`] to take the tray handle exactly once.
pub(crate) fn take_tray() -> Option<tray::Handle> {
    TRAY.get()?.lock().ok()?.take()
}
