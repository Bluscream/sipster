//! Contact and call-history provider configuration.
//!
//! FRITZ!Box, Google, `CardDAV` and the local vCard folder. These moved out of
//! the Contacts window into Settings, and out of `app` into here, because
//! together they are longer than everything else the app does.

use sipster_core::Config;
use sipster_integrations::{
    cancel_pending_auth, CardDavClient, CardDavConfig, FritzConfig, GoogleContactsClient,
    SyncManager,
};

use super::{Message, SipsterApp, Task};
use crate::{contacts, settings};

impl SipsterApp {
    /// Contact/history provider configuration and call blocking.
    ///
    /// Split out of `on_settings` only to keep that dispatcher readable; these
    /// arrived together when provider setup moved here out of the Contacts and
    /// History windows.
    pub(super) fn on_provider_settings(&mut self, msg: settings::Message) -> Task<Message> {
        use settings::Message as S;
        match msg {
            S::FritzHostChanged(v) => {
                self.config.integration.fritzbox.host = v;
                self.persist();
            }
            S::FritzPortChanged(v) => {
                // Keep what was typed; only commit a value that parses, so
                // clearing the field to retype does not reset it to the default.
                if let Ok(port) = v.trim().parse::<u16>() {
                    self.config.integration.fritzbox.port = port;
                    self.persist();
                }
                self.settings.draft_fritz_port = v;
            }
            S::FritzUserChanged(v) => {
                self.config.integration.fritzbox.username = v;
                self.persist();
            }
            S::FritzPassChanged(v) => {
                self.config.integration.fritzbox.password = v;
                self.persist();
            }
            S::FritzEnabledToggled(enabled) => {
                self.config.integration.fritzbox.enabled = enabled;
                if enabled {
                    let fb = &self.config.integration.fritzbox;
                    self.sync_manager.set_fritzbox(Some(FritzConfig {
                        host: fb.host.clone(),
                        port: fb.port,
                        username: fb.username.clone(),
                        password: fb.password.clone(),
                        tls: fb.tls,
                        cert_fingerprint: fb.cert_fingerprint.clone(),
                    }));
                } else {
                    self.sync_manager.set_fritzbox(None);
                }
                self.persist();
            }

            // Google account OAuth flow:
            S::StreamingMode(on) => {
                self.config.ui.streaming_mode = on;
                self.persist();
            }
            S::PickGoogleJsonFile => {
                let default_dir = std::env::var_os("XDG_DOWNLOAD_DIR")
                    .map(std::path::PathBuf::from)
                    .or_else(|| crate::consts::home_dir().map(|h| h.join("Downloads")));
                let mut builder = rfd::FileDialog::new()
                    .add_filter("JSON Files", &["json"])
                    .set_title(rust_i18n::t!("pick_google_json").to_string());
                if let Some(dir) = default_dir {
                    builder = builder.set_directory(dir);
                }
                if let Some(path) = builder.pick_file() {
                    let path_str = path.to_string_lossy().to_string();
                    self.settings.draft_google_json_path = path_str;
                    self.import_google_client_json();
                }
            }
            S::GoogleClientIdChanged(v) => {
                self.settings.draft_google_client_id = v;
                self.store_google_client();
            }
            S::GoogleClientSecretChanged(v) => {
                self.settings.draft_google_client_secret = v;
                self.store_google_client();
            }
            other => return self.on_carddav_settings(other),
        }
        Task::none()
    }

