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
        let account = self
            .engine()
            .as_ref()
            .map(|engine| engine.account().clone())
            .or_else(|| self.config.accounts.first().cloned())
            .unwrap_or_default();
        self.settings.load_account(&account);
        // The provider panels are always on screen now, so their text drafts
        // are seeded when the window opens rather than when a panel expands.
        self.settings.draft_fritz_port = self.config.integration.fritzbox.port.to_string();
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
            S::AccountEnabled(v) => self.settings.account_enabled = v,
            S::SelectAccount(index) => return self.select_account(index),
            S::AddAccount => return self.add_account(),
            S::RemoveAccount => return self.remove_account(),
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
                let account = self
                    .engine()
                    .as_ref()
                    .map(|engine| engine.account().clone())
                    .or_else(|| self.config.accounts.first().cloned())
                    .unwrap_or_default();
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
        self.settings.notice = Some("Reconnecting…".into());
        self.status = "Applying account settings…".into();

        // Persist first: if the reconnect fails the user still has the values
        // they typed, and can fix them without retyping everything. Only the
        // selected account is replaced; the others are left as they are.
        if self.config.accounts.len() <= self.active_account {
            self.config.accounts.resize(self.active_account + 1, account.clone());
        }
        self.config.accounts[self.active_account] = account;
        self.persist();

        // The bridge owns engine lifetime, so ask it to rebuild rather than
        // swapping the handles from under the running event loop.
        engine_bridge::reconfigure(self.config.accounts.clone());
        Task::none()
    }

    /// Pushes the device selection into the running engine and saves it.
    pub(super) fn apply_devices(&mut self) -> Task<Message> {
        self.config.audio.input = self.devices.input.clone();
        self.config.audio.output = self.devices.output.clone();
        self.persist();

        // Every account shares the one microphone and speaker, so the change
        // goes to all of them rather than only the selected one.
        let engines = self.engines.clone();
        let selection = self.devices.clone();
        Task::future(async move {
            let mut failed = None;
            for engine in engines {
                if let Err(e) = engine.set_devices(selection.clone()).await {
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
                self.settings.notice = Some("Saved".into());
            }
            Err(e) => {
                tracing::error!(error = %e, "could not save settings");
                self.settings.error = Some(format!("Could not save: {e}"));
            }
        }
    }

    /// Switches which account the Account page edits and outgoing calls use.
    ///
    /// The draft is saved back first, so switching away from a half-typed
    /// account does not throw the edits away.
    fn select_account(&mut self, index: usize) -> Task<Message> {
        if index >= self.config.accounts.len() || index == self.active_account {
            return Task::none();
        }
        self.stash_draft();
        self.active_account = index;
        let account = self.config.accounts[index].clone();
        self.settings.load_account(&account);
        self.settings.notice = None;
        self.settings.error = None;
        Task::none()
    }

    /// Adds a blank account and switches to it.
    fn add_account(&mut self) -> Task<Message> {
        self.stash_draft();
        self.config.accounts.push(sipster_core::SipAccount::default());
        self.active_account = self.config.accounts.len() - 1;
        let account = self.config.accounts[self.active_account].clone();
        self.settings.load_account(&account);
        self.persist();
        self.settings.notice = Some("Account added — fill it in and apply".into());
        Task::none()
    }

    /// Removes the selected account.
    ///
    /// The last one is kept: with none at all the app has nothing to register
    /// and no form to fix it in.
    fn remove_account(&mut self) -> Task<Message> {
        if self.config.accounts.len() <= 1 {
            self.settings.error = Some("The last account cannot be removed".into());
            return Task::none();
        }
        self.config.accounts.remove(self.active_account);
        self.active_account = self.active_account.min(self.config.accounts.len() - 1);
        let account = self.config.accounts[self.active_account].clone();
        self.settings.load_account(&account);
        self.persist();
        self.settings.notice = Some("Account removed".into());
        engine_bridge::reconfigure(self.config.accounts.clone());
        Task::none()
    }

    /// Writes the form back into the selected account without reconnecting.
    ///
    /// Switching accounts would otherwise discard anything typed but not yet
    /// applied.
    fn stash_draft(&mut self) {
        let Ok(account) = self.settings.to_account(self.settings.transport) else {
            return;
        };
        if let Some(slot) = self.config.accounts.get_mut(self.active_account) {
            *slot = account;
        }
    }
}
