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
    button, checkbox, column, container, pick_list, row, rule, scrollable, stack, text, text_input,
    Space,
};
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
    Label(String),
    Registrar(String),
    Port(String),
    Username(String),
    AuthUser(String),
    Password(String),
    Expires(String),
    LocalPort(String),
    RevealPassword(bool),
    RevealFritzPassword(bool),
    RevealCardDavPassword(bool),
    RevealGoogleSecret(bool),
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
    StreamingMode(bool),
    ImportGoogleClientJson(String),

    // Integrations. Contact and history providers are account configuration,
    // so they belong here rather than inside the windows that display their
    // data — which is where they used to live, splitting account setup across
    // three windows.
    ToggleProvidersModal,
    ToggleLocalHistory(bool),
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
    pub label: String,
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
    pub show_providers: bool,
    pub draft_carddav_url: String,
    pub draft_carddav_user: String,
    pub draft_carddav_pass: String,
    /// The FRITZ!Box port as typed. Held as text for the same reason the
    /// account ports are: parsing every keystroke fights the user the moment
    /// they clear the field to retype it.
    pub draft_fritz_port: String,
    /// Google OAuth client credentials, which the user registers themselves.
    pub draft_google_client_id: String,
    pub draft_google_client_secret: String,
    /// Path typed into the `client_secret` JSON import box.
    pub draft_google_json_path: String,

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
    account: Option<&'a SipAccount>,
    first_run: bool,
    integration: &'a IntegrationSettings,
    config_path: &'a str,
) -> Element<'a, Message> {
    let selected = state.section.min(SECTIONS.len() - 1);
    let panel = match selected {
        0 => account_section(state, account, first_run, ui.streaming_mode),
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
    for (i, name) in SECTIONS.iter().enumerate() {
        let is_current = i == selected;
        index = index.push(
            button(text(*name).size(13))
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

/// A password field with a reveal toggle inside it.
///
/// The eye sits on top of the input via `stack`, with right padding on the
/// text so a long value scrolls behind rather than under the button. This
/// replaces a separate "Show password" checkbox, which took a whole row per
/// secret and left three of the four secret fields with no reveal at all.
fn secret_input<'a>(
    placeholder: &'a str,
    value: &'a str,
    revealed: bool,
    on_change: impl Fn(String) -> Message + 'a,
    on_reveal: impl Fn(bool) -> Message + 'a,
) -> Element<'a, Message> {
    let field = text_input(placeholder, value)
        .on_input(on_change)
        .secure(!revealed)
        .padding(iced::Padding::from(7).right(30))
        .size(14);

    // A geometric glyph, not an emoji: the default font has no 👁, so the
    // button rendered as nothing at all and the toggle looked missing.
    // Filled means visible, hollow means hidden.
    let eye = button(text(if revealed { "◉" } else { "○" }).size(14))
        .on_press(on_reveal(!revealed))
        .padding([2, 6])
        .style(move |theme: &iced::Theme, status| {
            let palette = theme.extended_palette();
            button::Style {
                background: None,
                // Dim while hidden, full strength while revealed, so the
                // current state is readable from the icon alone.
                text_color: if revealed || matches!(status, button::Status::Hovered) {
                    palette.background.base.text
                } else {
                    palette.background.strong.color
                },
                ..button::Style::default()
            }
        });

    stack![
        field,
        container(eye)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Center),
    ]
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
    mask: bool,
) -> Element<'a, Message> {
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

    // Identifying fields are hidden with `secure` rather than by substituting
    // a mask: these are editable, and feeding a mask back through `on_input`
    // would overwrite the real value.
    let hidden = |placeholder: &'static str, value: &'a str, on_change: fn(String) -> Message| {
        text_input(placeholder, value)
            .on_input(on_change)
            .secure(mask)
            .padding(7)
            .size(14)
            .into()
    };

    let content = column![
        field("Label", hidden("Fritz!Box", &state.label, Message::Label)),
        field(
            "Registrar",
            hidden("fritz.box", &state.registrar, Message::Registrar)
        ),
        field("Registrar port", input("5060", &state.port, Message::Port)),
        field(
            "Username",
            hidden("Benutzername", &state.username, Message::Username)
        ),
        field(
            "Auth user",
            hidden("same as username", &state.auth_user, Message::AuthUser)
        ),
        field(
            "Password",
            secret_input(
                "",
                &state.password,
                state.reveal_password,
                Message::Password,
                Message::RevealPassword,
            )
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
    let streaming_cb: Element<'_, Message> = checkbox(ui.streaming_mode)
        .label("Streaming mode — hide all names and numbers")
        .on_toggle(Message::StreamingMode)
        .size(15)
        .text_size(13)
        .into();

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
        column![
            tray_cb,
            uri_cb,
            streaming_cb,
            text(
                "Streaming mode masks every name, number and address to its first \
                 and last character across all windows, for screen sharing."
            )
            .size(11)
            .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
        ]
        .spacing(9)
        .into(),
    )
}

