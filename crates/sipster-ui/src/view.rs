//! Pure rendering: turns [`SipsterApp`] state into an Iced widget tree.

use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Alignment, Element, Length};
use sipster_core::{CallState, RegistrationState};

use crate::app::{Message, SipsterApp};

pub fn root(app: &SipsterApp) -> Element<'_, Message> {
    let content = column![
        text("Sipster").size(28),
        registration_badge(&app.registration),
        text(&app.status).size(14),
        Space::new().height(10),
        body(app),
    ]
    .align_x(Alignment::Center)
    .spacing(8)
    .max_width(380);

    container(content)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .padding(20)
        .into()
}

fn registration_badge(state: &RegistrationState) -> Element<'_, Message> {
    let label = match state {
        RegistrationState::Registered => "● registered",
        RegistrationState::Registering => "◌ registering",
        RegistrationState::Failed(_) => "● failed",
        RegistrationState::Unregistered => "○ offline",
    };
    text(label).size(12).into()
}

/// Shows the incoming-call prompt when ringing, otherwise the dialer.
fn body(app: &SipsterApp) -> Element<'_, Message> {
    if let Some(incoming) = &app.incoming {
        return incoming_prompt(&incoming.remote);
    }
    dialer(app)
}

fn incoming_prompt(remote: &str) -> Element<'_, Message> {
    column![
        text("Incoming call").size(20),
        text(remote.to_string()).size(16),
        Space::new().height(15),
        row![
            action_button("Answer", Message::AnswerPressed),
            action_button("Decline", Message::DeclinePressed),
        ]
        .spacing(12),
    ]
    .align_x(Alignment::Center)
    .spacing(8)
    .into()
}

fn dialer(app: &SipsterApp) -> Element<'_, Message> {
    let number_input = text_input("Number or extension…", &app.dial_number)
        .on_input(Message::DialInputChanged)
        .on_submit(Message::CallPressed)
        .padding(10)
        .size(20);

    let action = if app.active.is_some() {
        action_button("Hang Up", Message::HangupPressed)
    } else {
        action_button("Call", Message::CallPressed)
    };

    // While a call is up, show who we are talking to and its live state.
    let call_line: Element<'_, Message> = match &app.active {
        Some(call) => text(format!("{} — {}", call.remote, state_label(call.state)))
            .size(14)
            .into(),
        None => Space::new().height(0).into(),
    };

    column![
        number_input,
        Space::new().height(6),
        call_line,
        Space::new().height(6),
        dialpad(),
        Space::new().height(18),
        action,
    ]
    .align_x(Alignment::Center)
    .spacing(8)
    .into()
}

fn state_label(state: CallState) -> &'static str {
    match state {
        CallState::Dialing => "dialing",
        CallState::Ringing => "ringing",
        CallState::Active => "connected",
        CallState::Holding => "on hold",
        CallState::Terminated => "ended",
    }
}

fn dialpad() -> Element<'static, Message> {
    let key = |d: char| -> Element<'static, Message> {
        button(text(d.to_string()).size(22))
            .width(Length::Fixed(70.0))
            .height(Length::Fixed(50.0))
            .on_press(Message::DialPad(d))
            .into()
    };
    let backspace: Element<'static, Message> = button(text("⌫").size(20))
        .width(Length::Fixed(70.0))
        .height(Length::Fixed(50.0))
        .on_press(Message::Backspace)
        .into();

    column![
        row![key('1'), key('2'), key('3')].spacing(10),
        row![key('4'), key('5'), key('6')].spacing(10),
        row![key('7'), key('8'), key('9')].spacing(10),
        row![key('*'), key('0'), key('#')].spacing(10),
        row![key('+'), backspace].spacing(10),
    ]
    .spacing(10)
    .into()
}

fn action_button(label: &str, msg: Message) -> Element<'_, Message> {
    button(text(label.to_string()).size(18))
        .on_press(msg)
        .padding(12)
        .width(Length::Fixed(150.0))
        .into()
}
