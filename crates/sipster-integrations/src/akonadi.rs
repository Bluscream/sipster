//! Address books configured in KDE's Akonadi.
//!
//! Akonadi keeps contacts behind its own protocol, but it does not *store*
//! them all itself: a resource says where its data lives, and the two contact
//! resources people actually configure — `vcarddir` and `contacts` — both
//! point at a plain directory of vCards. Reading that directory needs no
//! Akonadi client, no D-Bus and no running session manager, and it is the same
//! format [`crate::vdir`] already parses.
//!
//! What this does *not* cover is the SQL-backed store an IMAP or Kolab
//! resource writes into, which is only reachable through the Akonadi protocol
//! itself. A user whose contacts live there gets nothing from this module.

use std::path::{Path, PathBuf};

/// Resource config files that name a directory of vCards.
///
/// Akonadi writes one file per configured resource, numbered from zero:
/// `akonadi_vcarddir_resource_0rc`. Older KDE releases put them straight in
/// `~/.config`; newer ones use the `akonadi/` subdirectory, and both layouts
/// still exist in the wild.
const RESOURCE_PREFIXES: [&str; 2] = ["akonadi_vcarddir_resource_", "akonadi_contacts_resource_"];

/// Directories where resource configs live, relative to `$HOME`.
const CONFIG_DIRS: [&str; 2] = [".config", ".config/akonadi"];

/// Every vCard directory Akonadi is configured to use.
///
/// Returns an empty list when Akonadi is not configured, which is the normal
/// case outside KDE and not an error.
#[must_use]
pub fn contact_directories() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let home = Path::new(&home);

    let mut found: Vec<PathBuf> = CONFIG_DIRS
        .iter()
        .flat_map(|dir| resource_configs(&home.join(dir)))
        .filter_map(|config| std::fs::read_to_string(&config).ok())
        .filter_map(|text| configured_path(&text))
        .map(|path| expand_home(&path, home))
        // A resource can point at a directory that no longer exists; offering
        // it would only produce a sync error the user cannot act on.
        .filter(|path| path.is_dir())
        .collect();

    // Two resources can name one directory, and reading it twice would list
    // every contact twice.
    found.sort();
    found.dedup();
    found
}

/// The resource config files in one directory.
fn resource_configs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_resource_config)
        })
        .collect()
}

/// Whether a filename is a contact resource's config.
fn is_resource_config(name: &str) -> bool {
    RESOURCE_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix) && name.ends_with("rc"))
}

/// The `Path=` value from a resource config.
///
/// These are `KConfig` INI files. Only the one key matters, and it is read
/// without a section check: a resource config has exactly one `Path`, and
/// pulling in an INI parser for a single line would not earn its place.
fn configured_path(text: &str) -> Option<PathBuf> {
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("Path="))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Expands a leading `~` — KDE writes paths both ways.
fn expand_home(path: &Path, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::{configured_path, expand_home, is_resource_config};
    use std::path::{Path, PathBuf};

    /// A real `akonadi_vcarddir_resource_0rc`, trimmed.
    const CONFIG: &str = "\
[General]
ReadOnly=false
Path=/home/blu/.local/share/contacts
AutoSync=true

[Akonadi]
Name=Personal Contacts
";

    #[test]
    fn reads_the_directory_out_of_a_resource_config() {
        assert_eq!(
            configured_path(CONFIG),
            Some(PathBuf::from("/home/blu/.local/share/contacts"))
        );
    }

    #[test]
    fn a_config_without_a_path_yields_nothing() {
        assert_eq!(configured_path("[General]\nAutoSync=true\n"), None);
        assert_eq!(configured_path("Path=\n"), None, "an empty path is not a path");
        assert_eq!(configured_path(""), None);
    }

    /// KDE writes both absolute and tilde paths.
    #[test]
    fn a_tilde_path_expands_to_the_home_directory() {
        let home = Path::new("/home/blu");
        assert_eq!(
            expand_home(Path::new("~/.local/share/contacts"), home),
            PathBuf::from("/home/blu/.local/share/contacts")
        );
        assert_eq!(
            expand_home(Path::new("/srv/contacts"), home),
            PathBuf::from("/srv/contacts")
        );
    }

    /// Akonadi writes a config per resource for mail, notes and calendars too;
    /// reading a maildir as vCards would be nonsense.
    #[test]
    fn only_contact_resources_are_picked_up() {
        assert!(is_resource_config("akonadi_vcarddir_resource_0rc"));
        assert!(is_resource_config("akonadi_contacts_resource_12rc"));
        assert!(!is_resource_config("akonadi_maildir_resource_0rc"));
        assert!(!is_resource_config("akonadi_ical_resource_0rc"));
        assert!(!is_resource_config("kmail2rc"));
        // A stray backup must not be mistaken for a live config.
        assert!(!is_resource_config("akonadi_vcarddir_resource_0rc.bak"));
    }
}
