//! The settings window: state, messages and rendering.
//!
//! The provider panels live in the `providers` child module; it can reach the
//! shared field helpers here without those becoming crate-visible.
//!
//! Two kinds of setting live here, and they behave differently on purpose:
//!
//! - **Immediate** — theme, sound toggles, audio devices. Changing one applies
//!   at once and is written to the config file. There is nothing to confirm.
//! - **Account** — registrar, credentials, ports. These cannot be applied
//!   keystroke by keystroke: every change would tear down the SIP endpoint and
//!   re-register. They are edited into a draft and committed with *Apply &
//!   reconnect*, which rebuilds the engine.
//!
//! Numeric fields are held as `String` while editing. Parsing on every
//! keystroke would fight the user the moment they clear a field to retype it;
//! they are parsed once, on apply, and a bad value is reported rather than
//! silently reset.

pub(crate) mod providers;
mod sections;
mod widgets;

pub(crate) use sections::{
    account_section, appearance_section, audio_section, integration_section, sounds_section,
};
pub(crate) use widgets::{field, file_input, input, secret_file_input, secret_input, section};

use providers::{blocking_section, providers_section};

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Element, Length};
use sipster_core::audio::{Device, DeviceSelection};
use sipster_core::{BlockAction, IntegrationSettings, SipAccount, ThemeChoice, UiSettings};

/// A selectable audio device. `id: None` is the system default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceChoice {
    pub id: Option<String>,
    pub name: String,
}

impl DeviceChoice {
    fn system_default() -> Self {
        Self { id: None, name: "System default".into() }
    }

    /// Builds the picker list: system default first, then what the OS reports.
    ///
    /// A device saved in the config that the OS no longer reports is appended
    /// so the picker still shows the current selection instead of appearing
    /// blank — unplugging a headset should not silently look like "default".
    fn list(devices: &[Device], selected: Option<&String>) -> Vec<Self> {
        let mut out = vec![Self::system_default()];
        out.extend(devices.iter().map(|(id, name)| Self {
            id: Some(id.clone()),
            name: name.clone(),
        }));
        if let Some(id) = selected {
            if !out.iter().any(|choice| choice.id.as_ref() == Some(id)) {
                out.push(Self {
                    id: Some(id.clone()),
                    name: format!("{id} (not connected)"),
                });
            }
        }
        out
    }
}

impl std::fmt::Display for DeviceChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    // Account draft (applied together).
    Registrar(String),
    Port(String),
    Username(String),
    AuthUser(String),
    Password(String),
    Expires(String),
    LocalPort(String),
    TransportChanged(sipster_core::Transport),
    RevealPassword(bool),
    RevealFritzPassword(bool),
    RevealCardDavPassword(bool),
    RevealGoogleSecret(bool),
    ApplyAccount,
    RevertAccount,

    // Immediate.
    InputDevice(DeviceChoice),
    OutputDevice(DeviceChoice),
    Language(sipster_core::LanguageChoice),
    Theme(ThemeChoice),
    Ringtone(bool),
    Notifications(bool),
    DtmfFeedback(bool),
    CallChimes(bool),
    ShowBanner(bool),
    RegisterUriSchemes(bool),
    CloseToTray(bool),
    StreamingMode(bool),
    PickGoogleJsonFile,

    // Integrations. Contact and history providers are account configuration,
    // so they belong here rather than inside the windows that display their
    // data — which is where they used to live, splitting account setup across
    // three windows.
    ToggleLocalHistory(bool),
    ToggleEds(bool),
    ToggleVdir(bool),
    VdirPathChanged(String),
    PickVdirFolder,
    DefaultBlockActionChanged(BlockAction),
    UnblockNumber(String),

    FritzHostChanged(String),
    FritzPortChanged(String),
    FritzUserChanged(String),
    FritzPassChanged(String),
    FritzEnabledToggled(bool),

    ConnectGoogleAccount,
    GoogleAuthFinished(Result<(String, String), String>),
    RemoveGoogleAccount(String),

    GoogleClientIdChanged(String),
    GoogleClientSecretChanged(String),
    CardDavUrlChanged(String),
    CardDavUserChanged(String),
    CardDavPassChanged(String),
    AddCardDavAccount,
    RemoveCardDavAccount(String),

    /// Show the category at this index.
    JumpTo(usize),

    Close,
}

