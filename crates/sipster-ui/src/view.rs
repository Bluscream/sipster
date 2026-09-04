//! Pure rendering: turns [`SipsterApp`] state into an Iced widget tree.

use iced::widget::{button, column, container, image, row, text, text_input, Space};
use iced::{Alignment, Element, Length};
use sipster_core::{CallState, RegistrationState};

use crate::app::{Message, SipsterApp};

pub fn root(app: &SipsterApp) -> Element<'_, Message> {
    let main_content = container(
        column![body(app)]
            .align_x(Alignment::Center)
            .spacing(0)
            .max_width(320),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .width(Length::Fill)
    .height(Length::Fill);

    let statusbar = statusbar(app);

    column![main_content, statusbar]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn statusbar(app: &SipsterApp) -> Element<'_, Message> {
    let (circle_char, circle_color, reg_text) = match &app.registration {
        RegistrationState::Registered => ("●", iced::Color::from_rgb(0.2, 0.85, 0.3), "Registered"),
        RegistrationState::Registering => ("●", iced::Color::from_rgb(0.95, 0.8, 0.2), "Registering…"),
        RegistrationState::Failed(err) => ("●", iced::Color::from_rgb(0.9, 0.25, 0.25), err.as_str()),
        RegistrationState::Unregistered => ("○", iced::Color::from_rgb(0.6, 0.6, 0.6), "Offline"),
    };

    let msg = match (&app.account_info, &app.registration) {
        (Some(info), RegistrationState::Registered) => format!("{reg_text} as {info}"),
        (Some(info), _) => format!("{reg_text} ({info})"),
        (None, _) => reg_text.to_string(),
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
fn body(app: &SipsterApp) -> Element<'_, Message> {
    if let Some(incoming) = &app.incoming {
        return incoming_prompt(&incoming.remote);
    }
    dialer(app)
}

fn incoming_prompt(remote: &str) -> Element<'_, Message> {
    let (display_name, sip_addr) = parse_caller_display(remote);

    column![
        text("Incoming call").size(22),
        Space::new().height(5),
        text(display_name).size(20),
        text(sip_addr).size(14),
        Space::new().height(20),
        row![
            action_button("Answer", Message::AnswerPressed, iced::Color::from_rgb(0.2, 0.75, 0.35)),
            action_button("Decline", Message::DeclinePressed, iced::Color::from_rgb(0.85, 0.25, 0.25)),
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
const BANNER: &[u8] = include_bytes!("../assets/banner.png");

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

fn dialer(app: &SipsterApp) -> Element<'_, Message> {
    let number_input = text_input("Number or extension…", &app.dial_number)
        .on_input(Message::DialInputChanged)
        .on_submit(Message::CallPressed)
        .padding(10)
        .size(20);

    let action = if app.active.is_some() {
        action_button("Hang Up", Message::HangupPressed, iced::Color::from_rgb(0.85, 0.25, 0.25))
    } else {
        action_button("Call", Message::CallPressed, iced::Color::from_rgb(0.2, 0.75, 0.35))
    };

    // While a call is up, show who we are talking to and its live state.
    let call_line: Element<'_, Message> = match &app.active {
        Some(call) => text(format!("{} — {}", call.remote, state_label(call.state)))
            .size(14)
            .into(),
        None => Space::new().height(0).into(),
    };

    let action_row = row![
        secondary_button('☰', Message::ContactsPressed),
        action,
        secondary_button('☏', Message::CallListPressed),
    ]
    .align_y(Alignment::Center)
    .spacing(8);

    let mut layout = column![].align_x(Alignment::Center).spacing(4);
    if app.ui().show_banner {
        layout = layout.push(banner()).push(Space::new().height(2));
    }

    layout
        .push(number_input)
        .push(Space::new().height(4))
        .push(call_line)
        .push(Space::new().height(4))
        .push(dialpad())
        .push(Space::new().height(10))
        .push(action_row)
        .into()
}

fn state_label(state: CallState) -> &'static str {
    match state {
        CallState::Dialing => "dialing",
        CallState::Ringing => "ringing",
        CallState::Active => "connected",
        CallState::Terminated => "ended",
    }
}

/// One key of the dialpad. All keys share a size so the grid stays square.
fn pad_key(label: &str, size: f32, msg: Message) -> Element<'static, Message> {
    button(
        container(text(label.to_owned()).size(size))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(Length::Fixed(72.0))
    .height(Length::Fixed(46.0))
    .on_press(msg)
    .into()
}

fn dialpad() -> Element<'static, Message> {
    let digit = |d: char| pad_key(&d.to_string(), 26.0, Message::DialPad(d));

    column![
        row![digit('1'), digit('2'), digit('3')].spacing(10),
        row![digit('4'), digit('5'), digit('6')].spacing(10),
        row![digit('7'), digit('8'), digit('9')].spacing(10),
        row![digit('*'), digit('0'), digit('#')].spacing(10),
        row![
            digit('+'),
            pad_key("C", 22.0, Message::ClearInput),
            pad_key("⌫", 24.0, Message::Backspace),
        ]
        .spacing(10),
    ]
    .spacing(8)
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

fn action_button(label: &str, msg: Message, bg_color: iced::Color) -> Element<'_, Message> {
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
