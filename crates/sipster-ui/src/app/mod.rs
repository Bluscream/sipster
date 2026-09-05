//! Application state, message handling, and the engine subscription.
//!
//! Split across `calling`, `lists`, `numbers`, `providers` and `settings_ops`
//! so no one file carries the whole app; those are child modules so they can
//! reach `SipsterApp`'s private state without widening it to the crate. What
//! stays here is the state itself, the message dispatch, and the parts every
//! child needs.
//!
//! All telephony work is delegated to [`SipEngine`]; this module only tracks
//! what to display and turns button presses into engine calls.

mod calling;
mod format;
mod hooks;
mod lists;
mod numbers;

pub(crate) use hooks::{run_custom_command, run_hook};
use format::{call_status, chrono_now_iso, dialable, display_name, registration_status};
use providers::build_sync_manager;
mod providers;
mod settings_ops;

use iced::window;
use iced::{Subscription, Task, Theme};
use sipster_core::audio::DeviceSelection;
use sipster_core::ipc::Command;
use sipster_core::{
    BlockAction, BlockedNumber, CallEvent, CallId, CallState, Config, RegistrationState,
    ThemeChoice,
};

use sipster_integrations::{CallRecord, CallType, RecordSource, SyncManager};

use crate::calls;
use crate::contacts;
use crate::engine_bridge::{self, EngineHandle};
use crate::glow::Glow;
use crate::pane;
use crate::settings;
use crate::sound;
use crate::tray;
use crate::view;

/// A call as the UI knows it — enough to render, nothing engine-internal.
#[derive(Debug, Clone)]
pub struct ActiveCall {
    /// Which account the call is on, so it is answered, held and hung up on
    /// the engine it actually belongs to.
    pub id: CallId,
    pub state: CallState,
    pub remote: String,
    /// Mirrors the far end's view, flipped only once a hold or resume is
    /// accepted — so the button never claims a hold that did not happen.
    pub on_hold: bool,
}

/// A ringing inbound call awaiting the user's decision.
#[derive(Debug, Clone)]
pub struct IncomingCall {
    pub id: CallId,
    pub remote: String,
}

pub struct SipsterApp {
    /// The engine for the configured account, once it has connected.
    engine: Option<EngineHandle>,
    pending_command: Option<Command>,
    pub registration: RegistrationState,
    pub account_info: String,
    /// The account's own numbers as the router reports them. `None` where the
    /// router knows nothing about it, or has not been asked yet. See
    /// [`numbers`].
    numbers: Option<sipster_integrations::fritzbox::AccountNumbers>,
    pub dial_number: String,
    pub status: String,
    pub active: Option<ActiveCall>,
    pub incoming: Option<IncomingCall>,
    tray: Option<tray::Handle>,
    /// Live while an inbound call is ringing; dropping it silences the ring.
    ringtone: Option<sound::Ringtone>,

    /// The dialer. Closing it quits; the settings window alone must not keep a
    /// daemon-mode app alive with nothing visible to close.
    main_window: Option<window::Id>,
    settings_window: Option<window::Id>,
    settings: settings::State,
    contacts_window: Option<window::Id>,
    contacts: contacts::State,
    /// Where the contact list currently lives. See [`crate::pane`].
    contacts_at: pane::Placement,
    calls_window: Option<window::Id>,
    calls: calls::State,
    /// Where the history list currently lives.
    calls_at: pane::Placement,
    /// Whether the dialer currently has focus. Tracked because Wayland offers
    /// no way to ask whether a window is minimized.
    main_focused: bool,
    /// Dialpad keys lit by recent input. See [`crate::glow`].
    glow: Glow,
    /// Set while a minimized dialer is being replaced. See
    /// [`SipsterApp::on_show_fallback`].
    reopening_main: bool,
    /// The dialer window's current width, so a docked pane can lay itself out
    /// to fit. Kept from resize events because iced's layout width is not
    /// otherwise readable from `view`.
    main_width: f32,
    sync_manager: SyncManager,
    /// Persisted preferences. The in-memory copy is authoritative; the file is
    /// rewritten whenever it changes.
    config: Config,
    config_path: String,
    /// Mirror of the engine's device selection, so the picker can render
    /// before an engine exists.
    devices: DeviceSelection,
}

