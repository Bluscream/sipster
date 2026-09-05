//! Pure rendering: turns [`SipsterApp`] state into an Iced widget tree.

use iced::widget::{button, column, container, image, row, text, text_input, Space};
use iced::{Alignment, Element, Length, Padding};
use sipster_core::{CallState, RegistrationState};

use crate::app::{Message, SipsterApp};

/// The dial field, so it can be focused when the window opens — otherwise
/// typing goes nowhere until the user clicks it, and the dialpad is the one
/// control people expect to drive from the keyboard.
pub fn dial_input_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new("dial-number")
}

pub fn root(app: &SipsterApp) -> Element<'_, Message> {
    let Some(pane) = app.docked_pane() else {
        return column![dialer_column(app, false), statusbar(app)]
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    };

    // Two columns once the window is wide enough for both; otherwise the pane
    // takes the dialpad's place, which is the only way to show a readable list
    // in a window the width of the dialer. See [`crate::pane`].
    let content: Element<'_, Message> = if crate::pane::fits_beside_dialer(app.main_width()) {
        row![
            container(dialer_column(app, false)).width(Length::Fixed(crate::pane::DIALER_WIDTH)),
            container(pane).width(Length::Fill).height(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    } else {
        column![
            dialer_column(app, true),
            container(pane).width(Length::Fill).height(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    };

    column![content, statusbar(app)]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The dialer on its own, without the status bar.
fn dialer_column(app: &SipsterApp, compact: bool) -> Element<'_, Message> {
    let mut main_column = column![body(app, compact)]
        .align_x(Alignment::Center)
        .spacing(0)
        .max_width(320);

    // Compact mode gives its height to the pane below it, so the dialer must
    // not also stretch to fill.
    let main_content = if app.ui().show_banner && !compact {
        container(main_column)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .width(Length::Fill)
            .height(Length::Fill)
    } else {
        main_column = main_column.padding(Padding {
            top: 14.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        });
        let stacked = container(main_column)
            .center_x(Length::Fill)
            .width(Length::Fill);
        // Compact mode hands its leftover height to the pane below it; on its
        // own the dialer still fills the window.
        if compact {
            stacked
        } else {
            stacked.height(Length::Fill)
        }
    };

    main_content.into()
}

/// A name or number as it should appear, masked in streaming mode.
fn show(value: &str, mask: bool) -> String {
    if mask {
        sipster_core::mask_identity(value)
    } else {
        value.to_string()
    }
}

fn statusbar(app: &SipsterApp) -> Element<'_, Message> {
    let registration = app.active_registration();
    let reg_registered = rust_i18n::t!("registered");
    let reg_registering = rust_i18n::t!("registering");
    let reg_offline = rust_i18n::t!("not_registered");
    let (circle_char, circle_color, reg_text) = match &registration {
        RegistrationState::Registered => ("●", iced::Color::from_rgb(0.2, 0.85, 0.3), reg_registered),
        RegistrationState::Registering => ("●", iced::Color::from_rgb(0.95, 0.8, 0.2), reg_registering),
        RegistrationState::Failed(err) => ("●", iced::Color::from_rgb(0.9, 0.25, 0.25), std::borrow::Cow::Owned(err.clone())),
        RegistrationState::Unregistered => ("○", iced::Color::from_rgb(0.6, 0.6, 0.6), reg_offline),
    };

    // The account line carries the SIP username and registrar, both of which
    // identify the user on a shared screen.
    let mask = app.ui().streaming_mode;
    
    let info_str = app.account_identity().map(|s| show(&s, mask));
    let number_str = app.active_numbers().map(|(_, ext)| show(ext, mask));
    
    let acc_info = match (info_str, number_str) {
        (Some(i), Some(n)) => format!("{i} - {n}"),
        (Some(i), None) => i,
        (None, Some(n)) => rust_i18n::t!("unknown", number = n).to_string(),
        (None, None) => String::new(),
    };

    let msg = match &registration {
        RegistrationState::Registered if !acc_info.is_empty() => acc_info,
        _ if !acc_info.is_empty() => format!("{reg_text} ({acc_info})"),
        _ => reg_text.to_string(),
    };

    let status_bar_content = row![
        text(circle_char).size(14).color(circle_color),
        text(msg).size(13),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    container(status_bar_content)
        .width(Length::Fill)
        .padding([6, 12])
        .into()
}

/// Shows the incoming-call prompt when ringing, otherwise the dialer.
fn body(app: &SipsterApp, compact: bool) -> Element<'_, Message> {
    if let Some(incoming) = &app.incoming {
        return incoming_prompt(&incoming.remote, app.ui().streaming_mode);
    }
    dialer(app, compact)
}

fn incoming_prompt(remote: &str, mask: bool) -> Element<'_, Message> {
    let (display_name, sip_addr) = parse_caller_display(remote);
    let (display_name, sip_addr) = (show(&display_name, mask), show(&sip_addr, mask));

    let incoming_title = rust_i18n::t!("incoming").to_string();
    let answer_label = rust_i18n::t!("answer");
    let decline_label = rust_i18n::t!("decline");

    column![
        text(incoming_title).size(22),
        Space::new().height(5),
        text(display_name).size(20),
        text(sip_addr).size(14),
        Space::new().height(20),
        row![
            action_button(&answer_label, Message::AnswerPressed, iced::Color::from_rgb(0.2, 0.75, 0.35)),
            action_button(&decline_label, Message::DeclinePressed, iced::Color::from_rgb(0.85, 0.25, 0.25)),
        ]
        .spacing(16),
    ]
    .align_x(Alignment::Center)
    .spacing(6)
    .into()
}

