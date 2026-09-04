//! Sipster desktop UI.
//!
//! This crate is a **skin**: it renders state and forwards user intent to
//! [`sipster_core::SipEngine`]. It contains no SIP, SDP, RTP or audio logic —
//! anything reusable by another frontend belongs in `sipster-core`.

mod app;
mod calls;
mod contacts;
mod engine_bridge;
mod settings;
mod sound;
mod tray;
mod ui;
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

/// The config file path and the config read from it at startup.
///
/// Loaded exactly once and shared by the app and the engine bridge. They used
/// to load it independently, which meant two reads that could disagree if the
/// file changed in between — and the settings window would then be editing a
/// different account from the one the engine was registered with.
static CONFIG: OnceLock<(std::path::PathBuf, sipster_core::Config)> = OnceLock::new();

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
  sipster [OPTIONS] [tel:|sip:|sips:|callto:|sipster: URI]

Only one copy of Sipster runs at a time. Starting it again forwards the
requested action to the running copy and exits, so these flags double as a
remote control:

Actions:
  -c, --call <TARGET>     Call a number, extension or SIP URI immediately
  -d, --dial <TARGET>     Pre-fill number in the dial box and focus the window
  -a, --answer            Answer the ringing call
      --hangup            Hang up the active call, or decline a ringing one
      --show              Raise and focus the window
  -q, --quit              Ask the running instance to quit

Options:
      --config-file <PATH>
                          Config file to use
                          (default: $XDG_CONFIG_HOME/sipster/sipster.toml)
      --log-file <PATH>   Append logs to PATH instead of stderr
  -s, --socket <PATH>     Control socket to use, overriding the config file
                          (default: $XDG_RUNTIME_DIR/sipster.sock)
      --no-single-instance
                          Start even if another copy is running. Intended for
                          development; both copies will contend for SIP ports.
  -h, --help              Show this help
  -V, --version           Show the version

Everything is configured in the settings window, which writes the config file.
That file is the only source of configuration — there are no environment
variables to set. On first run the settings window opens by itself.

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

    // Resolve and read the config before Iced starts: the dialer and the
    // engine bridge both need it, and neither can take arguments.
    let config_path = sipster_core::Config::path_from(&args);
    let config = sipster_core::Config::load(&config_path).unwrap_or_else(|e| {
        // A broken file must not stop the app; start on defaults and let the
        // settings window fix it.
        eprintln!("sipster: {}: {e}", config_path.display());
        sipster_core::Config::default()
    });
    tracing::info!(
        path = %config_path.display(),
        first_run = config.needs_setup(),
        "configuration loaded"
    );
    // The control socket may be named in the config; publish it before
    // anything tries to resolve it.
    ipc::set_configured_socket(config.ipc.socket.clone());
    let _ = CONFIG.set((config_path, config));

    if let Some(early_exit) = claim_instance(&args) {
        return early_exit;
    }

    // Spawn the tray icon. The handle is picked up by SipsterApp::new() below.
    // Not having a tray is fine — plenty of desktops have none.
    TRAY.set(std::sync::Mutex::new(tray::spawn())).ok();

    // Daemon rather than application: the settings window is a second real
    // window, which `iced::application` cannot host. Daemon mode starts with
    // no windows, so SipsterApp::boot opens the dialer, and closing the dialer
    // exits explicitly (see Message::WindowClosed) instead of leaving an
    // invisible process behind.
    iced::daemon(SipsterApp::boot, SipsterApp::update, SipsterApp::view)
        .title(SipsterApp::title)
        .subscription(SipsterApp::subscription)
        .theme(SipsterApp::theme)
        .run()
}

/// Settings for the dialer window, opened by [`SipsterApp::boot`].
///
/// Taller than the old 480 to make room for the wordmark above the number
/// field without squeezing the dialpad.
pub(crate) fn main_window_settings() -> iced::window::Settings {
    #[allow(unused_mut)] // only Linux mutates the platform-specific settings
    let mut window = iced::window::Settings {
        size: iced::Size::new(320.0, 560.0),
        min_size: Some(iced::Size::new(300.0, 520.0)),
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
    window
}

/// Settings for the settings window.
///
/// It carries the same app id as the dialer. Without it winit falls back to
/// the binary name (`sipster-linux-x86_64` from the release artifact), so the
/// window gets a generic placeholder icon and the compositor treats it as a
/// different application.
pub(crate) fn settings_window_settings() -> iced::window::Settings {
    iced::window::Settings {
        // Wide enough for the category index beside the panel.
        size: iced::Size::new(860.0, 700.0),
        min_size: Some(iced::Size::new(620.0, 400.0)),
        ..main_window_settings()
    }
}

/// Settings for the contacts window.
pub(crate) fn contacts_window_settings() -> iced::window::Settings {
    iced::window::Settings {
        size: iced::Size::new(480.0, 600.0),
        min_size: Some(iced::Size::new(360.0, 400.0)),
        ..main_window_settings()
    }
}

/// Settings for the call history window.
pub(crate) fn calls_window_settings() -> iced::window::Settings {
    iced::window::Settings {
        size: iced::Size::new(520.0, 600.0),
        min_size: Some(iced::Size::new(360.0, 400.0)),
        ..main_window_settings()
    }
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
            // Expected when a real instance already owns the socket: this
            // copy simply runs without remote control rather than stealing it.
            Err(e) => eprintln!("sipster: running without a control channel ({e})"),
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

/// The config file path, the startup config, and where its account came from.
///
/// # Panics
///
/// Panics only if called before `main` initialises it, which cannot happen:
/// both callers run inside the Iced runtime that `main` starts.
pub(crate) fn startup_config() -> &'static (std::path::PathBuf, sipster_core::Config) {
    CONFIG.get().expect("config is loaded before Iced starts")
}

/// Called by [`engine_bridge::run`] to take the primary state exactly once.
pub(crate) fn take_primary_state() -> Option<PrimaryState> {
    PRIMARY_STATE.get()?.lock().ok()?.take()
}

/// Called by [`SipsterApp::new`] to take the tray handle exactly once.
pub(crate) fn take_tray() -> Option<tray::Handle> {
    TRAY.get()?.lock().ok()?.take()
}

/// Registers Sipster as the default handler for telephony & SIP URI schemes on the desktop.
pub(crate) fn register_desktop_uri_schemes() {
    #[cfg(target_os = "linux")]
    {
        tokio::spawn(async {
            const SCHEMES: &[&str] = &[
                "x-scheme-handler/tel",
                "x-scheme-handler/sip",
                "x-scheme-handler/sips",
                "x-scheme-handler/callto",
                "x-scheme-handler/sipster",
            ];
            for scheme in SCHEMES {
                let status = tokio::process::Command::new("xdg-mime")
                    .args(["default", "sipster.desktop", scheme])
                    .status()
                    .await;
                match status {
                    Ok(s) if s.success() => {
                        tracing::info!(scheme, "registered sipster.desktop as default handler");
                    }
                    Ok(s) => {
                        tracing::warn!(scheme, exit_code = ?s.code(), "xdg-mime default failed");
                    }
                    Err(e) => {
                        tracing::warn!(scheme, error = %e, "could not run xdg-mime");
                    }
                }
            }
        });
    }
}
