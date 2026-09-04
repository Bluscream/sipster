//! Contacts window: state, messages and rendering.
//!
//! Shows contacts and nothing else. Provider credentials (FRITZ!Box, Google,
//! `CardDAV`) used to be configured from a modal in here, which split account
//! setup across two windows and meant this one owned draft credential fields it
//! had no business holding. They live in Settings, next to every other account.

use iced::widget::{column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Length, Padding};
use sipster_core::BlockAction;
use sipster_integrations::{normalize_number, number_contains, Contact, RecordSource};

use crate::ui;

#[derive(Debug, Clone)]
pub enum Message {
    SearchChanged(String),
    SyncPressed,
    ContactsLoaded(Vec<Contact>),
    /// Selects a contact, or clears the selection when it is already current.
    Select(String),
    DialContact(String),

    // Editing:
    OpenNewContact,
    OpenEditContact(Contact),
    EditNameChanged(String),
    EditPhoneChanged(String),
    EditEmailChanged(String),
    SaveContact,
    CancelEditContact,
    DeleteContact(String),

    // Blocking:
    BlockNumberPrompt(String, Option<String>),
    ConfirmBlockNumber(String, Option<String>, BlockAction),
    CancelBlockPrompt,
}

#[derive(Debug, Clone)]
pub struct EditContactDraft {
    pub id: Option<String>,
    pub name: String,
    pub phone: String,
    pub email: String,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub search: String,
    pub contacts: Vec<Contact>,
    pub loading: bool,
    pub error: Option<String>,
    /// Id of the expanded contact, if any.
    pub selected: Option<String>,
    pub edit_draft: Option<EditContactDraft>,
    pub block_prompt: Option<(String, Option<String>)>,
}

impl State {
    /// Toggles selection: clicking the open row closes it.
    pub fn toggle(&mut self, id: &str) {
        if self.selected.as_deref() == Some(id) {
            self.selected = None;
        } else {
            self.selected = Some(id.to_string());
        }
    }

    /// Contacts matching the search box.
    ///
    /// Numbers are compared normalized, so "030 123" finds `+49301234` —
    /// searching by a number as written in a phonebook used to miss.
    fn matching(&self) -> Vec<&Contact> {
        let needle = self.search.trim().to_lowercase();
        if needle.is_empty() {
            return self.contacts.iter().collect();
        }
        let digits = normalize_number(&needle);
        self.contacts
            .iter()
            .filter(|c| {
                if c.name.to_lowercase().contains(&needle) {
                    return true;
                }
                !digits.is_empty()
                    && c.numbers
                        .iter()
                        .any(|n| number_contains(&n.number, &digits))
            })
            .collect()
    }
}

pub fn view(state: &State) -> Element<'_, Message> {
    if let Some(draft) = &state.edit_draft {
        return edit_contact(draft);
    }
    if let Some((number, name)) = &state.block_prompt {
        return block_prompt(number, name.as_deref());
    }

    let matching = state.matching();
    let subtitle = if state.search.trim().is_empty() {
        format!("{} contacts", state.contacts.len())
    } else {
        format!("{} of {} contacts", matching.len(), state.contacts.len())
    };

    let toolbar = ui::toolbar(
        "Contacts",
        subtitle,
        vec![
            ui::tool_button("New", Some(Message::OpenNewContact)),
            ui::tool_button(
                if state.loading { "Syncing…" } else { "Sync" },
                (!state.loading).then_some(Message::SyncPressed),
            ),
        ],
    );

    let search = text_input("Search by name or number", &state.search)
        .on_input(Message::SearchChanged)
        .padding(8)
        .size(14);

    let body: Element<'_, Message> = if state.loading && state.contacts.is_empty() {
        ui::empty_state("Syncing…", "Fetching contacts from your providers.")
    } else if state.contacts.is_empty() {
        ui::empty_state(
            "No contacts yet",
            "Add one with New, or enable a provider in Settings › Integrations.",
        )
    } else if matching.is_empty() {
        ui::empty_state("No matches", "Nothing here matches that search.")
    } else {
        let mut list = column![].width(Length::Fill);
        for (index, contact) in matching.iter().enumerate() {
            if index > 0 {
                list = list.push(ui::separator());
            }
            list = list.push(contact_row(contact, state.selected.as_deref()));
        }
        scrollable(list).height(Length::Fill).into()
    };

    let mut content = column![toolbar, search].spacing(10);
    if let Some(error) = &state.error {
        content = content.push(
            text(error.clone())
                .size(12)
                .color(iced::Color::from_rgb(0.92, 0.35, 0.35)),
        );
    }
    content = content.push(body);

    container(content.padding(Padding::new(16.0)))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// One contact. Collapsed it is a name and a one-line number summary; expanded
