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

/// Extracts the dialable part of a SIP URI or raw number.
///
/// `"Alice" <sip:+49301234@fritz.box;user=phone>` becomes `+49301234`. Falls
/// back to the input when there is no URI structure, so a bare number passes
/// through unchanged.
#[must_use]
pub fn caller_number(remote: &str) -> &str {
    let raw = remote.trim();

    // Prefer the angle-bracketed URI when a display name is present, so the
    // name can never be matched against.
    let raw = raw
        .split_once('<')
        .map_or(raw, |(_, rest)| rest.split('>').next().unwrap_or(rest));

    // Strip the scheme, then take the user part before '@'. Everything after
    // is the host, which must never take part in matching: blocking "100" once
    // matched sip:alice@10.0.0.100.
    let without_scheme = raw
        .split_once(':')
        .filter(|(scheme, _)| matches!(*scheme, "sip" | "sips" | "tel" | "callto"))
        .map_or(raw, |(_, rest)| rest);

    let user = without_scheme
        .split('@')
        .next()
        .unwrap_or(without_scheme);

    // Drop URI parameters (`;user=phone`) that can trail the user part.
    user.split(';').next().unwrap_or(user).trim()
}

/// Reduces a number to comparable digits: `+49 (30) 12-34` becomes `+493012 34`
/// without the separators, i.e. `+493012 34` → `+4930 1234` → `+493 01234`.
///
/// Keeps a leading `+` so international and national forms stay distinct, and
/// keeps `*`/`#` because they are meaningful in extensions like `**610`.
#[must_use]
pub fn normalize_number(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '+' | '*' | '#'))
        .collect()
}

/// Digits a number must have before national/international suffix matching is
/// allowed. Below this, short internal extensions would collide.
const MIN_SIGNIFICANT: usize = 6;

/// Whether `remote` should be treated as the blocked entry `pattern`.
///
/// Matching is on the normalized *caller number* only, and is exact apart from
/// one deliberate allowance: a pattern without a country code matches a caller
/// that has one, so blocking `03012345` still catches `+493012345`.
///
/// An empty or non-numeric pattern never matches. That is not a detail — the
/// previous implementation used `remote_uri.contains(&pattern)`, so a blank
/// entry matched every string and silently rejected every inbound call.
#[must_use]
pub fn number_matches(remote: &str, pattern: &str) -> bool {
    let pattern = normalize_number(pattern);
    if pattern.is_empty() {
        return false;
    }
    let caller = normalize_number(caller_number(remote));
    if caller.is_empty() {
        return false;
    }
    if caller == pattern {
        return true;
    }

    // Compare national/international spellings by their trailing significant
    // digits, requiring enough of them that short extensions cannot collide.
    let trim_prefix = |n: &str| n.trim_start_matches('+').trim_start_matches('0').to_string();
    let (a, b) = (trim_prefix(&caller), trim_prefix(&pattern));
    if a.len() >= MIN_SIGNIFICANT && b.len() >= MIN_SIGNIFICANT {
        return a.ends_with(&b) || b.ends_with(&a);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{caller_number, normalize_number, number_matches};

    #[test]
    fn extracts_the_user_part_from_a_sip_uri() {
        assert_eq!(caller_number("sip:+49301234@fritz.box"), "+49301234");
        assert_eq!(caller_number("\"Alice\" <sip:611@fritz.box>"), "611");
        assert_eq!(caller_number("sip:611@fritz.box;user=phone"), "611");
        assert_eq!(caller_number("tel:+4930999"), "+4930999");
        // A bare number is already the answer.
        assert_eq!(caller_number("611"), "611");
    }

    /// The host must never take part in matching. Blocking "100" used to
    /// reject sip:alice@10.0.0.100 because the old check searched the whole URI.
    #[test]
    fn the_host_is_not_part_of_the_number() {
        assert!(!number_matches("sip:alice@10.0.0.100", "100"));
        assert!(!number_matches("sip:bob@100.example.com", "100"));
    }

    /// The old `contains` check made every longer number a match too.
    #[test]
    fn a_block_does_not_catch_longer_numbers() {
        assert!(number_matches("sip:100@fritz.box", "100"));
        assert!(!number_matches("sip:1001@fritz.box", "100"));
        assert!(!number_matches("sip:5100@fritz.box", "100"));
    }

    /// A blank entry previously matched everything, silently rejecting every
    /// inbound call.
    #[test]
    fn an_empty_pattern_never_matches() {
        assert!(!number_matches("sip:611@fritz.box", ""));
        assert!(!number_matches("sip:611@fritz.box", "   "));
        assert!(!number_matches("sip:611@fritz.box", "---"));
    }

    #[test]
    fn separators_and_spacing_are_ignored() {
        assert!(number_matches("sip:+493012345@fritz.box", "+49 (30) 123-45"));
        assert!(number_matches("sip:**610@fritz.box", "**610"));
    }

    #[test]
    fn national_and_international_forms_match() {
        assert!(number_matches("sip:+493012345@fritz.box", "03012345"));
        assert!(number_matches("sip:03012345@fritz.box", "+493012345"));
    }

    /// Short extensions must compare exactly; suffix matching them would make
    /// "610" block "5610".
    #[test]
    fn short_extensions_require_an_exact_match() {
        assert!(number_matches("sip:610@fritz.box", "610"));
        assert!(!number_matches("sip:5610@fritz.box", "610"));
    }

    #[test]
    fn normalization_keeps_meaningful_symbols() {
        assert_eq!(normalize_number("+49 (30) 12-34"), "+493012 34".replace(' ', ""));
        assert_eq!(normalize_number("**610"), "**610");
        assert_eq!(normalize_number("abc"), "");
    }
}
