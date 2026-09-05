//! Application state, message handling, and the engine subscription.
//!
//! Split across `lists`, `providers` and `calling` so no one file carries the
//! whole app; those are child modules so they can reach `SipsterApp`'s private
//! state without widening it to the crate.
//!
//! All telephony work is delegated to [`SipEngine`]; this module only tracks
//! what to display and turns button presses into engine calls.

mod calling;
mod lists;
mod providers;

use iced::window;
use iced::{Subscription, Task, Theme};
use sipster_core::audio::DeviceSelection;
use sipster_core::ipc::Command;
use sipster_core::{
    BlockAction, BlockedNumber, CallEvent, CallId, CallState, Config, RegistrationState,
    ThemeChoice,
};

use sipster_integrations::{
    CardDavClient, CardDavConfig, Contact, FritzConfig, GoogleContactsClient, NumberType, PhoneNumber,
    CallRecord, CallType, RecordSource, SyncManager,
};

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
    pub account: usize,
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
    pub account: usize,
    pub id: CallId,
    pub remote: String,
}

pub struct SipsterApp {
    /// One engine per enabled account, in config order — the same order the
    /// bridge tags its events with.
    engines: Vec<EngineHandle>,
    /// Which account outgoing calls use.
    active_account: usize,
    pending_command: Option<Command>,
    /// Registration state per account, parallel to `engines`.
    pub registration: Vec<RegistrationState>,
    pub account_info: Vec<String>,
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
    EngineReady(usize, EngineHandle),
    EngineFailed(String),
    Call(usize, CallEvent),
    Ipc(Command),
    // Periodic tray poll tick; drains tray::Request into handle_tray.
    TrayTick,
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
            engines: Vec::new(),
            active_account: 0,
            pending_command: None,
            registration: Vec::new(),
            account_info: Vec::new(),
            dial_number: String::new(),
            status: if first_run {
                "Welcome — fill in your SIP account to get started".into()
            } else {
                "Ready".into()
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
        if Some(window) == self.settings_window {
            "Sipster — Settings".into()
        } else if Some(window) == self.contacts_window {
            "Sipster — Contacts".into()
        } else if Some(window) == self.calls_window {
            "Sipster — Call History".into()
        } else {
            // The version lived in the settings About section, which is gone;
            // the title bar is where a version belongs anyway.
            concat!("Sipster ", env!("CARGO_PKG_VERSION")).into()
        }
    }

