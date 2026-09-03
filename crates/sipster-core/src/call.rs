use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallState {
    Idle,
    Dialing,
    Ringing,
    Active,
    Holding,
    Terminated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CallEvent {
    IncomingCall { id: CallId, remote_uri: String },
    Ringing { id: CallId },
    Connected { id: CallId },
    Terminated { id: CallId, reason: String },
    RegistrationSuccess,
    RegistrationFailed(String),
}
