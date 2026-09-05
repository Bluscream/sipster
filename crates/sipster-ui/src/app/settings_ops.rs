//! The settings window, and editing the accounts it holds.
//!
//! Kept out of `mod` because it is self-contained: opening and closing the
//! window, mapping its messages onto the draft form, and writing the result
//! back into the config. Nothing here touches a call.

use iced::window;
use iced::Task;

use super::{Message, SipsterApp};
use crate::engine_bridge;
use crate::settings;

impl SipsterApp {
    /// Opens the settings window, or focuses it if it is already open.
    pub(super) fn open_settings(&mut self) -> Task<Message> {
        if let Some(id) = self.settings_window {
            return window::gain_focus(id);
        }

        // Prefer the engine's account (what is actually registered); fall back
        // to the config, so a failed start still opens an editable form rather
        // than a blank one the user cannot fix.
        let account = self.engine().map_or_else(
            || self.config.account.clone(),
            |engine| engine.account().clone(),
        );
        self.settings.load_account(&account);
        // The provider panels are always on screen now, so their text drafts
        // are seeded when the window opens rather than when a panel expands.
        self.settings.draft_fritz_port = self.config.integration.fritzbox.port.to_string();
        self.settings.draft_google_client_id = self
            .config
            .integration
            .google_client_id
            .clone()
            .unwrap_or_default();
        self.settings.draft_google_client_secret = self
            .config
            .integration
            .google_client_secret
            .clone()
            .unwrap_or_default();
        self.settings.draft_vdir_path = self
            .config
            .integration
            .vdir_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.settings.notice = None;
        self.settings.error = None;

        let (id, open) = window::open(crate::settings_window_settings());
        self.settings_window = Some(id);

        // Device enumeration goes through cpal and can take a moment; keep it
        // off the UI thread so the window paints straight away.
        let load_devices = Task::future(async {
            let devices = tokio::task::spawn_blocking(|| {
                (
                    sipster_core::audio::input_devices(),
                    sipster_core::audio::output_devices(),
                )
            })
            .await
            .unwrap_or_default();
            Message::DevicesLoaded(devices.0, devices.1)
        });

        Task::batch([open.map(Message::SettingsOpened), load_devices])
    }

    pub(super) fn on_settings(&mut self, msg: settings::Message) -> Task<Message> {
        use settings::Message as S;

        // Any edit clears the last confirmation so stale feedback is not shown
        // next to a field the user is still changing.
        self.settings.notice = None;

        match msg {
            S::Registrar(v) => self.settings.registrar = v,
            S::Port(v) => self.settings.port = v,
            S::Username(v) => self.settings.username = v,
            S::AuthUser(v) => self.settings.auth_user = v,
            S::Password(v) => self.settings.password = v,
            S::Expires(v) => self.settings.expires = v,
            S::LocalPort(v) => self.settings.local_port = v,
            S::TransportChanged(t) => {
                self.settings.transport = t;
                // Moving between UDP/TCP (5060) and TLS (5061) changes the
                // usual port, so offer the new default rather than silently
                // keeping one that will not connect.
                let previous = sipster_core::Transport::ALL
                    .iter()
                    .find(|other| other.default_port().to_string() == self.settings.port);
                if previous.is_some() {
                    self.settings.port = t.default_port().to_string();
                }
            }
            S::RevealPassword(v) => self.settings.reveal_password = v,
            S::RevealFritzPassword(v) => self.settings.reveal_fritz_password = v,
            S::RevealCardDavPassword(v) => self.settings.reveal_carddav_password = v,
            S::RevealGoogleSecret(v) => self.settings.reveal_google_secret = v,

            S::RevertAccount => {
                let account = self.engine().map_or_else(
                    || self.config.account.clone(),
                    |engine| engine.account().clone(),
                );
                self.settings.load_account(&account);
                self.settings.error = None;
            }
            S::ApplyAccount => return self.apply_account(),

            S::InputDevice(choice) => {
                self.devices.input = choice.id;
                return self.apply_devices();
            }
            S::OutputDevice(choice) => {
                self.devices.output = choice.id;
                return self.apply_devices();
            }

            S::Language(lang) => {
                self.config.ui.language = lang;
                rust_i18n::set_locale(lang.code());
                self.persist();
            }
            S::Theme(theme) => {
                self.config.ui.theme = theme;
                self.persist();
            }
            S::Ringtone(on) => {
                self.config.ui.ringtone = on;
                // Turning the ringtone off during a ringing call should stop
                // the noise now, not after the caller gives up.
                if !on {
                    self.ringtone = None;
                }
                self.persist();
            }
            S::Notifications(on) => {
                self.config.ui.notifications = on;
                self.persist();
            }
            S::DtmfFeedback(on) => {
                self.config.ui.dtmf_feedback = on;
                self.persist();
            }
            S::CallChimes(on) => {
                self.config.ui.call_chimes = on;
                self.persist();
            }
            S::ShowBanner(on) => {
                self.config.ui.show_banner = on;
                self.persist();
            }
            S::RegisterUriSchemes(on) => {
                self.config.ui.register_uri_schemes = on;
                self.persist();
                if on {
                    crate::register_desktop_uri_schemes();
                }
            }
            S::CloseToTray(on) => {
                self.config.ui.close_to_tray = on;
                self.persist();
            }

            S::JumpTo(index) => self.settings.section = index,
            S::Close => {
                if let Some(id) = self.settings_window.take() {
                    return window::close(id);
                }
            }
            other => return self.on_provider_settings(other),
        }
        Task::none()
    }

    /// Commits the account draft: rebuild the engine, then save.
    pub(super) fn apply_account(&mut self) -> Task<Message> {
        let account = match self.settings.to_account(self.settings.transport) {
            Ok(account) => account,
            Err(e) => {
                self.settings.error = Some(e);
                return Task::none();
            }
        };

        self.settings.error = None;
        self.settings.notice = Some(rust_i18n::t!("reconnecting").into());
        self.status = rust_i18n::t!("applying_settings").into();

        // Persist first: if the reconnect fails the user still has the values
        // they typed, and can fix them without retyping everything. Only the
        // selected account is replaced; the others are left as they are.
        self.config.account = account;
        self.persist();

        // The bridge owns engine lifetime, so ask it to rebuild rather than
        // swapping the handles from under the running event loop.
        engine_bridge::reconfigure(self.config.account.clone());
        Task::none()
    }

    /// Pushes the device selection into the running engine and saves it.
    pub(super) fn apply_devices(&mut self) -> Task<Message> {
        self.config.audio.input = self.devices.input.clone();
        self.config.audio.output = self.devices.output.clone();
        self.persist();

        let engine = self.engine().cloned();
        let selection = self.devices.clone();
        Task::future(async move {
            let mut failed = None;
            if let Some(engine) = engine {
                if let Err(e) = engine.set_devices(selection).await {
                    failed = Some(e.to_string());
                }
            }
            Message::ActionDone(failed.map_or(Ok(()), Err))
        })
    }

    /// Writes the config file, reporting failures in the settings window.
    pub(super) fn persist(&mut self) {
        match self.config.save(&self.config_path) {
            Ok(()) => {
                self.settings.error = None;
                self.settings.notice = Some(rust_i18n::t!("saved").into());
            }
            Err(e) => {
                tracing::error!(error = %e, "could not save settings");
                self.settings.error = Some(rust_i18n::t!("save_failed", error = e).into());
            }
        }
    }

}
