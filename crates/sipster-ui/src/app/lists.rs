//! The contacts and history windows: opening them, streaming their data
//! in, and everything their messages ask for.

use super::{
    chrono_now_iso, run_hook, BlockAction, BlockedNumber, Message, RecordSource, SipsterApp, Task,
};
use crate::pane::Placement;
use crate::{calls, contacts};
use iced::window;

impl SipsterApp {
    /// Advances the contact list to its next placement.
    ///
    /// One button, three states: beside the dialer, in its own window, gone.
    pub(super) fn cycle_contacts(&mut self) -> Task<Message> {
        let next = self.contacts_at.next();
        // Only one list may be docked; the dialer has room for one column.
        if next.is_docked() && self.calls_at.is_docked() {
            self.calls_at = Placement::Hidden;
        }
        self.contacts_at = next;
        // The button gives no other hint about which of the three states a
        // press just landed on.
        self.status = rust_i18n::t!("app.contacts_pane_status", placement = next.label()).to_string();
        tracing::info!(placement = %next.label(), "contacts pane");

        let mut tasks = vec![self.prefetch_contacts()];
        match next {
            Placement::Window => tasks.push(self.open_contacts()),
            // Leaving Window for Hidden means the window is still up.
            Placement::Hidden | Placement::Docked => {
                if let Some(id) = self.contacts_window.take() {
                    tasks.push(window::close(id));
                }
            }
        }
        Task::batch(tasks)
    }

    /// Advances the history list to its next placement. See [`Self::cycle_contacts`].
    pub(super) fn cycle_calls(&mut self) -> Task<Message> {
        let next = self.calls_at.next();
        if next.is_docked() && self.contacts_at.is_docked() {
            self.contacts_at = Placement::Hidden;
        }
        self.calls_at = next;
        self.status = rust_i18n::t!("app.history_pane_status", placement = next.label()).to_string();
        tracing::info!(placement = %next.label(), "history pane");

        let mut tasks = vec![self.prefetch_calls()];
        match next {
            Placement::Window => tasks.push(self.open_calls()),
            Placement::Hidden | Placement::Docked => {
                if let Some(id) = self.calls_window.take() {
                    tasks.push(window::close(id));
                }
            }
        }
        Task::batch(tasks)
    }

    /// Starts a contact sync if nothing has been loaded yet.
    fn prefetch_contacts(&mut self) -> Task<Message> {
        if self.contacts_at == Placement::Hidden
            || !self.contacts.contacts.is_empty()
            || self.contacts.loading
        {
            return Task::none();
        }
        self.contacts.loading = true;
        self.stream_contacts()
    }

    /// Starts a history sync if nothing has been loaded yet.
    fn prefetch_calls(&mut self) -> Task<Message> {
        if self.calls_at == Placement::Hidden || !self.calls.calls.is_empty() || self.calls.loading {
            return Task::none();
        }
        self.calls.loading = true;
        self.stream_calls()
    }

    /// Forces the contact list into its own window, from the tray, a
    /// `sipster://open/contacts` URI or the command line.
    pub(super) fn show_contacts_window(&mut self) -> Task<Message> {
        self.contacts_at = Placement::Window;
        Task::batch([self.prefetch_contacts(), self.open_contacts()])
    }

    /// Forces the history list into its own window. See [`Self::show_contacts_window`].
    pub(super) fn show_calls_window(&mut self) -> Task<Message> {
        self.calls_at = Placement::Window;
        Task::batch([self.prefetch_calls(), self.open_calls()])
    }

    /// Opens the contacts window, or focuses it if already open.
    pub(super) fn open_contacts(&mut self) -> Task<Message> {
        if let Some(id) = self.contacts_window {
            return window::gain_focus(id);
        }

        let (id, open) = window::open(crate::contacts_window_settings());
        self.contacts_window = Some(id);

        // Reuse whatever the startup prefetch already has; only sync when
        // there is nothing to show, so opening the window is instant.
        if self.contacts.contacts.is_empty() && !self.contacts.loading {
            self.contacts.loading = true;
            return Task::batch([open.map(Message::ContactsOpened), self.stream_contacts()]);
        }
        open.map(Message::ContactsOpened)
    }

