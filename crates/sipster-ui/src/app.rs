//! Application state, message handling, and the engine subscription.
//!
//! All telephony work is delegated to [`SipEngine`]; this module only tracks
//! what to display and turns button presses into engine calls.

use iced::{Subscription, Task, Theme};
use sipster_core::{CallEvent, CallId, CallState, RegistrationState};

use crate::engine_bridge::{self, EngineHandle};
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
}

#[derive(Debug, Clone)]
pub enum Message {
    // From the engine bridge:
    EngineReady(EngineHandle),
    EngineFailed(String),
    Call(CallEvent),
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
        };
        (app, Task::none())
    }

    // Signature is dictated by iced::application(..).subscription(..).
    #[allow(clippy::unused_self)]
    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::run(engine_bridge::run)
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
        Task::none()
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