fn parse_caller_display(raw: &str) -> (String, String) {
    if let Some(start) = raw.find('"') {
        if let Some(end) = raw[start + 1..].find('"') {
            let name = &raw[start + 1..start + 1 + end];
            let rest = raw[start + 1 + end + 1..].trim();
            let addr = rest
                .trim_start_matches('<')
                .split('>')
                .next()
                .unwrap_or(rest);
            return (name.to_string(), addr.to_string());
        }
    }
    (raw.to_string(), String::new())
}

/// The wordmark above the number field, which doubles as the settings button.
///
/// There is no menu bar and no room for a gear in the 320 px dialer, so the
/// banner carries the affordance. It is a borderless button so it still reads
/// as a logo rather than a control.
fn banner() -> Element<'static, Message> {
    let Some(handle) = banner_handle() else {
        // Undecodable artwork must not cost us the settings entry point.
        return secondary_button('⚙', Message::OpenSettings);
    };

    button(
        image(handle.clone())
            .width(Length::Fixed(196.0))
            .content_fit(iced::ContentFit::Contain),
    )
    .on_press(Message::OpenSettings)
    .padding(0)
    .style(|_theme, _status| button::Style {
        background: None,
        ..button::Style::default()
    })
    .into()
}

/// The wordmark, embedded so the binary stays self-contained.
const BANNER: &[u8] = include_bytes!("../../../assets/banner.png");

/// The banner as a decoded, cached image handle.
///
/// Both details here are load-bearing, and getting either wrong makes the
/// banner vanish whenever the settings window is open:
///
/// - **Built once.** Every `Handle` constructor assigns `Id::unique()`, so
///   calling one inside `view` mints a brand-new id on every frame and the
///   renderer's cache can never hit.
/// - **Decoded here, not by the renderer.** `Handle::from_bytes` defers PNG
///   decoding to a background worker. `iced_wgpu` clears its image `hits` set
///   on every present, so a second window's frame evicts this image, and the
///   re-decode is asynchronous — the dialer draws nothing while it waits, then
///   loses the entry again on the settings window's next frame. Handing over
///   finished RGBA skips that path entirely.
fn banner_handle() -> Option<&'static image::Handle> {
    static HANDLE: std::sync::OnceLock<Option<image::Handle>> = std::sync::OnceLock::new();

    HANDLE
        .get_or_init(|| {
            let decoded = ::image::load_from_memory(BANNER)
                .inspect_err(|e| tracing::warn!(error = %e, "could not decode the banner"))
                .ok()?
                .to_rgba8();
            let (width, height) = (decoded.width(), decoded.height());
            Some(image::Handle::from_rgba(width, height, decoded.into_raw()))
        })
        .as_ref()
}