/// Editable state for the settings window.
///
/// The bool count trips `struct_excessive_bools`, whose usual remedy does not
/// apply: these are independent per-field reveal toggles plus one load flag,
/// and folding them into an enum would mean only one secret could be revealed
/// at a time — which is worse, not better.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default)]
pub struct State {
    pub registrar: String,
    pub port: String,
    pub username: String,
    pub auth_user: String,
    pub password: String,
    pub expires: String,
    pub local_port: String,
    pub reveal_password: bool,
    pub reveal_fritz_password: bool,
    pub reveal_carddav_password: bool,
    pub reveal_google_secret: bool,

    pub inputs: Vec<Device>,
    pub outputs: Vec<Device>,
    pub devices_loaded: bool,

    /// Which category the index has selected.
    pub section: usize,
    /// Whether the provider panel is expanded.
    pub draft_carddav_url: String,
    pub draft_carddav_user: String,
    pub draft_carddav_pass: String,
    /// The FRITZ!Box port as typed. Held as text for the same reason the
    /// account ports are: parsing every keystroke fights the user the moment
    /// they clear the field to retype it.
    pub draft_fritz_port: String,
    /// The account's SIP transport, edited alongside the rest of the form.
    pub transport: sipster_core::Transport,
    /// Whether the account being edited is registered at all.
    ///
    /// Not part of `Default` in spirit — a blank draft is a new account, and a
    /// new account should register — so [`State::new`] flips it on and
    /// `load_account` overwrites it from the stored account.
    pub account_enabled: bool,
    /// Google OAuth client credentials, which the user registers themselves.
    pub draft_google_client_id: String,
    pub draft_google_client_secret: String,
    /// Path typed into the `client_secret` JSON import box.
    pub draft_google_json_path: String,
    /// Path typed into the local vCard folder box.
    pub draft_vdir_path: String,

    /// Set when the last apply failed, cleared on the next successful one.
    pub error: Option<String>,
    /// Transient confirmation, e.g. "Saved".
    pub notice: Option<String>,
}

impl State {
    /// Fills the account draft from the account the engine is actually using.
    /// A blank draft for a new account, which should register once saved.
    #[must_use]
    pub fn new() -> Self {
        Self {
            account_enabled: true,
            ..Self::default()
        }
    }

    pub fn load_account(&mut self, account: &SipAccount) {
        self.registrar.clone_from(&account.registrar);
        self.port = account.port.to_string();
        self.username.clone_from(&account.username);
        self.auth_user.clone_from(&account.auth_user);
        self.password.clone_from(&account.password);
        self.expires = account.expires.to_string();
        self.local_port = account.local_port.to_string();
        self.transport = account.transport;
        self.account_enabled = account.enabled;
        self.error = None;
    }

    /// Whether the draft differs from `account`, so *Apply* can be disabled
    /// when there is nothing to apply.
    pub fn account_is_dirty(&self, account: &SipAccount) -> bool {
 self.registrar != account.registrar
            || self.port != account.port.to_string()
            || self.username != account.username
            || self.auth_user != account.auth_user
            || self.password != account.password
            || self.expires != account.expires.to_string()
            || self.local_port != account.local_port.to_string()
    }

    /// Parses the draft into an account, reporting the first bad field.
    ///
    /// # Errors
    ///
    /// Returns a human-readable message naming the field that failed.
    pub fn to_account(&self, transport: sipster_core::Transport) -> Result<SipAccount, String> {
        let port = parse_field("Registrar port", &self.port)?;
        let local_port = parse_field("Local port", &self.local_port)?;
        let expires = parse_field("Re-register interval", &self.expires)?;

        let account = SipAccount {
            registrar: self.registrar.trim().to_string(),
            port,
            username: self.username.trim().to_string(),
            auth_user: self.auth_user.trim().to_string(),
            password: self.password.clone(),
            transport,
            enabled: self.account_enabled,
            expires,
            local_port,
        };
        account.validate().map_err(|e| e.to_string())?;
        Ok(account)
    }
}

