//! Contacts window: state, messages and rendering.

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Length, Padding};
use sipster_integrations::Contact;

#[derive(Debug, Clone)]
pub enum Message {
    SearchChanged(String),
    SyncPressed,
    ContactsLoaded(Vec<Contact>),
    DialContact(String),
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub search: String,
    pub contacts: Vec<Contact>,
    pub loading: bool,
}

pub fn view(state: &State) -> Element<'_, Message> {
    let title = text("Contacts").size(22);
    let count_text = text(format!("{} contacts", state.contacts.len()))
        .size(13)
        .color(iced::Color::from_rgb(0.6, 0.6, 0.6));

    let sync_btn = button(text(if state.loading { "Syncing…" } else { "⟳ Sync" }).size(14))
        .on_press_maybe(if state.loading { None } else { Some(Message::SyncPressed) })
        .padding([6, 12]);

    let top_bar = row![
        column![title, count_text].spacing(2),
        Space::new().width(Length::Fill),
        sync_btn,
    ]
    .align_y(Alignment::Center);

    let search_bar = text_input("Search contacts by name or number…", &state.search)
        .on_input(Message::SearchChanged)
        .padding(10)
        .size(15);

    let search_term = state.search.trim().to_lowercase();
    let filtered: Vec<&Contact> = state
        .contacts
        .iter()
        .filter(|c| {
            if search_term.is_empty() {
                return true;
            }
            c.name.to_lowercase().contains(&search_term)
                || c.numbers.iter().any(|n| n.number.contains(&search_term))
        })
        .collect();

    let list_content: Element<'_, Message> = if state.loading && state.contacts.is_empty() {
        container(text("Syncing contacts from local storage & FRITZ!Box…").size(14))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    } else if filtered.is_empty() {
        container(
            text(if state.search.is_empty() {
                "No contacts found. Click 'Sync' to import from FRITZ!Box or local address book."
            } else {
                "No matching contacts found."
            })
            .size(14)
            .color(iced::Color::from_rgb(0.6, 0.6, 0.6)),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    } else {
        let mut list_col = column![].spacing(8);
        for contact in filtered {
            list_col = list_col.push(contact_card(contact));
        }
        scrollable(list_col).into()
    };

    let content = column![
        top_bar,
        Space::new().height(8),
        search_bar,
        Space::new().height(12),
        list_content,
    ]
    .spacing(6)
    .padding(Padding::new(16.0))
    .width(Length::Fill)
    .height(Length::Fill);

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn contact_card(contact: &Contact) -> Element<'_, Message> {
    let name = text(&contact.name).size(16);
    let source_badge = text(format!("{}", contact.source))
        .size(11)
        .color(iced::Color::from_rgb(0.5, 0.5, 0.5));

    let mut numbers_col = column![].spacing(4);
    for num in &contact.numbers {
        let number_str = num.number.clone();
        let call_btn = button(text("📞 Call").size(12))
            .on_press(Message::DialContact(number_str.clone()))
            .padding([3, 8]);

        let num_row = row![
            text(format!("{}: {}", num.number_type, num.number)).size(13),
            Space::new().width(Length::Fill),
            call_btn,
        ]
        .align_y(Alignment::Center)
        .spacing(8);

        numbers_col = numbers_col.push(num_row);
    }

    let card_content = column![
        row![name, Space::new().width(Length::Fill), source_badge].align_y(Alignment::Center),
        Space::new().height(4),
        numbers_col,
    ]
    .spacing(4)
    .padding(10);

    container(card_content)
        .width(Length::Fill)
        .style(|theme: &iced::Theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(palette.background.weak.color.into()),
                border: iced::border::rounded(6),
                ..container::Style::default()
            }
        })
        .into()
}
