//! One function per page of the settings window.
//!
//! Each builds its page from the controls in `widgets` and the state in
//! `State`; none of them decides what happens next, which is `app`'s job.

use iced::widget::{button, checkbox, column, pick_list, row, text, text_input};
use iced::{Element, Length};
use sipster_core::audio::DeviceSelection;
use sipster_core::{ThemeChoice, UiSettings};

use super::{field, input, secret_input, section, AccountContext, DeviceChoice, Message, State};

pub(crate) fn account_section<'a>(
    state: &'a State,
    mask: bool,
    accounts: &AccountContext<'a>,
) -> Element<'a, Message> {
    let (account, first_run) = (accounts.current, accounts.first_run);
    let dirty = account.is_none_or(|acc| state.account_is_dirty(acc));

    let revert_lbl = rust_i18n::t!("revert").to_string();

    let apply_reconnect_str = rust_i18n::t!("apply_reconnect").to_string();
    let mut apply = button(text(apply_reconnect_str).size(14));
    let mut revert = button(text(revert_lbl).size(14));
    if dirty {
        apply = apply.on_press(Message::ApplyAccount);
        revert = revert.on_press(Message::RevertAccount);
    }

    let hidden = |placeholder: &str, value: &'a str, on_change: fn(String) -> Message| {
        text_input(placeholder, value)
            .on_input(on_change)
            .secure(mask)
            .padding(7)
            .size(14)
            .into()
    };

    let host_lbl = rust_i18n::t!("registrar").to_string();
    let port_lbl = rust_i18n::t!("registrar_port").to_string();
    let user_lbl = rust_i18n::t!("username").to_string();
    let auth_lbl = rust_i18n::t!("auth_user").to_string();
    let pass_lbl = rust_i18n::t!("password").to_string();
    let exp_lbl = rust_i18n::t!("re_register_every").to_string();
    let local_port_lbl = rust_i18n::t!("port").to_string();
    let transport_lbl = rust_i18n::t!("transport").to_string();

    let content = column![
        field(
            host_lbl,
            hidden("fritz.box", &state.registrar, Message::Registrar)
        ),
        field(port_lbl, input("5060", &state.port, Message::Port)),
        field(
            user_lbl,
            hidden("", &state.username, Message::Username)
        ),
        field(
            auth_lbl,
            hidden(&rust_i18n::t!("auth_user_placeholder"), &state.auth_user, Message::AuthUser)
        ),
        field(
            pass_lbl,
            secret_input(
                "",
                &state.password,
                state.reveal_password,
                Message::Password,
                Message::RevealPassword,
            )
        ),
        field(
            exp_lbl,
            input("600", &state.expires, Message::Expires)
        ),
        field(
            local_port_lbl,
            input("5060", &state.local_port, Message::LocalPort)
        ),
        field(
            transport_lbl,
            pick_list(
                sipster_core::Transport::ALL,
                Some(state.transport),
                Message::TransportChanged,
            )
            .text_size(13)
            .into(),
        ),
        row![apply, revert].spacing(10),
    ]
    .spacing(9);

    let hint = if first_run {
        rust_i18n::t!("account_hint_first").to_string()
    } else {
        rust_i18n::t!("account_hint_applied").to_string()
    };

    let title_acc = rust_i18n::t!("account").to_string();
    section(title_acc, Some(hint), content.into())
}

pub(crate) fn audio_section<'a>(state: &'a State, devices: &'a DeviceSelection) -> Element<'a, Message> {
    let audio_title = rust_i18n::t!("audio").to_string();
    if !state.devices_loaded {
        let looking_str = rust_i18n::t!("looking_audio").to_string();
        return section(
            audio_title,
            None::<&str>,
            text(looking_str).size(13).into(),
        );
    }

    let inputs = DeviceChoice::list(&state.inputs, devices.input.as_ref());
    let outputs = DeviceChoice::list(&state.outputs, devices.output.as_ref());

    let selected = |list: &[DeviceChoice], id: Option<&String>| {
        list.iter().find(|c| c.id.as_ref() == id).cloned()
    };

    let input_pick = pick_list(
        inputs.clone(),
        selected(&inputs, devices.input.as_ref()),
        Message::InputDevice,
    )
    .text_size(13)
    .padding(7)
    .width(Length::Fill);

    let output_pick = pick_list(
        outputs.clone(),
        selected(&outputs, devices.output.as_ref()),
        Message::OutputDevice,
    )
    .text_size(13)
    .padding(7)
    .width(Length::Fill);

    let mic_lbl = rust_i18n::t!("microphone").to_string();
    let speaker_lbl = rust_i18n::t!("speaker").to_string();
    let audio_hint_str = rust_i18n::t!("audio_hint").to_string();

    section(
        audio_title,
        Some(audio_hint_str),
        column![
            field(mic_lbl, input_pick.into()),
            field(speaker_lbl, output_pick.into()),
        ]
        .spacing(9)
        .into(),
    )
}

