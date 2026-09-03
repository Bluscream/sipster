//! Application state, message handling, and the engine subscription.
//!
//! All telephony work is delegated to [`SipEngine`]; this module only tracks
//! what to display and turns button presses into engine calls.

use iced::{Subscription, Task, Theme};
use sipster_core::ipc::Command;
use sipster_core::{CallEvent, CallId, CallState, RegistrationState};

use crate::engine_bridge::{self, EngineHandle};
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
    // Async results:
    Dialed(Result<CallId, String>),
    ActionDone(Result<(), String>),
}

impl SipsterApp {
    pub fn new() -> (Self, Task<Message>) {
        let app = Self {
            engine: None,
            pending_command: None,
            registration: RegistrationState::Unregistered,
            account_info: None,
            dial_number: String::new(),
            status: "Ready".into(),
            active: None,
            incoming: None,
            tray: crate::take_tray(),
            ringtone: None,
        };
        (app, Task::none())
    }

    // Signature is dictated by iced::application(..).subscription(..).
    #[allow(clippy::unused_self)]
    pub fn subscription(&self) -> Subscription<Message> {
        // engine_bridge::run is a fn()-pointer; it grabs the IPC receiver
        // from the process-global OnceLock in main.rs exactly once.
        // Subsequent subscription calls get None — the stream keeps running.
        let engine_sub = Subscription::run(engine_bridge::run);
        // Poll the tray channel every 100 ms.
        let tray_sub = iced::time::every(std::time::Duration::from_millis(100))
            .map(|_| Message::TrayTick);
        Subscription::batch([engine_sub, tray_sub])
    }

    // Signature is dictated by iced::application(..).theme(..).
    #[allow(clippy::unused_self)]
    pub fn theme(&self) -> Theme {
        Theme::Dark
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
            Message::Ipc(cmd) => {
                if self.engine.is_none() {
                    self.pending_command = Some(cmd);
                    Task::none()
                } else {
                    self.handle_ipc(cmd)
                }
            }
            Message::TrayTick => {
                // Drain one pending tray request per tick (non-blocking).
                if let Some(req) = self.tray.as_ref().and_then(crate::tray::Handle::poll) {
                    return self.handle_tray(req);
                }
                Task::none()
            }
            Message::DialInputChanged(v) => {
                self.dial_number = v;
                Task::none()
            }
            Message::DialPad(d) => {
                self.dial_number.push(d);
                sound::dtmf(d);
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
            Message::ContactsPressed => {
                self.status = "Contacts sync (TR-064 / KDE) planned".into();
                Task::none()
            }
            Message::CallListPressed => {
                self.status = "Call list sync planned".into();
                Task::none()
            }
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

    pub fn view(&self) -> iced::Element<'_, Message> {
        view::root(self)
    }

    fn handle_ipc(&mut self, cmd: Command) -> Task<Message> {
        match cmd {
            Command::Call { target } => {
                self.dial_number = target;
                self.dial()
            }
            Command::Answer => self.answer(),
            Command::Hangup => {
                if self.incoming.is_some() {
                    self.decline()
                } else {
                    self.hangup()
                }
            }
            Command::Show => Task::none(),
            Command::Quit => iced::exit(),
        }
    }

    fn handle_tray(&mut self, req: tray::Request) -> Task<Message> {
        match req {
            tray::Request::Show => Task::none(), // window focus handled by OS/compositor
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
                sound::notify_incoming(&remote_uri);
                // Assigning drops any previous ringtone, so a second inbound
                // call cannot leave two rings overlapping.
                self.ringtone = Some(sound::start_ringing());
                self.incoming = Some(IncomingCall { id, remote: remote_uri });
                self.status = "Incoming call…".into();
            }
            CallEvent::StateChanged { id, state } => {
                self.apply_state(id, state);
            }
            CallEvent::Terminated { id, reason } => {
                if self.active.as_ref().is_some_and(|c| c.id == id) {
                    self.active = None;
                    sound::call_ended();
                }
                if self.incoming.as_ref().is_some_and(|c| c.id == id) {
                    self.incoming = None;
                    self.ringtone = None;
                }
                self.status = format!("Call ended: {reason}");
            }
        }
        self.sync_tray_state();
        Task::none()
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
        sound::call_started();
        let engine = engine.clone();
        let target = self.dial_number.clone();
        self.status = format!("Dialing {target}…");
        Task::future(async move { Message::Dialed(engine.dial(&target).await.map_err(|e| e.to_string())) })
    }

    fn hangup(&mut self) -> Task<Message> {
        let (Some(engine), Some(call)) = (&self.engine, self.active.take()) else {
            return Task::none();
        };
        sound::call_ended();
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
