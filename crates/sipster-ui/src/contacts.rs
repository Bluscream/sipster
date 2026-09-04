//! Contacts window: state, messages and rendering.

use iced::widget::{button, checkbox, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Length, Padding};
use sipster_core::{BlockAction, IntegrationSettings};
use sipster_integrations::{Contact, RecordSource};

#[derive(Debug, Clone)]
pub enum Message {
    SearchChanged(String),
    SyncPressed,
    ContactsLoaded(Vec<Contact>),
    DialContact(String),

    // Contact modal:
    OpenNewContact,
    OpenEditContact(Contact),
    EditNameChanged(String),
    EditPhoneChanged(String),
    EditEmailChanged(String),
    SaveContact,
    CancelEditContact,
    DeleteContact(String),

    // Providers & Settings modal:
    ToggleProvidersModal,
    FritzHostChanged(String),
    FritzPortChanged(String),
    FritzUserChanged(String),
    FritzPassChanged(String),
    FritzEnabledToggled(bool),

    // Google accounts:
    ConnectGoogleAccount,
    GoogleAuthFinished(Result<(String, String), String>), // (email, refresh_token)
    RemoveGoogleAccount(String),

    // CardDAV accounts:
    CardDavUrlChanged(String),
    CardDavUserChanged(String),
    CardDavPassChanged(String),
    AddCardDavAccount,
    RemoveCardDavAccount(String),

    // Block number:
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

    // Overlay state:
    pub edit_draft: Option<EditContactDraft>,
    pub show_providers_modal: bool,
    pub block_prompt: Option<(String, Option<String>)>,

    // Draft inputs for providers:
    pub draft_carddav_url: String,
    pub draft_carddav_user: String,
    pub draft_carddav_pass: String,
}

