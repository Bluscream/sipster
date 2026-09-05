//! The form controls the settings pages are built from.
//!
//! Kept apart from the pages themselves: these know nothing about accounts or
//! providers, only about looking and behaving like the rest of the window.

use iced::widget::{button, column, container, row, rule, stack, text, text_input};
use iced::{Alignment, Element, Length};

use super::Message;

pub(crate) fn section<'a>(
    title: impl iced::widget::text::IntoFragment<'a>,
    hint: Option<impl iced::widget::text::IntoFragment<'a>>,
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
pub(crate) fn field<'a>(
    label: impl iced::widget::text::IntoFragment<'a>,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
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
pub(crate) fn secret_input<'a>(
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

pub(crate) fn file_input<'a>(
    placeholder: &'a str,
    value: &'a str,
    on_change: impl Fn(String) -> Message + 'a,
    on_pick: Message,
) -> Element<'a, Message> {
    let field = text_input(placeholder, value)
        .on_input(on_change)
        .padding(iced::Padding::from(7).right(30))
        .size(14);

    // Geometric symbol rather than emoji so it renders on standard fonts
    let icon = button(text("▤").size(14))
        .on_press(on_pick)
        .padding([2, 6])
        .style(move |theme: &iced::Theme, status| {
            let palette = theme.extended_palette();
            button::Style {
                background: None,
                text_color: if matches!(status, button::Status::Hovered) {
                    palette.background.base.text
                } else {
                    palette.background.strong.color
                },
                ..button::Style::default()
            }
        });

    stack![
        field,
        container(icon)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Center),
    ]
    .into()
}

pub(crate) fn input<'a>(
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