/// Contact and history providers.
///
/// Collapsed by default: most people configure this once, and it is long.
fn providers_section<'a>(
    state: &'a State,
    integration: &'a IntegrationSettings,
    config_path: &'a str,
) -> Element<'a, Message> {
    let summary = format!(
        "{} FRITZ!Box · {} Google · {} CardDAV",
        if integration.fritzbox.enabled { "1" } else { "0" },
        integration.google_accounts.iter().filter(|a| a.enabled).count(),
        integration.carddav_accounts.iter().filter(|a| a.enabled).count(),
    );

    let toggle = button(
        text(if state.show_providers { "Hide" } else { "Configure" }).size(13),
    )
    .on_press(Message::ToggleProvidersModal)
    .padding([5, 11]);

    let header = row![
        text(summary).size(13),
        Space::new().width(Length::Fill),
        toggle,
    ]
    .align_y(Alignment::Center);

    let mut content = column![header].spacing(10);

    if state.show_providers {
        content = content.push(rule::horizontal(1)).push(fritzbox_panel(state, integration));
        content = content.push(rule::horizontal(1)).push(google_panel(state, integration));
        content = content.push(rule::horizontal(1)).push(carddav_panel(state, integration));
    }

    content = content.push(
        checkbox(integration.local_history_enabled)
            .label("Record placed and received calls to local history")
            .on_toggle(Message::ToggleLocalHistory)
            .size(15)
            .text_size(13),
    );

    // The config path lived in an About section that was otherwise just a
    // version number; it belongs where credentials are entered.
    content = content.push(
        text(format!("Stored in {config_path}"))
            .size(11)
            .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
    );

    section(
        "Integrations",
        Some("Where contacts and call history come from."),
        content.into(),
    )
}

fn fritzbox_panel<'a>(
    state: &'a State,
    integration: &'a IntegrationSettings,
) -> Element<'a, Message> {
    let fb = &integration.fritzbox;
    // Owned: the widget borrows for 'a, and a temporary from to_string() would
    // not live that long.
    column![
        text("FRITZ!Box").size(14),
        checkbox(fb.enabled)
            .label("Sync the router phonebook and call list")
            .on_toggle(Message::FritzEnabledToggled)
            .size(15)
            .text_size(13),
        field("Host", input("fritz.box", &fb.host, Message::FritzHostChanged)),
        field("Port", port_input(&state.draft_fritz_port)),
        field("Username", input("", &fb.username, Message::FritzUserChanged)),
        field(
            "Password",
            secret_input(
                "",
                &fb.password,
                state.reveal_fritz_password,
                Message::FritzPassChanged,
                Message::RevealFritzPassword,
            )
        ),
    ]
    .spacing(8)
    .into()
}

/// The port field, whose value is an owned string rather than a temporary.
fn port_input(value: &str) -> Element<'_, Message> {
    text_input("49000", value)
        .on_input(Message::FritzPortChanged)
        .padding(7)
        .size(14)
        .into()
}