#[derive(Debug, Clone)]
pub enum Message {
    // From the engine bridge:
    EngineReady(EngineHandle),
    EngineFailed(String),
    Call(CallEvent),
    Ipc(Command),
    /// The tray icon asked for something.
    Tray(crate::tray::Request),
    /// One animation frame, while a dialpad key is still glowing.
    GlowTick,
    // User intent:
    DialInputChanged(String),
    DialPad(char),
    Backspace,
    ClearInput,
    CallPressed,
    HangupPressed,
    HoldPressed,
    TransferPressed,
    /// A hold or resume the far end accepted.
    HoldChanged(bool),
    /// The router answered with the numbers of every device it knows.
    RouterNumbers(Vec<sipster_integrations::fritzbox::AccountNumbers>),
    AnswerPressed,
    DeclinePressed,
    ContactsPressed,
    WindowResized(window::Id, iced::Size),
    MainFocusChanged(window::Id, bool),
    ShowFallback(window::Id),
    CallListPressed,
    MainOpened(window::Id),
    // Settings window:
    OpenSettings,
    SettingsOpened(window::Id),
    Settings(settings::Message),
    DevicesLoaded(Vec<sipster_core::audio::Device>, Vec<sipster_core::audio::Device>),
    // Contacts window:
    ContactsOpened(window::Id),
    Contacts(contacts::Message),
    // Call list window:
    CallsOpened(window::Id),
    Calls(calls::Message),
    WindowClosed(window::Id),
    // Async results:
    Dialed(Result<CallId, String>),
    ActionDone(Result<(), String>),
}

impl SipsterApp {
    /// Boots the app and opens the dialer window.
    ///
    /// Daemon mode starts with no windows at all, so the main one is opened
    /// here rather than declared as application settings.
    pub fn boot() -> (Self, Task<Message>) {
        // Shared with the engine bridge, so the settings window always edits
        // the same account the engine was built from.
        let (config_path, config) = crate::startup_config();
        let (config, config_path) = (config.clone(), config_path.clone());
        // Nothing is configured and there is no environment variable that
        // could be supplying an account behind our back, so the user has to
        // fill Settings in before anything works. Open it for them.
        let first_run = config.needs_setup();

        let (main_id, open) = window::open(crate::main_window_settings());

        let app = Self {
            engine: None,
            pending_command: None,
            registration: RegistrationState::Unregistered,
            account_info: String::new(),
            numbers: None,
            dial_number: String::new(),
            status: if first_run {
                rust_i18n::t!("welcome").into()
            } else {
                rust_i18n::t!("ready").into()
            },
            active: None,
            incoming: None,
            tray: crate::take_tray(),
            ringtone: None,
            main_window: Some(main_id),
            settings_window: None,
            settings: settings::State::new(),
            contacts_window: None,
            contacts_at: pane::Placement::default(),
            contacts: contacts::State::default(),
            calls_window: None,
            calls_at: pane::Placement::default(),
            main_width: pane::DIALER_WIDTH,
            main_focused: true,
            glow: Glow::default(),
            reopening_main: false,
            calls: calls::State {
                // The badge has to survive a restart, or it would nag again
                // every launch.
                missed_seen_until: config.ui.missed_seen_until.clone(),
                ..calls::State::default()
            },
            sync_manager: build_sync_manager(&config),
            devices: DeviceSelection {
                input: config.audio.input.clone(),
                output: config.audio.output.clone(),
            },
            config_path: config_path.display().to_string(),
            config,
        };

        let mut startup = open.map(Message::SettingsOpened);
        if first_run {
            startup = startup.chain(Task::done(Message::OpenSettings));
        } else {
            // Warm the caches immediately rather than when a window is first
            // opened. The router can take seconds to answer, and that time is
            // free while the user is still looking at the dialer. Skipped on
            // first run, where there is nothing configured to sync from.
            startup = startup.chain(Task::batch([
                Task::done(Message::Contacts(contacts::Message::SyncPressed)),
                Task::done(Message::Calls(calls::Message::SyncPressed)),
            ]));
        }
        (app, startup)
    }