    /// The `CardDAV` account list and the local vCard folder.
    fn on_carddav_settings(&mut self, msg: settings::Message) -> Task<Message> {
        use settings::Message as S;
        match msg {
            S::CardDavUrlChanged(v) => {
                self.settings.draft_carddav_url = v;
            }
            S::CardDavUserChanged(v) => {
                self.settings.draft_carddav_user = v;
            }
            S::CardDavPassChanged(v) => {
                self.settings.draft_carddav_pass = v;
            }
            S::AddCardDavAccount => {
                let url = self.settings.draft_carddav_url.trim().to_string();
                if !url.is_empty() {
                    let id = format!(
                        "carddav-{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis()
                    );
                    self.config.integration.carddav_accounts.push(sipster_core::CardDavAccountConfig {
                        id,
                        name: url.clone(),
                        url: url.clone(),
                        username: self.settings.draft_carddav_user.clone(),
                        password: self.settings.draft_carddav_pass.clone(),
                        enabled: true,
                    });
                    self.settings.draft_carddav_url.clear();
                    self.settings.draft_carddav_user.clear();
                    self.settings.draft_carddav_pass.clear();
                    self.persist();

                    let c_clients = self
                        .config
                        .integration
                        .carddav_accounts
                        .iter()
                        .filter(|a| a.enabled)
                        .map(|a| {
                            CardDavClient::new(CardDavConfig {
                                url: a.url.clone(),
                                username: a.username.clone(),
                                password: a.password.clone(),
                            })
                        })
                        .collect();
                    self.sync_manager.set_carddav_accounts(c_clients);
                    return Task::done(Message::Contacts(contacts::Message::SyncPressed));
                }
            }
            S::RemoveCardDavAccount(account_id) => {
                self.config.integration.carddav_accounts.retain(|a| a.id != account_id);
                self.persist();
                let c_clients = self
                    .config
                    .integration
                    .carddav_accounts
                    .iter()
                    .filter(|a| a.enabled)
                    .map(|a| {
                        CardDavClient::new(CardDavConfig {
                            url: a.url.clone(),
                            username: a.username.clone(),
                            password: a.password.clone(),
                        })
                    })
                    .collect();
                self.sync_manager.set_carddav_accounts(c_clients);
                return Task::done(Message::Contacts(contacts::Message::SyncPressed));
            }

            // Call blocking:
            S::ToggleLocalHistory(enabled) => {
                self.config.integration.local_history_enabled = enabled;
                self.persist();
            }
            S::DefaultBlockActionChanged(action) => {
                self.config.integration.default_block_action = action;
                self.persist();
            }
            S::UnblockNumber(number) => {
                self.config
                    .integration
                    .blocked_numbers
                    .retain(|b| b.number != number);
                self.persist();
            }
            other => return self.on_vdir_settings(other),
        }
        Task::none()
    }

    /// The local vCard folder — the nearest thing Linux has to a shared
    /// contact store, and the one provider that needs no account at all.
    fn on_vdir_settings(&mut self, msg: settings::Message) -> Task<Message> {
        use settings::Message as S;
        match msg {
            S::ToggleEds(enabled) => {
                self.config.integration.eds_enabled = enabled;
                self.sync_manager.set_eds(enabled);
                self.persist();
                return Task::done(Message::Contacts(contacts::Message::SyncPressed));
            }
            S::ToggleVdir(enabled) => {
                self.config.integration.vdir_enabled = enabled;
                self.apply_vdir();
                self.persist();
                return Task::done(Message::Contacts(contacts::Message::SyncPressed));
            }
            S::VdirPathChanged(path) => {
                self.settings.draft_vdir_path.clone_from(&path);
                let trimmed = path.trim();
                self.config.integration.vdir_path = (!trimmed.is_empty())
                    .then(|| crate::consts::expand_home_path(trimmed));
                self.apply_vdir();
                self.persist();
            }
            S::PickVdirFolder => {
                let default_dir = if self.settings.draft_vdir_path.trim().is_empty() {
                    crate::consts::default_contacts_dir().or_else(crate::consts::home_dir)
                } else {
                    Some(crate::consts::expand_home_path(self.settings.draft_vdir_path.trim()))
                };
                let mut builder = rfd::FileDialog::new().set_title(rust_i18n::t!("pick_vcard_folder").to_string());
                if let Some(dir) = default_dir {
                    builder = builder.set_directory(dir);
                }
                if let Some(folder) = builder.pick_folder() {
                    let path_str = folder.to_string_lossy().to_string();
                    self.settings.draft_vdir_path.clone_from(&path_str);
                    self.config.integration.vdir_path = Some(folder);
                    self.apply_vdir();
                    self.persist();
                    return Task::done(Message::Contacts(contacts::Message::SyncPressed));
                }
            }
            other => return self.on_google_settings(other),
        }
        Task::none()
    }

