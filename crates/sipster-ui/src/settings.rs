//! The settings window: state, messages and rendering.
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

use iced::widget::{
    button, checkbox, column, container, pick_list, row, rule, scrollable, text, text_input, Space,
};
use iced::{Alignment, Element, Length};
use sipster_core::audio::{Device, DeviceSelection};
use sipster_core::{SipAccount, ThemeChoice, UiSettings};

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
    Label(String),
    Registrar(String),
    Port(String),
    Username(String),
    AuthUser(String),
    Password(String),
    Expires(String),
    LocalPort(String),
    RevealPassword(bool),
    ApplyAccount,
    RevertAccount,

    // Immediate.
    InputDevice(DeviceChoice),
    OutputDevice(DeviceChoice),
    Theme(ThemeChoice),
    Ringtone(bool),
    Notifications(bool),
    DtmfFeedback(bool),
    CallChimes(bool),
    ShowBanner(bool),
    RegisterUriSchemes(bool),
    CloseToTray(bool),

    Close,
}

/// Editable state for the settings window.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub label: String,
    pub registrar: String,
    pub port: String,
    pub username: String,
    pub auth_user: String,
    pub password: String,
    pub expires: String,
    pub local_port: String,
    pub reveal_password: bool,

    pub inputs: Vec<Device>,
    pub outputs: Vec<Device>,
    pub devices_loaded: bool,

    /// Set when the last apply failed, cleared on the next successful one.
    pub error: Option<String>,
    /// Transient confirmation, e.g. "Saved".
    pub notice: Option<String>,
}

impl State {
    /// Fills the account draft from the account the engine is actually using.
    pub fn load_account(&mut self, account: &SipAccount) {
        self.label.clone_from(&account.label);
        self.registrar.clone_from(&account.registrar);
        self.port = account.port.to_string();
        self.username.clone_from(&account.username);
        self.auth_user.clone_from(&account.auth_user);
        self.password.clone_from(&account.password);
        self.expires = account.expires.to_string();
        self.local_port = account.local_port.to_string();
        self.error = None;
    }

    /// Whether the draft differs from `account`, so *Apply* can be disabled
    /// when there is nothing to apply.
    pub fn account_is_dirty(&self, account: &SipAccount) -> bool {
        self.label != account.label
            || self.registrar != account.registrar
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
            label: self.label.trim().to_string(),
            registrar: self.registrar.trim().to_string(),
            port,
            username: self.username.trim().to_string(),
            auth_user: self.auth_user.trim().to_string(),
            password: self.password.clone(),
            transport,
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

/// Renders the whole settings window.
pub fn view<'a>(
    state: &'a State,
    ui: &'a UiSettings,
    devices: &'a DeviceSelection,
    account: Option<&'a SipAccount>,
    config_path: &'a str,
    first_run: bool,
) -> Element<'a, Message> {
    let body = column![
        account_section(state, account, first_run),
        audio_section(state, devices),
        appearance_section(ui),
        sounds_section(ui),
        integration_section(ui),
        about_section(config_path),
    ]
    .spacing(26)
    .padding(24)
    .max_width(560);

    let scroller = scrollable(body).height(Length::Fill).width(Length::Fill);

    column![scroller, footer(state)]
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
        (None, None) => text("Changes apply immediately unless noted.").size(12).into(),
    };

    container(
        row![
            message,
            Space::new().width(Length::Fill),
            button(text("Close").size(14)).on_press(Message::Close),
        ]
        .align_y(Alignment::Center)
        .spacing(12),
    )
    .padding([10, 20])
    .width(Length::Fill)
    .into()
}

fn section<'a>(
    title: &'a str,
    hint: Option<&'a str>,
    content: Element<'a, Message>,
) -> Element<'a, Message> {
    let mut header = column![text(title).size(17)].spacing(3);
    if let Some(hint) = hint {
        header = header.push(
            text(hint)
                .size(12)
                .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
        );
    }
    column![header, rule::horizontal(1), content]
        .spacing(10)
        .into()
}

