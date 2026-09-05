//! vCard parsing, shared by every provider that speaks it.
//!
//! `CardDAV` responses, `vdir` directories and KDE's directory address books
//! are all the same format, so they share one parser rather than each growing
//! its own. The previous copy lived inside `carddav` and was missing three
//! things that real files rely on:
//!
//! - **Line unfolding.** RFC 6350 lets a long property wrap onto the next line
//!   with a leading space or tab. Unfolded naively, a wrapped `TEL` produced a
//!   truncated number and a stray unparseable line.
//! - **`UID`.** Contacts were keyed by their display name, so two people
//!   called "J. Smith" collapsed into one entry.
//! - **vCard 4.0 `tel:` URIs.** `TEL;VALUE=uri:tel:+49301234` yielded the
//!   number `tel:+49301234`, which is not dialable.

use crate::model::{Contact, NumberType, PhoneNumber, RecordSource};

/// Splits a stream that may hold many vCards into individual card texts.
pub fn split_cards(text: &str) -> Vec<&str> {
    let mut cards = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("BEGIN:VCARD") {
        let Some(end) = rest[start..].find("END:VCARD") else { break };
        cards.push(&rest[start..start + end + "END:VCARD".len()]);
        rest = &rest[start + end + "END:VCARD".len()..];
    }
    cards
}

/// Joins folded continuation lines back onto the property they belong to.
///
/// RFC 6350 §3.2: a line break followed by a single space or tab is a fold,
/// not a new property.
fn unfold(card: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in card.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        match line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            Some(continuation) if !lines.is_empty() => {
                if let Some(last) = lines.last_mut() {
                    last.push_str(continuation);
                }
            }
            _ => lines.push(line.to_string()),
        }
    }
    lines
}

/// Splits `NAME;PARAMS:value` into its parts.
fn split_property(line: &str) -> Option<(&str, &str, &str)> {
    // The first colon that is not inside a quoted parameter ends the name.
    let mut in_quotes = false;
    let colon = line.char_indices().find_map(|(i, c)| match c {
        '"' => {
            in_quotes = !in_quotes;
            None
        }
        ':' if !in_quotes => Some(i),
        _ => None,
    })?;

    let (head, value) = (&line[..colon], line[colon + 1..].trim());
    let (name, params) = head.split_once(';').unwrap_or((head, ""));
    Some((name.trim(), params, value))
}

/// Strips the `tel:` / `mailto:` scheme vCard 4.0 wraps values in.
fn strip_uri_scheme(value: &str) -> &str {
    value
        .strip_prefix("tel:")
        .or_else(|| value.strip_prefix("mailto:"))
        .unwrap_or(value)
}

fn number_type(params: &str) -> NumberType {
    let upper = params.to_ascii_uppercase();
    if upper.contains("CELL") || upper.contains("MOBILE") {
        NumberType::Mobile
    } else if upper.contains("FAX") {
        NumberType::Fax
    } else if upper.contains("WORK") {
        NumberType::Work
    } else if upper.contains("HOME") {
        NumberType::Home
    } else {
        NumberType::Other("other".into())
    }
}

