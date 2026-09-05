//! FRITZ!Box TR-064 integration: phonebook and call list synchronization.
//!
//! The SOAP client lives here; the XML it reads and writes is in `xml`, and
//! the digest authentication TR-064 demands is in `digest`.

mod digest;
mod xml;

pub use xml::{parse_call_list_xml, parse_phonebook_xml};

use digest::{build_digest_header, parse_auth_header};
use xml::{escape_xml, extract_xml_tag};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::warn;

use crate::model::{CallRecord, Contact};

/// Errors originating from TR-064 router communication or XML parsing.
#[derive(Debug, thiserror::Error)]
pub enum FritzError {
    #[error("Network error: {0}")]
    Network(#[from] Box<ureq::Error>),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Authentication failed for user '{user}': {detail}")]
    AuthFailed { user: String, detail: String },
    #[error("SOAP action error ({action}): {detail}")]
    SoapAction { action: String, detail: String },
}

impl From<ureq::Error> for FritzError {
    fn from(err: ureq::Error) -> Self {
        Self::Network(Box::new(err))
    }
}

/// A certificate fingerprint reported back from a first TLS connection.
type LearnedCert = Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>;

/// A certificate fingerprint learned during the last sync, waiting to be
/// stored in the config.
///
/// A global because the sync runs deep inside a provider with no route back to
/// the settings, and the value is written once on first contact with a router.
pub static LEARNED_FINGERPRINT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Takes the fingerprint learned since the last call, if any.
pub fn take_learned_fingerprint() -> Option<String> {
    LEARNED_FINGERPRINT.lock().ok()?.take()
}

/// AVM's TR-064 TLS port. Fixed on the device and not the same as the plain
/// one, so it is not derived from the configured port.
pub const TLS_PORT: u16 = 49443;

/// AVM's plain-HTTP TR-064 port, and the historical default of the `port`
/// setting.
pub const PLAIN_PORT: u16 = 49000;

/// Configuration credentials to connect to a FRITZ!Box TR-064 interface.
#[derive(Clone)]
pub struct FritzConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// Talk to the router over TLS. See [`crate::pinned_tls`].
    pub tls: bool,
    /// The router certificate's fingerprint, learned on first use. Empty means
    /// "not learned yet"; the first TLS connection fills it in.
    pub cert_fingerprint: String,
}

/// Redacts the router password, which on a FRITZ!Box is also the admin
/// password for the whole router.
impl std::fmt::Debug for FritzConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FritzConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("cert_fingerprint", &self.cert_fingerprint)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl Default for FritzConfig {
    fn default() -> Self {
        Self {
            host: "fritz.box".into(),
            port: PLAIN_PORT,
            username: String::new(),
            password: String::new(),
            tls: true,
            cert_fingerprint: String::new(),
        }
    }
}

/// Client for FRITZ!Box TR-064 SOAP telephony and contact services.
#[derive(Debug, Clone)]
pub struct FritzBoxClient {
    config: FritzConfig,
    /// The digest challenge from the last 401, reused for later calls.
    ///
    /// TR-064 answers every unauthenticated request with a 401, so each SOAP
    /// call cost two HTTP round-trips. The realm and nonce stay valid for a
    /// while, so remembering them lets subsequent calls authenticate on the
    /// first try; a stale nonce simply 401s again and refreshes this.
    challenge: Arc<Mutex<Option<HashMap<String, String>>>>,
}

impl FritzBoxClient {
    pub fn new(mut config: FritzConfig) -> Self {
        // TR-064 listens on two ports, and the setting holds the plain one
        // because that is what it has always meant. Switching to TLS without
        // also switching the port would just fail to connect, so a port left
        // at the plain default follows the transport. An explicitly chosen
        // port is left alone.
        if config.tls && config.port == PLAIN_PORT {
            config.port = TLS_PORT;
        }
        Self {
            config,
            challenge: Arc::new(Mutex::new(None)),
        }
    }

