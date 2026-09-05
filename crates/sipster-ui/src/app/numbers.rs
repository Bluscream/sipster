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
    // Two logging branches either side of a lookup; the macros are what
    // clippy is counting, not the logic.
    #[allow(clippy::cognitive_complexity)]
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

        let router_contacts = router_device_contacts(found);
        if router_contacts.is_empty() {
            tracing::debug!(
                devices = found.len(),
                "the router listed no telephony device with an internal number"
            );
        } else {
            tracing::info!(
                count = router_contacts.len(),
                "adding the router's telephony devices to contacts"
            );
            // Most will merge straight into a phonebook entry that already
            // names the same device; the count is what arrived, not what was
            // added.
            self.contacts.merge(router_contacts);
        }
    }

    /// Returns the internal and external numbers for the active account, if known.
    pub fn active_numbers(&self) -> Option<(&str, &str)> {
        let entry = self.numbers.as_ref()?;
        Some((entry.internal.as_str(), entry.external.as_str()))
    }

    /// How the account identifies itself in the status bar, as
    /// `620@192.168.2.1:5060`.
    ///
    /// The extension people actually dial to reach this copy, rather than the
    /// SIP username — the username is a login, and on a FRITZ!Box it says
    /// nothing about which phone rings. Falls back to the username until the
    /// router has been asked, which is also the answer for any registrar that
    /// cannot be asked at all.
    pub fn account_identity(&self) -> Option<String> {
        let account = self
            .engine()
            .map_or(&self.config.account, |engine| engine.account());
        if account.registrar.trim().is_empty() {
            return None;
        }

        let who = self
            .numbers
            .as_ref()
            .map(|entry| entry.internal.as_str())
            .filter(|internal| !internal.is_empty())
            .unwrap_or(account.username.as_str());

        // The port is only worth the space when it is not the one implied by
        // the transport — 5060 for UDP and TCP, 5061 for TLS. Showing `:5060`
        // on every line is noise.
        if account.port == account.transport.default_port() {
            Some(format!("{who}@{}", account.registrar))
        } else {
            Some(format!("{who}@{}:{}", account.registrar, account.port))
        }
    }
}

/// Turns the router's telephony devices into contacts.
///
/// Every extension on the router is dialable from here, so each becomes a
/// contact named after the device. The `**` prefix is what makes the call
/// internal — dialling a bare `622` sends it to the outside line instead.
fn router_device_contacts(
    found: &[sipster_integrations::fritzbox::AccountNumbers],
) -> Vec<sipster_integrations::Contact> {
    found
        .iter()
        .filter(|dev| !dev.internal.trim().is_empty())
        .map(|dev| {
            let internal = dev.internal.trim();
            let number = if internal.starts_with("**") {
                internal.to_string()
            } else {
                format!("**{internal}")
            };
            let name = if dev.phone_name.trim().is_empty() {
                format!("Internal {internal}")
            } else {
                dev.phone_name.trim().to_string()
            };
            sipster_integrations::Contact {
                id: format!("fritzbox-dev-{}", dev.username),
                name,
                numbers: vec![sipster_integrations::PhoneNumber {
                    number,
                    number_type: sipster_integrations::NumberType::Intern,
                    priority: 1,
                }],
                emails: Vec::new(),
                merged_from: Vec::new(),
                source: sipster_integrations::RecordSource::FritzBox {
                    phonebook_id: 0,
                    phonebook_name: "Router Devices".to_string(),
                },
            }
        })
        .collect()
}
