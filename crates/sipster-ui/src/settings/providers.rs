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

    let history_label = rust_i18n::t!("settings.record_history");
    content = content.push(rule::horizontal(1)).push(
        checkbox(integration.local_history_enabled)
            .label(history_label.to_string())
            .on_toggle(Message::ToggleLocalHistory)
            .size(15)
            .text_size(13),
    );

    // The config path lived in an About section that was otherwise just a
    // version number; it belongs where credentials are entered.
    let stored_in = rust_i18n::t!("settings.stored_in", path = config_path);
    content = content.push(
        text(stored_in)
            .size(11)
            .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
    );

    let title_integ = rust_i18n::t!("categories.integrations").to_string();
    let hint_integ = rust_i18n::t!("settings.integrations_sub").to_string();

    section(
        title_integ,
        Some(hint_integ),
        content.into(),
    )
}

fn fritzbox_panel<'a>(
    state: &'a State,
    integration: &'a IntegrationSettings,
) -> Element<'a, Message> {
    let fb = &integration.fritzbox;

    let title_fb = rust_i18n::t!("settings.fritzbox").to_string();
    let sync_fb = rust_i18n::t!("settings.fritzbox_sync").to_string();
    let host_lbl = rust_i18n::t!("settings.host").to_string();
    let port_lbl = rust_i18n::t!("settings.port").to_string();
    let user_lbl = rust_i18n::t!("settings.username").to_string();
    let pass_lbl = rust_i18n::t!("settings.password").to_string();

    column![
        text(title_fb).size(14),
        checkbox(fb.enabled)
            .label(sync_fb)
            .on_toggle(Message::FritzEnabledToggled)
            .size(15)
            .text_size(13),
        field(host_lbl, input("fritz.box", &fb.host, Message::FritzHostChanged)),
        field(port_lbl, port_input(&state.draft_fritz_port)),
        field(user_lbl, input("", &fb.username, Message::FritzUserChanged)),
        field(
            pass_lbl,
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
    let title_g = rust_i18n::t!("settings.google_contacts").to_string();
    let note_g = rust_i18n::t!("settings.google_oauth_note").to_string();
    let rm_lbl = rust_i18n::t!("ui.remove").to_string();

    let mut content = column![
        text(title_g).size(14),
        text(note_g).size(12),
    ]
    .spacing(6);

    for account in &integration.google_accounts {
        content = content.push(
            row![
                text(account.email.clone()).size(13),
                Space::new().width(Length::Fill),
                button(text(rm_lbl.clone()).size(12))
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

    let json_field_lbl = rust_i18n::t!("settings.client_secret_json").to_string();
    let id_field_lbl = rust_i18n::t!("settings.client_id").to_string();
    let secret_field_lbl = rust_i18n::t!("settings.client_secret").to_string();
    let connect_btn_lbl = rust_i18n::t!("settings.connect_google").to_string();

    content
        .push(field(
            json_field_lbl,
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
            id_field_lbl,
            input(
                "…apps.googleusercontent.com",
                &state.draft_google_client_id,
                Message::GoogleClientIdChanged,
            ),
        ))
        .push(field(
            secret_field_lbl,
            secret_input(
                "",
                &state.draft_google_client_secret,
                state.reveal_google_secret,
                Message::GoogleClientSecretChanged,
                Message::RevealGoogleSecret,
            ),
        ))
        .push(
            button(text(connect_btn_lbl).size(13))
                .on_press_maybe(ready.then_some(Message::ConnectGoogleAccount))
                .padding([5, 11]),
        )
        .into()
}

fn carddav_panel<'a>(
    state: &'a State,
    integration: &'a IntegrationSettings,
) -> Element<'a, Message> {
    let title_cd = rust_i18n::t!("settings.carddav").to_string();
    let rm_lbl = rust_i18n::t!("ui.remove").to_string();

    let mut content = column![text(title_cd).size(14)].spacing(6);

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
                button(text(rm_lbl.clone()).size(12))
                    .on_press(Message::RemoveCardDavAccount(account.id.clone()))
                    .padding([3, 9])
                    .style(button::danger),
            ]
            .align_y(Alignment::Center)
            .spacing(6),
        );
    }

    let can_add = !state.draft_carddav_url.trim().is_empty();

    let url_lbl = rust_i18n::t!("settings.url").to_string();
    let user_lbl = rust_i18n::t!("settings.username").to_string();
    let pass_lbl = rust_i18n::t!("settings.password").to_string();
    let add_btn_lbl = rust_i18n::t!("settings.add_address_book").to_string();

    content
        .push(field(
            url_lbl,
            input(
                "https://dav.example.com/addressbooks/me/default/",
                &state.draft_carddav_url,
                Message::CardDavUrlChanged,
            ),
        ))
        .push(field(
            user_lbl,
            input("", &state.draft_carddav_user, Message::CardDavUserChanged),
        ))
        .push(field(
            pass_lbl,
            secret_input(
                "",
                &state.draft_carddav_pass,
                state.reveal_carddav_password,
                Message::CardDavPassChanged,
                Message::RevealCardDavPassword,
            ),
        ))
        .push(
            button(text(add_btn_lbl).size(13))
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
    let found = sipster_integrations::VdirStore::discover();
    let discovered = if found.is_empty() {
        rust_i18n::t!("settings.none_found").to_string()
    } else {
        found
            .iter()
            .map(|store| store.root().display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };

    let eds_available = sipster_integrations::eds_available();
    let eds_note = if eds_available {
        rust_i18n::t!("settings.evolution_found").to_string()
    } else {
        rust_i18n::t!("settings.evolution_not_found").to_string()
    };

    let title_eds = rust_i18n::t!("settings.evolution").to_string();
    let desc_eds = rust_i18n::t!("settings.evolution_desc").to_string();
    let read_eds = rust_i18n::t!("settings.evolution_read").to_string();

    let title_vdir = rust_i18n::t!("settings.local_vcard").to_string();
    let desc_vdir = rust_i18n::t!("settings.local_vcard_desc").to_string();
    let read_vdir = rust_i18n::t!("settings.local_vcard_read").to_string();
    let folder_lbl = rust_i18n::t!("settings.folder").to_string();
    let auto_detected = rust_i18n::t!("settings.auto_detected", discovered = discovered).to_string();

    column![
        text(title_eds).size(14),
        text(desc_eds).size(12),
        checkbox(integration.eds_enabled)
            .label(read_eds)
            .on_toggle(Message::ToggleEds)
            .size(15)
            .text_size(13),
        text(eds_note)
            .size(11)
            .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
        rule::horizontal(1),
        text(title_vdir).size(14),
        text(desc_vdir).size(12),
        checkbox(integration.vdir_enabled)
            .label(read_vdir)
            .on_toggle(Message::ToggleVdir)
            .size(15)
            .text_size(13),
        field(
            folder_lbl,
            input(
                crate::consts::LOCAL_CONTACTS_DIR_DISPLAY,
                &state.draft_vdir_path,
                Message::VdirPathChanged,
            ),
        ),
        text(auto_detected)
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

    let default_action_lbl = rust_i18n::t!("settings.default_action").to_string();
    let mut content = column![field(default_action_lbl, action_pick.into())].spacing(8);

    if integration.blocked_numbers.is_empty() {
        let nothing_lbl = rust_i18n::t!("settings.nothing_blocked").to_string();
        content = content.push(
            text(nothing_lbl)
                .size(12)
                .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
        );
    } else {
        let unblock_lbl = rust_i18n::t!("ui.unblock").to_string();
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
                    button(text(unblock_lbl.clone()).size(12))
                        .on_press(Message::UnblockNumber(blocked.number.clone()))
                        .padding([3, 9]),
                ]
                .align_y(Alignment::Center)
                .spacing(6),
            );
        }
    }

    let title_block = rust_i18n::t!("categories.call_blocking").to_string();
    let sub_block = rust_i18n::t!("settings.call_blocking_sub").to_string();

    section(
        title_block,
        Some(sub_block),
        content.into(),
    )
}
