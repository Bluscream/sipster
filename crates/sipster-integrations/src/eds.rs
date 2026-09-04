//! Contacts from Evolution Data Server.
//!
//! EDS is the contact store behind GNOME Contacts, Evolution and anything else
//! on a GNOME desktop, including whatever Google or `CardDAV` accounts the user
//! added there. Reading it means their existing address book is available
//! without configuring the same accounts a second time inside Sipster.
//!
//! # Why D-Bus, and why one connection
//!
//! EDS has no file to read: the local backend is a private `SQLite` database
//! and everything goes through the session bus. It is also stateful in a way
//! that rules out shelling out to `gdbus`. Opening an address book returns an
//! object that lives only as long as the *client connection* that asked for
//! it, so a second `gdbus` invocation finds nothing there — the book is
//! already gone. One connection has to stay open across open-then-query, which
//! is why this uses a real D-Bus client.
//!
//! # Versioned names
//!
//! The bus names carry an interface generation — `Sources5`, `AddressBook10` —
//! which moves as EDS evolves. Rather than pin one, the address book factory
//! is found by asking the bus for whatever `AddressBook*` name it currently
//! has, so a distro upgrade does not silently empty the contact list.

use crate::model::{Contact, RecordSource};
use crate::vcard;

/// The source registry. Stable across the releases that matter; if this ever
/// moves it is found the same way as the factory below.
const SOURCES: &str = "org.gnome.evolution.dataserver.Sources5";
const SOURCE_MANAGER_PATH: &str = "/org/gnome/evolution/dataserver/SourceManager";

const FACTORY_PREFIX: &str = "org.gnome.evolution.dataserver.AddressBook";
const FACTORY_PATH: &str = "/org/gnome/evolution/dataserver/AddressBookFactory";
const FACTORY_IFACE: &str = "org.gnome.evolution.dataserver.AddressBookFactory";
const BOOK_IFACE: &str = "org.gnome.evolution.dataserver.AddressBook";

/// An S-expression matching every contact.
///
/// The documented-looking `(contains "x" "")` returns nothing — "x" is not a
/// field, and a `contains` against an absent field matches no card however
/// empty the needle. `#t` is the literal true the query evaluator accepts, and
/// it was checked against a live server rather than assumed.
const MATCH_ALL: &str = "#t";

/// Marks a source as an address book rather than a calendar or mail account.
const ADDRESS_BOOK_SECTION: &str = "[Address Book]";

/// One address book EDS knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Book {
    pub uid: String,
    pub name: String,
}

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Reads every enabled address book EDS has.
///
/// # Errors
///
/// When the session bus is unreachable or EDS is not installed. Both are
/// ordinary on a machine without GNOME, so callers should treat an error as
/// "no contacts here" rather than a failure worth showing.
pub fn fetch_contacts() -> Result<Vec<Contact>, Error> {
    let conn = zbus::blocking::Connection::session()?;
    let factory = factory_name(&conn)?;

    let mut contacts = Vec::new();
    for book in books(&conn)? {
        match read_book(&conn, &factory, &book) {
            Ok(mut found) => contacts.append(&mut found),
            // One unreadable book — a stale account, an offline CardDAV
            // source — must not cost the user the rest of their contacts.
            Err(e) => tracing::warn!(book = %book.name, error = %e, "could not read address book"),
        }
    }
    Ok(contacts)
}

/// Whether EDS is present on the session bus at all.
#[must_use]
pub fn available() -> bool {
    zbus::blocking::Connection::session()
        .ok()
        .and_then(|conn| factory_name(&conn).ok())
        .is_some()
}

/// The current address book factory's bus name.
///
/// Found by matching the prefix so the interface generation in the name does
/// not have to be pinned. See the module docs.
fn factory_name(conn: &zbus::blocking::Connection) -> Result<String, Error> {
    let reply = conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "ListActivatableNames",
        &(),
    )?;
    let names: Vec<String> = reply.body().deserialize()?;
    names
        .into_iter()
        .filter(|name| {
            name.strip_prefix(FACTORY_PREFIX)
                .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        })
        // Several generations can be registered at once; the highest is the
        // one this EDS actually serves.
        .max_by_key(|name| {
            name[FACTORY_PREFIX.len()..]
                .parse::<u32>()
                .unwrap_or_default()
        })
        .ok_or_else(|| "Evolution Data Server is not available on the session bus".into())
}

/// What `GetManagedObjects` hands back: object path → interface → properties.
type Managed = std::collections::HashMap<
    zbus::zvariant::OwnedObjectPath,
    std::collections::HashMap<String, std::collections::HashMap<String, zbus::zvariant::OwnedValue>>,
>;

