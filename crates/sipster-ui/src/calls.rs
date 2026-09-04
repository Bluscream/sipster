//! Call list window: state, messages and rendering.

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Length, Padding};
use sipster_integrations::{CallRecord, CallType};

#[derive(Debug, Clone)]
pub enum Message {
    SearchChanged(String),
    SyncPressed,
    CallsLoaded(Vec<CallRecord>),
    DialNumber(String),
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub search: String,
    pub calls: Vec<CallRecord>,
    pub loading: bool,
}

pub fn view(state: &State) -> Element<'_, Message> {
    let title = text("Call History").size(22);
    let count_text = text(format!("{} entries", state.calls.len()))
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

    let search_bar = text_input("Search call history by name or number…", &state.search)
        .on_input(Message::SearchChanged)
        .padding(10)
        .size(15);

    let search_term = state.search.trim().to_lowercase();
    let filtered: Vec<&CallRecord> = state
        .calls
        .iter()
        .filter(|c| {
            if search_term.is_empty() {
                return true;
            }
            c.remote_number.contains(&search_term)
                || c.remote_name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(&search_term))
        })
        .collect();

    let list_content: Element<'_, Message> = if state.loading && state.calls.is_empty() {
        container(text("Syncing call list from local storage & FRITZ!Box…").size(14))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    } else if filtered.is_empty() {
        container(
            text(if state.search.is_empty() {
                "No calls recorded yet."
            } else {
                "No matching calls found."
            })
            .size(14)
            .color(iced::Color::from_rgb(0.6, 0.6, 0.6)),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    } else {
        let mut list_col = column![].spacing(6);
        for call in filtered {
            list_col = list_col.push(call_card(call));
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

fn call_card(call: &CallRecord) -> Element<'_, Message> {
    let (icon, color) = match call.call_type {
        CallType::Incoming => ("↙", iced::Color::from_rgb(0.2, 0.75, 0.35)),
        CallType::Outgoing => ("↗", iced::Color::from_rgb(0.3, 0.6, 0.95)),
        CallType::Missed => ("✕", iced::Color::from_rgb(0.85, 0.25, 0.25)),
        CallType::Rejected => ("⊘", iced::Color::from_rgb(0.85, 0.5, 0.2)),
    };

    let icon_text = text(icon).size(18).color(color);

    let display_title = if let Some(name) = &call.remote_name {
        if name.trim().is_empty() {
            call.remote_number.clone()
        } else {
            format!("{name} ({})", call.remote_number)
        }
    } else {
        call.remote_number.clone()
    };

    let title_line = text(display_title).size(15);
    let details_line = text(format!(
        "{} • {} • {} • {}",
        call.call_type,
        call.timestamp,
        format_duration(call.duration_seconds),
        call.source
    ))
    .size(12)
    .color(iced::Color::from_rgb(0.55, 0.55, 0.55));

    let dial_target = call.remote_number.clone();
    let call_btn = button(text("📞 Call").size(12))
        .on_press(Message::DialNumber(dial_target))
        .padding([4, 10]);

    let card_content = row![
        icon_text,
        Space::new().width(10),
        column![title_line, details_line].spacing(2),
        Space::new().width(Length::Fill),
        call_btn,
    ]
    .align_y(Alignment::Center)
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

fn format_duration(seconds: u32) -> String {
    if seconds == 0 {
        return "0s".into();
    }
    let mins = seconds / 60;
    let secs = seconds % 60;
    if mins >= 60 {
        let hrs = mins / 60;
        let rem_mins = mins % 60;
        format!("{hrs}h {rem_mins}m {secs}s")
    } else if mins > 0 {
        format!("{mins}m {secs}s")
    } else {
        format!("{secs}s")
    }
}
