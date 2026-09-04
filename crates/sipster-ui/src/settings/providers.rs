//! The Integrations and Call blocking panels.
//!
//! Where contacts and call history come from, and which callers are refused.
//! Split from the rest of Settings because between them these panels are
//! longer than every other category put together.

use iced::widget::{button, checkbox, column, pick_list, row, rule, text, text_input, Space};
use iced::{Alignment, Element, Length};
use sipster_core::{BlockAction, IntegrationSettings};

use super::{field, input, secret_input, section, Message, State};

/// Contact and history providers.
///
/// Every panel is shown inline. They used to be hidden behind a "Configure"
/// button because Settings was one long scrolling page and this section is
/// longer than all the others put together; now that each category has a page
/// of its own, the button only added a click.
pub(super) fn providers_section<'a>(
    state: &'a State,
    integration: &'a IntegrationSettings,
    config_path: &'a str,
) -> Element<'a, Message> {
    let summary = format!(
        "{} FRITZ!Box · {} Google · {} CardDAV",
        if integration.fritzbox.enabled { "1" } else { "0" },
        integration.google_accounts.iter().filter(|a| a.enabled).count(),
        integration.carddav_accounts.iter().filter(|a| a.enabled).count(),
    );

    let mut content = column![text(summary).size(13)].spacing(10);

    content = content.push(rule::horizontal(1)).push(fritzbox_panel(state, integration));
    content = content.push(rule::horizontal(1)).push(google_panel(state, integration));
    content = content.push(rule::horizontal(1)).push(carddav_panel(state, integration));
    content = content.push(rule::horizontal(1)).push(vdir_panel(state, integration));

    content = content.push(rule::horizontal(1)).push(
        checkbox(integration.local_history_enabled)
            .label("Record placed and received calls to local history")
            .on_toggle(Message::ToggleLocalHistory)
            .size(15)
            .text_size(13),
    );

    // The config path lived in an About section that was otherwise just a
    // version number; it belongs where credentials are entered.
    content = content.push(
        text(format!("Stored in {config_path}"))
            .size(11)
            .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
    );

    section(
        "Integrations",
        Some("Where contacts and call history come from."),
        content.into(),
    )
}

fn fritzbox_panel<'a>(
    state: &'a State,
    integration: &'a IntegrationSettings,
) -> Element<'a, Message> {
    let fb = &integration.fritzbox;
    // Owned: the widget borrows for 'a, and a temporary from to_string() would
    // not live that long.
    column![
        text("FRITZ!Box").size(14),
        checkbox(fb.enabled)
            .label("Sync the router phonebook and call list")
            .on_toggle(Message::FritzEnabledToggled)
            .size(15)
            .text_size(13),
        field("Host", input("fritz.box", &fb.host, Message::FritzHostChanged)),
        field("Port", port_input(&state.draft_fritz_port)),
        field("Username", input("", &fb.username, Message::FritzUserChanged)),
        field(
            "Password",
            secret_input(
                "",
                &fb.password,
                state.reveal_fritz_password,
                Message::FritzPassChanged,
                Message::RevealFritzPassword,
            )
        ),
    ]
    .spacing(8)
    .into()
}

/// The port field, whose value is an owned string rather than a temporary.
fn port_input(value: &str) -> Element<'_, Message> {
    text_input("49000", value)
        .on_input(Message::FritzPortChanged)
        .padding(7)
        .size(14)
        .into()
}

fn google_panel<'a>(
    state: &'a State,
    integration: &'a IntegrationSettings,
) -> Element<'a, Message> {
    let mut content = column![
        text("Google Contacts").size(14),
        // Sipster ships no OAuth credentials — see the note in
        // sipster-integrations::google for why bundling them is neither
        // possible nor meaningful.
        text(
            "Needs your own OAuth client: Google Cloud console › Credentials › \
             OAuth client ID › Desktop app."
        )
        .size(12),
    ]
    .spacing(6);

    for account in &integration.google_accounts {
        content = content.push(
            row![
                text(account.email.clone()).size(13),
                Space::new().width(Length::Fill),
                button(text("Remove").size(12))
                    .on_press(Message::RemoveGoogleAccount(account.id.clone()))
                    .padding([3, 9])
                    .style(button::danger),
            ]
            .align_y(Alignment::Center)
            .spacing(6),
        );
    }

    let ready = !state.draft_google_client_id.trim().is_empty()
        && !state.draft_google_client_secret.trim().is_empty();

    content
        .push(field(
            "client_secret JSON",
            text_input(
                "path to client_secret_….json downloaded from Google",
                &state.draft_google_json_path,
            )
            .on_input(Message::ImportGoogleClientJson)
            .padding(7)
            .size(14)
            .into(),
        ))
        .push(field(
            "Client ID",
            input(
                "…apps.googleusercontent.com",
                &state.draft_google_client_id,
                Message::GoogleClientIdChanged,
            ),
        ))
        .push(field(
            "Client secret",
            secret_input(
                "",
                &state.draft_google_client_secret,
                state.reveal_google_secret,
                Message::GoogleClientSecretChanged,
                Message::RevealGoogleSecret,
            ),
        ))
        .push(
            button(text("Connect a Google account").size(13))
                .on_press_maybe(ready.then_some(Message::ConnectGoogleAccount))
                .padding([5, 11]),
        )
        .into()
}