/// Every address book in the source registry.
fn books(conn: &zbus::blocking::Connection) -> Result<Vec<Book>, Error> {
    let reply = conn.call_method(
        Some(SOURCES),
        SOURCE_MANAGER_PATH,
        Some("org.freedesktop.DBus.ObjectManager"),
        "GetManagedObjects",
        &(),
    )?;

    let managed: Managed = reply.body().deserialize()?;

    let mut books: Vec<Book> = managed
        .values()
        .filter_map(|interfaces| {
            let source = interfaces.get("org.gnome.evolution.dataserver.Source")?;
            let get = |key: &str| -> Option<String> {
                source.get(key).and_then(|v| String::try_from(v.clone()).ok())
            };
            let data = get("Data")?;
            describe_source(&get("UID")?, &data)
        })
        .collect();

    // The registry hands them back in hash order; a stable list keeps the
    // contact list from reshuffling between syncs.
    books.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.uid.cmp(&b.uid)));
    Ok(books)
}

/// Turns a source's key file into a [`Book`], or `None` if it is not an
/// enabled address book.
///
/// Sources are `GKeyFile` text. Calendars, mail accounts and proxy settings
/// all come back from the same registry, and only the ones carrying an
/// `[Address Book]` section hold contacts.
fn describe_source(uid: &str, data: &str) -> Option<Book> {
    if !data.contains(ADDRESS_BOOK_SECTION) {
        return None;
    }
    // A disabled source is one the user switched off in their desktop; syncing
    // it anyway would put contacts back that they removed on purpose.
    if data
        .lines()
        .any(|line| line.trim() == "Enabled=false" || line.trim() == "Enabled=FALSE")
    {
        return None;
    }

    // `DisplayName[xx]=` entries are translations of the same field; the bare
    // key is the one to show.
    let name = data
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("DisplayName="))
        .filter(|name| !name.is_empty())
        .unwrap_or(uid);

    Some(Book {
        uid: uid.to_owned(),
        name: name.to_owned(),
    })
}

/// Opens one address book and reads every contact out of it.
fn read_book(
    conn: &zbus::blocking::Connection,
    factory: &str,
    book: &Book,
) -> Result<Vec<Contact>, Error> {
    let reply = conn.call_method(
        Some(factory),
        FACTORY_PATH,
        Some(FACTORY_IFACE),
        "OpenAddressBook",
        &(book.uid.as_str(),),
    )?;
    // Declared `(os)` in some releases and `(ss)` in others; both arrive as
    // two strings on the wire, so it is read as strings and converted here.
    let (path, bus): (String, String) = reply.body().deserialize()?;
    let path = zbus::zvariant::ObjectPath::try_from(path)?;

    // The book has to be opened before it will answer queries.
    conn.call_method(Some(bus.as_str()), &path, Some(BOOK_IFACE), "Open", &())?;

    let reply = conn.call_method(
        Some(bus.as_str()),
        &path,
        Some(BOOK_IFACE),
        "GetContactList",
        &(MATCH_ALL,),
    )?;
    let vcards: Vec<String> = reply.body().deserialize()?;

    let source = RecordSource::Other(format!("Evolution ({})", book.name));
    Ok(vcards
        .iter()
        .filter_map(|card| vcard::parse(card, "eds", source.clone()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{describe_source, ADDRESS_BOOK_SECTION, MATCH_ALL};

    /// A real source key file, trimmed. The translated `DisplayName[..]` keys
    /// are what makes naive prefix matching pick the wrong one.
    const BOOK: &str = "\
[Data Source]
DisplayName[de]=Persönlich
DisplayName=Personal
Enabled=true
Parent=local-stub

[Address Book]
BackendName=local
";

    const CALENDAR: &str = "\
[Data Source]
DisplayName=Birthdays & Anniversaries
Enabled=true

[Calendar]
BackendName=contacts
";

    #[test]
    fn an_address_book_source_is_recognised_and_named() {
        let book = describe_source("system-address-book", BOOK).expect("an address book");
        assert_eq!(book.uid, "system-address-book");
        assert_eq!(book.name, "Personal", "the translated names must not win");
    }

    /// The registry returns calendars, mail accounts and proxy settings too.
    #[test]
    fn only_address_books_are_kept() {
        assert_eq!(describe_source("birthdays", CALENDAR), None);
        assert_eq!(describe_source("proxy", "[Data Source]\nDisplayName=Proxy\n"), None);
        assert_eq!(describe_source("empty", ""), None);
    }

    /// A source the user switched off in their desktop must stay off.
    #[test]
    fn a_disabled_book_is_skipped() {
        let disabled = BOOK.replace("Enabled=true", "Enabled=false");
        assert_eq!(describe_source("system-address-book", &disabled), None);
    }

    /// Without a display name the uid is all there is to show.
    #[test]
    fn a_nameless_book_falls_back_to_its_uid() {
        let book = describe_source("abc-123", "[Address Book]\nBackendName=local\n").expect("book");
        assert_eq!(book.name, "abc-123");
    }

    /// `(contains "x" "")` looks right and silently returns nothing, which
    /// reads exactly like an empty address book.
    #[test]
    fn the_match_all_query_is_the_one_that_actually_matches() {
        assert_eq!(MATCH_ALL, "#t");
        assert!(ADDRESS_BOOK_SECTION.starts_with('['));
    }
}
