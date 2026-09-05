//! Shared widget vocabulary for the list windows.
//!
//! Contacts and history are the same shape — a toolbar, a search field, and a
//! long scrollable list whose rows expand into actions — so they share the
//! pieces rather than each growing their own. Both previously carried their own
//! copies of every button, card and empty state, which is how they drifted into
//! looking like two different applications.
//!
//! The house style, matching the settings window:
//!
//! - Rows, not cards. A boxed card per contact turns a list into a wall.
//! - Actions appear on the selected row only. Putting Call and Block on every
//!   number meant a contact with four numbers rendered eight buttons.
//! - Text labels, not emoji. Emoji render at the mercy of the system font and
//!   sit at a different weight to everything around them.

use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Element, Length};

/// Muted foreground for secondary text, derived from the active theme rather
/// than hard-coded, so it stays legible in the light themes too.
pub fn muted(theme: &iced::Theme) -> iced::Color {
    theme.extended_palette().background.strong.color
}

/// Secondary label: smaller and dimmed.
pub fn caption<'a, M: 'a>(content: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(content)
        .size(12)
        .style(|theme: &iced::Theme| text::Style { color: Some(muted(theme)) })
        .into()
}

/// A window header: title, a count, and trailing controls.
pub fn toolbar<'a, M: 'a>(
    title: &'a str,
    subtitle: String,
    actions: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    let mut trailing = row![].spacing(6).align_y(Alignment::Center);
    for action in actions {
        trailing = trailing.push(action);
    }

    row![
        column![text(title).size(20), caption(subtitle)].spacing(1),
        Space::new().width(Length::Fill),
        trailing,
    ]
    .align_y(Alignment::Center)
    .into()
}

/// A compact toolbar button.
pub fn tool_button<'a, M: Clone + 'a>(label: &'a str, on_press: Option<M>) -> Element<'a, M> {
    button(text(label).size(13))
        .on_press_maybe(on_press)
        .padding([5, 11])
        .into()
}

/// As [`tool_button`], for a label built at render time.
pub fn tool_button_owned<'a, M: Clone + 'a>(label: String, on_press: Option<M>) -> Element<'a, M> {
    button(text(label).size(13))
        .on_press_maybe(on_press)
        .padding([5, 11])
        .into()
}

/// A low-emphasis action shown inside an expanded row.
pub fn row_action<'a, M: Clone + 'a>(label: &'a str, on_press: M) -> Element<'a, M> {
    button(text(label).size(12))
        .on_press(on_press)
        .padding([3, 9])
        .style(button::secondary)
        .into()
}

/// A destructive action shown inside an expanded row.
pub fn row_action_danger<'a, M: Clone + 'a>(label: &'a str, on_press: M) -> Element<'a, M> {
    button(text(label).size(12))
        .on_press(on_press)
        .padding([3, 9])
        .style(button::danger)
        .into()
}

/// A filter chip. The selected one is filled, the rest are quiet.
pub fn chip_owned<'a, M: Clone + 'a>(label: String, selected: bool, on_press: M) -> Element<'a, M> {
    button(text(label).size(12))
        .on_press(on_press)
        .padding([4, 10])
        .style(if selected { button::primary } else { button::text })
        .into()
}

/// One list row: a clickable summary that reveals `expanded` when selected.
///
/// The whole row is the hit target, so selecting is a click anywhere rather
/// than aiming at a control.
pub fn list_row<'a, M: Clone + 'a>(
    summary: Element<'a, M>,
    expanded: Option<Element<'a, M>>,
    selected: bool,
    on_select: M,
) -> Element<'a, M> {
    let head = button(summary)
        .on_press(on_select)
        .padding([8, 10])
        .width(Length::Fill)
        .style(move |theme: &iced::Theme, status| {
            let palette = theme.extended_palette();
            let background = match (selected, status) {
                (true, _) => Some(palette.background.weak.color.into()),
                (false, button::Status::Hovered) => Some(palette.background.weakest.color.into()),
                _ => None,
            };
            button::Style {
                background,
                text_color: palette.background.base.text,
                border: iced::border::rounded(5),
                ..button::Style::default()
            }
        });

    // Trailing room for the scrollbar: without it the right-hand label of
    // every row sits underneath it and is clipped.
    let head = container(head).padding(iced::Padding::default().right(10));

    let mut stack = column![head].width(Length::Fill);
    if let Some(expanded) = expanded {
        stack = stack.push(container(expanded).padding([2, 12]).width(Length::Fill));
    }
    stack.into()
}

/// Centred message for an empty or still-loading list.
pub fn empty_state<'a, M: 'a>(headline: &'a str, hint: &'a str) -> Element<'a, M> {
    container(
        column![text(headline).size(15), caption(hint)]
            .spacing(5)
            .align_x(Alignment::Center),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

/// A thin separator between rows.
pub fn separator<'a, M: 'a>() -> Element<'a, M> {
    container(Space::new().height(1))
        .width(Length::Fill)
        .style(|theme: &iced::Theme| container::Style {
            background: Some(theme.extended_palette().background.weak.color.into()),
            ..container::Style::default()
        })
        .into()
}