    /// Streams contact batches into the window as each provider answers.
    ///
    /// `Task::run` turns the channel into a stream of messages, so the list
    /// fills in progressively instead of appearing all at once after the
    /// slowest provider. A final `SyncFinished` clears the spinner.
    pub(super) fn stream_contacts(&self) -> Task<Message> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.sync_manager.sync_contacts_streaming(tx);
        Task::run(
            tokio_stream::wrappers::UnboundedReceiverStream::new(rx),
            |batch| Message::Contacts(contacts::Message::ContactsBatch(batch)),
        )
        .chain(Task::done(Message::Contacts(
            contacts::Message::SyncFinished,
        )))
        // Same trigger as the contact sync: both ask the same router, and an
        // account's numbers can change under it just as its phonebook can.
        .chain(self.discover_numbers())
    }

    /// Persists a router certificate fingerprint learned during a sync.
    ///
    /// Trust on first use: the first TLS connection to the router records what
    /// it presented, and every connection after that requires the same
    /// certificate. Stored here because the sync runs too deep to reach the
    /// config itself.
    fn store_learned_certificate(&mut self) {
        let Some(fingerprint) = sipster_integrations::take_learned_fingerprint() else {
            return;
        };
        if self.config.integration.fritzbox.cert_fingerprint == fingerprint {
            return;
        }
        self.config.integration.fritzbox.cert_fingerprint = fingerprint;
        self.persist();

        // Hand the pin to the sync manager so the next sync verifies against
        // it rather than learning all over again.
        let fb = &self.config.integration.fritzbox;
        self.sync_manager
            .set_fritzbox(Some(sipster_integrations::FritzConfig {
                host: fb.host.clone(),
                port: fb.port,
                username: fb.username.clone(),
                password: fb.password.clone(),
                tls: fb.tls,
                cert_fingerprint: fb.cert_fingerprint.clone(),
            }));
    }

    /// Marks every missed call currently listed as seen.
    ///
    /// Recorded against the newest one rather than a flag, so a call that
    /// arrives afterwards still shows up as unseen.
    fn acknowledge_missed(&mut self) {
        let Some(newest) = self.calls.newest_missed().map(str::to_owned) else {
            return;
        };
        if self.config.ui.missed_seen_until.as_deref() == Some(newest.as_str()) {
            return;
        }
        self.calls.missed_seen_until = Some(newest.clone());
        self.config.ui.missed_seen_until = Some(newest);
        self.persist();
    }

    /// Streams call batches into the history window. See [`stream_contacts`].
    ///
    /// [`stream_contacts`]: Self::stream_contacts
    pub(super) fn stream_calls(&self) -> Task<Message> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.sync_manager.sync_calls_streaming(tx);
        Task::run(
            tokio_stream::wrappers::UnboundedReceiverStream::new(rx),
            |batch| Message::Calls(calls::Message::CallsBatch(batch)),
        )
        .chain(Task::done(Message::Calls(calls::Message::SyncFinished)))
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn on_contacts(&mut self, msg: contacts::Message) -> Task<Message> {
        match msg {
            contacts::Message::Select(id) => {
                self.contacts.toggle(&id);
                Task::none()
            }
            contacts::Message::SearchChanged(val) => {
                self.contacts.search = val;
                Task::none()
            }
            contacts::Message::ToggleFilterMenu => {
                self.contacts.filter_open = !self.contacts.filter_open;
                Task::none()
            }
            contacts::Message::ToggleSource(source, shown) => {
                self.contacts.toggle_source(&source, shown);
                Task::none()
            }
            contacts::Message::SyncPressed => {
                self.contacts.contacts.clear();
                self.contacts.loading = true;
                self.stream_contacts()
            }
            contacts::Message::ContactsBatch(batch) => {
                self.contacts.merge(batch);
                Task::none()
            }
            contacts::Message::SyncFinished => {
                self.contacts.loading = false;
                self.store_learned_certificate();
                if let Some(ref cmd) = self.config.commands.on_contacts_synced {
                    let _ = run_hook(cmd, &[("count", &self.contacts.contacts.len().to_string())]);
                }
                Task::none()
            }
            contacts::Message::DialContact(target) => {
                self.dial_number = target;
                let dial_task = self.dial();
                if let Some(id) = self.main_window {
                    Task::batch([window::gain_focus(id), dial_task])
                } else {
                    dial_task
                }
            }

            // Contact modal:
            contacts::Message::OpenEditContact(c) => {
                let template_cmd = match &c.source {
                    RecordSource::Google { .. } => &self.config.commands.edit_google,
                    RecordSource::FritzBox { .. } => &self.config.commands.edit_fritzbox,
                    RecordSource::CardDav { .. } => &self.config.commands.edit_carddav,
                    RecordSource::Local => &self.config.commands.edit_local,
                    RecordSource::Other(s) if s.contains("Evolution") => &self.config.commands.edit_eds,
                    RecordSource::Other(_) => &self.config.commands.edit_default,
                };

                let short_id = c.id.rsplit('-').next().unwrap_or(&c.id);
                let primary_num = c.primary_number().unwrap_or_default();
                let account = match &c.source {
                    RecordSource::CardDav { account } => account.as_str(),
                    RecordSource::Google { email } => email.as_str(),
                    _ => "",
                };
                let (phonebook_id_str, registrar_str) = match &c.source {
                    RecordSource::FritzBox { phonebook_id, .. } => (
                        phonebook_id.to_string(),
                        self.config.account.registrar.as_str(),
                    ),
                    _ => (String::new(), ""),
                };
                let path = crate::consts::default_contacts_dir_string();
                let target = if account.is_empty() {
                    path.clone()
                } else {
                    account.to_string()
                };

                // Every one of these comes from a contact provider — a router
                // phonebook, a Google account, a CardDAV server — so none of
                // them may reach the shell as syntax. See `run_hook`.
                let _ = run_hook(
                    template_cmd,
                    &[
                        ("id", &c.id),
                        ("short_id", short_id),
                        ("phonebook_id", &phonebook_id_str),
                        ("registrar", registrar_str),
                        ("account", account),
                        ("path", &path),
                        ("target", &target),
                        ("name", &c.name),
                        ("number", primary_num),
                        ("source", &c.source.to_string()),
                    ],
                );
                Task::none()
            }
            contacts::Message::DeleteContact(id) => {
                if let Some(dir) = crate::consts::default_contacts_dir() {
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("vcf") {
                                if let Ok(content) = std::fs::read_to_string(&path) {
                                    if content.contains(&id) || path.to_string_lossy().contains(&id) {
                                        let _ = std::fs::remove_file(path);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                Task::done(Message::Contacts(contacts::Message::SyncPressed))
            }

            // Providers modal:
            contacts::Message::BlockNumberPrompt(number, name) => {
                self.contacts.block_prompt = Some((number, name));
                Task::none()
            }
            contacts::Message::ConfirmBlockNumber(number, name, action) => {
                self.contacts.block_prompt = None;
                self.block_number(&number, name, action);
                Task::none()
            }
            contacts::Message::CancelBlockPrompt => {
                self.contacts.block_prompt = None;
                Task::none()
            }
        }
    }

    /// Opens the call history window, or focuses it if already open.
    pub(super) fn open_calls(&mut self) -> Task<Message> {
        if let Some(id) = self.calls_window {
            return window::gain_focus(id);
        }

        let (id, open) = window::open(crate::calls_window_settings());
        self.calls_window = Some(id);

        if self.calls.calls.is_empty() && !self.calls.loading {
            self.calls.loading = true;
            return Task::batch([open.map(Message::CallsOpened), self.stream_calls()]);
        }
        open.map(Message::CallsOpened)
    }

    pub(super) fn on_calls(&mut self, msg: calls::Message) -> Task<Message> {
        match msg {
            calls::Message::SearchChanged(val) => {
                self.calls.search = val;
                Task::none()
            }
            calls::Message::SyncPressed => {
                self.calls.calls.clear();
                self.calls.loading = true;
                self.stream_calls()
            }
            calls::Message::CallsBatch(batch) => {
                self.calls.merge(batch);
                Task::none()
            }
            calls::Message::SyncFinished => {
                self.calls.loading = false;
                if let Some(ref cmd) = self.config.commands.on_history_synced {
                    let _ = run_hook(cmd, &[("count", &self.calls.calls.len().to_string())]);
                }
                Task::none()
            }
            calls::Message::DialNumber(target) => {
                self.dial_number = target;
                let dial_task = self.dial();
                if let Some(id) = self.main_window {
                    Task::batch([window::gain_focus(id), dial_task])
                } else {
                    dial_task
                }
            }

            // In-window Settings & Call Blocking:
            calls::Message::Select(id) => {
                self.calls.toggle(&id);
                Task::none()
            }
            calls::Message::FilterChanged(filter) => {
                self.calls.filter = filter;
                // Opening the Missed filter is the user seeing them, so the
                // badge clears here rather than counting for ever.
                if filter == calls::Filter::Missed {
                    self.acknowledge_missed();
                }
                Task::none()
            }
            calls::Message::ClearHistoryPressed => {
                let _ = self.sync_manager.local_store().clear_calls();
                Task::done(Message::Calls(calls::Message::SyncPressed))
            }
            calls::Message::BlockNumberPrompt(number, name) => {
                self.calls.block_prompt = Some((number, name));
                Task::none()
            }
            calls::Message::ConfirmBlockNumber(number, name, action) => {
                self.calls.block_prompt = None;
                self.block_number(&number, name, action);
                Task::none()
            }
            calls::Message::CancelBlockPrompt => {
                self.calls.block_prompt = None;
                Task::none()
            }
        }
    }

    /// Adds or replaces a block rule, storing the caller's number rather than
    /// whatever string the UI happened to show.
    ///
    /// Entries with nothing dialable in them are rejected: they can never match
    /// (see `number_matches`), so storing one only produces a rule the user
    /// believes is protecting them.
    pub(super) fn block_number(&mut self, number: &str, name: Option<String>, action: BlockAction) {
        let number = sipster_integrations::caller_number(number).to_string();
        if sipster_integrations::normalize_number(&number).is_empty() {
            self.status = "Cannot block an entry with no number in it".into();
            return;
        }

        let blocked = &mut self.config.integration.blocked_numbers;
        blocked.retain(|b| !sipster_integrations::number_matches(&number, &b.number));
        blocked.push(BlockedNumber {
            number,
            name,
            action,
            added_at: chrono_now_iso(),
        });
        self.persist();
    }
}