    fn cached_challenge(&self) -> Option<HashMap<String, String>> {
        self.challenge
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn remember_challenge(&self, params: &HashMap<String, String>) {
        *self
            .challenge
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(params.clone());
    }

    /// Performs an authenticated TR-064 SOAP request.
    /// The HTTP agent for this router.
    ///
    /// Over TLS the certificate is self-signed, so the agent pins it by
    /// fingerprint rather than trusting a certificate authority. A fingerprint
    /// learned on a first connection comes back through the second value. See
    /// [`crate::pinned_tls`].
    fn agent(&self) -> (ureq::Agent, LearnedCert) {
        if self.config.tls {
            let (agent, seen) = crate::pinned_agent(self.config.cert_fingerprint.clone());
            (agent, Some(seen))
        } else {
            (crate::http_agent(), None)
        }
    }

    /// Publishes a certificate fingerprint learned on a first TLS connection,
    /// so it can be pinned from here on.
    fn record_learned_certificate(
        &self,
        learned: Option<&std::sync::Arc<std::sync::Mutex<Option<String>>>>,
    ) {
        let Some(fingerprint) = learned
            .and_then(|seen| seen.lock().ok()?.take())
        else {
            return;
        };
        tracing::info!(
            host = %self.config.host,
            %fingerprint,
            "learned the router's TLS certificate"
        );
        if let Ok(mut pending) = LEARNED_FINGERPRINT.lock() {
            *pending = Some(fingerprint);
        }
    }

    pub fn soap_call(
        &self,
        control_url: &str,
        service_type: &str,
        action: &str,
        args: &[(&str, &str)],
    ) -> Result<String, FritzError> {
        use std::fmt::Write as _;
        let mut args_xml = String::new();
        for (k, v) in args {
            // Escaped: argument values are interpolated straight into the SOAP
            // envelope, so an unescaped '<' or '&' would corrupt the request
            // (and a crafted value could inject elements).
            let _ = write!(args_xml, "<{k}>{}</{k}>", escape_xml(v));
        }

        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/" xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
  <s:Body>
    <u:{action} xmlns:u="{service_type}">
      {args_xml}
    </u:{action}>
  </s:Body>
</s:Envelope>"#
        );

        let soap_action = format!("{service_type}#{action}");
        // The port already follows the transport, see `new`.
        let url = if self.config.tls {
            format!("https://{}:{}{control_url}", self.config.host, self.config.port)
        } else {
            format!("http://{}:{}{control_url}", self.config.host, self.config.port)
        };
        let started = std::time::Instant::now();

        let (agent, learned) = self.agent();
        let mut request = agent
            .post(&url)
            .set("Content-Type", "text/xml; charset=\"utf-8\"")
            .set("SOAPAction", &soap_action);

        // Authenticate up front when we already hold a challenge, so only the
        // first call of a session pays for the 401 round-trip.
        let preauthorized = self.cached_challenge();
        if let Some(params) = &preauthorized {
            request = request.set(
                "Authorization",
                &build_digest_header(
                    &self.config.username,
                    &self.config.password,
                    "POST",
                    control_url,
                    params,
                ),
            );
        }
        let res = request.send_string(&body);

        self.record_learned_certificate(learned.as_ref());

        match res {
            Ok(response) => {
                tracing::debug!(action, elapsed_ms = started.elapsed().as_millis(), "SOAP (no auth)");
                Ok(response.into_string()?)
            }
            Err(ureq::Error::Status(401, response)) => {
                let auth_header = response.header("WWW-Authenticate").unwrap_or_default();
                let auth_params = parse_auth_header(auth_header);
                self.remember_challenge(&auth_params);
                let auth_val = build_digest_header(
                    &self.config.username,
                    &self.config.password,
                    "POST",
                    control_url,
                    &auth_params,
                );

                let authenticated_req = agent.post(&url)
                    .set("Content-Type", "text/xml; charset=\"utf-8\"")
                    .set("SOAPAction", &soap_action)
                    .set("Authorization", &auth_val)
                    .send_string(&body);

                match authenticated_req {
                    Ok(auth_ok) => {
                        tracing::debug!(
                            action,
                            elapsed_ms = started.elapsed().as_millis(),
                            "SOAP (401 + digest retry)"
                        );
                        Ok(auth_ok.into_string()?)
                    }
                    Err(ureq::Error::Status(401, err_resp)) => {
                        let text = err_resp.into_string().unwrap_or_default();
                        Err(FritzError::AuthFailed {
                            user: self.config.username.clone(),
                            detail: text,
                        })
                    }
                    Err(other) => Err(other.into()),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Fetches all contacts across all phonebooks in the FRITZ!Box.
    pub fn fetch_contacts(&self) -> Result<Vec<Contact>, FritzError> {
        let overall = std::time::Instant::now();
        let targets = self.phonebook_targets()?;
        let phonebook_count = targets.len();

        // The downloads dominate: the router generates each phonebook on
        // demand, and the largest took 8.5s while four others took under
        // 100ms. Sequentially that is the sum; in parallel it is the slowest.
        // Built once and shared: over TLS this carries the certificate pin, and
        // the downloads are served by the same router as the SOAP calls. Using
        // the default agent here made every download fail certificate
        // validation.
        let (agent, _) = self.agent();

        let all_contacts: Vec<Contact> = std::thread::scope(|scope| {
            let handles: Vec<_> = targets
                .iter()
                .map(|(pbid, pb_name, pb_url)| {
                    let agent = &agent;
                    scope.spawn(move || download_phonebook(agent, *pbid, pb_name, pb_url))
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .flatten()
                .collect()
        });

        tracing::info!(
            phonebooks = phonebook_count,
            contacts = all_contacts.len(),
            elapsed_ms = overall.elapsed().as_millis(),
            "FRITZ!Box phonebook sync finished"
        );
        Ok(all_contacts)
    }

    /// Resolves every phonebook's id, name and download URL.
    fn phonebook_targets(&self) -> Result<Vec<(u32, String, String)>, FritzError> {
        let xml = self.soap_call(
            "/upnp/control/x_contact",
            "urn:dslforum-org:service:X_AVM-DE_OnTel:1",
            "GetPhonebookList",
            &[],
        )?;

        let list = extract_xml_tag(&xml, "NewPhonebookList").unwrap_or_default();
        let mut targets = Vec::new();

        for id_str in list.split(',') {
            let id_str = id_str.trim();
            let Ok(pbid) = id_str.parse::<u32>() else { continue };

            let pb_res = self.soap_call(
                "/upnp/control/x_contact",
                "urn:dslforum-org:service:X_AVM-DE_OnTel:1",
                "GetPhonebook",
                &[("NewPhonebookID", id_str)],
            )?;

            let name = extract_xml_tag(&pb_res, "NewPhonebookName")
                .unwrap_or_else(|| format!("Phonebook {pbid}"));
            let url = extract_xml_tag(&pb_res, "NewPhonebookURL").unwrap_or_default();
            if !url.is_empty() {
                targets.push((pbid, name, url));
            }
        }
        Ok(targets)
    }

    /// Fetches the recent call list from the FRITZ!Box.
    pub fn fetch_calls(&self) -> Result<Vec<CallRecord>, FritzError> {
        let xml = self.soap_call(
            "/upnp/control/x_contact",
            "urn:dslforum-org:service:X_AVM-DE_OnTel:1",
            "GetCallList",
            &[],
        )?;

        let Some(call_list_url) = extract_xml_tag(&xml, "NewCallListURL") else {
            return Err(FritzError::SoapAction {
                action: "GetCallList".into(),
                detail: "Missing NewCallListURL in response".into(),
            });
        };

        let cl_resp = self.agent().0.get(&call_list_url).call()?.into_string()?;
        Ok(parse_call_list_xml(&cl_resp))
    }

    /// Asks the router which numbers each registered SIP client answers to.
    ///
    /// A SIP account knows its username and registrar and nothing else — not
    /// the extension people dial to reach it, nor the number it presents when
    /// it calls out. The router knows both, and this is the only way to ask.
    ///
    /// Returns one entry per telephony device the router has configured,
    /// including ones that are not us; match on
    /// [`username`](AccountNumbers::username).
    ///
    /// # Errors
    ///
    /// Fails if the client count cannot be read. An individual client that
    /// cannot be read is logged and skipped, so one bad entry does not cost
    /// the rest.
    pub fn fetch_account_numbers(&self) -> Result<Vec<AccountNumbers>, FritzError> {
        let xml = self.soap_call(
            VOIP_CONTROL_URL,
            VOIP_SERVICE,
            "X_AVM-DE_GetNumberOfClients",
            &[],
        )?;
        let count: u32 = extract_xml_tag(&xml, "NewX_AVM-DE_NumberOfClients")
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(0);

        let mut found = Vec::new();
        for index in 0..count {
            let index = index.to_string();
            // `X_AVM-DE_GetClient` (no suffix) exists on paper but answers 500
            // on current firmware; `GetClient3` is the one that works.
            let client = match self.soap_call(
                VOIP_CONTROL_URL,
                VOIP_SERVICE,
                "X_AVM-DE_GetClient3",
                &[("NewX_AVM-DE_ClientIndex", &index)],
            ) {
                Ok(client) => client,
                Err(err) => {
                    warn!(%index, %err, "could not read a telephony client from the router");
                    continue;
                }
            };

            let username = extract_xml_tag(&client, "NewX_AVM-DE_ClientUsername").unwrap_or_default();
            if username.is_empty() {
                // A configured but unused slot. Nothing can match it.
                continue;
            }
            found.push(AccountNumbers {
                username,
                phone_name: extract_xml_tag(&client, "NewX_AVM-DE_PhoneName").unwrap_or_default(),
                internal: extract_xml_tag(&client, "NewX_AVM-DE_InternalNumber").unwrap_or_default(),
                external: extract_xml_tag(&client, "NewX_AVM-DE_OutGoingNumber").unwrap_or_default(),
            });
        }
        Ok(found)
    }

    /// The country code the router dials out with, as `0049`.
    ///
    /// # Errors
    ///
    /// Fails if the SOAP call fails or the response omits the code.
    pub fn fetch_country_code(&self) -> Result<String, FritzError> {
        let xml = self.soap_call(VOIP_CONTROL_URL, VOIP_SERVICE, "GetVoIPCommonCountryCode", &[])?;
        extract_xml_tag(&xml, "NewVoIPCountryCode").ok_or_else(|| FritzError::SoapAction {
            action: "GetVoIPCommonCountryCode".into(),
            detail: "response carried no country code".into(),
        })
    }
}

/// The TR-064 `VoIP` service, which knows about telephony devices and numbers.
const VOIP_SERVICE: &str = "urn:dslforum-org:service:X_VoIP:1";
const VOIP_CONTROL_URL: &str = "/upnp/control/x_voip";

/// The numbers the router associates with one registered SIP client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountNumbers {
    /// SIP username, as configured on the router. The key to match an account.
    pub username: String,
    /// The router's own label for the device, such as `Blu-PC`.
    pub phone_name: String,
    /// The extension other phones on this router dial to reach it, `620`.
    /// Empty when the router has not assigned one.
    pub internal: String,
    /// The number presented to the outside world on outgoing calls.
    /// Empty when the client has no outgoing number of its own.
    pub external: String,
}

/// Downloads and parses one phonebook. Failures are reported and skipped so a
/// single unavailable phonebook does not lose the others.
fn download_phonebook(agent: &ureq::Agent, pbid: u32, pb_name: &str, pb_url: &str) -> Vec<Contact> {
    let started = std::time::Instant::now();

    let body = agent
        .get(pb_url)
        .call()
        .map_err(|e| e.to_string())
        .and_then(|resp| resp.into_string().map_err(|e| e.to_string()));

    match body {
        Ok(xml) => {
            let bytes = xml.len();
            let parsed = parse_phonebook_xml(&xml, pbid, pb_name);
            tracing::debug!(
                phonebook = %pb_name,
                contacts = parsed.len(),
                bytes,
                elapsed_ms = started.elapsed().as_millis(),
                "downloaded phonebook"
            );
            parsed
        }
        Err(e) => {
            warn!(phonebook = %pb_name, error = %e, "could not download phonebook");
            Vec::new()
        }
    }
}

// ── XML Parsers ─────────────────────────────────────────────────────────────

