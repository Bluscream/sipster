//! `CardDAV` and vCard contact synchronization provider.

use crate::model::{Contact, RecordSource};

/// Configuration for connecting to a `CardDAV` address book.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct CardDavConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}

/// Redacts the account password.
impl std::fmt::Debug for CardDavConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardDavConfig")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// A `CardDAV` client.
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

        let mut req = crate::http_agent().get(url);
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

        // One shared vCard parser for every provider that speaks it.
        for card in crate::vcard::split_cards(&body) {
            if let Some(contact) = crate::vcard::parse(
                card,
                "carddav",
                RecordSource::CardDav { account: self.config.url.clone() },
            ) {
                contacts.push(contact);
            }
        }

        Ok(contacts)
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