fn google_panel<'a>(
    state: &'a State,
    integration: &'a IntegrationSettings,
) -> Element<'a, Message> {
    let mut content = column![
        text("Google Contacts").size(14),
        // Sipster ships no OAuth credentials — see the note in
        // sipster-integrations::google for why bundling them is neither
        // possible nor meaningful.
        text(
            "Needs your own OAuth client: Google Cloud console › Credentials › \
             OAuth client ID › Desktop app."
        )
        .size(12),
    ]
    .spacing(6);

    for account in &integration.google_accounts {
        content = content.push(
            row![
                text(account.email.clone()).size(13),
                Space::new().width(Length::Fill),
                button(text("Remove").size(12))
                    .on_press(Message::RemoveGoogleAccount(account.id.clone()))
                    .padding([3, 9])
                    .style(button::danger),
            ]
            .align_y(Alignment::Center)
            .spacing(6),
        );
    }

    let ready = !state.draft_google_client_id.trim().is_empty()
        && !state.draft_google_client_secret.trim().is_empty();

    content
        .push(field(
            "client_secret JSON",
            text_input(
                "path to client_secret_….json downloaded from Google",
                &state.draft_google_json_path,
            )
            .on_input(Message::ImportGoogleClientJson)
            .padding(7)
            .size(14)
            .into(),
        ))
        .push(field(
            "Client ID",
            input(
                "…apps.googleusercontent.com",
                &state.draft_google_client_id,
                Message::GoogleClientIdChanged,
            ),
        ))
        .push(field(
            "Client secret",
            secret_input(
                "",
                &state.draft_google_client_secret,
                state.reveal_google_secret,
                Message::GoogleClientSecretChanged,
                Message::RevealGoogleSecret,
            ),
        ))
        .push(
            button(text("Connect a Google account").size(13))
                .on_press_maybe(ready.then_some(Message::ConnectGoogleAccount))
                .padding([5, 11]),
        )
        .into()
}

fn carddav_panel<'a>(
    state: &'a State,
    integration: &'a IntegrationSettings,
) -> Element<'a, Message> {
    let mut content = column![text("CardDAV").size(14)].spacing(6);

    for account in &integration.carddav_accounts {
        content = content.push(
            row![
                column![
                    text(account.name.clone()).size(13),
                    text(account.url.clone())
                        .size(11)
                        .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
                ]
                .spacing(1),
                Space::new().width(Length::Fill),
                button(text("Remove").size(12))
                    .on_press(Message::RemoveCardDavAccount(account.id.clone()))
                    .padding([3, 9])
                    .style(button::danger),
            ]
            .align_y(Alignment::Center)
            .spacing(6),
        );
    }

    let can_add = !state.draft_carddav_url.trim().is_empty();

    content
        .push(field(
            "URL",
            input(
                "https://dav.example.com/addressbooks/me/default/",
                &state.draft_carddav_url,
                Message::CardDavUrlChanged,
            ),
        ))
        .push(field(
            "Username",
            input("", &state.draft_carddav_user, Message::CardDavUserChanged),
        ))
        .push(field(
            "Password",
            secret_input(
                "",
                &state.draft_carddav_pass,
                state.reveal_carddav_password,
                Message::CardDavPassChanged,
                Message::RevealCardDavPassword,
            ),
        ))
        .push(
            button(text("Add address book").size(13))
                .on_press_maybe(can_add.then_some(Message::AddCardDavAccount))
                .padding([5, 11]),
        )
        .into()
}

/// Blocked numbers, listed so a rule can actually be found and removed.
fn blocking_section(integration: &IntegrationSettings) -> Element<'_, Message> {
    let action_pick = pick_list(
        BlockAction::ALL,
        Some(integration.default_block_action),
        Message::DefaultBlockActionChanged,
    )
    .text_size(13)
    .padding(7)
    .width(Length::Fill);

    let mut content = column![field("Default action", action_pick.into())].spacing(8);

    if integration.blocked_numbers.is_empty() {
        content = content.push(
            text("Nothing blocked. Block a caller from Contacts or History.")
                .size(12)
                .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
        );
    } else {
        for blocked in &integration.blocked_numbers {
            let label = blocked.name.clone().map_or_else(
                || blocked.number.clone(),
                |name| format!("{name} ({})", blocked.number),
            );
            content = content.push(
                row![
                    column![
                        text(label).size(13),
                        text(blocked.action.label())
                            .size(11)
                            .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
                    ]
                    .spacing(1),
                    Space::new().width(Length::Fill),
                    button(text("Unblock").size(12))
                        .on_press(Message::UnblockNumber(blocked.number.clone()))
                        .padding([3, 9]),
                ]
                .align_y(Alignment::Center)
                .spacing(6),
            );
        }
    }

    section(
        "Call blocking",
        Some("Applies to incoming calls, matched on the caller's number."),
        content.into(),
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