fn carddav_panel<'a>(
    state: &'a State,
    integration: &'a IntegrationSettings,
) -> Element<'a, Message> {
    let mut content = column![text("CardDAV").size(14)].spacing(6);

    for account in &integration.carddav_accounts {
        content = content.push(
            row![
                column![
                    text(account.name.clone()).size(13),
                    text(account.url.clone())
                        .size(11)
                        .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
                ]
                .spacing(1),
                Space::new().width(Length::Fill),
                button(text("Remove").size(12))
                    .on_press(Message::RemoveCardDavAccount(account.id.clone()))
                    .padding([3, 9])
                    .style(button::danger),
            ]
            .align_y(Alignment::Center)
            .spacing(6),
        );
    }

    let can_add = !state.draft_carddav_url.trim().is_empty();

    content
        .push(field(
            "URL",
            input(
                "https://dav.example.com/addressbooks/me/default/",
                &state.draft_carddav_url,
                Message::CardDavUrlChanged,
            ),
        ))
        .push(field(
            "Username",
            input("", &state.draft_carddav_user, Message::CardDavUserChanged),
        ))
        .push(field(
            "Password",
            secret_input(
                "",
                &state.draft_carddav_pass,
                state.reveal_carddav_password,
                Message::CardDavPassChanged,
                Message::RevealCardDavPassword,
            ),
        ))
        .push(
            button(text("Add address book").size(13))
                .on_press_maybe(can_add.then_some(Message::AddCardDavAccount))
                .padding([5, 11]),
        )
        .into()
}

/// Local vCard directory — the closest thing to a Linux-wide contact store.
fn vdir_panel<'a>(
    state: &'a State,
    integration: &'a IntegrationSettings,
) -> Element<'a, Message> {
    // Several can be found at once: the conventional directory plus each
    // address book Akonadi is configured with.
    let found = sipster_integrations::VdirStore::discover();
    let discovered = if found.is_empty() {
        "none found".to_string()
    } else {
        found
            .iter()
            .map(|store| store.root().display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };

    // Evolution is listed with the folder because they are the two providers
    // that need no account: both read what the desktop already has.
    let eds_available = sipster_integrations::eds_available();
    let eds_note = if eds_available {
        "Found on this desktop.".to_string()
    } else {
        "Not running on this desktop; the setting has no effect here.".to_string()
    };

    column![
        text("Evolution / GNOME Contacts").size(14),
        text(
            "The GNOME desktop's own address book, including any Google or \
             CardDAV account already added there."
        )
        .size(12),
        checkbox(integration.eds_enabled)
            .label("Read contacts from Evolution Data Server")
            .on_toggle(Message::ToggleEds)
            .size(15)
            .text_size(13),
        text(eds_note)
            .size(11)
            .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
        rule::horizontal(1),
        text("Local vCard folder").size(14),
        text(
            "A directory of .vcf files — what vdirsyncer, khard, Radicale and KDE's \
             directory address books all read and write."
        )
        .size(12),
        checkbox(integration.vdir_enabled)
            .label("Read contacts from a local vCard folder")
            .on_toggle(Message::ToggleVdir)
            .size(15)
            .text_size(13),
        field(
            "Folder",
            input(
                "~/.local/share/contacts",
                &state.draft_vdir_path,
                Message::VdirPathChanged,
            ),
        ),
        text(format!("Auto-detected: {discovered}"))
            .size(11)
            .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
    ]
    .spacing(8)
    .into()
}

/// Blocked numbers, listed so a rule can actually be found and removed.
pub(super) fn blocking_section(integration: &IntegrationSettings) -> Element<'_, Message> {
    let action_pick = pick_list(
        BlockAction::ALL,
        Some(integration.default_block_action),
        Message::DefaultBlockActionChanged,
    )
    .text_size(13)
    .padding(7)
    .width(Length::Fill);

    let mut content = column![field("Default action", action_pick.into())].spacing(8);

    if integration.blocked_numbers.is_empty() {
        content = content.push(
            text("Nothing blocked. Block a caller from Contacts or History.")
                .size(12)
                .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
        );
    } else {
        for blocked in &integration.blocked_numbers {
            let label = blocked.name.clone().map_or_else(
                || blocked.number.clone(),
                |name| format!("{name} ({})", blocked.number),
            );
            content = content.push(
                row![
                    column![
                        text(label).size(13),
                        text(blocked.action.label())
                            .size(11)
                            .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
                    ]
                    .spacing(1),
                    Space::new().width(Length::Fill),
                    button(text("Unblock").size(12))
                        .on_press(Message::UnblockNumber(blocked.number.clone()))
                        .padding([3, 9]),
                ]
                .align_y(Alignment::Center)
                .spacing(6),
            );
        }
    }

    section(
        "Call blocking",
        Some("Applies to incoming calls, matched on the caller's number."),
        content.into(),
    )
}
