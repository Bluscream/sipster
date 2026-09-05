//! Reading the XML a FRITZ!Box returns, and writing the XML it expects.
//!
//! Hand-rolled rather than a parser dependency: the shapes are small and
//! fixed, and every one of them is exercised by the tests below.

use crate::model::{CallRecord, CallType, Contact, NumberType, PhoneNumber, RecordSource};

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

pub(super) fn parse_duration_seconds(duration_str: &str) -> u32 {
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

pub(super) fn extract_xml_tag(haystack: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = haystack.find(&open)?;
    let end = haystack[start + open.len()..].find(&close)?;
    let raw = &haystack[start + open.len()..start + open.len() + end];
    Some(unescape_xml(raw.trim()))
}

/// Escapes the five XML predefined entities.
pub(super) fn escape_xml(raw: &str) -> String {
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
pub(super) fn unescape_xml(raw: &str) -> String {
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

pub(super) fn decode_numeric_entity(entity: &str) -> Option<char> {
    let digits = entity.strip_prefix('#')?;
    let code = digits.strip_prefix('x').map_or_else(
        || digits.parse::<u32>().ok(),
        |hex| u32::from_str_radix(hex, 16).ok(),
    )?;
    char::from_u32(code)
}

// ── Digest Authentication Helpers ───────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{
        escape_xml, parse_call_list_xml, parse_duration_seconds, parse_phonebook_xml,
        unescape_xml,
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