    // Signature is dictated by iced::daemon(..).subscription(..).
    pub fn subscription(&self) -> Subscription<Message> {
        // engine_bridge::run is a fn()-pointer; it grabs the IPC receiver
        // from the process-global OnceLock in main.rs exactly once.
        // Subsequent subscription calls get None — the stream keeps running.
        let engine_sub = Subscription::run(engine_bridge::run);
        // Poll the tray channel every 100 ms.
        let tray_sub = iced::time::every(std::time::Duration::from_millis(100))
            .map(|_| Message::TrayTick);
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

    /// Drains one pending tray request per tick (non-blocking).
    fn on_tray_tick(&mut self) -> Task<Message> {
        let Some(req) = self.tray.as_ref().and_then(crate::tray::Handle::poll) else {
            return Task::none();
        };
        tracing::debug!(?req, "tray request");
        self.handle_tray(req)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::EngineReady(index, engine) => self.on_engine_ready(index, engine),
            Message::EngineFailed(err) => {
                self.status = format!("Engine error: {err}");
                Task::none()
            }
            Message::Call(index, event) => self.on_call_event(index, event),
            Message::Ipc(cmd) => self.on_ipc(cmd),
            Message::TrayTick => {
                self.on_tray_tick()
            }
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
                self.status = format!("Call failed: {e}");
                Task::none()
            }
            Message::ActionDone(Err(e)) => {
                tracing::error!("Call action failed: {e}");
                self.status = format!("Error: {e}");
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

    /// The account picker's contents: one label per configured account.
    ///
    /// Falls back to the username, then to a numbered placeholder, so an
    /// account with no label is still distinguishable from its neighbours.
    fn account_choices(&self) -> settings::AccountContext<'_> {
        settings::AccountContext {
            labels: self
                .config
                .accounts
                .iter()
                .enumerate()
                .map(|(i, account)| {
                    // Derived from the account itself; a blank one has nothing
                    // to derive from yet, so it is numbered until filled in.
                    if account.registrar.trim().is_empty() {
                        format!("Account {}", i + 1)
                    } else {
                        account.label()
                    }
                })
                .collect(),
            selected: self.active_account,
            current: self.engine().map(|e| e.account()),
            first_run: self.config.needs_setup(),
        }
    }

    /// The engine outgoing calls use.
    pub fn engine(&self) -> Option<&EngineHandle> {
        self.engines.get(self.active_account)
    }

    /// The engine for `account`, for acting on a call that arrived on it
    /// rather than on whichever account is currently selected.
    pub fn engine_for(&self, account: usize) -> Option<&EngineHandle> {
        self.engines.get(account)
    }

    /// How the active account is described in the status bar.
    pub fn active_account_info(&self) -> Option<&str> {
        self.account_info.get(self.active_account).map(String::as_str)
    }

    /// The active account's registration state.
    pub fn active_registration(&self) -> RegistrationState {
        self.registration
            .get(self.active_account)
            .cloned()
            .unwrap_or(RegistrationState::Unregistered)
    }

    /// How many accounts are registered, and how many there are.
    pub fn registered_count(&self) -> (usize, usize) {
        let registered = self
            .registration
            .iter()
            .filter(|state| matches!(state, RegistrationState::Registered))
            .count();
        (registered, self.registration.len())
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


    /// Opens the settings window, or focuses it if it is already open.
    pub(super) fn open_settings(&mut self) -> Task<Message> {
        if let Some(id) = self.settings_window {
            return window::gain_focus(id);
        }

        // Prefer the engine's account (what is actually registered); fall back
        // to the config, so a failed start still opens an editable form rather
        // than a blank one the user cannot fix.
        let account = self
            .engine()
            .as_ref()
            .map(|engine| engine.account().clone())
            .or_else(|| self.config.accounts.first().cloned())
            .unwrap_or_default();
        self.settings.load_account(&account);
        // The provider panels are always on screen now, so their text drafts
        // are seeded when the window opens rather than when a panel expands.
        self.settings.draft_fritz_port = self.config.integration.fritzbox.port.to_string();
        self.settings.draft_vdir_path = self
            .config
            .integration
            .vdir_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.settings.notice = None;
        self.settings.error = None;

        let (id, open) = window::open(crate::settings_window_settings());
        self.settings_window = Some(id);

        // Device enumeration goes through cpal and can take a moment; keep it
        // off the UI thread so the window paints straight away.
        let load_devices = Task::future(async {
            let devices = tokio::task::spawn_blocking(|| {
                (
                    sipster_core::audio::input_devices(),
                    sipster_core::audio::output_devices(),
                )
            })
            .await
            .unwrap_or_default();
            Message::DevicesLoaded(devices.0, devices.1)
        });

        Task::batch([open.map(Message::SettingsOpened), load_devices])
    }

    pub(super) fn on_settings(&mut self, msg: settings::Message) -> Task<Message> {
        use settings::Message as S;

        // Any edit clears the last confirmation so stale feedback is not shown
        // next to a field the user is still changing.
        self.settings.notice = None;

        match msg {
            S::Registrar(v) => self.settings.registrar = v,
            S::Port(v) => self.settings.port = v,
            S::Username(v) => self.settings.username = v,
            S::AuthUser(v) => self.settings.auth_user = v,
            S::Password(v) => self.settings.password = v,
            S::Expires(v) => self.settings.expires = v,
            S::LocalPort(v) => self.settings.local_port = v,
            S::AccountEnabled(v) => self.settings.account_enabled = v,
            S::SelectAccount(index) => return self.select_account(index),
            S::AddAccount => return self.add_account(),
            S::RemoveAccount => return self.remove_account(),
            S::TransportChanged(t) => {
                self.settings.transport = t;
                // Moving between UDP/TCP (5060) and TLS (5061) changes the
                // usual port, so offer the new default rather than silently
                // keeping one that will not connect.
                let previous = sipster_core::Transport::ALL
                    .iter()
                    .find(|other| other.default_port().to_string() == self.settings.port);
                if previous.is_some() {
                    self.settings.port = t.default_port().to_string();
                }
            }
            S::RevealPassword(v) => self.settings.reveal_password = v,
            S::RevealFritzPassword(v) => self.settings.reveal_fritz_password = v,
            S::RevealCardDavPassword(v) => self.settings.reveal_carddav_password = v,
            S::RevealGoogleSecret(v) => self.settings.reveal_google_secret = v,

            S::RevertAccount => {
                let account = self
                    .engine()
                    .as_ref()
                    .map(|engine| engine.account().clone())
                    .or_else(|| self.config.accounts.first().cloned())
                    .unwrap_or_default();
                self.settings.load_account(&account);
                self.settings.error = None;
            }
            S::ApplyAccount => return self.apply_account(),

            S::InputDevice(choice) => {
                self.devices.input = choice.id;
                return self.apply_devices();
            }
            S::OutputDevice(choice) => {
                self.devices.output = choice.id;
                return self.apply_devices();
            }

            S::Theme(theme) => {
                self.config.ui.theme = theme;
                self.persist();
            }
            S::Ringtone(on) => {
                self.config.ui.ringtone = on;
                // Turning the ringtone off during a ringing call should stop
                // the noise now, not after the caller gives up.
                if !on {
                    self.ringtone = None;
                }
                self.persist();
            }
            S::Notifications(on) => {
                self.config.ui.notifications = on;
                self.persist();
            }
            S::DtmfFeedback(on) => {
                self.config.ui.dtmf_feedback = on;
                self.persist();
            }
            S::CallChimes(on) => {
                self.config.ui.call_chimes = on;
                self.persist();
            }
            S::ShowBanner(on) => {
                self.config.ui.show_banner = on;
                self.persist();
            }
            S::RegisterUriSchemes(on) => {
                self.config.ui.register_uri_schemes = on;
                self.persist();
                if on {
                    crate::register_desktop_uri_schemes();
                }
            }
            S::CloseToTray(on) => {
                self.config.ui.close_to_tray = on;
                self.persist();
            }

            S::JumpTo(index) => self.settings.section = index,
            S::Close => {
                if let Some(id) = self.settings_window.take() {
                    return window::close(id);
                }
            }
            other => return self.on_provider_settings(other),
        }
        Task::none()
    }

    /// Commits the account draft: rebuild the engine, then save.
    pub(super) fn apply_account(&mut self) -> Task<Message> {
        let account = match self.settings.to_account(self.settings.transport) {
            Ok(account) => account,
            Err(e) => {
                self.settings.error = Some(e);
                return Task::none();
            }
        };

        self.settings.error = None;
        self.settings.notice = Some("Reconnecting…".into());
        self.status = "Applying account settings…".into();

        // Persist first: if the reconnect fails the user still has the values
        // they typed, and can fix them without retyping everything. Only the
        // selected account is replaced; the others are left as they are.
        if self.config.accounts.len() <= self.active_account {
            self.config.accounts.resize(self.active_account + 1, account.clone());
        }
        self.config.accounts[self.active_account] = account;
        self.persist();

        // The bridge owns engine lifetime, so ask it to rebuild rather than
        // swapping the handles from under the running event loop.
        engine_bridge::reconfigure(self.config.accounts.clone());
        Task::none()
    }

    /// Pushes the device selection into the running engine and saves it.
    pub(super) fn apply_devices(&mut self) -> Task<Message> {
        self.config.audio.input = self.devices.input.clone();
        self.config.audio.output = self.devices.output.clone();
        self.persist();

        // Every account shares the one microphone and speaker, so the change
        // goes to all of them rather than only the selected one.
        let engines = self.engines.clone();
        let selection = self.devices.clone();
        Task::future(async move {
            let mut failed = None;
            for engine in engines {
                if let Err(e) = engine.set_devices(selection.clone()).await {
                    failed = Some(e.to_string());
                }
            }
            Message::ActionDone(failed.map_or(Ok(()), Err))
        })
    }

    /// Writes the config file, reporting failures in the settings window.
    pub(super) fn persist(&mut self) {
        match self.config.save(&self.config_path) {
            Ok(()) => {
                self.settings.error = None;
                self.settings.notice = Some("Saved".into());
            }
            Err(e) => {
                tracing::error!(error = %e, "could not save settings");
                self.settings.error = Some(format!("Could not save: {e}"));
            }
        }
    }

}

/// The dialable number from a SIP URI.
///
/// Local records used to store the whole `From` header —
/// `"Alice" <sip:611@fritz.box>;tag=179BED3B…` — as the number, which made
/// history unreadable and meant "Call back" dialled a string containing a
/// dialog tag.
fn dialable(remote: &str) -> String {
    sipster_integrations::caller_number(remote).to_string()
}

/// The display name from a SIP URI, if it carries one worth showing.
fn display_name(remote: &str) -> Option<String> {
    let raw = remote.trim();
    let name = raw.split_once('<').map(|(name, _)| name)?;
    let name = name.trim().trim_matches('"').trim();
    (!name.is_empty() && name != sipster_integrations::caller_number(raw))
        .then(|| name.to_string())
}

/// Expands a leading `~/` so a typed path behaves the way a shell would.
fn expand_home(path: &str) -> String {
    path.strip_prefix("~/").map_or_else(
        || path.to_string(),
        |rest| {
            std::env::var("HOME").map_or_else(
                |_| path.to_string(),
                |home| format!("{home}/{rest}"),
            )
        },
    )
}

fn registration_status(state: &RegistrationState) -> String {
    match state {
        RegistrationState::Unregistered => "Not registered".into(),
        RegistrationState::Registering => "Registering…".into(),
        RegistrationState::Registered => "Registered".into(),
        RegistrationState::Failed(e) => format!("Registration failed: {e}"),
    }
}

fn call_status(state: CallState) -> String {
    match state {
        CallState::Dialing => "Dialing…".into(),
        CallState::Ringing => "Ringing…".into(),
        CallState::Active => "In call".into(),
        CallState::Terminated => "Call ended".into(),
    }
}

fn chrono_now_iso() -> String {
    let now = std::time::SystemTime::now();
    let duration = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    // Format simple readable timestamp YYYY-MM-DD HH:MM:SS from unix secs
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    // Approximate calendar date
    let year = 1970 + days / 365;
    let day_of_year = days % 365;
    let month = (day_of_year / 30) + 1;
    let day = (day_of_year % 30) + 1;
    format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02}:{s:02}")
}

/// Builds the sync manager from a config: every provider the user has
/// configured, wired up before the first sync starts.
fn build_sync_manager(config: &Config) -> SyncManager {
            let mut sm = SyncManager::new();
            let fb = &config.integration.fritzbox;
            if fb.enabled {
                sm.set_fritzbox(Some(FritzConfig {
                    host: fb.host.clone(),
                    port: fb.port,
                    username: fb.username.clone(),
                    password: fb.password.clone(),
                }));
            }
            let g_clients = config
                .integration
                .google_accounts
                .iter()
                .filter(|a| a.enabled)
                .map(|a| {
                    GoogleContactsClient::new(
                        a.id.clone(),
                        a.email.clone(),
                        a.refresh_token.clone(),
                        a.client_id.clone(),
                        a.client_secret.clone(),
                    )
                })
                .collect();
            sm.set_google_accounts(g_clients);

            let c_clients = config
                .integration
                .carddav_accounts
                .iter()
                .filter(|a| a.enabled)
                .map(|a| {
                    CardDavClient::new(CardDavConfig {
                        url: a.url.clone(),
                        username: a.username.clone(),
                        password: a.password.clone(),
                    })
                })
                .collect();
            sm.set_carddav_accounts(c_clients);

            sm.set_eds(config.integration.eds_enabled);
            // The local vCard folder, configured or auto-detected.
            if config.integration.vdir_enabled {
                sm.set_vdir(
                    config
                        .integration
                        .vdir_path
                        .clone()
                        .map_or_else(sipster_integrations::VdirStore::discover, |path| {
                            vec![sipster_integrations::VdirStore::new(path)]
                        }),
                );
            }
            sm
}

/// Puts the keyboard caret in the dial field.
///
/// iced 0.14 keeps this on `widget::operation` rather than on `text_input`;
/// the field is addressed by the id it was built with.
fn focus_dial_input() -> Task<Message> {
    iced::widget::operation::focus(view::dial_input_id())
}

impl SipsterApp {
    /// Records an engine that finished connecting.
    ///
    /// Engines arrive one at a time as each account registers, so the vectors
    /// are grown to fit rather than assumed to be the right length.
    fn on_engine_ready(&mut self, index: usize, engine: EngineHandle) -> Task<Message> {
        let account = engine.account();
        let info = if account.port == 5060 {
            format!("{} at {}", account.username, account.registrar)
        } else {
            format!("{} at {}:{}", account.username, account.registrar, account.port)
        };

        if self.account_info.len() <= index {
            self.account_info.resize(index + 1, String::new());
        }
        self.account_info[index] = info;
        if self.registration.len() <= index {
            self.registration
                .resize(index + 1, RegistrationState::Unregistered);
        }
        if self.engines.len() <= index {
            self.engines.resize(index + 1, engine.clone());
        }
        self.engines[index] = engine;

        // The settings form edits whichever account is selected.
        if index == self.active_account {
            let account = self.engines[index].account().clone();
            self.settings.load_account(&account);
        }
        self.status = "Ready".into();

        if let Some(cmd) = self.pending_command.take() {
            return self.handle_ipc(cmd);
        }
        Task::none()
    }
}

impl SipsterApp {
    /// Switches which account the Account page edits and outgoing calls use.
    ///
    /// The draft is saved back first, so switching away from a half-typed
    /// account does not throw the edits away.
    fn select_account(&mut self, index: usize) -> Task<Message> {
        if index >= self.config.accounts.len() || index == self.active_account {
            return Task::none();
        }
        self.stash_draft();
        self.active_account = index;
        let account = self.config.accounts[index].clone();
        self.settings.load_account(&account);
        self.settings.notice = None;
        self.settings.error = None;
        Task::none()
    }