    /// Points the sync manager at the configured vCard folder, or the
    /// auto-detected one when no path is set.
    pub(super) fn apply_vdir(&mut self) {
        let store = if self.config.integration.vdir_enabled {
            self.config
                .integration
                .vdir_path
                .clone()
                .map_or_else(sipster_integrations::VdirStore::discover, |path| {
                    vec![sipster_integrations::VdirStore::new(path)]
                })
        } else {
            Vec::new()
        };
        self.sync_manager.set_vdir(store);
    }

    /// Fills the Google client id/secret from the JSON Google hands out.
    ///
    /// The file is read from wherever the user downloaded it and only its two
    /// fields are kept, in their own `0600` config. Nothing is copied into the
    /// repository — a client secret in a public repo is a published secret,
    /// which is why none ships with Sipster.
    /// Saves the typed client into the config, blank meaning "not set".
    ///
    /// Written as it is typed rather than on connect, so it survives closing
    /// the window without signing in — it is a property of the installation,
    /// not of the sign-in that happens to follow.
    fn store_google_client(&mut self) {
        let field = |value: &str| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        };
        let integration = &mut self.config.integration;
        integration.google_client_id = field(&self.settings.draft_google_client_id);
        integration.google_client_secret = field(&self.settings.draft_google_client_secret);
        self.persist();
    }

    /// The OAuth client Sipster presents to Google.
    ///
    /// One per installation rather than per account: it names the
    /// application, not the person signing in.
    fn google_client(&self) -> (Option<String>, Option<String>) {
        let integration = &self.config.integration;
        (
            integration.google_client_id.clone(),
            integration.google_client_secret.clone(),
        )
    }

    pub(super) fn import_google_client_json(&mut self) {
        let path = self.settings.draft_google_json_path.trim();
        if path.is_empty() {
            return;
        }

        let expanded = crate::consts::expand_home_path(path);

        let Ok(text) = std::fs::read_to_string(&expanded) else {
            // Silent while the user is still typing the path; only a complete,
            // readable file counts as an attempt.
            return;
        };

        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            self.settings.error = Some(rust_i18n::t!("invalid_json").into());
            return;
        };

        // Google nests the credentials under "installed" for desktop clients
        // and "web" for web ones; accept either, and a bare object too.
        let creds = json
            .get("installed")
            .or_else(|| json.get("web"))
            .unwrap_or(&json);

        let id = creds.get("client_id").and_then(|v| v.as_str()).unwrap_or_default();
        let secret = creds
            .get("client_secret")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        if id.is_empty() || secret.is_empty() {
            self.settings.error =
                Some(rust_i18n::t!("no_creds_json").into());
            return;
        }

        self.settings.draft_google_client_id = id.to_string();
        self.settings.draft_google_client_secret = secret.to_string();
        self.settings.error = None;
        self.settings.notice = Some(rust_i18n::t!("creds_loaded").into());
    }

    /// The Google account flow, which is long enough to stand on its own.
    fn on_google_settings(&mut self, msg: settings::Message) -> Task<Message> {
        use settings::Message as S;
        match msg {
            S::ConnectGoogleAccount => {
                cancel_pending_auth();
                let client_id = self.settings.draft_google_client_id.trim().to_string();
                let secret = self.settings.draft_google_client_secret.trim().to_string();
                self.settings.notice = Some(rust_i18n::t!("waiting_browser").into());
                self.settings.error = None;
                return Task::future(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        GoogleContactsClient::authorize(&client_id, &secret, 8765)
                    })
                    .await
                    .map_err(|e| rust_i18n::t!("signin_failed", error = e).to_string())
                    .and_then(|inner| inner);
                    Message::Settings(S::GoogleAuthFinished(result))
                });
            }
            S::GoogleAuthFinished(result) => {
                self.contacts.loading = false;
                match result {
                    Ok((email, refresh_token)) => {
                        let id = format!(
                            "google-{}",
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis()
                        );
                        self.config.integration.google_accounts.retain(|a| a.email != email);
                        self.config.integration.google_accounts.push(sipster_core::GoogleAccountConfig {
                            id: id.clone(),
                            email: email.clone(),
                            refresh_token: refresh_token.clone(),
                            enabled: true,
                        });
                        self.persist();

                        // Refresh sync manager's google clients. One client
                        // identifies the installation, so every account uses
                        // the same pair.
                        let (client_id, client_secret) = self.google_client();
                        let g_clients = self
                            .config
                            .integration
                            .google_accounts
                            .iter()
                            .filter(|a| a.enabled)
                            .map(|a| {
                                GoogleContactsClient::new(
                                    a.id.clone(),
                                    a.email.clone(),
                                    a.refresh_token.clone(),
                                    client_id.clone(),
                                    client_secret.clone(),
                                )
                            })
                            .collect();
                        self.sync_manager.set_google_accounts(g_clients);
                        return Task::done(Message::Contacts(contacts::Message::SyncPressed));
                    }
                    Err(e) => {
                        self.settings.notice = None;
                        self.settings.error = Some(e);
                    }
                }
            }
            S::RemoveGoogleAccount(account_id) => {
                self.config.integration.google_accounts.retain(|a| a.id != account_id);
                self.persist();
                let (client_id, client_secret) = self.google_client();
                let g_clients = self
                    .config
                    .integration
                    .google_accounts
                    .iter()
                    .filter(|a| a.enabled)
                    .map(|a| {
                        GoogleContactsClient::new(
                            a.id.clone(),
                            a.email.clone(),
                            a.refresh_token.clone(),
                            client_id.clone(),
                            client_secret.clone(),
                        )
                    })
                    .collect();
                self.sync_manager.set_google_accounts(g_clients);
                return Task::done(Message::Contacts(contacts::Message::SyncPressed));
            }

            // CardDAV accounts:
            _ => {}
        }
        Task::none()
    }
}