/// it lists each number with its own actions.
fn contact_row<'a>(contact: &'a Contact, selected: Option<&str>) -> Element<'a, Message> {
    let is_selected = selected == Some(contact.id.as_str());

    let summary_line = if contact.numbers.is_empty() {
        "no number".to_string()
    } else {
        contact
            .numbers
            .iter()
            .map(|n| n.number.as_str())
            .collect::<Vec<_>>()
            .join("  ·  ")
    };

    // Router phonebooks frequently store a bare number as the name. Printing
    // it again underneath is pure noise, so the second line is dropped when it
    // says nothing new.
    let mut heading = column![text(&contact.name).size(15)].spacing(1);
    if normalize_number(&summary_line) != normalize_number(&contact.name)
        || contact.numbers.len() > 1
    {
        heading = heading.push(ui::caption(summary_line));
    }

    let summary = row![
        heading,
        Space::new().width(Length::Fill),
        // Bounded so a long phonebook name cannot squeeze out the contact.
        container(ui::caption(contact.source.to_string())).max_width(150),
    ]
    .align_y(Alignment::Center)
    .spacing(10)
    .into();

    let expanded = is_selected.then(|| contact_detail(contact));

    ui::list_row(
        summary,
        expanded,
        is_selected,
        Message::Select(contact.id.clone()),
    )
}

fn contact_detail(contact: &Contact) -> Element<'_, Message> {
    let mut detail = column![].spacing(5).padding(Padding::from([4, 0]));

    for number in &contact.numbers {
        detail = detail.push(
            row![
                text(format!("{}", number.number_type)).size(12).width(Length::Fixed(64.0)),
                text(&number.number).size(13),
                Space::new().width(Length::Fill),
                ui::row_action("Call", Message::DialContact(number.number.clone())),
                ui::row_action_danger(
                    "Block",
                    Message::BlockNumberPrompt(
                        number.number.clone(),
                        Some(contact.name.clone()),
                    ),
                ),
            ]
            .align_y(Alignment::Center)
            .spacing(6),
        );
    }

    if !contact.emails.is_empty() {
        detail = detail.push(ui::caption(contact.emails.join("  ·  ")));
    }

    // Only local contacts are editable; the rest are owned by their provider,
    // so offering Edit on them would promise something sync would overwrite.
    let editable = matches!(contact.source, RecordSource::Local);
    let mut actions = row![].spacing(6);
    if editable {
        actions = actions
            .push(ui::row_action("Edit", Message::OpenEditContact(contact.clone())))
            .push(ui::row_action_danger(
                "Delete",
                Message::DeleteContact(contact.id.clone()),
            ));
    } else {
        actions = actions.push(ui::caption(format!(
            "Synced from {} — edit it there",
            contact.source
        )));
    }

    column![detail, actions].spacing(6).into()
}

/// Full-window editor, reached from a row and dismissed back to the list.
fn edit_contact(draft: &EditContactDraft) -> Element<'_, Message> {
    let heading = if draft.id.is_some() { "Edit contact" } else { "New contact" };

    let field = |label: &'static str, placeholder: &'static str, value: &str,
                 on_change: fn(String) -> Message| {
        row![
            text(label).size(13).width(Length::Fixed(80.0)),
            text_input(placeholder, value).on_input(on_change).padding(7).size(14),
        ]
        .align_y(Alignment::Center)
        .spacing(10)
    };

    let can_save = !draft.name.trim().is_empty() && !normalize_number(&draft.phone).is_empty();

    let content = column![
        ui::toolbar(
            heading,
            "Stored on this computer only".into(),
            vec![
                ui::tool_button("Cancel", Some(Message::CancelEditContact)),
                ui::tool_button("Save", can_save.then_some(Message::SaveContact)),
            ],
        ),
        Space::new().height(6),
        field("Name", "Full name", &draft.name, Message::EditNameChanged),
        field("Number", "+49 30 123456", &draft.phone, Message::EditPhoneChanged),
        field("Email", "optional", &draft.email, Message::EditEmailChanged),
        Space::new().height(4),
        // Say why Save is unavailable rather than leaving a dead button.
        if can_save {
            ui::caption("")
        } else {
            ui::caption("A name and at least one number are required.")
        },
    ]
    .spacing(8)
    .padding(Padding::new(16.0));

    container(content).width(Length::Fill).height(Length::Fill).into()
}