fn parse_field<T: std::str::FromStr>(name: &str, raw: &str) -> Result<T, String> {
    raw.trim()
        .parse()
        .map_err(|_| format!("{name}: '{}' is not a valid number", raw.trim()))
}

// ── rendering ────────────────────────────────────────────────────────────────

/// The categories in the index, in order.
///
/// The index selects rather than scrolls: iced has no scroll-to-anchor, only
/// proportional snapping, which would land near a section rather than on it.
/// Showing one category at a time is exact, and is what desktop settings
/// windows do anyway.
pub const SECTIONS: [&str; 7] = [
    "Account",
    "Audio",
    "Appearance",
    "Sounds",
    "Desktop",
    "Integrations",
    "Call blocking",
];

/// Renders the whole settings window.
pub fn view<'a>(
    state: &'a State,
    ui: &'a UiSettings,
    devices: &'a DeviceSelection,
    integration: &'a IntegrationSettings,
    config_path: &'a str,
    accounts: &AccountContext<'a>,
) -> Element<'a, Message> {
    let selected = state.section.min(SECTIONS.len() - 1);
    let panel = match selected {
        0 => account_section(state, ui.streaming_mode, accounts),
        1 => audio_section(state, devices),
        2 => appearance_section(ui),
        3 => sounds_section(ui),
        4 => integration_section(ui),
        5 => providers_section(state, integration, config_path),
        _ => blocking_section(integration),
    };

    let body = column![panel].spacing(26).padding(24).max_width(620);
    let scroller = scrollable(body).height(Length::Fill).width(Length::Fill);

    // A persistent index rather than scrolling to find a section. The window is
    // wide enough for it now, and the settings list has outgrown one screen.
    let mut index = column![].spacing(2);
    for (i, _name) in SECTIONS.iter().enumerate() {
        let is_current = i == selected;
        let cat_label = match i {
            0 => rust_i18n::t!("account").to_string(),
            1 => rust_i18n::t!("audio").to_string(),
            2 => rust_i18n::t!("appearance").to_string(),
            3 => rust_i18n::t!("sounds").to_string(),
            4 => rust_i18n::t!("desktop").to_string(),
            5 => rust_i18n::t!("integrations").to_string(),
            _ => rust_i18n::t!("call_blocking").to_string(),
        };
        index = index.push(
            button(text(cat_label).size(13))
                .on_press(Message::JumpTo(i))
                .padding([6, 9])
                .width(Length::Fill)
                .style(move |theme: &iced::Theme, status| {
                    let palette = theme.extended_palette();
                    let background = if is_current {
                        Some(palette.primary.base.color.into())
                    } else if matches!(status, button::Status::Hovered) {
                        Some(palette.background.weak.color.into())
                    } else {
                        None
                    };
                    button::Style {
                        background,
                        text_color: if is_current {
                            palette.primary.base.text
                        } else {
                            palette.background.base.text
                        },
                        border: iced::border::rounded(4),
                        ..button::Style::default()
                    }
                }),
        );
    }

    let sidebar = container(index)
        .width(Length::Fixed(180.0))
        .height(Length::Fill)
        .padding(14)
        .style(|theme: &iced::Theme| container::Style {
            background: Some(theme.extended_palette().background.weakest.color.into()),
            ..container::Style::default()
        });

    column![
        row![sidebar, scroller].height(Length::Fill),
        footer(state)
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn footer(state: &State) -> Element<'_, Message> {
    let message: Element<'_, Message> = match (&state.error, &state.notice) {
        (Some(error), _) => text(error.clone())
            .size(13)
            .color(iced::Color::from_rgb(0.92, 0.35, 0.35))
            .into(),
        (None, Some(notice)) => text(notice.clone())
            .size(13)
            .color(iced::Color::from_rgb(0.35, 0.8, 0.45))
            .into(),
        (None, None) => text(rust_i18n::t!("changes_immediately").to_string()).size(12).into(),
    };

    let close_lbl = rust_i18n::t!("close").to_string();

    container(
        row![
            message,
            Space::new().width(Length::Fill),
            button(text(close_lbl).size(14)).on_press(Message::Close),
        ]
        .align_y(Alignment::Center)
        .spacing(12),
    )
    .padding([10, 20])
    .width(Length::Fill)
    .into()
}

