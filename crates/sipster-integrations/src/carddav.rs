//! `CardDAV` and vCard contact synchronization provider.

use crate::model::{Contact, NumberType, PhoneNumber, RecordSource};

/// Configuration for connecting to a `CardDAV` address book.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CardDavConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}

/// A `CardDAV` client stub ready for `WebDAV` PROPFIND / REPORT requests and vCard parsing.
#[derive(Debug, Clone)]
pub struct CardDavClient {
    pub config: CardDavConfig,
}

impl CardDavClient {
    pub fn new(config: CardDavConfig) -> Self {
        Self { config }
    }

    /// Fetches contacts from the remote `CardDAV` server.
    pub fn fetch_contacts(&self) -> Result<Vec<Contact>, String> {
        // vCard 3.0 / 4.0 parser and CardDAV PROPFIND pipeline will be implemented here
        Ok(Vec::new())
    }

    /// Parses a single vCard (vCard 3.0 or 4.0) into a unified `Contact`.
    pub fn parse_vcard(vcard_data: &str, account_label: &str) -> Option<Contact> {
        let mut fn_name = None;
        let mut numbers = Vec::new();
        let mut emails = Vec::new();

        for line in vcard_data.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("FN:") {
                fn_name = Some(rest.trim().to_string());
            } else if line.starts_with("TEL") {
                if let Some((attrs, num)) = line.split_once(':') {
                    let num = num.trim();
                    if !num.is_empty() {
                        let attrs_upper = attrs.to_ascii_uppercase();
                        let num_type = if attrs_upper.contains("CELL") || attrs_upper.contains("MOBILE") {
                            NumberType::Mobile
                        } else if attrs_upper.contains("WORK") {
                            NumberType::Work
                        } else if attrs_upper.contains("FAX") {
                            NumberType::Fax
                        } else {
                            NumberType::Home
                        };
                        numbers.push(PhoneNumber {
                            number: num.to_string(),
                            number_type: num_type,
                            priority: if attrs_upper.contains("PREF") { 1 } else { 2 },
                        });
                    }
                }
            } else if line.starts_with("EMAIL") {
                if let Some((_, email)) = line.split_once(':') {
                    let email = email.trim();
                    if !email.is_empty() {
                        emails.push(email.to_string());
                    }
                }
            }
        }

        let name = fn_name?;
        Some(Contact {
            id: format!("carddav-{account_label}-{}", name.replace(' ', "_")),
            name,
            numbers,
            emails,
            source: RecordSource::CardDav {
                account: account_label.to_string(),
            },
        })
    }
}
