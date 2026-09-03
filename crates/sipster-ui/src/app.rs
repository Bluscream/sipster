//! Application state, message handling, and the engine subscription.
//!
//! All telephony work is delegated to [`SipEngine`]; this module only tracks
//! what to display and turns button presses into engine calls.

use iced::{Subscription, Task, Theme};
use sipster_core::ipc::Command;
use sipster_core::{CallEvent, CallId, CallState, RegistrationState};

use crate::engine_bridge::{self, EngineHandle};
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
    pub registration: RegistrationState,
    pub dial_number: String,
    pub status: String,
    pub active: Option<ActiveCall>,
    pub incoming: Option<IncomingCall>,
    tray: Option<tray::Handle>,
}

#[derive(Debug, Clone)]
pub enum Message {
    // From the engine bridge:
    EngineReady(EngineHandle),
    EngineFailed(String),
    Call(CallEvent),
    Ipc(Command),
    // From the tray:
    TrayRequest(tray::Request),
    // Periodic tray poll tick:
    TrayTick,
    // User intent:
    DialInputChanged(String),
    DialPad(char),
    Backspace,
    CallPressed,
    HangupPressed,
    AnswerPressed,
    DeclinePressed,
    // Async results:
    Dialed(Result<CallId, String>),
    ActionDone(Result<(), String>),
}

impl SipsterApp {
    pub fn new() -> (Self, Task<Message>) {
        let app = Self {
            engine: None,
            registration: RegistrationState::Unregistered,
            dial_number: String::new(),
            status: "Starting…".into(),
            active: None,
            incoming: None,
            tray: crate::take_tray(),
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
                self.engine = Some(engine);
                self.status = "Engine ready".into();
                Task::none()
            }
            Message::EngineFailed(err) => {
                self.status = format!("Engine error: {err}");
                Task::none()
            }
            Message::Call(event) => self.on_call_event(event),
            Message::Ipc(cmd) => self.handle_ipc(cmd),
            Message::TrayTick => {
                // Drain one pending tray request per tick (non-blocking).
                if let Some(req) = self.tray.as_ref().and_then(|t| t.requests.try_recv().ok()) {
                    return self.handle_tray(req);
                }
                Task::none()
            }
            Message::TrayRequest(req) => self.handle_tray(req),
            Message::DialInputChanged(v) => {
                self.dial_number = v;
                Task::none()
            }
            Message::DialPad(d) => {
                self.dial_number.push(d);
                Task::none()
            }
            Message::Backspace => {
                self.dial_number.pop();
                Task::none()
            }
            Message::CallPressed => self.dial(),
            Message::HangupPressed => self.hangup(),
            Message::AnswerPressed => self.answer(),
            Message::DeclinePressed => self.decline(),
            Message::Dialed(Err(e)) => {
                self.status = format!("Call failed: {e}");
                Task::none()
            }
            Message::ActionDone(Err(e)) => {
                self.status = format!("Error: {e}");
                Task::none()
            }
            Message::Dialed(Ok(_)) | Message::ActionDone(Ok(())) => Task::none(),
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
                self.incoming = Some(IncomingCall { id, remote: remote_uri.clone() });
                self.status = format!("Incoming call from {remote_uri}");
            }
            CallEvent::StateChanged { id, state } => {
                self.apply_state(id, state);
            }
            CallEvent::Terminated { id, reason } => {
                if self.active.as_ref().is_some_and(|c| c.id == id) {
                    self.active = None;
                }
                if self.incoming.as_ref().is_some_and(|c| c.id == id) {
                    self.incoming = None;
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
        let engine = engine.clone();
        let target = self.dial_number.clone();
        self.status = format!("Dialing {target}…");
        Task::future(async move { Message::Dialed(engine.dial(&target).await.map_err(|e| e.to_string())) })
    }

    fn hangup(&mut self) -> Task<Message> {
        let (Some(engine), Some(call)) = (&self.engine, &self.active) else {
            return Task::none();
        };
        let (engine, id) = (engine.clone(), call.id);
        Task::future(async move { Message::ActionDone(engine.hangup(id).await.map_err(|e| e.to_string())) })
    }

    fn answer(&mut self) -> Task<Message> {
        let (Some(engine), Some(call)) = (&self.engine, &self.incoming) else {
            return Task::none();
        };
        let (engine, id) = (engine.clone(), call.id);
        self.incoming = None;
        Task::future(async move { Message::ActionDone(engine.answer(id).await.map_err(|e| e.to_string())) })
    }

    fn decline(&mut self) -> Task<Message> {
        let (Some(engine), Some(call)) = (&self.engine, &self.incoming) else {
            return Task::none();
        };
        let (engine, id) = (engine.clone(), call.id);
        self.incoming = None;
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
        CallState::Holding => "On hold".into(),
        CallState::Terminated => "Call ended".into(),
    }
}
