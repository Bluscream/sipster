//! What the window looks like, and what it is willing to show.
//!
//! Includes [`mask_identity`], which is the whole of streaming mode: every
//! name and number on screen goes through it.

use serde::{Deserialize, Serialize};

/// Which colour theme the UI should use.
///
/// A closed set rather than a free string so an unreadable value cannot end up
/// in the file; the UI maps each to an `iced::Theme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    Dracula,
    Nord,
    SolarizedDark,
    GruvboxDark,
    CatppuccinMocha,
    TokyoNight,
}

impl ThemeChoice {
    /// Every choice, for populating a picker.
    pub const ALL: [Self; 8] = [
        Self::Dark,
        Self::Light,
        Self::Dracula,
        Self::Nord,
        Self::SolarizedDark,
        Self::GruvboxDark,
        Self::CatppuccinMocha,
        Self::TokyoNight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::Dracula => "Dracula",
            Self::Nord => "Nord",
            Self::SolarizedDark => "Solarized Dark",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::TokyoNight => "Tokyo Night",
        }
    }
}

impl std::fmt::Display for ThemeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LanguageChoice {
    #[default]
    English,
    German,
}

impl LanguageChoice {
    pub const ALL: [Self; 2] = [Self::English, Self::German];

    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::German => "de",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::German => "Deutsch",
        }
    }
}

impl std::fmt::Display for LanguageChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Presentation and local-feedback preferences. None of this reaches the wire.
///
/// The bool count trips `struct_excessive_bools`, whose usual remedy — folding
/// them into an enum or a state machine — does not apply: these are genuinely
/// independent on/off preferences, and each one is a checkbox in the settings
/// window and a self-describing key in the TOML file. Grouping them would make
/// both worse.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub language: LanguageChoice,
    pub theme: ThemeChoice,
    /// Ring the speaker while an inbound call is pending.
    pub ringtone: bool,
    /// Raise a desktop notification for an inbound call.
    pub notifications: bool,
    /// Local DTMF beep when a dialpad key is pressed. Not sent to the peer.
    pub dtmf_feedback: bool,
    /// Short chimes when a call starts and ends.
    pub call_chimes: bool,
    /// Show the wordmark above the dialpad.
    pub show_banner: bool,
    /// Register and set as default handler for tel:, sip:, sips:, callto:, and sipster: URI schemes.
    pub register_uri_schemes: bool,
    /// Keep running in the background when the dialer window is closed if a system tray icon is active.
    pub close_to_tray: bool,
    /// Mask names and numbers everywhere they are displayed, leaving only the
    /// first and last character. For screen sharing and recording.
    pub streaming_mode: bool,
    /// Timestamp of the newest missed call the user has already looked at.
    ///
    /// The badge on the History window's Missed filter counts only what is
    /// newer than this, so it reads as an unread marker rather than a running
    /// total. Persisted, because a badge that came back on every restart
    /// would be exactly the nag it is meant not to be.
    #[serde(default)]
    pub missed_seen_until: Option<String>,
    /// Contact sources the user switched off in the Filter dropdown, by
    /// display name.
    ///
    /// Held as the hidden ones rather than the shown ones so a source that
    /// appears later — a newly added Google account, a phonebook the router
    /// only just returned — is visible by default rather than silently
    /// filtered out.
    #[serde(default)]
    pub hidden_contact_sources: Vec<String>,
    /// The call-history filter, remembered as chosen.
    #[serde(default)]
    pub history_filter: String,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            language: LanguageChoice::default(),
            theme: ThemeChoice::default(),
            ringtone: true,
            notifications: true,
            dtmf_feedback: true,
            call_chimes: true,
            show_banner: true,
            register_uri_schemes: false,
            close_to_tray: true,
            streaming_mode: false,
            missed_seen_until: None,
            hidden_contact_sources: Vec::new(),
            history_filter: String::new(),
        }
    }
}

/// Masks a name or number for [`UiSettings::streaming_mode`].
///
/// Keeps the first and last character so entries stay tellable apart and the
/// layout keeps its shape, and hides everything between:
/// `Alice Smith` becomes `A…h`, `+49301234567` becomes `+…7`.
///
/// One- and two-character values are replaced outright rather than returned
/// as-is, since `A…A` would leak the whole thing.
#[must_use]
pub fn mask_identity(value: &str) -> String {
    let trimmed = value.trim();
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    match chars.next_back() {
        // 3 or more characters: keep both ends.
        Some(last) if trimmed.chars().count() > 2 => format!("{first}…{last}"),
        // 1-2 characters: nothing can be safely revealed.
        _ => "…".to_string(),
    }
}

#[cfg(test)]
mod mask_tests {
    use super::mask_identity;

    #[test]
    fn keeps_only_the_outer_characters() {
        assert_eq!(mask_identity("Alice Smith"), "A…h");
        assert_eq!(mask_identity("+49301234567"), "+…7");
        assert_eq!(mask_identity("**610"), "*…0");
    }

    /// Short values cannot keep both ends without revealing everything.
    #[test]
    fn very_short_values_reveal_nothing() {
        assert_eq!(mask_identity("ab"), "…");
        assert_eq!(mask_identity("a"), "…");
        assert_eq!(mask_identity(""), "");
    }

    /// Slicing by byte would panic or split a character in half.
    #[test]
    fn handles_multi_byte_characters() {
        assert_eq!(mask_identity("Müller"), "M…r");
        assert_eq!(mask_identity("日本語です"), "日…す");
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(mask_identity("  Alice  "), "A…e");
    }
}
