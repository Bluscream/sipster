//! Turning engine values into the short strings the window shows.
//!
//! Nothing here decides anything; each function answers "how should this
//! read", which is why they are apart from the state that decides.

use sipster_core::{CallState, RegistrationState};

/// The dialable number from a SIP URI.
///
/// Local records used to store the whole `From` header —
/// `"Alice" <sip:611@fritz.box>;tag=179BED3B…` — as the number, which made
/// history unreadable and meant "Call back" dialled a string containing a
/// dialog tag.
pub(super) fn dialable(remote: &str) -> String {
    sipster_integrations::caller_number(remote).to_string()
}

/// The display name from a SIP URI, if it carries one worth showing.
pub(super) fn display_name(remote: &str) -> Option<String> {
    let raw = remote.trim();
    let name = raw.split_once('<').map(|(name, _)| name)?;
    let name = name.trim().trim_matches('"').trim();
    (!name.is_empty() && name != sipster_integrations::caller_number(raw))
        .then(|| name.to_string())
}

pub(super) fn registration_status(state: &RegistrationState) -> String {
    match state {
        RegistrationState::Unregistered => rust_i18n::t!("not_registered").into(),
        RegistrationState::Registering => rust_i18n::t!("registering").into(),
        RegistrationState::Registered => rust_i18n::t!("registered").into(),
        RegistrationState::Failed(e) => rust_i18n::t!("failed", error = e).into(),
    }
}

pub(super) fn call_status(state: CallState) -> String {
    match state {
        CallState::Dialing => rust_i18n::t!("dialing").into(),
        CallState::Ringing => rust_i18n::t!("ringing").into(),
        CallState::Active => rust_i18n::t!("active").into(),
        CallState::Terminated => rust_i18n::t!("terminated").into(),
    }
}

pub(super) fn chrono_now_iso() -> String {
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
