//! Google Contacts OAuth 2.0 and People API integration.
//!
//! Supports multi-account Google People API contact synchronization.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use tracing::info;

use crate::model::{Contact, NumberType, PhoneNumber, RecordSource};

/// How long to wait for the user to finish signing in before giving up.
const AUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Set when the application is shutting down, so a pending sign-in stops
/// Generation counter for the current auth attempt. Incrementing this immediately
/// invalidates and aborts any previously running `wait_for_code` loop.
static AUTH_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Asks any in-flight sign-in to give up. Call on shutdown or before starting a new flow.
pub fn cancel_pending_auth() {
    AUTH_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Starts a new auth session, incrementing the generation counter to invalidate previous ones.
fn begin_auth() -> u64 {
    AUTH_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

fn cancelled(generation: u64) -> bool {
    AUTH_GENERATION.load(std::sync::atomic::Ordering::SeqCst) != generation
}

const FAILURE_PAGE: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n    <!DOCTYPE html><html><body style='font-family:sans-serif;text-align:center;padding:50px;'>    <h2>Sign-in was not completed</h2><p>You can close this tab and try again in Sipster.</p>    </body></html>";

/// Opens `url` in the user's default browser.
fn open_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";

    std::process::Command::new(program)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

// There are deliberately no built-in OAuth credentials.
//
// The pair previously hard-coded here could not have worked: the "secret"
// embedded the client id's own random segment, so it was invented rather than
// issued by Google. Shipping it made the integration look configured while
// every token request would have failed with invalid_client — and a real
// desktop client secret is not a secret once published anyway.
//
// Users register their own OAuth client (Google Cloud console → Credentials →
// OAuth client ID → Desktop app) and paste the id and secret into Settings.

/// Structure representing a token response from Google OAuth 2.0.
#[derive(Clone, serde::Deserialize)]
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
#[derive(Clone)]
pub struct GoogleContactsClient {
    pub account_id: String,
    pub email: String,
    pub refresh_token: String,
    pub client_id: String,
    pub client_secret: String,
}

/// Redacts the refresh token and client secret; both grant access to the
/// user's contacts on their own.
impl std::fmt::Debug for GoogleContactsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleContactsClient")
            .field("account_id", &self.account_id)
            .field("email", &self.email)
            .field("refresh_token", &"<redacted>")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

/// The token response carries a live access token; never print it.
impl std::fmt::Debug for GoogleTokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleTokenResponse")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .field("expires_in", &self.expires_in)
            .field("token_type", &self.token_type)
            .finish()
    }
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
            client_id: client_id.unwrap_or_default(),
            client_secret: client_secret.unwrap_or_default(),
        }
    }

    /// Whether this account has the OAuth credentials it needs.
    ///
    /// Checked before any request so a missing client id reports "not
    /// configured" instead of an opaque `invalid_client` from Google.
    pub fn is_configured(&self) -> bool {
        !self.client_id.trim().is_empty()
            && !self.client_secret.trim().is_empty()
            && !self.refresh_token.trim().is_empty()
    }

    /// Refreshes the OAuth 2.0 access token using the stored refresh token.
    pub fn refresh_access_token(&self) -> Result<String, String> {
        if !self.is_configured() {
            return Err(format!(
                "Google account {} has no OAuth client id/secret — add them in Settings",
                self.email
            ));
        }
        let resp = crate::http_agent()
            .post("https://oauth2.googleapis.com/token")
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

            let resp = crate::http_agent()
                .get(&url)
                .set("Authorization", &format!("Bearer {access_token}"))
                .call()
                .map_err(|e| format!("People API request failed: {e}"))?;

            let body: serde_json::Value = resp
                .into_json()
                .map_err(|e| format!("People API JSON parsing failed: {e}"))?;

            log_page(&self.email, &body);

            if let Some(connections) = body.get("connections").and_then(|c| c.as_array()) {
                for person in connections {
                    if let Some(contact) = self.parse_person(person) {
                        contacts.push(contact);
                    }
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

    fn parse_person(&self, person: &serde_json::Value) -> Option<Contact> {
        let name = person
            .get("names")
            .and_then(|n| n.as_array())
            .and_then(|arr| arr.first())
            .and_then(|obj| obj.get("displayName"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();

        if name.is_empty() {
            return None;
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

        Some(Contact {
            id: format!("google-{}-{resource_name}", self.account_id),
            name: name.to_string(),
            numbers,
            emails,
            source: RecordSource::Google {
                email: self.email.clone(),
            },
        })
    }

    /// Runs the whole desktop OAuth flow and returns `(email, refresh_token)`.
    ///
    /// Opens the consent page in the user's browser, waits for Google to
    /// redirect back to a local listener, exchanges the code, and reads the
    /// account's address. The caller only has to supply its own client
    /// credentials.
    ///
    /// # Errors
    ///
    /// Reports a missing client id, a browser that never came back within
    /// [`AUTH_TIMEOUT`], or any rejection from Google.
    pub fn authorize(
        client_id: &str,
        client_secret: &str,
        port: u16,
    ) -> Result<(String, String), String> {
        if client_id.trim().is_empty() || client_secret.trim().is_empty() {
            return Err("enter your Google OAuth client id and secret first".into());
        }

        // Google desktop clients register "http://localhost" and accept any
        // port on it. Sending 127.0.0.1 instead risks redirect_uri_mismatch.
        // Cancel any previous in-flight flow and record this flow's generation
        let generation = begin_auth();
        let redirect_uri = format!("http://localhost:{port}");

        // If an old listener was just cancelled, give it up to 1 second to release the port
        let bind_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let listener = loop {
            match TcpListener::bind(format!("127.0.0.1:{port}")) {
                Ok(l) => break l,
                Err(e) => {
                    if cancelled(generation) {
                        return Err("sign-in cancelled".into());
                    }
                    if std::time::Instant::now() >= bind_deadline {
                        return Err(format!("could not listen on port {port}: {e}"));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        };

        // Open the consent page only once the listener is up, so the redirect
        // cannot arrive before anything is waiting for it.
        let auth_url = Self::build_auth_url(client_id, &redirect_uri);
        if let Err(e) = open_in_browser(&auth_url) {
            return Err(format!(
                "could not open a browser ({e}). Visit this URL manually:\n{auth_url}"
            ));
        }

        let code = Self::wait_for_code(&listener, generation)?;
        let token = Self::exchange_auth_code(&code, &redirect_uri, client_id, client_secret)?;
        let refresh_token = token
            .refresh_token
            .ok_or("Google returned no refresh token; remove Sipster from your account's third-party access and try again")?;
        let email = Self::fetch_user_email(&token.access_token)?;
        Ok((email, refresh_token))
    }

    /// Waits for the browser redirect and extracts the authorization code.
    fn wait_for_code(listener: &TcpListener, generation: u64) -> Result<String, String> {
        // Without a deadline this blocked forever when the user closed the tab
        // or never completed consent, pinning the worker thread for the rest of
        // the session.
        let deadline = std::time::Instant::now() + AUTH_TIMEOUT;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("could not configure the redirect listener: {e}"))?;

        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(accepted) => break accepted,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if cancelled(generation) {
                        return Err("sign-in cancelled".into());
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err("timed out waiting for the Google sign-in to finish".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                Err(e) => return Err(format!("failed to accept redirect connection: {e}")),
            }
        };
        stream
            .set_nonblocking(false)
            .map_err(|e| format!("could not read the redirect: {e}"))?;

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

        // Google reports consent failures in the query string too.
        if request_line.contains("error=") {
            if let Err(e) = stream.write_all(FAILURE_PAGE.as_bytes()) {
                tracing::debug!(error = %e, "could not write the sign-in failure page");
            }
            return Err("Google sign-in was denied or cancelled".into());
        }

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
        if let Err(e) = stream.write_all(http_response.as_bytes()) {
            // The sign-in itself already succeeded; only the browser's
            // confirmation page was lost.
            tracing::debug!(error = %e, "could not write the sign-in confirmation page");
        }
        Ok(code)
    }

    /// Exchanges the authorization code for access and refresh tokens.
    pub fn exchange_auth_code(
        code: &str,
        redirect_uri: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<GoogleTokenResponse, String> {
        let resp = crate::http_agent()
            .post("https://oauth2.googleapis.com/token")
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
        let resp = crate::http_agent()
            .get("https://www.googleapis.com/oauth2/v2/userinfo")
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

/// Reports what one People API page actually contained.
///
/// Distinguishes "this account has no saved contacts" from "we asked for the
/// wrong thing": the API reports its own totals, so an empty page alongside a
/// non-zero total means the request is at fault, not the address book.
fn log_page(email: &str, body: &serde_json::Value) {
    tracing::debug!(
        email,
        returned = body.get("connections").and_then(|c| c.as_array()).map_or(0, Vec::len),
        total_people = ?body.get("totalPeople"),
        total_items = ?body.get("totalItems"),
        keys = ?body.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()),
        "People API page"
    );
}

#[cfg(test)]
mod auth_cancel_tests {
    use super::{begin_auth, cancel_pending_auth, cancelled, GoogleContactsClient};
    use std::net::TcpListener;

    /// The wait runs on a `spawn_blocking` thread, and dropping a tokio
    /// runtime waits for those to finish. Before this was cancellable, a
    /// sign-in nobody completed kept the whole process alive for the full
    /// `AUTH_TIMEOUT` after `--quit`, and it had to be killed instead.
    #[test]
    fn cancelling_stops_the_wait_instead_of_running_out_the_timeout() {
        let generation = begin_auth();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");

        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            cancel_pending_auth();
        });

        let started = std::time::Instant::now();
        let result = GoogleContactsClient::wait_for_code(&listener, generation);
        let waited = started.elapsed();

        assert!(result.is_err(), "no browser ever connected");
        assert!(
            waited < std::time::Duration::from_secs(5),
            "gave up after {waited:?}, so cancellation was not observed"
        );
    }

    /// Clicking Connect again while a flow is pending must abandon the old
    /// one, or the second flow fights the first for the redirect port.
    #[test]
    fn starting_a_second_flow_invalidates_the_first() {
        let first = begin_auth();
        let second = begin_auth();

        assert_ne!(first, second, "each flow gets its own generation");
        assert!(cancelled(first), "the first flow is abandoned");
        assert!(!cancelled(second), "the newest flow is the live one");
    }
}
