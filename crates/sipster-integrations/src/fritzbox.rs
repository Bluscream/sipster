//! FRITZ!Box TR-064 integration: phonebook and call list synchronization.

use std::collections::HashMap;
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

/// Configuration credentials to connect to a FRITZ!Box TR-064 interface.
#[derive(Debug, Clone)]
pub struct FritzConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

impl Default for FritzConfig {
    fn default() -> Self {
        Self {
            host: "192.168.2.1".into(),
            port: 49000,
            username: String::new(),
            password: String::new(),
        }
    }
}

impl FritzConfig {
    /// Discovers credentials from environment variables (`FRITZ_HOST`, `FRITZ_USERNAME`, `FRITZ_PASSWORD`).
    pub fn from_env() -> Option<Self> {
        let host = std::env::var("FRITZ_HOST").unwrap_or_else(|_| "192.168.2.1".into());
        let username = std::env::var("FRITZ_USERNAME").ok()?;
        let password = std::env::var("FRITZ_PASSWORD").ok()?;
        Some(Self {
            host,
            port: 49000,
            username,
            password,
        })
    }
}

/// Client for FRITZ!Box TR-064 SOAP telephony and contact services.
#[derive(Debug, Clone)]
pub struct FritzBoxClient {
    config: FritzConfig,
}

impl FritzBoxClient {
    pub fn new(config: FritzConfig) -> Self {
        Self { config }
    }

    /// Performs an authenticated TR-064 SOAP request.
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
            let _ = write!(args_xml, "<{k}>{v}</{k}>");
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
        let url = format!("http://{}:{}{control_url}", self.config.host, self.config.port);

        let res = ureq::post(&url)
            .set("Content-Type", "text/xml; charset=\"utf-8\"")
            .set("SOAPAction", &soap_action)
            .send_string(&body);

        match res {
            Ok(response) => Ok(response.into_string()?),
            Err(ureq::Error::Status(401, response)) => {
                let auth_header = response.header("WWW-Authenticate").unwrap_or_default();
                let auth_params = parse_auth_header(auth_header);
                let auth_val = build_digest_header(
                    &self.config.username,
                    &self.config.password,
                    "POST",
                    control_url,
                    &auth_params,
                );

                let authenticated_req = ureq::post(&url)
                    .set("Content-Type", "text/xml; charset=\"utf-8\"")
                    .set("SOAPAction", &soap_action)
                    .set("Authorization", &auth_val)
                    .send_string(&body);

                match authenticated_req {
                    Ok(auth_ok) => Ok(auth_ok.into_string()?),
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
        let xml = self.soap_call(
            "/upnp/control/x_contact",
            "urn:dslforum-org:service:X_AVM-DE_OnTel:1",
            "GetPhonebookList",
            &[],
        )?;

        let pblist_str = extract_xml_tag(&xml, "NewPhonebookList").unwrap_or_default();
        let mut all_contacts = Vec::new();

        for id_str in pblist_str.split(',') {
            let id_str = id_str.trim();
            if id_str.is_empty() {
                continue;
            }
            let Ok(pbid) = id_str.parse::<u32>() else { continue };

            // Query phonebook details
            let pb_res = self.soap_call(
                "/upnp/control/x_contact",
                "urn:dslforum-org:service:X_AVM-DE_OnTel:1",
                "GetPhonebook",
                &[("NewPhonebookID", id_str)],
            )?;

            let pb_name = extract_xml_tag(&pb_res, "NewPhonebookName")
                .unwrap_or_else(|| format!("Phonebook {pbid}"));
            let pb_url = extract_xml_tag(&pb_res, "NewPhonebookURL").unwrap_or_default();

            if !pb_url.is_empty() {
                match ureq::get(&pb_url).call() {
                    Ok(resp) => {
                        let xml_content = resp.into_string()?;
                        let parsed = parse_phonebook_xml(&xml_content, pbid, &pb_name);
                        all_contacts.extend(parsed);
                    }
                    Err(e) => {
                        warn!(phonebook = %pb_name, error = %e, "could not download phonebook URL");
                    }
                }
            }
        }

        Ok(all_contacts)
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

        let cl_resp = ureq::get(&call_list_url).call()?.into_string()?;
        Ok(parse_call_list_xml(&cl_resp))
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
    Some(haystack[start + open.len()..start + open.len() + end].trim().to_string())
}

// ── Digest Authentication Helpers ───────────────────────────────────────────

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
            let cnonce = "0a4f113b";
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