fn dialer(app: &SipsterApp, compact: bool) -> Element<'_, Message> {
    // The dial field is hidden with `secure`, not by rewriting its value:
    // substituting a mask would feed the mask back through `on_input` and
    // destroy what the user typed.
    let placeholder = rust_i18n::t!("number_placeholder");
    let number_input = text_input(&placeholder, &app.dial_number)
        .id(dial_input_id())
        .secure(app.ui().streaming_mode)
        .on_input(Message::DialInputChanged)
        .on_submit(Message::CallPressed)
        .padding(10)
        .size(20);

    let hangup_label = rust_i18n::t!("hangup");
    let call_label = rust_i18n::t!("call");
    let action = if app.active.is_some() {
        action_button(&hangup_label, Message::HangupPressed, iced::Color::from_rgb(0.85, 0.25, 0.25))
    } else {
        action_button(&call_label, Message::CallPressed, iced::Color::from_rgb(0.2, 0.75, 0.35))
    };

    // While a call is up, show who we are talking to and its live state.
    let call_line: Element<'_, Message> = match &app.active {
        Some(call) => text(format!(
            "{} — {}",
            show(&call.remote, app.ui().streaming_mode),
            state_label(call.state)
        ))
        .size(14)
        .into(),
        None => Space::new().height(0).into(),
    };

    // The list buttons cycle hidden → docked → window, so they light up to
    // show the list is somewhere rather than reading as plain "open".
    let action_row = row![
        list_button('☰', app.contacts_at(), Message::ContactsPressed),
        action,
        list_button('☏', app.calls_at(), Message::CallListPressed),
    ]
    .align_y(Alignment::Center)
    .spacing(8);

    // Hold and transfer only exist while there is a call to apply them to.
    let resume_label = rust_i18n::t!("resume");
    let hold_label = rust_i18n::t!("hold");
    let transfer_label = rust_i18n::t!("transfer");
    let in_call_row: Element<'_, Message> = match &app.active {
        Some(call) => row![
            call_action(
                if call.on_hold { &resume_label } else { &hold_label },
                Message::HoldPressed,
            ),
            // Blind transfer sends the call to whatever is in the number
            // field, so it stays disabled until there is something to send to.
            call_action_maybe(
                &transfer_label,
                (!app.dial_number.trim().is_empty()).then_some(Message::TransferPressed),
            ),
        ]
        .spacing(8)
        .into(),
        None => Space::new().height(0).into(),
    };

    let mut layout = column![].align_x(Alignment::Center).spacing(4);
    if app.ui().show_banner && !compact {
        layout = layout.push(banner()).push(Space::new().height(2));
    }

    layout = layout
        .push(number_input)
        .push(Space::new().height(4))
        .push(call_line)
        .push(Space::new().height(4));

    // The dialpad is what a docked pane displaces in a narrow window; the
    // number field and the call row stay, so a call can still be placed.
    if !compact {
        layout = layout.push(dialpad(app)).push(Space::new().height(10));
    }

    layout.push(action_row).push(in_call_row).into()
}

fn state_label(state: CallState) -> String {
    match state {
        CallState::Dialing => rust_i18n::t!("dialing").to_string(),
        CallState::Ringing => rust_i18n::t!("ringing").to_string(),
        CallState::Active => rust_i18n::t!("connected").to_string(),
        CallState::Terminated => rust_i18n::t!("terminated").to_string(),
    }
}

/// One key of the dialpad. All keys share a size so the grid stays square.
///
/// `glow` is how recently the key was struck, 1.0 to 0.0; see [`crate::glow`].
fn pad_key(label: &str, size: f32, msg: Message, glow: f32) -> Element<'static, Message> {
    button(
        container(text(label.to_owned()).size(size))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(Length::Fixed(72.0))
    .height(Length::Fixed(46.0))
    .on_press(msg)
    .style(move |theme, status| {
        let mut style = button::primary(theme, status);
        if glow <= 0.0 {
            return style;
        }
        // Lift the key towards white rather than swapping in a second colour,
        // so it reads as the same key lit up and still works on every theme.
        if let Some(iced::Background::Color(base)) = style.background {
            style.background = Some(iced::Background::Color(lighten(base, glow * 0.55)));
        }
        style.border.color = lighten(style.border.color, glow);
        style.border.width = style.border.width.max(1.0);
        style
    })
    .into()
}

