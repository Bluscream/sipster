//! Contacts window: state, messages and rendering.
//!
//! Shows contacts and nothing else. Provider credentials (FRITZ!Box, Google,
//! `CardDAV`) used to be configured from a modal in here, which split account
//! setup across two windows and meant this one owned draft credential fields it
//! had no business holding. They live in Settings, next to every other account.

use iced::widget::{checkbox, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Length, Padding};
use sipster_core::BlockAction;
use sipster_integrations::{normalize_number, number_contains, Contact, RecordSource};

use crate::ui;

/// A name, number or address as it should appear, masked in streaming mode.
fn show(value: &str, mask: bool) -> String {
    if mask {
        sipster_core::mask_identity(value)
    } else {
        value.to_string()
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    SearchChanged(String),
    SyncPressed,
    ToggleFilterMenu,
    /// Show or hide one contact source by its display name.
    ToggleSource(String, bool),
    /// One batch from one provider, appended as it arrives.
    ContactsBatch(Vec<Contact>),
    SyncFinished,
    /// Selects a contact, or clears the selection when it is already current.
    Select(String),
    DialContact(String),

    OpenEditContact(Contact),
    DeleteContact(String),

    // Blocking:
    BlockNumberPrompt(String, Option<String>),
    ConfirmBlockNumber(String, Option<String>, BlockAction),
    CancelBlockPrompt,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub search: String,
    pub contacts: Vec<Contact>,
    pub loading: bool,
    pub error: Option<String>,
    /// Id of the expanded contact, if any.
    pub selected: Option<String>,
    pub block_prompt: Option<(String, Option<String>)>,
    /// Sources the user has switched off, by their display name.
    ///
    /// Held as a set of hidden names rather than shown ones so that a source
    /// appearing for the first time — a newly added Google account, a
    /// phonebook the router only just returned — is visible by default.
    pub hidden_sources: std::collections::BTreeSet<String>,
    /// Whether the Filter dropdown is open.
    pub filter_open: bool,
}

impl State {
    /// Every source present in the loaded contacts, with how many each holds.
    ///
    /// Derived from the contacts rather than from configuration, so it lists
    /// what actually arrived — one entry per Google account, per FRITZ!Box
    /// phonebook, per vCard folder.
    #[must_use]
    pub fn sources(&self) -> Vec<(String, usize)> {
        let mut counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for contact in &self.contacts {
            *counts.entry(contact.source.to_string()).or_default() += 1;
        }
        counts.into_iter().collect()
    }

    /// Shows or hides one source.
    pub fn toggle_source(&mut self, source: &str, shown: bool) {
        if shown {
            self.hidden_sources.remove(source);
        } else {
            self.hidden_sources.insert(source.to_owned());
        }
    }

    /// Whether `source` is currently shown.
    #[must_use]
    pub fn source_shown(&self, source: &str) -> bool {
        !self.hidden_sources.contains(source)
    }

    /// Toggles selection: clicking the open row closes it.
    pub fn toggle(&mut self, id: &str) {
        if self.selected.as_deref() == Some(id) {
            self.selected = None;
        } else {
            self.selected = Some(id.to_string());
        }
    }

    /// Merges an incoming batch, keeping the list sorted and de-duplicated so
    /// it stays coherent while providers are still arriving.
    pub fn merge(&mut self, batch: Vec<Contact>) {
        self.contacts.extend(batch);
        self.contacts.sort_by_cached_key(|c| c.name.trim().to_lowercase());
        self.contacts.dedup_by(|a, b| {
            let same_name = a.name.trim().eq_ignore_ascii_case(b.name.trim());
            let same_number = if a.numbers.is_empty() && b.numbers.is_empty() {
                true
            } else {
                a.numbers.iter().any(|na| {
                    let norm_a = normalize_number(&na.number);
                    b.numbers.iter().any(|nb| norm_a == normalize_number(&nb.number))
                })
            };
            same_name && same_number
        });
    }

    /// Contacts matching the search box.
    ///
    /// Numbers are compared normalized, so "030 123" finds `+49301234` —
    /// searching by a number as written in a phonebook used to miss.
    fn matching(&self) -> Vec<&Contact> {
        let needle = self.search.trim().to_lowercase();
        let digits = normalize_number(&needle);
        // Source first, then search. Filtering only inside the search branch
        // left the common case — no search term — showing everything, which
        // made the filter look broken exactly when it was most likely used.
        self.contacts
            .iter()
            .filter(|c| self.source_shown(&c.source.to_string()))
            .filter(|c| {
                if needle.is_empty() || c.name.to_lowercase().contains(&needle) {
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

pub fn view(state: &State, mask: bool) -> Element<'_, Message> {

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
            ui::tool_button_owned(
                if state.hidden_sources.is_empty() {
                    "Filter".to_string()
                } else {
                    // Say how many are off, so a half-empty list is never a
                    // mystery.
                    format!("Filter ({})", state.hidden_sources.len())
                },
                Some(Message::ToggleFilterMenu),
            ),
            ui::tool_button(
                if state.loading { "Syncing…" } else { "Sync" },
                (!state.loading).then_some(Message::SyncPressed),
            ),
        ],
    );

    let filter_menu = filter_menu(state);

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
            list = list.push(contact_row(contact, state.selected.as_deref(), mask));
        }
        scrollable(list).height(Length::Fill).into()
    };

    let mut content = column![toolbar].spacing(10);
    if let Some(menu) = filter_menu {
        content = content.push(menu);
    }
    content = content.push(search);
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
fn contact_row<'a>(
    contact: &'a Contact,
    selected: Option<&str>,
    mask: bool,
) -> Element<'a, Message> {
    let is_selected = selected == Some(contact.id.as_str());

    let summary_line = if contact.numbers.is_empty() {
        "no number".to_string()
    } else {
        contact
            .numbers
            .iter()
            .map(|n| show(&n.number, mask))
            .collect::<Vec<_>>()
            .join("  ·  ")
    };

    // Router phonebooks frequently store a bare number as the name. Printing
    // it again underneath is pure noise, so the second line is dropped when it
    // says nothing new.
    let mut heading = column![text(show(&contact.name, mask)).size(15)].spacing(1);
    if mask
        || normalize_number(&summary_line) != normalize_number(&contact.name)
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

    let expanded = is_selected.then(|| contact_detail(contact, mask));

    ui::list_row(
        summary,
        expanded,
        is_selected,
        Message::Select(contact.id.clone()),
    )
}

