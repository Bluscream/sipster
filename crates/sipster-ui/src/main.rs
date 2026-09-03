use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Alignment, Element, Length, Task, Theme};
use sipster_core::{CallId, CallState, SipAccount, SipClient};
use std::sync::Arc;

pub fn main() -> iced::Result {
    tracing_subscriber::fmt::init();
    iced::application(SipsterApp::new, SipsterApp::update, SipsterApp::view)
        .title("Sipster")
        .theme(SipsterApp::theme)
        .run()
}

struct SipsterApp {
    account: SipAccount,
    client: Option<Arc<SipClient>>,
    dial_number: String,
    status_text: String,
    current_call: Option<(CallId, CallState)>,
}

#[derive(Debug, Clone)]
enum Message {
    DialInputChanged(String),
    DialPadPressed(char),
    CallButtonPressed,
    HangupButtonPressed,
    AccountConnected,
}

impl SipsterApp {
    fn new() -> (Self, Task<Message>) {
        let account = SipAccount::default();
        let app = Self {
            account,
            client: None,
            dial_number: String::new(),
            status_text: "Ready".into(),
            current_call: None,
        };

        (app, Task::done(Message::AccountConnected))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DialInputChanged(num) => {
                self.dial_number = num;
                Task::none()
            }
            Message::DialPadPressed(digit) => {
                self.dial_number.push(digit);
                Task::none()
            }
            Message::CallButtonPressed => {
                if !self.dial_number.is_empty() {
                    let call_id = CallId::new();
                    self.status_text = format!("Dialing {}...", self.dial_number);
                    self.current_call = Some((call_id, CallState::Dialing));
                }
                Task::none()
            }
            Message::HangupButtonPressed => {
                self.status_text = "Call terminated".into();
                self.current_call = None;
                Task::none()
            }
            Message::AccountConnected => {
                if let Ok(client) = SipClient::new(self.account.clone()) {
                    self.client = Some(Arc::new(client));
                    self.status_text = "Connected (SIP ready)".into();
                }
                Task::none()
            }
        }
    }

    #[allow(clippy::unused_self)]
    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn view(&self) -> Element<'_, Message> {
        let status = text(&self.status_text).size(14);

        let number_input = text_input("Enter phone number or extension...", &self.dial_number)
            .on_input(Message::DialInputChanged)
            .padding(10)
            .size(20);

        let dial_btn = |digit: char| -> Element<'_, Message> {
            button(text(digit.to_string()).size(22))
                .width(Length::Fixed(70.0))
                .height(Length::Fixed(50.0))
                .on_press(Message::DialPadPressed(digit))
                .into()
        };

        let row1 = row![dial_btn('1'), dial_btn('2'), dial_btn('3')].spacing(10);
        let row2 = row![dial_btn('4'), dial_btn('5'), dial_btn('6')].spacing(10);
        let row3 = row![dial_btn('7'), dial_btn('8'), dial_btn('9')].spacing(10);
        let row4 = row![dial_btn('*'), dial_btn('0'), dial_btn('#')].spacing(10);

        let dialpad = column![row1, row2, row3, row4].spacing(10);

        let action_row = if self.current_call.is_some() {
            row![
                button(text("Hang Up").size(18))
                    .on_press(Message::HangupButtonPressed)
                    .padding(12)
                    .width(Length::Fixed(150.0))
            ]
        } else {
            row![
                button(text("Call").size(18))
                    .on_press(Message::CallButtonPressed)
                    .padding(12)
                    .width(Length::Fixed(150.0))
            ]
        };

        let content = column![
            text("Sipster Phone").size(28),
            status,
            Space::new().height(15),
            number_input,
            Space::new().height(15),
            dialpad,
            Space::new().height(20),
            action_row,
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
}
