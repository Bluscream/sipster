//! Google Contacts OAuth 2.0 and People API integration.
//!
//! Supports multi-account Google People API contact synchronization.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use tracing::info;

use crate::model::{Contact, NumberType, PhoneNumber, RecordSource};

/// Default Google OAuth Client ID for desktop applications.
pub const DEFAULT_CLIENT_ID: &str = "1032895697664-9sbl5m06fl5r9q5a9n519l2p8vd1b2h1.apps.googleusercontent.com";
pub const DEFAULT_CLIENT_SECRET: &str = "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l2p8";

/// Structure representing a token response from Google OAuth 2.0.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GoogleTokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_type: Option<String>,
}

/// Information about the authorized Google user.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GoogleUserInfo {
    pub email: Option<String>,
}

/// Client for communicating with Google People API v1.
#[derive(Debug, Clone)]
pub struct GoogleContactsClient {
    pub account_id: String,
    pub email: String,
    pub refresh_token: String,
    pub client_id: String,
    pub client_secret: String,
}

impl GoogleContactsClient {
    pub fn new(
        account_id: String,
        email: String,
        refresh_token: String,
        client_id: Option<String>,
        client_secret: Option<String>,
    ) -> Self {
        Self {
            account_id,
            email,
            refresh_token,
            client_id: client_id.unwrap_or_else(|| DEFAULT_CLIENT_ID.to_string()),
            client_secret: client_secret.unwrap_or_else(|| DEFAULT_CLIENT_SECRET.to_string()),
        }
    }

    /// Refreshes the OAuth 2.0 access token using the stored refresh token.
    pub fn refresh_access_token(&self) -> Result<String, String> {
        let resp = ureq::post("https://oauth2.googleapis.com/token")
            .send_form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", self.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .map_err(|e| format!("failed to refresh Google token: {e}"))?;

        let token_resp: GoogleTokenResponse = resp
            .into_json()
            .map_err(|e| format!("invalid token response JSON: {e}"))?;

        Ok(token_resp.access_token)
    }