pub(crate) fn appearance_section(ui: &UiSettings) -> Element<'_, Message> {
    let language_pick = pick_list(
        sipster_core::LanguageChoice::ALL,
        Some(ui.language),
        Message::Language,
    )
    .text_size(13)
    .padding(7)
    .width(Length::Fill);

    let theme = pick_list(ThemeChoice::ALL, Some(ui.theme), Message::Theme)
        .text_size(13)
        .padding(7)
        .width(Length::Fill);

    let title_app = rust_i18n::t!("appearance").to_string();
    let lang_lbl = rust_i18n::t!("language").to_string();
    let theme_lbl = rust_i18n::t!("theme").to_string();
    let banner_lbl = rust_i18n::t!("show_banner").to_string();
    let streaming_cb_lbl = rust_i18n::t!("streaming_mode_cb").to_string();
    let desc_streaming = rust_i18n::t!("streaming_mode_desc").to_string();

    section(
        title_app,
        None::<&str>,
        column![
            field(lang_lbl, language_pick.into()),
            field(theme_lbl, theme.into()),
            field(
                "",
                checkbox(ui.show_banner)
                    .label(banner_lbl)
                    .on_toggle(Message::ShowBanner)
                    .size(15)
                    .text_size(13)
                    .into()
            ),
            // Streaming mode changes what every name and number on screen
            // looks like, which is appearance. It used to sit under Desktop
            // beside the tray and URI-handler toggles, which are about how
            // Sipster fits into the desktop rather than what it shows.
            field(
                "",
                checkbox(ui.streaming_mode)
                    .label(streaming_cb_lbl)
                    .on_toggle(Message::StreamingMode)
                    .size(15)
                    .text_size(13)
                    .into()
            ),
            text(desc_streaming)
                .size(11)
                .color(iced::Color::from_rgb(0.62, 0.62, 0.66)),
        ]
        .spacing(9)
        .into(),
    )
}

pub(crate) fn sounds_section(ui: &UiSettings) -> Element<'_, Message> {
    let toggle = |label: String, value: bool, msg: fn(bool) -> Message| {
        checkbox(value)
            .label(label)
            .on_toggle(msg)
            .size(15)
            .text_size(13)
    };

    let title_snd = rust_i18n::t!("sounds").to_string();
    let hint_snd = rust_i18n::t!("sounds_hint").to_string();
    let ring_lbl = rust_i18n::t!("ring_incoming").to_string();
    let notif_lbl = rust_i18n::t!("desktop_notifications").to_string();
    let dtmf_lbl = rust_i18n::t!("beep_dialpad").to_string();
    let chimes_lbl = rust_i18n::t!("call_chimes").to_string();

    section(
        title_snd,
        Some(hint_snd),
        column![
            toggle(ring_lbl, ui.ringtone, Message::Ringtone),
            toggle(
                notif_lbl,
                ui.notifications,
                Message::Notifications
            ),
            toggle(
                dtmf_lbl,
                ui.dtmf_feedback,
                Message::DtmfFeedback
            ),
            toggle(
                chimes_lbl,
                ui.call_chimes,
                Message::CallChimes
            ),
        ]
        .spacing(9)
        .into(),
    )
}

pub(crate) fn integration_section(ui: &UiSettings) -> Element<'_, Message> {
    let uri_cb_lbl = rust_i18n::t!("register_uri_cb").to_string();
    let uri_cb: Element<'_, Message> = checkbox(ui.register_uri_schemes)
        .label(uri_cb_lbl)
        .on_toggle(Message::RegisterUriSchemes)
        .size(15)
        .text_size(13)
        .into();

    let tray_cb_lbl = rust_i18n::t!("close_to_tray_cb").to_string();
    let tray_cb: Element<'_, Message> = checkbox(ui.close_to_tray)
        .label(tray_cb_lbl)
        .on_toggle(Message::CloseToTray)
        .size(15)
        .text_size(13)
        .into();

    let title_desk = rust_i18n::t!("desktop_integration").to_string();
    let hint_desk = rust_i18n::t!("desktop_integration_hint").to_string();

    section(
        title_desk,
        Some(hint_desk),
        column![tray_cb, uri_cb]
        .spacing(9)
        .into(),
    )
}