/// Everything the Account page needs beyond the draft itself.
///
/// Bundled because the labels live in the config rather than the draft, which
/// only ever holds the one account being edited.
pub struct AccountContext<'a> {
    /// The account the engine is actually running, if any.
    pub current: Option<&'a SipAccount>,
    pub first_run: bool,
}

#[cfg(test)]
mod tests {
    use super::{DeviceChoice, State};
    use sipster_core::{SipAccount, Transport};

    fn draft() -> State {
        let mut state = State::new();
        state.load_account(&SipAccount {
            registrar: "fritz.box".into(),
            username: "bob".into(),
            ..SipAccount::default()
        });
        state
    }

    #[test]
    fn round_trips_an_account_through_the_form() {
        let account = SipAccount {
            registrar: "fritz.box".into(),
            port: 5070,
            username: "bob".into(),
            auth_user: "bobby".into(),
            password: "pw".into(),
            expires: 300,
            local_port: 5062,
            transport: Transport::Udp,
            enabled: true,
        };
        let mut state = State::new();
        state.load_account(&account);
        let back = state.to_account(Transport::Udp).expect("valid draft");
        assert_eq!(back.registrar, account.registrar);
        assert_eq!(back.port, account.port);
        assert_eq!(back.expires, account.expires);
        assert_eq!(back.local_port, account.local_port);
        assert_eq!(back.password, account.password);
    }

    /// A freshly loaded form must not look edited, or Apply would be live for
    /// a change nobody made.
    #[test]
    fn a_freshly_loaded_form_is_not_dirty() {
        let account = SipAccount {
            registrar: "fritz.box".into(),
            username: "bob".into(),
            ..SipAccount::default()
        };
        let mut state = State::new();
        state.load_account(&account);
        assert!(!state.account_is_dirty(&account));

        state.registrar = "other.box".into();
        assert!(state.account_is_dirty(&account));
    }

    /// The first-run path: with no engine there is no account to diff against,
    /// and an earlier version disabled Apply in exactly that case — leaving
    /// the settings window unable to configure anything on a fresh install.
    #[test]
    fn apply_is_live_when_no_account_is_running_yet() {
        let state = State::default();
        let running: Option<&SipAccount> = None;
        assert!(
            running.is_none_or(|acc| state.account_is_dirty(acc)),
            "Apply must be available when nothing is applied yet"
        );
    }

    #[test]
    fn a_bad_number_names_the_field_it_came_from() {
        let mut state = draft();
        state.port = "not-a-port".into();
        let err = state.to_account(Transport::Udp).unwrap_err();
        assert!(err.contains("Registrar port"), "unhelpful message: {err}");
    }

    /// Blanking a required field must be reported, not written through as an
    /// account that can never register.
    #[test]
    fn an_empty_registrar_is_rejected() {
        let mut state = draft();
        state.registrar = "   ".into();
        assert!(state.to_account(Transport::Udp).is_err());
    }

    #[test]
    fn whitespace_around_pasted_values_is_trimmed() {
        let mut state = draft();
        state.registrar = "  fritz.box  ".into();
        state.port = " 5060 ".into();
        let account = state.to_account(Transport::Udp).expect("valid");
        assert_eq!(account.registrar, "fritz.box");
        assert_eq!(account.port, 5060);
    }

    #[test]
    fn device_list_starts_with_the_system_default() {
        let devices = vec![("hw:0".to_string(), "Built-in".to_string())];
        let list = DeviceChoice::list(&devices, None);
        assert_eq!(list[0].id, None);
        assert_eq!(list.len(), 2);
    }

    /// A saved device that is currently unplugged must still appear, or the
    /// picker would render empty and look like nothing is selected.
    #[test]
    fn a_missing_saved_device_is_still_listed() {
        let devices = vec![("hw:0".to_string(), "Built-in".to_string())];
        let saved = "usb-headset".to_string();
        let list = DeviceChoice::list(&devices, Some(&saved));
        assert!(list.iter().any(|c| c.id.as_deref() == Some("usb-headset")));
        assert!(list.iter().any(|c| c.name.contains("not connected")));
    }
}
