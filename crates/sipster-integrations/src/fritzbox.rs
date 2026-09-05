//! FRITZ!Box TR-064 integration: phonebook and call list synchronization.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use md5::{Digest, Md5};
use tracing::warn;

use crate::model::{CallRecord, CallType, Contact, NumberType, PhoneNumber, RecordSource};

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
            port: 49000,
            username: String::new(),
            password: String::new(),
            tls: false,
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
    pub fn new(config: FritzConfig) -> Self {
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
        // Over TLS the port is the router's TLS port, not the plain one — AVM
        // serves TR-064 on 49000 unencrypted and 49443 encrypted.
        let url = if self.config.tls {
            format!("https://{}:{TLS_PORT}{control_url}", self.config.host)
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
        // validation while the SOAP calls succeeded — six empty phonebooks and
        // a sync that reported success.
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

/// Parses FRITZ!Box phonebook XML `<phonebooks>` structure.
pub fn parse_phonebook_xml(xml: &str, pbid: u32, pb_name: &str) -> Vec<Contact> {
    let mut contacts = Vec::new();
    let mut rest = xml;

    while let Some(start) = rest.find("<contact>") {
        let Some(end) = rest[start..].find("</contact>") else { break };
        let chunk = &rest[start + 9..start + end];
        rest = &rest[start + end + 10..];

        let real_name = extract_xml_tag(chunk, "realName").unwrap_or_default().trim().to_string();
        let unique_id = extract_xml_tag(chunk, "uniqueid").unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if real_name.is_empty() {
            continue;
        }

        // Parse numbers
        let mut numbers = Vec::new();
        let mut num_rest = chunk;
        while let Some(num_start) = num_rest.find("<number") {
            let Some(tag_close) = num_rest[num_start..].find('>') else { break };
            let attr_part = &num_rest[num_start..num_start + tag_close];
            let after_tag = &num_rest[num_start + tag_close + 1..];
            let Some(val_end) = after_tag.find("</number>") else { break };
            let number_val = after_tag[..val_end].trim().to_string();
            num_rest = &after_tag[val_end + 9..];

            if number_val.is_empty() {
                continue;
            }

            let num_type = if attr_part.contains(r#"type="mobile""#) {
                NumberType::Mobile
            } else if attr_part.contains(r#"type="work""#) {
                NumberType::Work
            } else if attr_part.contains(r#"type="fax""#) {
                NumberType::Fax
            } else if attr_part.contains(r#"type="intern""#) {
                NumberType::Intern
            } else if attr_part.contains(r#"type="vanity""#) {
                NumberType::Vanity
            } else {
                NumberType::Home
            };

            let prio = if attr_part.contains(r#"prio="1""#) { 1 } else { 2 };

            numbers.push(PhoneNumber {
                number: number_val,
                number_type: num_type,
                priority: prio,
            });
        }

        contacts.push(Contact {
            id: format!("fritzbox-{pbid}-{unique_id}"),
            name: real_name,
            numbers,
            emails: Vec::new(),
            source: RecordSource::FritzBox {
                phonebook_id: pbid,
                phonebook_name: pb_name.to_string(),
            },
        });
    }

    contacts
}

/// Parses FRITZ!Box calllist XML `<root><Call>` structure.
pub fn parse_call_list_xml(xml: &str) -> Vec<CallRecord> {
    let mut records = Vec::new();
    let mut rest = xml;

    while let Some(start) = rest.find("<Call>") {
        let Some(end) = rest[start..].find("</Call>") else { break };
        let chunk = &rest[start + 6..start + end];
        rest = &rest[start + end + 7..];

        let id = extract_xml_tag(chunk, "Id").unwrap_or_default();
        let type_code = extract_xml_tag(chunk, "Type").unwrap_or_default();
        let caller_num = extract_xml_tag(chunk, "Caller").unwrap_or_default();
        let called_party = extract_xml_tag(chunk, "Called").unwrap_or_default();
        let name = extract_xml_tag(chunk, "Name").filter(|s| !s.trim().is_empty());
        let date = extract_xml_tag(chunk, "Date").unwrap_or_default();
        let duration_str = extract_xml_tag(chunk, "Duration").unwrap_or_default();
        let device = extract_xml_tag(chunk, "Device").filter(|s| !s.trim().is_empty());

        let (call_type, remote_num, local_num) = match type_code.as_str() {
            "2" => (CallType::Missed, caller_num, called_party),
            "3" => (CallType::Outgoing, called_party, caller_num),
            "10" => (CallType::Rejected, caller_num, called_party),
            _ => (CallType::Incoming, caller_num, called_party),
        };

        // Parse duration mm:ss or hh:mm
        let duration_seconds = parse_duration_seconds(&duration_str);

        records.push(CallRecord {
            id: format!("fritzbox-call-{id}"),
            call_type,
            remote_number: remote_num,
            remote_name: name,
            local_party: device.or(Some(local_num)),
            timestamp: date,
            duration_seconds,
            source: RecordSource::FritzBox {
                phonebook_id: 0,
                phonebook_name: "Router Call Log".into(),
            },
        });
    }

    records
}

fn parse_duration_seconds(duration_str: &str) -> u32 {
    let parts: Vec<&str> = duration_str.split(':').collect();
    match parts.len() {
        2 => {
            let m: u32 = parts[0].parse().unwrap_or(0);
            let s: u32 = parts[1].parse().unwrap_or(0);
            m * 60 + s
        }
        3 => {
            let h: u32 = parts[0].parse().unwrap_or(0);
            let m: u32 = parts[1].parse().unwrap_or(0);
            let s: u32 = parts[2].parse().unwrap_or(0);
            h * 3600 + m * 60 + s
        }
        _ => 0,
    }
}

fn extract_xml_tag(haystack: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = haystack.find(&open)?;
    let end = haystack[start + open.len()..].find(&close)?;
    let raw = &haystack[start + open.len()..start + open.len() + end];
    Some(unescape_xml(raw.trim()))
}

/// Escapes the five XML predefined entities.
fn escape_xml(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Decodes the five predefined entities plus numeric references.
///
/// Without this a contact stored as `M&amp;uuml;ller &amp; Sohn` was shown
/// verbatim, entities and all, in the contact list and the caller display.
fn unescape_xml(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        let Some(semi) = after.find(';').filter(|end| *end <= 10) else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let entity = &after[1..semi];
        match entity {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            numeric if numeric.starts_with('#') => {
                if let Some(c) = decode_numeric_entity(numeric) {
                    out.push(c);
                } else {
                    out.push_str(&after[..=semi]);
                }
            }
            _ => out.push_str(&after[..=semi]),
        }
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    out
}

fn decode_numeric_entity(entity: &str) -> Option<char> {
    let digits = entity.strip_prefix('#')?;
    let code = digits.strip_prefix('x').map_or_else(
        || digits.parse::<u32>().ok(),
        |hex| u32::from_str_radix(hex, 16).ok(),
    )?;
    char::from_u32(code)
}

// ── Digest Authentication Helpers ───────────────────────────────────────────

/// A random 16-hex-digit client nonce.
fn fresh_cnonce() -> String {
    // uuid v4 is already a dependency and is backed by a CSPRNG; its simple
    // form gives us 32 hex digits, of which 16 are plenty.
    uuid::Uuid::new_v4().simple().to_string()[..16].to_string()
}

fn md5_hex(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn parse_auth_header(header: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let header = header.trim_start_matches("Digest ");
    for part in header.split(',') {
        if let Some((k, v)) = part.trim().split_once('=') {
            let key = k.trim().to_string();
            let val = v.trim().trim_matches('"').to_string();
            map.insert(key, val);
        }
    }
    map
}

fn build_digest_header(
    user: &str,
    pass: &str,
    method: &str,
    uri: &str,
    auth_params: &HashMap<String, String>,
) -> String {
    let realm = auth_params.get("realm").cloned().unwrap_or_default();
    let nonce = auth_params.get("nonce").cloned().unwrap_or_default();
    let qop = auth_params.get("qop").cloned();

    let ha1 = md5_hex(format!("{user}:{realm}:{pass}").as_bytes());
    let ha2 = md5_hex(format!("{method}:{uri}").as_bytes());

    if let Some(qop_val) = qop.as_deref() {
        if qop_val.contains("auth") {
            let nc = "00000001";
            // A fresh client nonce per request. The previous constant
            // ("0a4f113b", straight out of the RFC 2617 example) meant every
            // request produced an identical digest for a given server nonce,
            // which is exactly what cnonce exists to prevent.
            let cnonce = fresh_cnonce();
            let resp = md5_hex(format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}").as_bytes());
            return format!(
                "Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{resp}\", qop=auth, nc={nc}, cnonce=\"{cnonce}\""
            );
        }
    }

    let response = md5_hex(format!("{ha1}:{nonce}:{ha2}").as_bytes());
    format!(
        "Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\""
    )
}

#[cfg(test)]
mod tests {
    use super::{
        escape_xml, fresh_cnonce, parse_call_list_xml, parse_duration_seconds,
        parse_phonebook_xml, unescape_xml,
    };
    use crate::model::{CallType, NumberType};

    #[test]
    fn parses_a_phonebook_entry_with_typed_numbers() {
        let xml = r#"<phonebook><contact><person><realName>Alice Smith</realName></person>
            <telephony><number type="mobile" prio="1">+4915112345</number>
            <number type="work">03012345</number></telephony>
            <uniqueid>42</uniqueid></contact></phonebook>"#;
        let contacts = parse_phonebook_xml(xml, 0, "Main");
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].name, "Alice Smith");
        assert_eq!(contacts[0].numbers.len(), 2);
        assert_eq!(contacts[0].numbers[0].number_type, NumberType::Mobile);
        assert_eq!(contacts[0].numbers[0].priority, 1);
        assert_eq!(contacts[0].primary_number(), Some("+4915112345"));
    }

    /// A contact whose name contains an ampersand came back with the raw
    /// entity in it and was displayed that way.
    #[test]
    fn entities_in_names_are_decoded() {
        let xml = r"<contact><realName>M&#252;ller &amp; Sohn</realName>
            <number type='work'>123</number><uniqueid>1</uniqueid></contact>";
        let contacts = parse_phonebook_xml(xml, 0, "Main");
        assert_eq!(contacts[0].name, "Müller & Sohn");
    }

    #[test]
    fn a_contact_without_a_name_is_skipped() {
        let xml = r"<contact><realName>  </realName><number>123</number></contact>";
        assert!(parse_phonebook_xml(xml, 0, "Main").is_empty());
    }

    #[test]
    fn parses_the_call_list_types() {
        let xml = r"<root>
            <Call><Id>1</Id><Type>1</Type><Caller>0301</Caller><Called>620</Called>
                  <Date>01.01.26 10:00</Date><Duration>0:42</Duration></Call>
            <Call><Id>2</Id><Type>2</Type><Caller>0302</Caller><Called>620</Called>
                  <Date>01.01.26 11:00</Date><Duration>0:00</Duration></Call>
            <Call><Id>3</Id><Type>3</Type><Caller>620</Caller><Called>0303</Called>
                  <Date>01.01.26 12:00</Date><Duration>1:02:03</Duration></Call>
            </root>";
        let calls = parse_call_list_xml(xml);
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].call_type, CallType::Incoming);
        assert_eq!(calls[0].duration_seconds, 42);
        assert_eq!(calls[1].call_type, CallType::Missed);
        // Outgoing swaps the parties: the remote is who we called.
        assert_eq!(calls[2].call_type, CallType::Outgoing);
        assert_eq!(calls[2].remote_number, "0303");
        assert_eq!(calls[2].duration_seconds, 3723);
    }

    #[test]
    fn duration_parsing_handles_both_shapes() {
        assert_eq!(parse_duration_seconds("0:42"), 42);
        assert_eq!(parse_duration_seconds("1:02:03"), 3723);
        assert_eq!(parse_duration_seconds(""), 0);
        assert_eq!(parse_duration_seconds("nonsense"), 0);
    }

    /// Values are interpolated into the SOAP envelope, so they must be escaped.
    #[test]
    fn xml_escaping_round_trips() {
        assert_eq!(escape_xml("a&b<c>\"d\""), "a&amp;b&lt;c&gt;&quot;d&quot;");
        assert_eq!(unescape_xml("a&amp;b&lt;c&gt;"), "a&b<c>");
        // An unknown entity is left alone rather than mangled.
        assert_eq!(unescape_xml("100 &unknown; 200"), "100 &unknown; 200");
        // A bare ampersand is not an entity.
        assert_eq!(unescape_xml("Tom & Jerry"), "Tom & Jerry");
    }

    /// A constant client nonce defeats digest replay protection.
    #[test]
    fn client_nonces_differ_between_requests() {
        let (a, b) = (fresh_cnonce(), fresh_cnonce());
        assert_ne!(a, b);
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// Truncation on a multi-byte boundary would panic; names routinely have
    /// non-ASCII in them.
    #[test]
    fn malformed_xml_does_not_panic() {
        let _ = parse_phonebook_xml("<contact><realName>Ünfinished", 0, "Main");
        let _ = parse_phonebook_xml("", 0, "Main");
        let _ = parse_call_list_xml("<Call><Id>1</Id>");
        let _ = unescape_xml("&#xZZZZ; &# ; &");
    }
}
