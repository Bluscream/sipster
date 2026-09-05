//! Discovering each account's own phone numbers from the router.
//!
//! A SIP account is configured with a username, a registrar and a password.
//! None of that says what number reaches it. On a FRITZ!Box the account is one
//! of several telephony devices, and the router assigns it an extension other
//! phones in the house dial — `620` — and an outgoing number it presents to
//! the world. Sipster could not previously tell the user either.
//!
//! The router knows both, so it is asked. Matching is by SIP username, which
//! is what the account and the router's client list have in common.

use iced::Task;

use super::{Message, SipsterApp};

impl SipsterApp {
    /// Asks the router which numbers belong to each configured account.
    ///
    /// Returns an empty task when no router is configured, which is the
    /// common case for anyone not on a FRITZ!Box.
    pub(super) fn discover_numbers(&self) -> Task<Message> {
        let fb = &self.config.integration.fritzbox;
        if !fb.enabled || fb.host.trim().is_empty() {
            return Task::none();
        }

        let config = sipster_integrations::FritzConfig {
            host: fb.host.clone(),
            port: fb.port,
            username: fb.username.clone(),
            password: fb.password.clone(),
            tls: fb.tls,
            cert_fingerprint: fb.cert_fingerprint.clone(),
        };

        // TR-064 is blocking and talks to the network, so it must not run on
        // the UI thread; one SOAP call per configured device adds up.
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let client = sipster_integrations::fritzbox::FritzBoxClient::new(config);
                    match client.fetch_account_numbers() {
                        Ok(found) => found,
                        Err(err) => {
                            tracing::warn!(%err, "could not ask the router for our own numbers");
                            Vec::new()
                        }
                    }
                })
                .await
                .unwrap_or_default()
            },
            Message::RouterNumbers,
        )
    }

    /// Records the numbers the router reported for our account.
    ///
    /// The router lists every telephony device it knows, most of which are not
    /// us — other handsets, a DECT phone, a mobile app. Only the entry whose
    /// SIP username matches the configured account is kept.
    pub(super) fn on_router_numbers(
        &mut self,
        found: &[sipster_integrations::fritzbox::AccountNumbers],
    ) {
        self.numbers = found
            .iter()
            .find(|entry| entry.username == self.config.account.username)
            .cloned();
        if let Some(entry) = &self.numbers {
            tracing::info!(
                username = %entry.username,
                internal = %entry.internal,
                external = %entry.external,
                phone_name = %entry.phone_name,
                "the router knows this account's numbers"
            );
        }

        // Auto-insert router telephony devices into contacts as internal numbers with ** prefix
        let mut router_contacts = Vec::new();
        for dev in found {
            let internal = dev.internal.trim();
            if internal.is_empty() {
                continue;
            }
            let int_num = if internal.starts_with("**") {
                internal.to_string()
            } else {
                format!("**{internal}")
            };
            let name = if !dev.phone_name.trim().is_empty() {
                dev.phone_name.trim().to_string()
            } else {
                format!("Internal {internal}")
            };
            router_contacts.push(sipster_integrations::Contact {
                id: format!("fritzbox-dev-{}", dev.username),
                name,
                numbers: vec![sipster_integrations::PhoneNumber {
                    number: int_num,
                    number_type: sipster_integrations::NumberType::Intern,
                    priority: 1,
                }],
                emails: Vec::new(),
                source: sipster_integrations::RecordSource::FritzBox {
                    phonebook_id: 0,
                    phonebook_name: "Router Devices".to_string(),
                },
            });
        }
        if !router_contacts.is_empty() {
            self.contacts.merge(router_contacts);
        }
    }

    /// Returns the internal and external numbers for the active account, if known.
    pub fn active_numbers(&self) -> Option<(&str, &str)> {
        let entry = self.numbers.as_ref()?;
        Some((entry.internal.as_str(), entry.external.as_str()))
    }
}
