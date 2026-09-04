//! Application state, message handling, and the engine subscription.
//!
//! All telephony work is delegated to [`SipEngine`]; this module only tracks
//! what to display and turns button presses into engine calls.

use iced::window;
use iced::{Subscription, Task, Theme};
use sipster_core::audio::DeviceSelection;
use sipster_core::ipc::Command;
use sipster_core::{
    CallEvent, CallId, CallState, Config, RegistrationState, SipAccount, ThemeChoice,
};

use sipster_integrations::{CallRecord, CallType, RecordSource, SyncManager};

use crate::calls;
use crate::contacts;
use crate::engine_bridge::{self, EngineHandle};
use crate::settings;
use crate::sound;
use crate::tray;
use crate::view;

/// A call as the UI knows it — enough to render, nothing engine-internal.
#[derive(Debug, Clone)]
pub struct ActiveCall {
    pub id: CallId,
    pub state: CallState,
    pub remote: String,
}

/// A ringing inbound call awaiting the user's decision.
#[derive(Debug, Clone)]
pub struct IncomingCall {
    pub id: CallId,
    pub remote: String,
}

pub struct SipsterApp {
    engine: Option<EngineHandle>,
    pending_command: Option<Command>,
    pub registration: RegistrationState,
    pub account_info: Option<String>,
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
    calls_window: Option<window::Id>,
    calls: calls::State,
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
    // Periodic tray poll tick; drains tray::Request into handle_tray.
    TrayTick,
    // User intent:
    DialInputChanged(String),
    DialPad(char),
    Backspace,
    ClearInput,
    CallPressed,
    HangupPressed,
    AnswerPressed,
    DeclinePressed,
    ContactsPressed,
    CallListPressed,
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
            account_info: None,
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
            settings: settings::State::default(),
            contacts_window: None,
            contacts: contacts::State::default(),
            calls_window: None,
            calls: calls::State::default(),
            sync_manager: SyncManager::new(),
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
            "Sipster".into()
        }
    }

    // Signature is dictated by iced::daemon(..).subscription(..).
    #[allow(clippy::unused_self)]
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
        Subscription::batch([
            engine_sub,
            tray_sub,
            key_sub,
            window::close_events().map(Message::WindowClosed),
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

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::EngineReady(engine) => {
                let acc = engine.account();
                self.account_info = Some(if acc.port == 5060 {
                    format!("{} at {}", acc.username, acc.registrar)
                } else {
                    format!("{} at {}:{}", acc.username, acc.registrar, acc.port)
                });
                self.settings.load_account(self.engine_account_of(&engine));
                self.engine = Some(engine);
                self.status = "Ready".into();
                if let Some(cmd) = self.pending_command.take() {
                    return self.handle_ipc(cmd);
                }
                Task::none()
            }
            Message::EngineFailed(err) => {
                self.status = format!("Engine error: {err}");
                Task::none()
            }
            Message::Call(event) => self.on_call_event(event),
            Message::Ipc(cmd) => self.on_ipc(cmd),
            Message::TrayTick => {
                // Drain one pending tray request per tick (non-blocking).
                if let Some(req) = self.tray.as_ref().and_then(crate::tray::Handle::poll) {
                    return self.handle_tray(req);
                }
                Task::none()
            }
            Message::DialInputChanged(v) => {
                self.on_dial_input_changed(v);
                Task::none()
            }
            Message::DialPad(d) => {
                self.dial_number.push(d);
                if self.config.ui.dtmf_feedback {
                    sound::dtmf(d);
                }
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
            Message::AnswerPressed => self.answer(),
            Message::DeclinePressed => self.decline(),
            Message::ContactsPressed => self.open_contacts(),
            Message::CallListPressed => self.open_calls(),
            Message::OpenSettings
            | Message::SettingsOpened(_)
            | Message::DevicesLoaded(..)
            | Message::ContactsOpened(_)
            | Message::Contacts(_)
            | Message::CallsOpened(_)
            | Message::Calls(_)
            | Message::WindowClosed(_)
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
        }
    }

    pub fn view(&self, window: window::Id) -> iced::Element<'_, Message> {
        if Some(window) == self.settings_window {
            let account = self.engine.as_ref().map(|e| e.account());
            return settings::view(
                &self.settings,
                &self.config.ui,
                &self.devices,
                account,
                &self.config_path,
                self.config.needs_setup(),
            )
            .map(Message::Settings);
        }
        if Some(window) == self.contacts_window {
            return contacts::view(&self.contacts).map(Message::Contacts);
        }
        if Some(window) == self.calls_window {
            return calls::view(&self.calls).map(Message::Calls);
        }
        view::root(self)
    }

    // ── windows ───────────────────────────────────────────────────────────────

    /// Window lifecycle and everything auxiliary windows send.
    fn on_window_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenSettings => self.open_settings(),
            Message::SettingsOpened(id) => {
                // The boot task routes the dialer's own id through here, so
                // only adopt one if we are not already tracking a main window.
                if self.main_window.is_none() {
                    self.main_window = Some(id);
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
            Message::WindowClosed(id) => {
                if Some(id) == self.settings_window {
                    self.settings_window = None;
                } else if Some(id) == self.contacts_window {
                    self.contacts_window = None;
                } else if Some(id) == self.calls_window {
                    self.calls_window = None;
                } else if Some(id) == self.main_window {
                    // Daemon mode outlives its windows, so closing the dialer
                    // has to be an explicit quit rather than a hide.
                    return iced::exit();
                }
                Task::none()
            }
            Message::Settings(msg) => self.on_settings(msg),
            // The dispatcher in `update` only routes the arms above here.
            _ => Task::none(),
        }
    }

    #[allow(clippy::unused_self)]
    fn engine_account_of<'a>(&self, engine: &'a EngineHandle) -> &'a SipAccount {
        engine.account()
    }

    /// Opens the settings window, or focuses it if it is already open.
    fn open_settings(&mut self) -> Task<Message> {
        if let Some(id) = self.settings_window {
            return window::gain_focus(id);
        }

        // Prefer the engine's account (what is actually registered); fall back
        // to the config, so a failed start still opens an editable form rather
        // than a blank one the user cannot fix.
        let account = self
            .engine
            .as_ref()
            .map(|engine| engine.account().clone())
            .or_else(|| self.config.accounts.first().cloned())
            .unwrap_or_default();
        self.settings.load_account(&account);
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

    /// Opens the contacts window, or focuses it if already open.
    fn open_contacts(&mut self) -> Task<Message> {
        if let Some(id) = self.contacts_window {
            return window::gain_focus(id);
        }

        let (id, open) = window::open(crate::contacts_window_settings());
        self.contacts_window = Some(id);

        let sync_mgr = self.sync_manager.clone();
        let load_contacts = Task::future(async move {
            let contacts = sync_mgr.sync_contacts().await;
            Message::Contacts(contacts::Message::ContactsLoaded(contacts))
        });

        self.contacts.loading = true;
        Task::batch([open.map(Message::ContactsOpened), load_contacts])
    }

    fn on_contacts(&mut self, msg: contacts::Message) -> Task<Message> {
        match msg {
            contacts::Message::SearchChanged(val) => {
                self.contacts.search = val;
                Task::none()
            }
            contacts::Message::SyncPressed => {
                self.contacts.loading = true;
                let sync_mgr = self.sync_manager.clone();
                Task::future(async move {
                    let contacts = sync_mgr.sync_contacts().await;
                    Message::Contacts(contacts::Message::ContactsLoaded(contacts))
                })
            }
            contacts::Message::ContactsLoaded(contacts) => {
                self.contacts.contacts = contacts;
                self.contacts.loading = false;
                Task::none()
            }
            contacts::Message::DialContact(target) => {
                self.dial_number = target;
                let dial_task = self.dial();
                if let Some(id) = self.main_window {
                    Task::batch([window::gain_focus(id), dial_task])
                } else {
                    dial_task
                }
            }
        }
    }

    /// Opens the call history window, or focuses it if already open.
    fn open_calls(&mut self) -> Task<Message> {
        if let Some(id) = self.calls_window {
            return window::gain_focus(id);
        }

        let (id, open) = window::open(crate::calls_window_settings());
        self.calls_window = Some(id);

        let sync_mgr = self.sync_manager.clone();
        let load_calls = Task::future(async move {
            let calls = sync_mgr.sync_calls().await;
            Message::Calls(calls::Message::CallsLoaded(calls))
        });

        self.calls.loading = true;
        Task::batch([open.map(Message::CallsOpened), load_calls])
    }

    fn on_calls(&mut self, msg: calls::Message) -> Task<Message> {
        match msg {
            calls::Message::SearchChanged(val) => {
                self.calls.search = val;
                Task::none()
            }
            calls::Message::SyncPressed => {
                self.calls.loading = true;
                let sync_mgr = self.sync_manager.clone();
                Task::future(async move {
                    let calls = sync_mgr.sync_calls().await;
                    Message::Calls(calls::Message::CallsLoaded(calls))
                })
            }
            calls::Message::CallsLoaded(calls) => {
                self.calls.calls = calls;
                self.calls.loading = false;
                Task::none()
            }
            calls::Message::DialNumber(target) => {
                self.dial_number = target;
                let dial_task = self.dial();
                if let Some(id) = self.main_window {
                    Task::batch([window::gain_focus(id), dial_task])
                } else {
                    dial_task
                }
            }
        }
    }

    fn on_settings(&mut self, msg: settings::Message) -> Task<Message> {
        use settings::Message as S;

        // Any edit clears the last confirmation so stale feedback is not shown
        // next to a field the user is still changing.
        self.settings.notice = None;

        match msg {
            S::Label(v) => self.settings.label = v,
            S::Registrar(v) => self.settings.registrar = v,
            S::Port(v) => self.settings.port = v,
            S::Username(v) => self.settings.username = v,
            S::AuthUser(v) => self.settings.auth_user = v,
            S::Password(v) => self.settings.password = v,
            S::Expires(v) => self.settings.expires = v,
            S::LocalPort(v) => self.settings.local_port = v,
            S::RevealPassword(v) => self.settings.reveal_password = v,

            S::RevertAccount => {
                let account = self
                    .engine
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

            S::Close => {
                if let Some(id) = self.settings_window.take() {
                    return window::close(id);
                }
            }
        }
        Task::none()
    }

    /// Commits the account draft: rebuild the engine, then save.
    fn apply_account(&mut self) -> Task<Message> {
        let transport = self
            .engine
            .as_ref()
            .map_or(sipster_core::Transport::Udp, |e| e.account().transport);

        let account = match self.settings.to_account(transport) {
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
        // they typed, and can fix them without retyping everything.
        self.config.accounts = vec![account.clone()];
        self.persist();

        // The bridge owns engine lifetime, so ask it to rebuild rather than
        // swapping the handle from under the running event loop.
        engine_bridge::reconfigure(account);
        Task::none()
    }

    /// Pushes the device selection into the running engine and saves it.
    fn apply_devices(&mut self) -> Task<Message> {
        self.config.audio.input = self.devices.input.clone();
        self.config.audio.output = self.devices.output.clone();
        self.persist();

        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        let selection = self.devices.clone();
        Task::future(async move {
            Message::ActionDone(engine.set_devices(selection).await.map_err(|e| e.to_string()))
        })
    }

    /// Writes the config file, reporting failures in the settings window.
    fn persist(&mut self) {
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

    /// Routes a control command, deferring it if it needs an engine we do not
    /// have yet.
    ///
    /// Only telephony commands need one. Show and Quit must run immediately:
    /// on first run there is no engine, and parking a Quit made an
    /// unconfigured instance impossible to stop with `sipster --quit`.
    fn on_ipc(&mut self, cmd: Command) -> Task<Message> {
        let needs_engine = !matches!(
            cmd,
            Command::Show
                | Command::Dial { .. }
                | Command::OpenSettings
                | Command::OpenContacts
                | Command::OpenCallList
                | Command::Quit
        );
        if needs_engine && self.engine.is_none() {
            self.pending_command = Some(cmd);
            return Task::none();
        }
        self.handle_ipc(cmd)
    }

    fn handle_ipc(&mut self, cmd: Command) -> Task<Message> {
        match cmd {
            Command::Call { target } => {
                self.dial_number = target;
                let dial_task = self.dial();
                if let Some(id) = self.main_window {
                    Task::batch([window::gain_focus(id), dial_task])
                } else {
                    dial_task
                }
            }
            Command::Dial { target } => {
                self.dial_number = target;
                if let Some(id) = self.main_window {
                    window::gain_focus(id)
                } else {
                    Task::none()
                }
            }
            Command::Answer => self.answer(),
            Command::Hangup => {
                if self.incoming.is_some() {
                    self.decline()
                } else {
                    self.hangup()
                }
            }
            Command::Show => {
                if let Some(id) = self.main_window {
                    window::gain_focus(id)
                } else {
                    Task::none()
                }
            }
            Command::OpenSettings => self.open_settings(),
            Command::OpenContacts => self.open_contacts(),
            Command::OpenCallList => self.open_calls(),
            Command::Quit => iced::exit(),
        }
    }

    fn handle_tray(&mut self, req: tray::Request) -> Task<Message> {
        match req {
            tray::Request::Show => {
                if let Some(id) = self.main_window {
                    window::gain_focus(id)
                } else {
                    Task::none()
                }
            }
            tray::Request::OpenSettings => self.open_settings(),
            tray::Request::OpenCallList => self.open_calls(),
            tray::Request::OpenContacts => self.open_contacts(),
            tray::Request::Answer => self.answer(),
            tray::Request::Hangup => {
                if self.incoming.is_some() {
                    self.decline()
                } else {
                    self.hangup()
                }
            }
            tray::Request::Quit => iced::exit(),
        }
    }

    fn on_call_event(&mut self, event: CallEvent) -> Task<Message> {
        match event {
            CallEvent::Registration(state) => {
                self.status = registration_status(&state);
                self.registration = state;
            }
            CallEvent::IncomingCall { id, remote_uri, .. } => {
                if self.config.ui.notifications {
                    sound::notify_incoming(&remote_uri);
                }
                // Record incoming call in local history
                self.sync_manager.record_local_call(CallRecord {
                    id: format!("local-in-{id}"),
                    call_type: CallType::Incoming,
                    remote_number: remote_uri.clone(),
                    remote_name: None,
                    local_party: self.account_info.clone(),
                    timestamp: chrono_now_iso(),
                    duration_seconds: 0,
                    source: RecordSource::Local,
                });

                // Assigning drops any previous ringtone, so a second inbound
                // call cannot leave two rings overlapping.
                self.ringtone = self.config.ui.ringtone.then(sound::start_ringing);
                self.incoming = Some(IncomingCall { id, remote: remote_uri });
                self.status = "Incoming call…".into();
            }
            CallEvent::StateChanged { id, state } => {
                self.apply_state(id, state);
            }
            CallEvent::Terminated { id, reason } => {
                if let Some(active) = self.active.take().filter(|c| c.id == id) {
                    self.chime(sound::call_ended);
                    // Update/record local termination if desired
                    self.sync_manager.record_local_call(CallRecord {
                        id: format!("local-term-{id}"),
                        call_type: CallType::Outgoing,
                        remote_number: active.remote,
                        remote_name: None,
                        local_party: self.account_info.clone(),
                        timestamp: chrono_now_iso(),
                        duration_seconds: 0,
                        source: RecordSource::Local,
                    });
                }
                if let Some(incoming) = self.incoming.take().filter(|c| c.id == id) {
                    self.ringtone = None;
                    self.sync_manager.record_local_call(CallRecord {
                        id: format!("local-missed-{id}"),
                        call_type: CallType::Missed,
                        remote_number: incoming.remote,
                        remote_name: None,
                        local_party: self.account_info.clone(),
                        timestamp: chrono_now_iso(),
                        duration_seconds: 0,
                        source: RecordSource::Local,
                    });
                }
                self.status = format!("Call ended: {reason}");
            }
        }
        self.sync_tray_state();
        Task::none()
    }
    /// Plays a call chime, unless the user turned chimes off.
    fn chime(&self, play: fn()) {
        if self.config.ui.call_chimes {
            play();
        }
    }

    fn sync_tray_state(&self) {
        let Some(tray) = &self.tray else { return };
        let state = if self.incoming.is_some() {
            tray::CallState::Ringing
        } else if self.active.is_some() {
            tray::CallState::InCall
        } else {
            tray::CallState::Idle
        };
        tray.set_call_state(state);
    }
    fn apply_state(&mut self, id: CallId, state: CallState) {
        let remote = self
            .active
            .as_ref()
            .filter(|c| c.id == id)
            .map(|c| c.remote.clone())
            .or_else(|| self.incoming.as_ref().filter(|c| c.id == id).map(|c| c.remote.clone()))
            .unwrap_or_else(|| self.dial_number.clone());
        self.active = Some(ActiveCall { id, state, remote });
        self.status = call_status(state);
    }

    fn dial(&mut self) -> Task<Message> {
        let (Some(engine), false) = (&self.engine, self.dial_number.is_empty()) else {
            return Task::none();
        };
        self.chime(sound::call_started);
        let engine = engine.clone();
        let target = self.dial_number.clone();

        self.sync_manager.record_local_call(CallRecord {
            id: format!("local-out-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()),
            call_type: CallType::Outgoing,
            remote_number: target.clone(),
            remote_name: None,
            local_party: self.account_info.clone(),
            timestamp: chrono_now_iso(),
            duration_seconds: 0,
            source: RecordSource::Local,
        });

        self.status = format!("Dialing {target}…");
        Task::future(async move { Message::Dialed(engine.dial(&target).await.map_err(|e| e.to_string())) })
    }

    fn hangup(&mut self) -> Task<Message> {
        let (Some(engine), Some(call)) = (&self.engine, self.active.take()) else {
            return Task::none();
        };
        self.chime(sound::call_ended);
        let engine = engine.clone();
        let id = call.id;
        self.status = "Hanging up…".into();
        self.sync_tray_state();
        Task::future(async move { Message::ActionDone(engine.hangup(id).await.map_err(|e| e.to_string())) })
    }

    fn answer(&mut self) -> Task<Message> {
        let (Some(engine), Some(call)) = (&self.engine, self.incoming.take()) else {
            return Task::none();
        };
        self.ringtone = None;
        let (engine, id) = (engine.clone(), call.id);
        self.status = "Answering call…".into();
        self.active = Some(ActiveCall {
            id,
            state: CallState::Active,
            remote: call.remote,
        });
        Task::future(async move { Message::ActionDone(engine.answer(id).await.map_err(|e| e.to_string())) })
    }

    fn decline(&mut self) -> Task<Message> {
        let (Some(engine), Some(call)) = (&self.engine, self.incoming.take()) else {
            return Task::none();
        };
        self.ringtone = None;
        let (engine, id) = (engine.clone(), call.id);
        self.status = "Call declined".into();
        Task::future(async move { Message::ActionDone(engine.hangup(id).await.map_err(|e| e.to_string())) })
    }

    fn on_dial_input_changed(&mut self, input: String) {
        if self.config.ui.dtmf_feedback && input.len() > self.dial_number.len() {
            if let Some(ch) = input.chars().last() {
                sound::dtmf(ch);
            }
        }
        self.dial_number = input;
    }
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