fn contact_detail(contact: &Contact, mask: bool) -> Element<'_, Message> {
    let mut detail = column![].spacing(5).padding(Padding::from([4, 0]));

    for number in &contact.numbers {
        detail = detail.push(
            row![
                text(format!("{}", number.number_type)).size(12).width(Length::Fixed(64.0)),
                text(show(&number.number, mask)).size(13),
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
        let emails: Vec<String> = contact.emails.iter().map(|e| show(e, mask)).collect();
        detail = detail.push(ui::caption(emails.join("  ·  ")));
    }

    let editable_externally = match contact.source {
        RecordSource::Google { .. } | RecordSource::Local | RecordSource::FritzBox { .. } => true,
        _ => false,
    };
    
    let mut actions = row![].spacing(6);
    if editable_externally {
        actions = actions
            .push(ui::row_action("Edit", Message::OpenEditContact(contact.clone())));
    }
    if matches!(contact.source, RecordSource::Local) {
        actions = actions
            .push(ui::row_action_danger(
                "Delete",
                Message::DeleteContact(contact.id.clone()),
            ));
    } else {
        actions = actions.push(ui::caption(format!(
            "Synced from {}",
            contact.source
        )));
    }

    column![detail, actions].spacing(6).into()
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

/// The Filter dropdown: one toggle per source the contacts actually came from.
///
/// `None` when closed. Rendered inline under the toolbar rather than as an
/// overlay — iced has no popup primitive, and a panel that pushes the list
/// down is easier to read than one that covers it.
fn filter_menu(state: &State) -> Option<Element<'_, Message>> {
    if !state.filter_open {
        return None;
    }

    let sources = state.sources();
    if sources.is_empty() {
        return Some(ui::caption("Nothing synced yet"));
    }

    let mut list = column![].spacing(4);
    for (source, count) in sources {
        let shown = state.source_shown(&source);
        let label = format!("{source}  ({count})");
        let name = source.clone();
        list = list.push(
            checkbox(shown)
                .label(label)
                .on_toggle(move |on| Message::ToggleSource(name.clone(), on))
                .size(15)
                .text_size(13),
        );
    }

    Some(
        container(list)
            .padding(10)
            .width(Length::Fill)
            .style(container::bordered_box)
            .into(),
    )
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

    /// A contact from `source`, for the source-filter tests.
    fn from_source(name: &str, source: RecordSource) -> Contact {
        Contact { source, ..contact(name, "+49301111111") }
    }

    fn mixed() -> State {
        State {
            contacts: vec![
                from_source("Ada", RecordSource::Google { email: "a@example.com".into() }),
                from_source("Grace", RecordSource::Google { email: "b@example.com".into() }),
                from_source(
                    "Router Entry",
                    RecordSource::FritzBox { phonebook_id: 0, phonebook_name: "Telefonbuch".into() },
                ),
                from_source("Local Person", RecordSource::Local),
            ],
            ..State::default()
        }
    }

    /// One entry per source that actually returned contacts, with counts —
    /// two Google accounts are two entries, not one.
    #[test]
    fn sources_are_listed_per_account_with_counts() {
        let sources = mixed().sources();
        let names: Vec<&str> = sources.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "FRITZ!Box (Telefonbuch)",
                "Google (a@example.com)",
                "Google (b@example.com)",
                "Local",
            ]
        );
        assert!(sources.iter().all(|(_, count)| *count == 1));
    }

    /// The filter must work with no search term — the common case, and the
    /// one an early return for an empty search originally skipped.
    #[test]
    fn hiding_a_source_works_without_a_search_term() {
        let mut s = mixed();
        assert!(s.search.is_empty());
        s.toggle_source("Local", false);
        assert!(!s.matching().iter().any(|c| c.name == "Local Person"));
    }

    #[test]
    fn hiding_a_source_removes_only_its_contacts() {
        let mut s = mixed();
        s.toggle_source("Google (a@example.com)", false);

        let shown: Vec<&str> = s.matching().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(shown, vec!["Grace", "Router Entry", "Local Person"]);
        assert!(!s.source_shown("Google (a@example.com)"));
        assert!(s.source_shown("Google (b@example.com)"));
    }

    #[test]
    fn showing_a_source_again_brings_its_contacts_back() {
        let mut s = mixed();
        s.toggle_source("Local", false);
        assert_eq!(s.matching().len(), 3);
        s.toggle_source("Local", true);
        assert_eq!(s.matching().len(), 4);
        assert!(s.hidden_sources.is_empty());
    }

    /// A source that appears for the first time — a newly added account, a
    /// phonebook the router only just returned — must be visible without the
    /// user going to look for it.
    #[test]
    fn a_new_source_is_shown_by_default() {
        let mut s = mixed();
        s.toggle_source("Google (a@example.com)", false);
        s.contacts.push(from_source(
            "Newcomer",
            RecordSource::CardDav { account: "work".into() },
        ));
        assert!(s.source_shown("CardDAV (work)"));
        assert!(s.matching().iter().any(|c| c.name == "Newcomer"));
    }

    /// Filtering and searching have to compose, not override each other.
    #[test]
    fn the_source_filter_applies_on_top_of_the_search() {
        let mut s = mixed();
        s.search = "a".into();
        let before = s.matching().len();
        s.toggle_source("Google (a@example.com)", false);
        assert_eq!(s.matching().len(), before - 1);
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