/// Mixes `color` towards white by `amount` (0.0 to 1.0).
fn lighten(color: iced::Color, amount: f32) -> iced::Color {
    let mix = amount.clamp(0.0, 1.0);
    iced::Color {
        r: color.r + (1.0 - color.r) * mix,
        g: color.g + (1.0 - color.g) * mix,
        b: color.b + (1.0 - color.b) * mix,
        a: color.a,
    }
}

fn dialpad(app: &SipsterApp) -> Element<'static, Message> {
    let glow = app.glow();
    let digit =
        |d: char| pad_key(&d.to_string(), 26.0, Message::DialPad(d), glow.amount(d));

    column![
        row![digit('1'), digit('2'), digit('3')].spacing(10),
        row![digit('4'), digit('5'), digit('6')].spacing(10),
        row![digit('7'), digit('8'), digit('9')].spacing(10),
        row![digit('*'), digit('0'), digit('#')].spacing(10),
        row![
            digit('+'),
            pad_key("C", 22.0, Message::ClearInput, glow.amount('C')),
            pad_key("⌫", 24.0, Message::Backspace, glow.amount('⌫')),
        ]
        .spacing(10),
    ]
    .spacing(8)
    .into()
}

/// A list toggle, tinted while its list is showing somewhere.
fn list_button(
    glyph: char,
    placement: crate::pane::Placement,
    msg: Message,
) -> Element<'static, Message> {
    let color = match placement {
        crate::pane::Placement::Hidden => iced::Color::from_rgb(0.7, 0.7, 0.7),
        crate::pane::Placement::Docked => iced::Color::from_rgb(0.35, 0.7, 1.0),
        crate::pane::Placement::Window => iced::Color::from_rgb(0.4, 0.85, 0.6),
    };
    button(
        container(text(glyph.to_string()).size(20))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(Length::Fixed(44.0))
    .height(Length::Fixed(36.0))
    .on_press(msg)
    .style(move |theme, status| {
        let mut style = button::text(theme, status);
        style.text_color = color;
        style
    })
    .into()
}

/// A small labelled action shown only while a call is up.
fn call_action(label: &str, msg: Message) -> Element<'static, Message> {
    call_action_maybe(label, Some(msg))
}

/// As [`call_action`], but dimmed and inert when there is nothing to do.
fn call_action_maybe(label: &str, msg: Option<Message>) -> Element<'static, Message> {
    button(
        container(text(label.to_owned()).size(13))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(Length::Fixed(88.0))
    .height(Length::Fixed(28.0))
    .on_press_maybe(msg)
    .style(button::secondary)
    .into()
}

/// A muted, borderless glyph button flanking the main call action.
fn secondary_button(glyph: char, msg: Message) -> Element<'static, Message> {
    button(
        container(text(glyph.to_string()).size(20))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(Length::Fixed(44.0))
    .height(Length::Fixed(36.0))
    .on_press(msg)
    .style(|theme, status| {
        let mut style = button::text(theme, status);
        style.text_color = iced::Color::from_rgb(0.7, 0.7, 0.7);
        style
    })
    .into()
}

fn action_button(label: &str, msg: Message, bg_color: iced::Color) -> Element<'static, Message> {
    button(
        container(text(label.to_string()).size(17))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .on_press(msg)
    .style(move |theme, status| {
        let mut style = button::primary(theme, status);
        style.background = Some(iced::Background::Color(match status {
            button::Status::Hovered => iced::Color {
                a: 0.9,
                ..bg_color
            },
            button::Status::Pressed => iced::Color {
                r: bg_color.r * 0.8,
                g: bg_color.g * 0.8,
                b: bg_color.b * 0.8,
                a: 1.0,
            },
            _ => bg_color,
        }));
        style.border.radius = 8.0.into();
        style
    })
    .height(Length::Fixed(36.0))
    .width(Length::Fixed(136.0))
    .into()
}
