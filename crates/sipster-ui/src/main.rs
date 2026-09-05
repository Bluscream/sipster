//! Sipster desktop UI.
//!
//! This crate is a **skin**: it renders state and forwards user intent to
//! [`sipster_core::SipEngine`]. It contains no SIP, SDP, RTP or audio logic —
//! anything reusable by another frontend belongs in `sipster-core`.

mod app;
mod calls;
mod consts;
mod contacts;
mod engine_bridge;
mod glow;
mod pane;
mod settings;
mod sound;
mod tray;
mod ui;
mod view;

rust_i18n::i18n!("locales", fallback = "en");

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

/// The tray's request stream, taken out of the handle before the application
/// claims the handle itself.
///
/// Separate from `TRAY` because `SipsterApp::boot` takes the handle to keep
/// the icon alive and to update its state, which would otherwise leave the
/// subscription with nothing to read.
static TRAY_REQUESTS: OnceLock<std::sync::Mutex<Option<tray::Requests>>> = OnceLock::new();

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
  -h, --help              Show this help
  -V, --version           Show the version

Everything else is configured in the settings window, which writes the config
file. That file is the only source of configuration: there are no environment
variables and no other flags. On first run the settings window opens by itself.

Actions are URIs, and they work whether or not the desktop has registered the
handler:

  sipster sipster://call/611      place a call
  sipster sipster://dial/611      fill the dial box
  sipster sipster://answer        answer the ringing call
  sipster sipster://hangup        end the current call
  sipster sipster://hold          hold, and sipster://resume to resume
  sipster sipster://transfer/623  transfer the current call
  sipster sipster://dtmf/5        send one DTMF digit
  sipster sipster://settings      open settings (also contacts, history)
  sipster sipster://show          focus the dialer
  sipster sipster://quit          quit the running copy

`tel:`, `sip:`, `sips:` and `callto:` URIs fill the dial box.

Run a second line by giving a second copy its own config:

  sipster --config-file ~/.config/sipster/work.toml

Each config is its own instance, with its own control socket, window and tray
icon. Address one of them by passing the same --config-file alongside the URI.

Logging is configured under [log] in the config file: `file` to also append to
a file, and `filter` for verbosity (the syntax RUST_LOG used to take).
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

    // Resolve and read the config before Iced starts: the dialer and the
    // engine bridge both need it, and neither can take arguments.
    //
    // Before logging, too, because logging is configured in the file. A
    // failure to read it therefore has only stderr to report to, which is why
    // it prints as well as logs.
    let config_path = sipster_core::Config::path_from(&args);
    let load_error = std::cell::RefCell::new(None);
    let config = sipster_core::Config::load(&config_path).unwrap_or_else(|e| {
        // A broken file must not stop the app; start on defaults and let the
        // settings window fix it. Reported once logging exists, and printed
        // now — silently running on defaults looks like the config was
        // ignored.
        eprintln!("sipster: {}: {e}", config_path.display());
        *load_error.borrow_mut() = Some(e.to_string());
        sipster_core::Config::default()
    });

    rust_i18n::set_locale(config.ui.language.code());

    init_logging(&config.log);
    if let Some(error) = load_error.into_inner() {
        tracing::error!(path = %config_path.display(), %error, "could not read the config");
    }
    tracing::info!(
        path = %config_path.display(),
        first_run = config.needs_setup(),
        "configuration loaded"
    );
    // The control socket may be named in the config; publish it before
    // anything tries to resolve it.
    ipc::set_configured_socket(config.ipc.socket.clone());
    // The instance lock and the default control socket are both named after
    // the config, so a second config is a second instance — no environment
    // overrides, which is what previously broke the Wayland connection.
    ipc::set_config_path(config_path.clone());
    let _ = CONFIG.set((config_path, config));

    if let Some(early_exit) = claim_instance(&args) {
        return early_exit;
    }

    // Spawn the tray icon. The handle is picked up by SipsterApp::new() below.
    // Not having a tray is fine — plenty of desktops have none.
    let mut tray = tray::spawn();
    let requests = tray.as_mut().and_then(tray::Handle::take_requests);
    TRAY_REQUESTS.set(std::sync::Mutex::new(requests)).ok();
    TRAY.set(std::sync::Mutex::new(tray)).ok();

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
            tracing::error!(error = %e, "could not claim the single-instance lock");
            eprintln!("sipster: could not claim the single-instance lock: {e}");
            Some(Ok(()))
        }
    }
}

fn store_primary_state(state: PrimaryState) {
    PRIMARY_STATE.set(std::sync::Mutex::new(Some(state))).ok();
}

/// Installs the tracing subscriber from the config's `[log]` section.
///
/// A log file that cannot be opened falls back to the console rather than
/// starting the app with logging silently switched off.
fn init_logging(settings: &sipster_core::LogSettings) {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let filter = tracing_subscriber::EnvFilter::try_new(&settings.filter).unwrap_or_else(|e| {
        eprintln!("sipster: log filter {:?} is not valid: {e}", settings.filter);
        tracing_subscriber::EnvFilter::new(sipster_core::default_log_filter())
    });

    let file = settings.file.as_ref().and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| {
                eprintln!(
                    "sipster: cannot write to {}: {e}; logging to the console only",
                    path.display()
                );
            })
            .ok()
    });

    // Both, not either. A log file used to redirect logging *away* from the
    // console, so anyone running Sipster from a terminal with one configured
    // saw nothing at all — including the messages that explain why something
    // did not work. The file is a record; the console is what someone
    // watching the run actually reads.
    let console = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let to_file = file.map(|file| {
        tracing_subscriber::fmt::layer()
            .with_writer(file)
            // Escape codes are noise in a file someone will grep or paste.
            .with_ansi(false)
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(to_file)
        .init();
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
/// Streams tray requests for `Subscription::run`.
///
/// A `fn()` pointer with no captures, like [`engine_bridge::run`], and it
/// takes the receiver exactly once; later calls from Iced re-rendering get an
/// empty stream while the original keeps running.
pub(crate) fn tray_requests() -> impl iced::futures::Stream<Item = tray::Request> {
    let requests = TRAY_REQUESTS
        .get()
        .and_then(|slot| slot.lock().ok())
        .and_then(|mut slot| slot.take());

    iced::futures::stream::unfold(requests, |requests| async move {
        let mut requests = requests?;
        // `None` means the tray was dropped, which ends the stream.
        let request = requests.recv().await?;
        Some((request, Some(requests)))
    })
}

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
