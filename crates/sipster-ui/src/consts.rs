//! Centralized path definitions and environment helper functions.

use std::path::PathBuf;

/// Display placeholder string for default local contacts directory (tilde form).
pub const LOCAL_CONTACTS_DIR_DISPLAY: &str = "~/.local/share/contacts";

/// Relative path to default local contacts directory from user home.
pub const LOCAL_CONTACTS_DIR_RELATIVE: &str = ".local/share/contacts";

/// Returns the user's home directory path if available.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Returns the full path to the default local contacts directory (`~/.local/share/contacts`).
pub fn default_contacts_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(LOCAL_CONTACTS_DIR_RELATIVE))
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
    if path == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().map_or_else(|| PathBuf::from(path), |home| home.join(rest));
    }
    PathBuf::from(path)
}