/// A labelled row. The fixed label column keeps every field aligned.
fn field<'a>(label: &'a str, control: Element<'a, Message>) -> Element<'a, Message> {
    row![
        text(label).size(13).width(Length::Fixed(132.0)),
        container(control).width(Length::Fill),
    ]
    .align_y(Alignment::Center)
    .spacing(10)
    .into()
}

fn input<'a>(
    placeholder: &'a str,
    value: &'a str,
    on_change: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    text_input(placeholder, value)
        .on_input(on_change)
        .padding(7)
        .size(14)
        .into()
}

fn account_section<'a>(
    state: &'a State,
    account: Option<&'a SipAccount>,
    first_run: bool,
) -> Element<'a, Message> {
    let password = {
        let mut widget = text_input("", &state.password)
            .on_input(Message::Password)
            .padding(7)
            .size(14);
        if !state.reveal_password {
            widget = widget.secure(true);
        }
        widget
    };

    // `account` is the one the engine is actually running. When there is none
    // — first run, or a failed connect — nothing has been applied yet, so
    // whatever is in the form is worth applying and Apply must be live.
    // Comparing against a non-existent account would leave the button dead
    // exactly when it is the only way forward; validation on Apply reports
    // anything still missing.
    let dirty = account.is_none_or(|acc| state.account_is_dirty(acc));

    let mut apply = button(text("Apply & reconnect").size(14));
    let mut revert = button(text("Revert").size(14));
    if dirty {
        apply = apply.on_press(Message::ApplyAccount);
        revert = revert.on_press(Message::RevertAccount);
    }

    let content = column![
        field("Label", input("Fritz!Box", &state.label, Message::Label)),
        field(
            "Registrar",
            input("fritz.box", &state.registrar, Message::Registrar)
        ),
        field("Registrar port", input("5060", &state.port, Message::Port)),
        field(
            "Username",
            input("Benutzername", &state.username, Message::Username)
        ),
        field(
            "Auth user",
            input("same as username", &state.auth_user, Message::AuthUser)
        ),
        field("Password", password.into()),
        field(
            "",
            checkbox(state.reveal_password)
                .label("Show password")
                .on_toggle(Message::RevealPassword)
                .size(15)
                .text_size(13)
                .into()
        ),
        field(
            "Re-register every",
            input("600", &state.expires, Message::Expires)
        ),
        field(
            "Local SIP port",
            input("5060", &state.local_port, Message::LocalPort)
        ),
        field("Transport", text("UDP (only transport implemented)").size(13).into()),
        row![apply, revert].spacing(10),
    ]
    .spacing(9);

    let hint = if first_run {
        "Nothing configured yet — fill these in and press Apply to connect."
    } else {
        "Applied together — reconnects and re-registers."
    };

    section("Account", Some(hint), content.into())
}

fn audio_section<'a>(state: &'a State, devices: &'a DeviceSelection) -> Element<'a, Message> {
    if !state.devices_loaded {
        return section(
            "Audio",
            None,
            text("Looking for audio devices…").size(13).into(),
        );
    }

    let inputs = DeviceChoice::list(&state.inputs, devices.input.as_ref());
    let outputs = DeviceChoice::list(&state.outputs, devices.output.as_ref());

    let selected = |list: &[DeviceChoice], id: Option<&String>| {
        list.iter().find(|c| c.id.as_ref() == id).cloned()
    };

    let input_pick = pick_list(
        inputs.clone(),
        selected(&inputs, devices.input.as_ref()),
        Message::InputDevice,
    )
    .text_size(13)
    .padding(7)
    .width(Length::Fill);

    let output_pick = pick_list(
        outputs.clone(),
        selected(&outputs, devices.output.as_ref()),
        Message::OutputDevice,
    )
    .text_size(13)
    .padding(7)
    .width(Length::Fill);

    section(
        "Audio",
        Some("Switches immediately, including on a call in progress."),
        column![
            field("Microphone", input_pick.into()),
            field("Speaker", output_pick.into()),
        ]
        .spacing(9)
        .into(),
    )
}