    /// Adds a blank account and switches to it.
    fn add_account(&mut self) -> Task<Message> {
        self.stash_draft();
        self.config.accounts.push(sipster_core::SipAccount::default());
        self.active_account = self.config.accounts.len() - 1;
        let account = self.config.accounts[self.active_account].clone();
        self.settings.load_account(&account);
        self.persist();
        self.settings.notice = Some("Account added — fill it in and apply".into());
        Task::none()
    }

    /// Removes the selected account.
    ///
    /// The last one is kept: with none at all the app has nothing to register
    /// and no form to fix it in.
    fn remove_account(&mut self) -> Task<Message> {
        if self.config.accounts.len() <= 1 {
            self.settings.error = Some("The last account cannot be removed".into());
            return Task::none();
        }
        self.config.accounts.remove(self.active_account);
        self.active_account = self.active_account.min(self.config.accounts.len() - 1);
        let account = self.config.accounts[self.active_account].clone();
        self.settings.load_account(&account);
        self.persist();
        self.settings.notice = Some("Account removed".into());
        engine_bridge::reconfigure(self.config.accounts.clone());
        Task::none()
    }

    /// Writes the form back into the selected account without reconnecting.
    ///
    /// Switching accounts would otherwise discard anything typed but not yet
    /// applied.
    fn stash_draft(&mut self) {
        let Ok(account) = self.settings.to_account(self.settings.transport) else {
            return;
        };
        if let Some(slot) = self.config.accounts.get_mut(self.active_account) {
            *slot = account;
        }
    }
}