/// Parses one vCard into a [`Contact`].
///
/// `id_prefix` namespaces the id so two providers cannot collide, and
/// `source` records where it came from.
///
/// Returns `None` when the card has no usable name, which is the one field
/// the contact list cannot render without.
pub fn parse(card: &str, id_prefix: &str, source: RecordSource) -> Option<Contact> {
    let mut formatted_name = None;
    let mut structured_name = None;
    let mut uid = None;
    let mut numbers = Vec::new();
    let mut emails = Vec::new();

    for line in unfold(card) {
        let Some((name, params, value)) = split_property(&line) else {
            continue;
        };
        if value.is_empty() {
            continue;
        }

        match name.to_ascii_uppercase().as_str() {
            "FN" => formatted_name = Some(value.to_string()),
            // `N` is Family;Given;Middle;Prefix;Suffix — a fallback when the
            // card has no FN, which older exporters omit.
            "N" if structured_name.is_none() => {
                let parts: Vec<&str> = value.split(';').collect();
                let given = parts.get(1).copied().unwrap_or_default().trim();
                let family = parts.first().copied().unwrap_or_default().trim();
                let joined = format!("{given} {family}").trim().to_string();
                if !joined.is_empty() {
                    structured_name = Some(joined);
                }
            }
            "UID" => uid = Some(strip_uri_scheme(value).to_string()),
            "TEL" => {
                let number = strip_uri_scheme(value).trim().to_string();
                if !number.is_empty() {
                    let upper = params.to_ascii_uppercase();
                    numbers.push(PhoneNumber {
                        number,
                        number_type: number_type(params),
                        priority: u8::from(!upper.contains("PREF")) + 1,
                    });
                }
            }
            "EMAIL" => emails.push(strip_uri_scheme(value).trim().to_string()),
            _ => {}
        }
    }

    let name = formatted_name.or(structured_name)?;
    // Prefer UID; fall back to the name only when the card has none.
    let id = uid.unwrap_or_else(|| name.replace(' ', "_"));

    Some(Contact {
        id: format!("{id_prefix}-{id}"),
        name,
        numbers,
        emails,
        merged_from: Vec::new(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::{parse, split_cards, unfold};
    use crate::model::{NumberType, RecordSource};

    fn card(body: &str) -> String {
        format!("BEGIN:VCARD\r\nVERSION:3.0\r\n{body}\r\nEND:VCARD")
    }

    #[test]
    fn parses_a_basic_card() {
        let text = card("FN:Alice Smith\r\nTEL;TYPE=CELL:+4915112345\r\nEMAIL:a@example.com");
        let contact = parse(&text, "vdir", RecordSource::Local).expect("parsed");
        assert_eq!(contact.name, "Alice Smith");
        assert_eq!(contact.numbers[0].number, "+4915112345");
        assert_eq!(contact.numbers[0].number_type, NumberType::Mobile);
        assert_eq!(contact.emails, vec!["a@example.com"]);
    }

    /// RFC 6350 folding. A wrapped number used to be truncated at the fold.
    #[test]
    fn unfolds_wrapped_lines() {
        let text = "BEGIN:VCARD\r\nFN:Alice\r\nTEL:+4930123\r\n 4567890\r\nEND:VCARD";
        let contact = parse(text, "vdir", RecordSource::Local).expect("parsed");
        assert_eq!(contact.numbers[0].number, "+49301234567890");
    }

    #[test]
    fn tab_folds_count_too() {
        let lines = unfold("FN:Alice\r\nNOTE:one\r\n\ttwo");
        assert_eq!(lines, vec!["FN:Alice", "NOTE:onetwo"]);
    }

    /// vCard 4.0 wraps values in a URI scheme; `tel:+49…` is not dialable.
    #[test]
    fn strips_the_tel_uri_scheme() {
        let text = card("FN:Bob\r\nTEL;VALUE=uri:tel:+49301234\r\nEMAIL;VALUE=uri:mailto:b@x.com");
        let contact = parse(&text, "vdir", RecordSource::Local).expect("parsed");
        assert_eq!(contact.numbers[0].number, "+49301234");
        assert_eq!(contact.emails, vec!["b@x.com"]);
    }

    /// Keying on the display name merged distinct people who share one.
    #[test]
    fn uid_keeps_same_named_people_apart() {
        let one = parse(&card("UID:abc-1\r\nFN:J. Smith\r\nTEL:1"), "vdir", RecordSource::Local);
        let two = parse(&card("UID:abc-2\r\nFN:J. Smith\r\nTEL:2"), "vdir", RecordSource::Local);
        assert_ne!(one.unwrap().id, two.unwrap().id);
    }

    #[test]
    fn falls_back_to_the_structured_name() {
        let text = card("N:Smith;Alice;;;\r\nTEL:123");
        assert_eq!(parse(&text, "vdir", RecordSource::Local).unwrap().name, "Alice Smith");
    }

    #[test]
    fn a_card_without_a_name_is_skipped() {
        assert!(parse(&card("TEL:123"), "vdir", RecordSource::Local).is_none());
    }

    /// A quoted parameter may contain a colon; splitting on the first one
    /// would cut the property name in half.
    #[test]
    fn quoted_parameters_do_not_confuse_the_split() {
        let tricky = card("FN:Alice\r\nTEL;TYPE=\"work:main\":+49301234");
        let contact = parse(&tricky, "vdir", RecordSource::Local).expect("parsed");
        assert_eq!(contact.numbers[0].number, "+49301234");
    }

    #[test]
    fn pref_sorts_first() {
        let text = card("FN:Alice\r\nTEL;TYPE=WORK:111\r\nTEL;TYPE=HOME,PREF:222");
        let contact = parse(&text, "vdir", RecordSource::Local).expect("parsed");
        assert_eq!(contact.primary_number(), Some("222"));
    }

    #[test]
    fn splits_a_stream_of_several_cards() {
        let text = format!("{}\r\n{}", card("FN:A\r\nTEL:1"), card("FN:B\r\nTEL:2"));
        assert_eq!(split_cards(&text).len(), 2);
    }

    #[test]
    fn malformed_input_does_not_panic() {
        assert!(split_cards("BEGIN:VCARD no end").is_empty());
        assert!(parse("", "vdir", RecordSource::Local).is_none());
        assert!(parse("BEGIN:VCARD\r\nFN:\r\nEND:VCARD", "vdir", RecordSource::Local).is_none());
    }
}
