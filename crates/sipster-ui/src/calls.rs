//! Call list window: state, messages, settings, and rendering.

use iced::widget::{button, checkbox, column, container, pick_list, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Length, Padding};
use sipster_core::{BlockAction, IntegrationSettings};
use sipster_integrations::{CallRecord, CallType};

#[derive(Debug, Clone)]
pub enum Message {
    SearchChanged(String),
    SyncPressed,
    CallsLoaded(Vec<CallRecord>),
    DialNumber(String),

    // In-window Settings & Call Blocking:
    ToggleSettingsModal,
    ToggleLocalHistory(bool),
    DefaultBlockActionChanged(BlockAction),
    ClearHistoryPressed,
    UnblockNumber(String),

    // Block number prompt:
    BlockNumberPrompt(String, Option<String>),
    ConfirmBlockNumber(String, Option<String>, BlockAction),
    CancelBlockPrompt,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub search: String,
    pub calls: Vec<CallRecord>,
    pub loading: bool,
    pub show_settings_modal: bool,
    pub block_prompt: Option<(String, Option<String>)>,
}

pub fn view<'a>(state: &'a State, settings: &'a IntegrationSettings) -> Element<'a, Message> {
    if let Some((number, name)) = &state.block_prompt {
        return block_number_modal(number, name.as_deref());
    }

    if state.show_settings_modal {
        return settings_modal(settings);
    }

    let title = text("Call History").size(22);
    let count_text = text(format!("{} entries", state.calls.len()))
        .size(13)
        .color(iced::Color::from_rgb(0.6, 0.6, 0.6));

    let settings_btn = button(text("⚙ Settings").size(13))
        .on_press(Message::ToggleSettingsModal)
        .padding([6, 12]);

    let sync_btn = button(text(if state.loading { "Syncing…" } else { "⟳ Sync" }).size(13))
        .on_press_maybe(if state.loading { None } else { Some(Message::SyncPressed) })
        .padding([6, 12]);

    let top_bar = row![
        column![title, count_text].spacing(2),
        Space::new().width(Length::Fill),
        settings_btn,
        Space::new().width(4),
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
        .on_press(Message::DialNumber(dial_target.clone()))
        .padding([4, 10]);

    let block_btn = button(text("⊘ Block").size(12))
        .on_press(Message::BlockNumberPrompt(dial_target, call.remote_name.clone()))
        .padding([4, 8]);

    let card_content = row![
        icon_text,
        Space::new().width(10),
        column![title_line, details_line].spacing(2),
        Space::new().width(Length::Fill),
        call_btn,
        Space::new().width(4),
        block_btn,
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

fn settings_modal(settings: &IntegrationSettings) -> Element<'_, Message> {
    let title = text("Call History & Blocking Settings").size(20);

    // 1. History settings
    let history_toggle = checkbox(settings.local_history_enabled)
        .label("Record in-app call history to local storage")
        .on_toggle(Message::ToggleLocalHistory)
        .size(15);

    let clear_btn = button(text("🗑 Clear Local Call History").size(13))
        .on_press(Message::ClearHistoryPressed)
        .padding([6, 12]);

    let history_sec = column![
        text("Call History Storage").size(16),
        history_toggle,
        Space::new().height(4),
        clear_btn,
    ].spacing(8);

    // 2. Blocked numbers list
    let mut blocked_rows = column![].spacing(6);
    if settings.blocked_numbers.is_empty() {
        blocked_rows = blocked_rows.push(
            text("No phone numbers blocked yet.")
                .size(13)
                .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
        );
    } else {
        for b in &settings.blocked_numbers {
            let num = b.number.clone();
            let b_row = row![
                text(format!("⊘ {} ({})", b.number, b.action)).size(13),
                Space::new().width(Length::Fill),
                button(text("Unblock").size(12))
                    .on_press(Message::UnblockNumber(num))
                    .padding([2, 8]),
            ].align_y(Alignment::Center);
            blocked_rows = blocked_rows.push(b_row);
        }
    }

    let default_action_picker = row![
        text("Default action for blocked calls:").size(13),
        Space::new().width(8),
        pick_list(
            &BlockAction::ALL[..],
            Some(settings.default_block_action),
            Message::DefaultBlockActionChanged
        ),
    ].align_y(Alignment::Center);

    let blocking_sec = column![
        text("Call Blocking Rules").size(16),
        default_action_picker,
        Space::new().height(4),
        blocked_rows,
    ].spacing(8);

    let done_btn = button(text("Done").size(14))
        .on_press(Message::ToggleSettingsModal)
        .padding([6, 20]);

    let card = column![
        title,
        Space::new().height(6),
        history_sec,
        Space::new().height(8),
        blocking_sec,
        Space::new().height(12),
        row![Space::new().width(Length::Fill), done_btn],
    ]
    .spacing(12)
    .padding(20)
    .max_width(500);

    let scroller = scrollable(card).width(Length::Fill).height(Length::Fill);

    container(container(scroller).style(|theme: &iced::Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(palette.background.base.color.into()),
            border: iced::border::rounded(8),
            ..container::Style::default()
        }
    }))
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .padding(16)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn block_number_modal<'a>(number: &'a str, name: Option<&'a str>) -> Element<'a, Message> {
    let title = text("Block Phone Number").size(20);
    let prompt_msg = text(format!(
        "Select an action for calls incoming from {}{}:",
        name.unwrap_or(""),
        if name.is_some() { format!(" ({number})") } else { number.to_string() }
    ))
    .size(14);

    let reject_btn = button(text("Reject (Instant SIP 603)").size(13))
        .on_press(Message::ConfirmBlockNumber(number.to_string(), name.map(String::from), BlockAction::Reject))
        .padding([8, 14]);

    let mute_btn = button(text("Mute (Silent Ring)").size(13))
        .on_press(Message::ConfirmBlockNumber(number.to_string(), name.map(String::from), BlockAction::Mute))
        .padding([8, 14]);

    let cancel_btn = button(text("Cancel").size(13))
        .on_press(Message::CancelBlockPrompt)
        .padding([8, 14]);

    let card = column![
        title,
        Space::new().height(6),
        prompt_msg,
        Space::new().height(16),
        row![reject_btn, Space::new().width(10), mute_btn, Space::new().width(Length::Fill), cancel_btn].align_y(Alignment::Center),
    ]
    .spacing(10)
    .padding(20)
    .max_width(460);

    container(container(card).style(|theme: &iced::Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(palette.background.base.color.into()),
            border: iced::border::rounded(8),
            ..container::Style::default()
        }
    }))
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .width(Length::Fill)
    .height(Length::Fill)
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