/// Confirmation for adding a block rule, shared in shape with the history one.
fn block_prompt<'a>(number: &'a str, name: Option<&'a str>) -> Element<'a, Message> {
    let who = name.map_or_else(|| number.to_string(), |n| format!("{n} ({number})"));

    let content = column![
        text("Block this caller?").size(18),
        ui::caption(who),
        Space::new().height(10),
        text("Reject answers with SIP 603 immediately. Mute lets it ring silently, with no notification.")
            .size(12),
        Space::new().height(12),
        row![
            ui::tool_button("Cancel", Some(Message::CancelBlockPrompt)),
            Space::new().width(Length::Fill),
            ui::tool_button(
                "Mute",
                Some(Message::ConfirmBlockNumber(
                    number.to_string(),
                    name.map(str::to_string),
                    BlockAction::Mute,
                )),
            ),
            ui::tool_button(
                "Reject",
                Some(Message::ConfirmBlockNumber(
                    number.to_string(),
                    name.map(str::to_string),
                    BlockAction::Reject,
                )),
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(4)
    .padding(Padding::new(20.0))
    .max_width(420);

    container(content)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::State;
    use sipster_integrations::{Contact, NumberType, PhoneNumber, RecordSource};

    fn contact(name: &str, number: &str) -> Contact {
        Contact {
            id: format!("local-{name}"),
            name: name.into(),
            numbers: vec![PhoneNumber {
                number: number.into(),
                number_type: NumberType::Home,
                priority: 1,
            }],
            emails: Vec::new(),
            source: RecordSource::Local,
        }
    }

    fn state() -> State {
        State {
            contacts: vec![
                contact("Alice Smith", "+49301234567"),
                contact("Bob Jones", "**610"),
            ],
            ..State::default()
        }
    }

    #[test]
    fn an_empty_search_matches_everything() {
        let mut s = state();
        s.search = "   ".into();
        assert_eq!(s.matching().len(), 2);
    }

    #[test]
    fn matches_on_name_case_insensitively() {
        let mut s = state();
        s.search = "alice".into();
        assert_eq!(s.matching().len(), 1);
    }

    /// A number typed the way a human writes it must still find the contact;
    /// the old substring compare missed anything with separators.
    #[test]
    fn matches_a_number_written_with_separators() {
        let mut s = state();
        s.search = "030 123".into();
        assert_eq!(s.matching().len(), 1, "spaced number should match");

        s.search = "+49 30 1234".into();
        assert_eq!(s.matching().len(), 1, "international form should match");
    }

    #[test]
    fn a_non_matching_search_returns_nothing() {
        let mut s = state();
        s.search = "zzz".into();
        assert!(s.matching().is_empty());
    }

    /// Letters in the search box must not be read as a number and match
    /// everything by way of an empty digit string.
    #[test]
    fn a_letters_only_search_does_not_match_numbers() {
        let mut s = state();
        s.search = "xyz".into();
        assert!(s.matching().is_empty());
    }

    #[test]
    fn selecting_the_open_row_closes_it() {
        let mut s = state();
        s.toggle("local-Alice Smith");
        assert_eq!(s.selected.as_deref(), Some("local-Alice Smith"));
        s.toggle("local-Alice Smith");
        assert_eq!(s.selected, None);
        s.toggle("local-Bob Jones");
        assert_eq!(s.selected.as_deref(), Some("local-Bob Jones"));
    }
}
