//! Centralized path definitions and environment helper functions.
//!
//! Every path that depends on the platform or the environment lives here, so
//! the rest of the UI never reaches for `HOME` or splices `.local/share` into
//! a string of its own.

use std::path::PathBuf;

/// Display placeholder string for default local contacts directory (tilde form).
pub const LOCAL_CONTACTS_DIR_DISPLAY: &str = "~/.local/share/contacts";

/// The directory name holding vCards inside the data directory.
///
/// The vdirsyncer/khard/Radicale convention, which is what the local provider
/// reads.
const CONTACTS_DIR_NAME: &str = "contacts";

/// Returns the user's home directory path if available.
///
/// `HOME` first, then `USERPROFILE`: the Windows builds have no `HOME`, so a
/// tilde path there used to expand to nothing at all.
pub fn home_dir() -> Option<PathBuf> {
    home_dir_from(env("HOME").as_deref(), env("USERPROFILE").as_deref())
}

/// The XDG data directory, or the platform's equivalent.
///
/// Honours `XDG_DATA_HOME` the same way `sipster_integrations` does. The two
/// disagreeing meant the folder offered in Settings was not the folder the
/// local provider would read.
pub fn data_dir() -> Option<PathBuf> {
    data_dir_from(env("XDG_DATA_HOME").as_deref(), home_dir())
}

/// Returns the full path to the default local contacts directory
/// (`$XDG_DATA_HOME/contacts`, or `~/.local/share/contacts`).
pub fn default_contacts_dir() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join(CONTACTS_DIR_NAME))
}

/// Returns the default contacts directory path as a `String` if available.
pub fn default_contacts_dir_string() -> String {
    default_contacts_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Expands a leading `~/` or `~` in a path string into a full [`PathBuf`],
/// on every platform.
pub fn expand_home_path(path: &str) -> PathBuf {
    expand_home_path_with(path, home_dir())
}

/// Reads an environment variable, treating blank as unset.
///
/// An empty `XDG_DATA_HOME` is set-but-meaningless, and joining onto it would
/// silently produce a relative path.
fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// The testable core of [`home_dir`].
fn home_dir_from(home: Option<&str>, user_profile: Option<&str>) -> Option<PathBuf> {
    home.or(user_profile).map(PathBuf::from)
}

/// The testable core of [`data_dir`].
fn data_dir_from(xdg_data_home: Option<&str>, home: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(dir) = xdg_data_home {
        return Some(PathBuf::from(dir));
    }
    home.map(|home| home.join(".local").join("share"))
}

/// The testable core of [`expand_home_path`].
fn expand_home_path_with(path: &str, home: Option<PathBuf>) -> PathBuf {
    if path == "~" {
        return home.unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home.map_or_else(|| PathBuf::from(path), |home| home.join(rest));
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::{data_dir_from, expand_home_path_with, home_dir_from, PathBuf};

    /// The Windows builds have no `HOME`, so without the fallback every tilde
    /// path there expanded to nothing.
    #[test]
    fn a_windows_home_is_found_through_user_profile() {
        assert_eq!(
            home_dir_from(None, Some(r"C:\Users\blu")),
            Some(PathBuf::from(r"C:\Users\blu"))
        );
        // HOME still wins where both exist.
        assert_eq!(
            home_dir_from(Some("/home/blu"), Some(r"C:\Users\blu")),
            Some(PathBuf::from("/home/blu"))
        );
        assert_eq!(home_dir_from(None, None), None);
    }

    /// `sipster_integrations` honours `XDG_DATA_HOME`; if this did not, the
    /// folder shown in Settings was not the folder actually read.
    #[test]
    fn the_data_directory_follows_xdg() {
        assert_eq!(
            data_dir_from(Some("/data"), Some(PathBuf::from("/home/blu"))),
            Some(PathBuf::from("/data"))
        );
        assert_eq!(
            data_dir_from(None, Some(PathBuf::from("/home/blu"))),
            Some(PathBuf::from("/home/blu/.local/share"))
        );
        assert_eq!(data_dir_from(None, None), None);
    }

    #[test]
    fn a_tilde_expands_only_at_the_start() {
        let home = || Some(PathBuf::from("/home/blu"));
        assert_eq!(expand_home_path_with("~", home()), PathBuf::from("/home/blu"));
        assert_eq!(
            expand_home_path_with("~/notes", home()),
            PathBuf::from("/home/blu/notes")
        );
        // Not a home reference: neither a bare `~name` nor one further in.
        assert_eq!(expand_home_path_with("~root/x", home()), PathBuf::from("~root/x"));
        assert_eq!(expand_home_path_with("/tmp/~/x", home()), PathBuf::from("/tmp/~/x"));
    }

    /// With no home to expand against, the path is left as written rather than
    /// silently becoming a relative one.
    #[test]
    fn without_a_home_the_path_is_untouched() {
        assert_eq!(expand_home_path_with("~/notes", None), PathBuf::from("~/notes"));
    }
}