pub fn view<'a>(state: &'a State, settings: &'a IntegrationSettings) -> Element<'a, Message> {
    if let Some(draft) = &state.edit_draft {
        return edit_contact_modal(draft);
    }

    if state.show_providers_modal {
        return providers_modal(state, settings);
    }

    if let Some((number, name)) = &state.block_prompt {
        return block_number_modal(number, name.as_deref());
    }

    let title = text("Contacts").size(22);
    let count_text = text(format!("{} contacts", state.contacts.len()))
        .size(13)
        .color(iced::Color::from_rgb(0.6, 0.6, 0.6));

    let add_btn = button(text("+ New Contact").size(13))
        .on_press(Message::OpenNewContact)
        .padding([6, 12]);

    let providers_btn = button(text("⚙ Providers").size(13))
        .on_press(Message::ToggleProvidersModal)
        .padding([6, 12]);

    let sync_btn = button(text(if state.loading { "Syncing…" } else { "⟳ Sync" }).size(13))
        .on_press_maybe(if state.loading { None } else { Some(Message::SyncPressed) })
        .padding([6, 12]);

    let top_bar = row![
        column![title, count_text].spacing(2),
        Space::new().width(Length::Fill),
        add_btn,
        Space::new().width(4),
        providers_btn,
        Space::new().width(4),
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
        container(text("Syncing contacts from providers…").size(14))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    } else if filtered.is_empty() {
        container(
            text(if state.search.is_empty() {
                "No contacts found. Add one with '+ New Contact' or sync from FRITZ!Box / Google / CardDAV."
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

    let is_local = matches!(contact.source, RecordSource::Local);

    let edit_btn = if is_local {
        button(text("✏ Edit").size(12))
            .on_press(Message::OpenEditContact(contact.clone()))
            .padding([3, 7])
    } else {
        button(text("👁 View").size(12))
            .on_press(Message::OpenEditContact(contact.clone()))
            .padding([3, 7])
    };

    let mut numbers_col = column![].spacing(4);
    for num in &contact.numbers {
        let number_str = num.number.clone();
        let call_btn = button(text("📞 Call").size(12))
            .on_press(Message::DialContact(number_str.clone()))
            .padding([3, 8]);

        let block_btn = button(text("⊘ Block").size(12))
            .on_press(Message::BlockNumberPrompt(number_str.clone(), Some(contact.name.clone())))
            .padding([3, 6]);

        let num_row = row![
            text(format!("{}: {}", num.number_type, num.number)).size(13),
            Space::new().width(Length::Fill),
            call_btn,
            Space::new().width(4),
            block_btn,
        ]
        .align_y(Alignment::Center)
        .spacing(4);

        numbers_col = numbers_col.push(num_row);
    }

    let mut header_row = row![name, Space::new().width(Length::Fill), edit_btn, Space::new().width(6), source_badge]
        .align_y(Alignment::Center);

    if is_local {
        let del_btn = button(text("🗑").size(12))
            .on_press(Message::DeleteContact(contact.id.clone()))
            .padding([3, 6]);
        header_row = header_row.push(Space::new().width(4)).push(del_btn);
    }

    let card_content = column![
        header_row,
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

fn edit_contact_modal(draft: &EditContactDraft) -> Element<'_, Message> {
    let title = text(if draft.id.is_some() { "Edit Contact" } else { "New Contact" }).size(20);

    let name_input = column![
        text("Name:").size(13),
        text_input("Contact name…", &draft.name)
            .on_input(Message::EditNameChanged)
            .padding(8),
    ].spacing(4);

    let phone_input = column![
        text("Phone Number:").size(13),
        text_input("Telephone or extension…", &draft.phone)
            .on_input(Message::EditPhoneChanged)
            .padding(8),
    ].spacing(4);

    let email_input = column![
        text("Email:").size(13),
        text_input("Email address (optional)…", &draft.email)
            .on_input(Message::EditEmailChanged)
            .padding(8),
    ].spacing(4);

    let buttons = row![
        Space::new().width(Length::Fill),
        button(text("Cancel").size(14))
            .on_press(Message::CancelEditContact)
            .padding([6, 14]),
        Space::new().width(10),
        button(text("Save Contact").size(14))
            .on_press(Message::SaveContact)
            .padding([6, 14]),
    ].align_y(Alignment::Center);

    let card = column![
        title,
        Space::new().height(10),
        name_input,
        phone_input,
        email_input,
        Space::new().height(16),
        buttons,
    ]
    .spacing(12)
    .padding(24)
    .max_width(420);

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

fn providers_modal<'a>(state: &'a State, settings: &'a IntegrationSettings) -> Element<'a, Message> {
    let title = text("Contacts & History Providers").size(20);

    // 1. Google Accounts Section
    let mut google_rows = column![].spacing(6);
    for acc in &settings.google_accounts {
        let acc_id = acc.id.clone();
        let acc_row = row![
            text(format!("Google: {}", acc.email)).size(13),
            Space::new().width(Length::Fill),
            button(text("Disconnect").size(12))
                .on_press(Message::RemoveGoogleAccount(acc_id))
                .padding([3, 8]),
        ]
        .align_y(Alignment::Center);
        google_rows = google_rows.push(acc_row);
    }

    let add_google_btn = button(text("+ Sign in with Google (OAuth 2.0)").size(13))
        .on_press(Message::ConnectGoogleAccount)
        .padding([6, 12]);

    let google_section = column![
        text("Google Contacts (Multi-Account)").size(16),
        text("Connect unlimited Google accounts with automatic OAuth sync.").size(12).color(iced::Color::from_rgb(0.6, 0.6, 0.6)),
        google_rows,
        add_google_btn,
    ].spacing(8);

    // 2. CardDAV Section
    let mut carddav_rows = column![].spacing(6);
    for cd in &settings.carddav_accounts {
        let cd_id = cd.id.clone();
        let row = row![
            text(format!("CardDAV: {}", cd.url)).size(13),
            Space::new().width(Length::Fill),
            button(text("Remove").size(12))
                .on_press(Message::RemoveCardDavAccount(cd_id))
                .padding([3, 8]),
        ]
        .align_y(Alignment::Center);
        carddav_rows = carddav_rows.push(row);
    }

    let carddav_form = row![
        text_input("CardDAV URL…", &state.draft_carddav_url).on_input(Message::CardDavUrlChanged).padding(6).width(Length::FillPortion(2)),
        text_input("User…", &state.draft_carddav_user).on_input(Message::CardDavUserChanged).padding(6).width(Length::FillPortion(1)),
        text_input("Password…", &state.draft_carddav_pass).on_input(Message::CardDavPassChanged).padding(6).secure(true).width(Length::FillPortion(1)),
        button(text("+ Add").size(13)).on_press(Message::AddCardDavAccount).padding([6, 12]),
    ].spacing(6);

    let carddav_section = column![
        text("CardDAV / vCard Servers").size(16),
        carddav_rows,
        carddav_form,
    ].spacing(8);

    // 3. FRITZ!Box Section
    let fb = &settings.fritzbox;
    let fb_toggle = checkbox(fb.enabled)
        .label("Enable FRITZ!Box TR-064 synchronization")
        .on_toggle(Message::FritzEnabledToggled)
        .size(15);

    let fb_form = column![
        row![
            column![text("Host:").size(12), text_input("192.168.2.1", &fb.host).on_input(Message::FritzHostChanged).padding(6)].width(Length::FillPortion(2)),
            column![text("Port:").size(12), text_input("49000", &fb.port.to_string()).on_input(Message::FritzPortChanged).padding(6)].width(Length::FillPortion(1)),
        ].spacing(8),
        row![
            column![text("Username:").size(12), text_input("Username…", &fb.username).on_input(Message::FritzUserChanged).padding(6)].width(Length::FillPortion(1)),
            column![text("Password:").size(12), text_input("Password…", &fb.password).on_input(Message::FritzPassChanged).padding(6).secure(true)].width(Length::FillPortion(1)),
        ].spacing(8),
    ].spacing(6);

    let fritz_section = column![
        text("FRITZ!Box Router Phonebook & Call History").size(16),
        fb_toggle,
        fb_form,
    ].spacing(8);

    let close_btn = button(text("Done").size(14))
        .on_press(Message::ToggleProvidersModal)
        .padding([6, 20]);

    let card = column![
        title,
        Space::new().height(6),
        google_section,
        Space::new().height(6),
        carddav_section,
        Space::new().height(6),
        fritz_section,
        Space::new().height(12),
        row![Space::new().width(Length::Fill), close_btn],
    ]
    .spacing(12)
    .padding(20)
    .max_width(520);

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
