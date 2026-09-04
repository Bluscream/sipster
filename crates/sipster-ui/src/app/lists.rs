//! The contacts and history windows: opening them, streaming their data
//! in, and everything their messages ask for.

use super::{
    chrono_now_iso, BlockAction, BlockedNumber, Contact, Message, NumberType, PhoneNumber,
    RecordSource, SipsterApp, Task,
};
use crate::{calls, contacts};
use iced::window;

impl SipsterApp {
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
            contacts::Message::OpenNewContact => {
                self.contacts.edit_draft = Some(contacts::EditContactDraft {
                    id: None,
                    name: String::new(),
                    phone: String::new(),
                    email: String::new(),
                });
                Task::none()
            }
            contacts::Message::OpenEditContact(c) => {
                let phone = c.numbers.first().map_or(String::new(), |p| p.number.clone());
                let email = c.emails.first().cloned().unwrap_or_default();
                self.contacts.edit_draft = Some(contacts::EditContactDraft {
                    id: Some(c.id),
                    name: c.name,
                    phone,
                    email,
                });
                Task::none()
            }
            contacts::Message::EditNameChanged(v) => {
                if let Some(ref mut d) = self.contacts.edit_draft {
                    d.name = v;
                }
                Task::none()
            }
            contacts::Message::EditPhoneChanged(v) => {
                if let Some(ref mut d) = self.contacts.edit_draft {
                    d.phone = v;
                }
                Task::none()
            }
            contacts::Message::EditEmailChanged(v) => {
                if let Some(ref mut d) = self.contacts.edit_draft {
                    d.email = v;
                }
                Task::none()
            }
            contacts::Message::SaveContact => {
                if let Some(draft) = self.contacts.edit_draft.take() {
                    let id = draft.id.unwrap_or_else(|| {
                        format!(
                            "local-contact-{}",
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis()
                        )
                    });
                    let mut numbers = Vec::new();
                    if !draft.phone.trim().is_empty() {
                        numbers.push(PhoneNumber {
                            number: draft.phone.trim().to_string(),
                            number_type: NumberType::Mobile,
                            priority: 1,
                        });
                    }
                    let mut emails = Vec::new();
                    if !draft.email.trim().is_empty() {
                        emails.push(draft.email.trim().to_string());
                    }
                    let contact = Contact {
                        id,
                        name: draft.name,
                        numbers,
                        emails,
                        source: RecordSource::Local,
                    };
                    let _ = self.sync_manager.local_store().upsert_contact(contact);
                    return Task::done(Message::Contacts(contacts::Message::SyncPressed));
                }
                Task::none()
            }
            contacts::Message::CancelEditContact => {
                self.contacts.edit_draft = None;
                Task::none()
            }
            contacts::Message::DeleteContact(id) => {
                let _ = self.sync_manager.local_store().delete_contact(&id);
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
                Task::none()
            }
            calls::Message::AddContact(number, name) => {
                // Seed the contact editor from the call and switch to it.
                self.contacts.edit_draft = Some(contacts::EditContactDraft {
                    id: None,
                    name: name.unwrap_or_default(),
                    phone: number,
                    email: String::new(),
                });
                self.open_contacts()
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
