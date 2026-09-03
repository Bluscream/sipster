use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Opaque identifier for a call, used to correlate UI actions and events.
///
/// This is Sipster's internal handle, distinct from the SIP `Call-ID` header
/// (which lives on the dialog inside the engine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CallId(pub Uuid);

impl CallId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for CallId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CallId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallDirection {
    Incoming,
    Outgoing,
}

/// Lifecycle of a single call. Transitions are driven by the engine; the UI
/// treats this as read-only display state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallState {
    /// Outgoing: INVITE sent, awaiting a provisional/final response.
    Dialing,
    /// A 180/183 was received (outgoing) or an INVITE arrived (incoming).
    Ringing,
    /// `200 OK` exchanged and `ACK`ed; media is flowing.
    Active,
    /// Terminated. The reason travels with [`CallEvent::Terminated`].
    Terminated,
}

/// Registration status for an account, surfaced to the UI status line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegistrationState {
    Unregistered,
    Registering,
    Registered,
    Failed(String),
}

/// Events emitted by the engine on its broadcast channel. The UI subscribes and
/// re-renders; nothing here carries engine-internal handles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallEvent {
    Registration(RegistrationState),
    IncomingCall { id: CallId, remote_uri: String, display_name: Option<String> },
    StateChanged { id: CallId, state: CallState },
    Terminated { id: CallId, reason: String },
}