    pub fn title(&self, window: window::Id) -> String {
        let app_name = "Sipster";
        if Some(window) == self.settings_window {
            format!("{app_name} — {}", rust_i18n::t!("settings"))
        } else if Some(window) == self.contacts_window {
            format!("{app_name} — {}", rust_i18n::t!("contacts"))
        } else if Some(window) == self.calls_window {
            format!("{app_name} — {}", rust_i18n::t!("history"))
        } else {
            // The version lived in the settings About section, which is gone;
            // the title bar is where a version belongs anyway.
            format!("{app_name} {}", env!("CARGO_PKG_VERSION"))
        }
    }

    // Signature is dictated by iced::daemon(..).subscription(..).
    pub fn subscription(&self) -> Subscription<Message> {
        // engine_bridge::run is a fn()-pointer; it grabs the IPC receiver
        // from the process-global OnceLock in main.rs exactly once.
        // Subsequent subscription calls get None — the stream keeps running.
        let engine_sub = Subscription::run(engine_bridge::run);
        // The tray pushes; nothing here polls for it.
        let tray_sub = Subscription::run(crate::tray_requests).map(Message::Tray);
        let key_sub = iced::event::listen_with(|event, _status, _window| {
            if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key,
                modifiers,
                ..
            }) = event
            {
                if modifiers.command() {
                    match key.as_ref() {
                        iced::keyboard::Key::Character("p" | "P") => Some(Message::OpenSettings),
                        iced::keyboard::Key::Character("k" | "K") => Some(Message::ContactsPressed),
                        iced::keyboard::Key::Character("h" | "H") => Some(Message::CallListPressed),
                        _ => None,
                    }
                } else {
                    None
                }
            } else {
                None
            }
        });
        // Frames only while a key is still fading; an idle dialer must not be
        // woken sixty times a second for an animation that is not running.
        let glow_sub = if self.glow.is_active() {
            iced::window::frames().map(|_| Message::GlowTick)
        } else {
            Subscription::none()
        };

        Subscription::batch([
            engine_sub,
            tray_sub,
            key_sub,
            glow_sub,
            window::close_events().map(Message::WindowClosed),
            window::resize_events().map(|(id, size)| Message::WindowResized(id, size)),
            window::events().filter_map(|(id, event)| match event {
                window::Event::Focused => Some(Message::MainFocusChanged(id, true)),
                window::Event::Unfocused => Some(Message::MainFocusChanged(id, false)),
                _ => None,
            }),
        ])
    }

    // Signature is dictated by iced::daemon(..).theme(..).
    pub fn theme(&self, _window: window::Id) -> Theme {
        match self.config.ui.theme {
            ThemeChoice::Dark => Theme::Dark,
            ThemeChoice::Light => Theme::Light,
            ThemeChoice::Dracula => Theme::Dracula,
            ThemeChoice::Nord => Theme::Nord,
            ThemeChoice::SolarizedDark => Theme::SolarizedDark,
            ThemeChoice::GruvboxDark => Theme::GruvboxDark,
            ThemeChoice::CatppuccinMocha => Theme::CatppuccinMocha,
            ThemeChoice::TokyoNight => Theme::TokyoNight,
        }
    }

    pub fn ui(&self) -> &sipster_core::UiSettings {
        &self.config.ui
    }

    // One arm per message, each delegating. Clippy scores the width as
    // complexity, but a dispatch table split across functions is harder to
    // check for completeness, not easier.
    #[allow(clippy::cognitive_complexity)]
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::EngineReady(engine) => self.on_engine_ready(engine),
            Message::EngineFailed(err) => {
                self.status = rust_i18n::t!("engine_error", error = err).to_string();
                Task::none()
            }
            Message::Call(event) => self.on_call_event(event),
            Message::Ipc(cmd) => self.on_ipc(cmd),
            Message::Tray(req) => self.handle_tray(req),
            Message::DialInputChanged(v) => self.on_dial_input_changed(v),
            Message::DialPad(d) => {
                self.glow.strike(d);
                if self.config.ui.dtmf_feedback {
                    sound::dtmf(d);
                }
                // On a call the pad drives the far end's phone menu; the
                // number field is the number already dialled, so appending to
                // it there would be nonsense.
                if let Some(task) = self.send_dtmf(d) {
                    return task;
                }
                self.dial_number.push(d);
                Task::none()
            }
            Message::Backspace => {
                self.dial_number.pop();
                Task::none()
            }
            Message::ClearInput => {
                self.dial_number.clear();
                Task::none()
            }
            Message::CallPressed => self.dial(),
            Message::HangupPressed => self.hangup(),
            Message::HoldPressed => self.toggle_hold(),
            Message::TransferPressed => self.transfer(),
            Message::HoldChanged(on_hold) => self.on_hold_changed(on_hold),
            Message::RouterNumbers(found) => {
                self.on_router_numbers(&found);
                Task::none()
            }
            Message::AnswerPressed => self.answer(),
            Message::DeclinePressed => self.decline(),
            Message::ContactsPressed => self.cycle_contacts(),
            Message::CallListPressed => self.cycle_calls(),
            Message::OpenSettings
            | Message::SettingsOpened(_)
            | Message::DevicesLoaded(..)
            | Message::ContactsOpened(_)
            | Message::Contacts(_)
            | Message::CallsOpened(_)
            | Message::Calls(_)
            | Message::WindowClosed(_)
            | Message::WindowResized(..)
            | Message::GlowTick
            | Message::MainFocusChanged(..)
            | Message::ShowFallback(_)
            | Message::Settings(_) => self.on_window_message(message),
            Message::Dialed(Err(e)) => {
                self.status = rust_i18n::t!("call_failed", error = e).to_string();
                Task::none()
            }
            Message::ActionDone(Err(e)) => {
                tracing::error!("Call action failed: {e}");
                self.status = rust_i18n::t!("error", error = e).to_string();
                Task::none()
            }
            Message::ActionDone(Ok(())) => {
                tracing::info!("Call action succeeded");
                Task::none()
            }
            Message::Dialed(Ok(_)) => Task::none(),
            Message::MainOpened(id) => {
                self.main_window = Some(id);
                // Focus the dial field so digits can be typed straight away.
                focus_dial_input()
            }
        }
    }

    /// Where the contact list currently is.
    pub fn contacts_at(&self) -> pane::Placement {
        self.contacts_at
    }

    /// Where the history list currently is.
    pub fn calls_at(&self) -> pane::Placement {
        self.calls_at
    }

    /// The account the settings window edits.
    fn account_choices(&self) -> settings::AccountContext<'_> {
        settings::AccountContext {
            current: self.engine().map(|e| e.account()),
            first_run: self.config.needs_setup(),
        }
    }

    /// The engine outgoing calls use.
    pub fn engine(&self) -> Option<&EngineHandle> {
        self.engine.as_ref()
    }

    /// How this end of a call is recorded in local history.
    pub fn local_party(&self) -> Option<String> {
        (!self.account_info.is_empty()).then(|| self.account_info.clone())
    }

    /// The account's registration state.
    pub fn active_registration(&self) -> RegistrationState {
        self.registration.clone()
    }

    /// Which dialpad keys are currently lit.
    pub fn glow(&self) -> &Glow {
        &self.glow
    }

    /// The dialer window's current width.
    pub fn main_width(&self) -> f32 {
        self.main_width
    }

    /// The docked list, if one is showing beside the dialer.
    pub fn docked_pane(&self) -> Option<iced::Element<'_, Message>> {
        if self.contacts_at.is_docked() {
            return Some(
                contacts::view(&self.contacts, self.config.ui.streaming_mode)
                    .map(Message::Contacts),
            );
        }
        if self.calls_at.is_docked() {
            return Some(calls::view(&self.calls, self.config.ui.streaming_mode).map(Message::Calls));
        }
        None
    }

    pub fn view(&self, window: window::Id) -> iced::Element<'_, Message> {
        if Some(window) == self.settings_window {
            return settings::view(
                &self.settings,
                &self.config.ui,
                &self.devices,
                &self.config.integration,
                &self.config_path,
                &self.account_choices(),
            )
            .map(Message::Settings);
        }
        if Some(window) == self.contacts_window {
            return contacts::view(&self.contacts, self.config.ui.streaming_mode)
                .map(Message::Contacts);
        }
        if Some(window) == self.calls_window {
            return calls::view(&self.calls, self.config.ui.streaming_mode).map(Message::Calls);
        }
        view::root(self)
    }

    // ── windows ───────────────────────────────────────────────────────────────

    /// Window lifecycle and everything auxiliary windows send.
    pub(super) fn on_window_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenSettings => self.open_settings(),
            Message::SettingsOpened(id) => {
                // The boot task routes the dialer's own id through here, so
                // only adopt one if we are not already tracking a main window.
                if self.main_window.is_none() {
                    self.main_window = Some(id);
                    return focus_dial_input();
                }
                Task::none()
            }
            Message::DevicesLoaded(inputs, outputs) => {
                self.settings.inputs = inputs;
                self.settings.outputs = outputs;
                self.settings.devices_loaded = true;
                Task::none()
            }
            Message::ContactsOpened(id) => {
                self.contacts_window = Some(id);
                Task::none()
            }
            Message::Contacts(msg) => self.on_contacts(msg),
            Message::CallsOpened(id) => {
                self.calls_window = Some(id);
                Task::none()
            }
            Message::Calls(msg) => self.on_calls(msg),
            Message::MainFocusChanged(id, focused) => {
                if Some(id) == self.main_window {
                    self.main_focused = focused;
                }
                Task::none()
            }
            Message::ShowFallback(id) => self.on_show_fallback(id),
            Message::GlowTick => {
                self.glow.tick();
                Task::none()
            }
            Message::WindowResized(id, size) => {
                if Some(id) == self.main_window {
                    self.main_width = size.width;
                }
                Task::none()
            }
            Message::WindowClosed(id) => {
                // A dialer we closed ourselves to get around a minimized
                // window; put the replacement up now the old one is gone.
                if self.reopening_main && self.main_window.is_none() {
                    self.reopening_main = false;
                    let (new_id, open) = window::open(crate::main_window_settings());
                    self.main_window = Some(new_id);
                    self.main_focused = true;
                    return open
                        .map(Message::MainOpened)
                        .chain(window::gain_focus(new_id));
                }
                if Some(id) == self.settings_window {
                    self.settings_window = None;
                } else if Some(id) == self.contacts_window {
                    self.contacts_window = None;
                    // Closing the window is the same as cycling past it, so
                    // the next press starts from Hidden rather than reopening
                    // a window that is no longer there.
                    if self.contacts_at.is_window() {
                        self.contacts_at = pane::Placement::Hidden;
                    }
                } else if Some(id) == self.calls_window {
                    self.calls_window = None;
                    if self.calls_at.is_window() {
                        self.calls_at = pane::Placement::Hidden;
                    }
                } else if Some(id) == self.main_window {
                    self.main_window = None;
                    // If close-to-tray is enabled AND the tray icon is working, keep running in background.
                    // Otherwise (or if tray failed), closing the dialer exits the app.
                    if !(self.config.ui.close_to_tray && self.tray.is_some()) {
                        // A pending Google sign-in runs on a blocking thread,
                        // which tokio waits for at shutdown; without this the
                        // process outlives the close by up to three minutes.
                        sipster_integrations::cancel_pending_auth();
                        return iced::exit();
                    }
                }
                Task::none()
            }
            Message::Settings(msg) => self.on_settings(msg),
            // The dispatcher in `update` only routes the arms above here.
            _ => Task::none(),
        }
    }

}

/// Puts the keyboard caret in the dial field.
///
/// iced 0.14 keeps this on `widget::operation` rather than on `text_input`;
/// the field is addressed by the id it was built with.
fn focus_dial_input() -> Task<Message> {
    iced::widget::operation::focus(view::dial_input_id())
}

impl SipsterApp {
    /// Records the engine that finished connecting.
    fn on_engine_ready(&mut self, engine: EngineHandle) -> Task<Message> {
        let account = engine.account();
        self.account_info = format!("{}@{}:{}", account.username, account.registrar, account.port);

        let account = account.clone();
        self.engine = Some(engine);
        self.settings.load_account(&account);
        self.status = "Ready".into();

        if let Some(ref cmd) = self.config.commands.on_app_start {
            let _ = run_custom_command(cmd);
        }

        if let Some(cmd) = self.pending_command.take() {
            return self.handle_ipc(cmd);
        }
        Task::none()
    }
}