fn appearance_section(ui: &UiSettings) -> Element<'_, Message> {
    let theme = pick_list(ThemeChoice::ALL, Some(ui.theme), Message::Theme)
        .text_size(13)
        .padding(7)
        .width(Length::Fill);

    section(
        "Appearance",
        None,
        column![
            field("Theme", theme.into()),
            field(
                "",
                checkbox(ui.show_banner)
                    .label("Show the Sipster banner above the dialpad")
                    .on_toggle(Message::ShowBanner)
                    .size(15)
                    .text_size(13)
                    .into()
            ),
        ]
        .spacing(9)
        .into(),
    )
}

fn sounds_section(ui: &UiSettings) -> Element<'_, Message> {
    let toggle = |label: &'static str, value: bool, msg: fn(bool) -> Message| {
        checkbox(value)
            .label(label)
            .on_toggle(msg)
            .size(15)
            .text_size(13)
    };

    section(
        "Sounds & notifications",
        Some("Local feedback only — none of this is sent to the other party."),
        column![
            toggle("Ring while a call is coming in", ui.ringtone, Message::Ringtone),
            toggle(
                "Desktop notification for incoming calls",
                ui.notifications,
                Message::Notifications
            ),
            toggle(
                "Beep on dialpad keys",
                ui.dtmf_feedback,
                Message::DtmfFeedback
            ),
            toggle(
                "Chime when a call starts and ends",
                ui.call_chimes,
                Message::CallChimes
            ),
        ]
        .spacing(9)
        .into(),
    )
}

fn integration_section(ui: &UiSettings) -> Element<'_, Message> {
    let uri_cb: Element<'_, Message> = checkbox(ui.register_uri_schemes)
        .label("Set Sipster as default handler for telephony & SIP links")
        .on_toggle(Message::RegisterUriSchemes)
        .size(15)
        .text_size(13)
        .into();

    let tray_cb: Element<'_, Message> = checkbox(ui.close_to_tray)
        .label("Close to system tray (keeps running in background)")
        .on_toggle(Message::CloseToTray)
        .size(15)
        .text_size(13)
        .into();

    section(
        "Desktop Integration",
        Some("Handles background tray operation and tel:, sip:, sips:, callto: links."),
        column![tray_cb, uri_cb]
            .spacing(9)
            .into(),
    )
}

fn about_section(config_path: &str) -> Element<'_, Message> {
    let dim = iced::Color::from_rgb(0.62, 0.62, 0.66);
    let line = |label: &'static str, value: String| -> Element<'_, Message> {
        row![
            text(label).size(12).width(Length::Fixed(132.0)).color(dim),
            text(value).size(12).color(dim),
        ]
        .spacing(10)
        .into()
    };

    let socket = sipster_core::ipc::socket_path();

    section(
        "About",
        Some("Set at startup; not editable here."),
        column![
            line("Version", format!("Sipster {}", env!("CARGO_PKG_VERSION"))),
            line("Config file", config_path.to_string()),
            line("Control socket", socket.display().to_string()),
            line(
                "Log level",
                std::env::var("RUST_LOG").unwrap_or_else(|_| "default (RUST_LOG unset)".into())
            ),
        ]
        .spacing(5)
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::{DeviceChoice, State};
    use sipster_core::{SipAccount, Transport};

    fn draft() -> State {
        let mut state = State::default();
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
            label: "Home".into(),
            registrar: "fritz.box".into(),
            port: 5070,
            username: "bob".into(),
            auth_user: "bobby".into(),
            password: "pw".into(),
            expires: 300,
            local_port: 5062,
            transport: Transport::Udp,
        };
        let mut state = State::default();
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
        let mut state = State::default();
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