/// Builds the sync manager from a config: every provider the user has
/// configured, wired up before the first sync starts.
pub(super) fn build_sync_manager(config: &Config) -> SyncManager {
            let mut sm = SyncManager::new();
            let fb = &config.integration.fritzbox;
            if fb.enabled {
                sm.set_fritzbox(Some(FritzConfig {
                    host: fb.host.clone(),
                    port: fb.port,
                    username: fb.username.clone(),
                        password: fb.password.clone(),
                        tls: fb.tls,
                        cert_fingerprint: fb.cert_fingerprint.clone(),
                    }));
            }
            let client_id = config.integration.google_client_id.clone();
            let client_secret = config.integration.google_client_secret.clone();
            let g_clients = config
                .integration
                .google_accounts
                .iter()
                .filter(|a| a.enabled)
                .map(|a| {
                    GoogleContactsClient::new(
                        a.id.clone(),
                        a.email.clone(),
                        a.refresh_token.clone(),
                        client_id.clone(),
                        client_secret.clone(),
                    )
                })
                .collect();
            sm.set_google_accounts(g_clients);

            let c_clients = config
                .integration
                .carddav_accounts
                .iter()
                .filter(|a| a.enabled)
                .map(|a| {
                    CardDavClient::new(CardDavConfig {
                        url: a.url.clone(),
                        username: a.username.clone(),
                        password: a.password.clone(),
                    })
                })
                .collect();
            sm.set_carddav_accounts(c_clients);

            sm.set_eds(config.integration.eds_enabled);
            // The local vCard folder, configured or auto-detected.
            if config.integration.vdir_enabled {
                sm.set_vdir(
                    config
                        .integration
                        .vdir_path
                        .clone()
                        .map_or_else(sipster_integrations::VdirStore::discover, |path| {
                            vec![sipster_integrations::VdirStore::new(path)]
                        }),
                );
            }
            sm
}
