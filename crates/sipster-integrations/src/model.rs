//! Data models and abstractions for contacts and call logs.

use serde::{Deserialize, Serialize};

/// Type of a phone number (e.g. mobile, work, home, internal extension).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NumberType {
    Home,
    Work,
    Mobile,
    Fax,
    Intern,
    Vanity,
    Other(String),
}

impl std::fmt::Display for NumberType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Home => write!(f, "Home"),
            Self::Work => write!(f, "Work"),
            Self::Mobile => write!(f, "Mobile"),
            Self::Fax => write!(f, "Fax"),
            Self::Intern => write!(f, "Internal"),
            Self::Vanity => write!(f, "Vanity"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

/// A specific phone number attached to a contact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneNumber {
    pub number: String,
    pub number_type: NumberType,
    pub priority: u8,
}

/// Source that provided the contact or call record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordSource {
    Local,
    FritzBox { phonebook_id: u32, phonebook_name: String },
    CardDav { account: String },
    Google { email: String },
    Other(String),
}

impl std::fmt::Display for RecordSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => write!(f, "Local"),
            Self::FritzBox { phonebook_name, .. } => write!(f, "FRITZ!Box ({phonebook_name})"),
            Self::CardDav { account } => write!(f, "CardDAV ({account})"),
            Self::Google { email } => write!(f, "Google ({email})"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

/// A unified contact entry merged from local storage, FRITZ!Box, or `CardDAV`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    /// Unique identifier across sync (e.g. `fritzbox-0-42` or `local-uuid`).
    pub id: String,
    /// Display or full name.
    pub name: String,
    /// Phone numbers associated with this contact.
    pub numbers: Vec<PhoneNumber>,
    /// Optional email addresses.
    pub emails: Vec<String>,
    /// Provider source.
    pub source: RecordSource,
}

impl Contact {
    /// Returns the primary number to dial for this contact.
    pub fn primary_number(&self) -> Option<&str> {
        self.numbers
            .iter()
            .min_by_key(|n| n.priority)
            .or_else(|| self.numbers.first())
            .map(|n| n.number.as_str())
    }
}

/// Direction / outcome of a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallType {
    Incoming,
    Missed,
    Outgoing,
    Rejected,
}

impl std::fmt::Display for CallType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Incoming => write!(f, "Incoming"),
            Self::Missed => write!(f, "Missed"),
            Self::Outgoing => write!(f, "Outgoing"),
            Self::Rejected => write!(f, "Rejected"),
        }
    }
}

/// A unified call record from local in-app activity or router call history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallRecord {
    /// Unique record ID.
    pub id: String,
    /// Type of call.
    pub call_type: CallType,
    /// Remote party phone number or SIP URI.
    pub remote_number: String,
    /// Resolved contact or caller name, if known.
    pub remote_name: Option<String>,
    /// Local extension or device name (e.g. "**620", "FRITZ!Fon", "Sipster").
    pub local_party: Option<String>,
    /// ISO-8601 formatted date/time string or human date.
    pub timestamp: String,
    /// Call duration string (e.g. "0:42" or "1:15:00") or seconds.
    pub duration_seconds: u32,
    /// Source of this call history record.
    pub source: RecordSource,
}