    /// Fetches all contacts from the user's Google account via the People API using pagination.
    pub fn fetch_contacts(&self) -> Result<Vec<Contact>, String> {
        let access_token = self.refresh_access_token()?;
        let mut contacts = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut url = "https://people.googleapis.com/v1/people/me/connections?personFields=names,phoneNumbers,emailAddresses&pageSize=100".to_string();
            if let Some(ref token) = page_token {
                url.push_str("&pageToken=");
                url.push_str(&urlencoding_simple(token));
            }

            let resp = ureq::get(&url)
                .set("Authorization", &format!("Bearer {access_token}"))
                .call()
                .map_err(|e| format!("People API request failed: {e}"))?;

            let body: serde_json::Value = resp
                .into_json()
                .map_err(|e| format!("People API JSON parsing failed: {e}"))?;

            if let Some(connections) = body.get("connections").and_then(|c| c.as_array()) {
                for person in connections {
                    let name = person
                        .get("names")
                        .and_then(|n| n.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|obj| obj.get("displayName"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .trim();

                    if name.is_empty() {
                        continue;
                    }

                    let resource_name = person
                        .get("resourceName")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();

                    let mut numbers = Vec::new();
                    if let Some(phones) = person.get("phoneNumbers").and_then(|p| p.as_array()) {
                        for phone in phones {
                            if let Some(val) = phone.get("value").and_then(|v| v.as_str()) {
                                let val = val.trim();
                                if !val.is_empty() {
                                    let phone_type = phone
                                        .get("type")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("other");

                                    let number_type = match phone_type.to_lowercase().as_str() {
                                        "mobile" | "cell" => NumberType::Mobile,
                                        "work" => NumberType::Work,
                                        "home" => NumberType::Home,
                                        "fax" => NumberType::Fax,
                                        other => NumberType::Other(other.to_string()),
                                    };

                                    numbers.push(PhoneNumber {
                                        number: val.to_string(),
                                        number_type,
                                        priority: if numbers.is_empty() { 1 } else { 2 },
                                    });
                                }
                            }
                        }
                    }

                    let mut emails = Vec::new();
                    if let Some(email_arr) = person.get("emailAddresses").and_then(|e| e.as_array()) {
                        for email in email_arr {
                            if let Some(val) = email.get("value").and_then(|v| v.as_str()) {
                                emails.push(val.trim().to_string());
                            }
                        }
                    }

                    contacts.push(Contact {
                        id: format!("google-{}-{resource_name}", self.account_id),
                        name: name.to_string(),
                        numbers,
                        emails,
                        source: RecordSource::Google {
                            email: self.email.clone(),
                        },
                    });
                }
            }

            // Check for next page
            if let Some(next_token) = body.get("nextPageToken").and_then(|t| t.as_str()) {
                if !next_token.is_empty() {
                    page_token = Some(next_token.to_string());
                    continue;
                }
            }
            break;
        }

        info!(count = contacts.len(), email = %self.email, "synced Google contacts");
        Ok(contacts)
    }

    /// Spins up a local redirect listener on `127.0.0.1:8765`, generates the Google OAuth URL,
    /// and waits for the browser redirect callback to obtain the authorization code.
    pub fn listen_for_auth_code(port: u16) -> Result<(String, String), String> {
        let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
            .map_err(|e| format!("could not bind redirect listener on port {port}: {e}"))?;

        let (mut stream, _) = listener
            .accept()
            .map_err(|e| format!("failed to accept redirect connection: {e}"))?;

        let mut reader = BufReader::new(&mut stream);
        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .map_err(|e| format!("failed to read HTTP request: {e}"))?;

        // Format: GET /?code=4/0Afge... HTTP/1.1
        let code = if let Some(code_start) = request_line.find("code=") {
            let rest = &request_line[code_start + 5..];
            let end = rest.find('&').or_else(|| rest.find(' ')).unwrap_or(rest.len());
            rest[..end].to_string()
        } else {
            return Err("no authorization code found in redirect URL".into());
        };

        // Send a friendly success response back to browser
        let html_body = "<!DOCTYPE html><html><body style='font-family:sans-serif;text-align:center;padding:50px;'>\
            <h2>Sipster &mdash; Google Account Connected!</h2>\
            <p>You can close this tab and return to Sipster.</p>\
            </body></html>";
        let http_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            html_body.len(),
            html_body
        );
        let _ = stream.write_all(http_response.as_bytes());

        let redirect_uri = format!("http://127.0.0.1:{port}");
        Ok((code, redirect_uri))
    }

    /// Exchanges the authorization code for access and refresh tokens.
    pub fn exchange_auth_code(
        code: &str,
        redirect_uri: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<GoogleTokenResponse, String> {
        let resp = ureq::post("https://oauth2.googleapis.com/token")
            .send_form(&[
                ("code", code),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("redirect_uri", redirect_uri),
                ("grant_type", "authorization_code"),
            ])
            .map_err(|e| format!("OAuth token exchange failed: {e}"))?;

        let token: GoogleTokenResponse = resp
            .into_json()
            .map_err(|e| format!("invalid token response JSON: {e}"))?;

        Ok(token)
    }

    /// Fetches the authenticated user's email address using the access token.
    pub fn fetch_user_email(access_token: &str) -> Result<String, String> {
        let resp = ureq::get("https://www.googleapis.com/oauth2/v2/userinfo")
            .set("Authorization", &format!("Bearer {access_token}"))
            .call()
            .map_err(|e| format!("failed to fetch user info: {e}"))?;

        let user_info: GoogleUserInfo = resp
            .into_json()
            .map_err(|e| format!("failed to parse user info: {e}"))?;

        user_info
            .email
            .ok_or_else(|| "no email found in user profile".into())
    }

    /// Builds the Google OAuth 2.0 authorization URL for the user to open in their browser.
    pub fn build_auth_url(client_id: &str, redirect_uri: &str) -> String {
        let scope = "https://www.googleapis.com/auth/contacts.readonly https://www.googleapis.com/auth/userinfo.email";
        format!(
            "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id={}&redirect_uri={}&scope={}&access_type=offline&prompt=consent",
            client_id,
            urlencoding_simple(redirect_uri),
            urlencoding_simple(scope)
        )
    }
}

fn urlencoding_simple(input: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}
