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
        let url = &self.config.url;
        if url.trim().is_empty() {
            return Ok(Vec::new());
        }

        let mut req = ureq::get(url);
        if !self.config.username.is_empty() {
            let auth = format!("{}:{}", self.config.username, self.config.password);
            let b64 = urlencoding_base64(&auth);
            req = req.set("Authorization", &format!("Basic {b64}"));
        }

        let http_resp = match req.call() {
            Ok(r) => r,
            Err(e) => return Err(format!("CardDAV server error: {e}")),
        };

        let body = http_resp.into_string().map_err(|e| format!("failed reading CardDAV response: {e}"))?;
        let mut contacts = Vec::new();

        // Split multiple vCards if present in the stream
        let mut text_cursor = body.as_str();
        while let Some(start) = text_cursor.find("BEGIN:VCARD") {
            let Some(end) = text_cursor[start..].find("END:VCARD") else { break };
            let vcard_text = &text_cursor[start..start + end + 9];
            text_cursor = &text_cursor[start + end + 9..];
            if let Some(contact) = Self::parse_vcard(vcard_text, &self.config.url) {
                contacts.push(contact);
            }
        }

        Ok(contacts)
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

fn urlencoding_base64(input: &str) -> String {
    const CHARSET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut out = String::new();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let b0 = bytes[i];
        let b1 = if i + 1 < len { bytes[i + 1] } else { 0 };
        let b2 = if i + 2 < len { bytes[i + 2] } else { 0 };

        let idx0 = (b0 >> 2) & 0x3F;
        let idx1 = ((b0 & 0x03) << 4) | ((b1 >> 4) & 0x0F);
        let idx2 = ((b1 & 0x0F) << 2) | ((b2 >> 6) & 0x03);
        let idx3 = b2 & 0x3F;

        out.push(CHARSET[idx0 as usize] as char);
        out.push(CHARSET[idx1 as usize] as char);
        if i + 1 < len {
            out.push(CHARSET[idx2 as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < len {
            out.push(CHARSET[idx3 as usize] as char);
        } else {
            out.push('=');
        }

        i += 3;
    }
    out
}
