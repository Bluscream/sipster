//! Local persistent storage for Sipster in-app call history and local contacts.

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{CallRecord, Contact};

/// File name for local call history and contacts.
const HISTORY_FILE: &str = "history.json";
const CONTACTS_FILE: &str = "contacts.json";
/// Cap on retained call records, so history cannot grow without bound.
const MAX_HISTORY_RECORDS: usize = 500;

/// Error type for local storage operations.
#[derive(Debug, thiserror::Error)]
pub enum LocalStoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Local store managing on-disk JSON storage in ~/.local/share/sipster (or
/// platform equivalent).
///
/// `data_dir` is `None` when no writable directory could be found. Reads then
/// return empty and writes are no-ops, so the app runs without history rather
/// than failing to start.
#[derive(Debug, Clone)]
pub struct LocalStore {
    data_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HistoryPayload {
    calls: Vec<CallRecord>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ContactsPayload {
    contacts: Vec<Contact>,
}

impl LocalStore {
    /// Initializes local storage in the standard user data directory.
    pub fn new() -> Result<Self, LocalStoreError> {
        let dir = data_directory().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "neither XDG_DATA_HOME nor HOME is set",
            )
        })?;
        Self::with_directory(dir)
    }

    /// Initializes local storage in a specified directory.
    ///
    /// The directory is created `0700`: it holds who called whom and when,
    /// which is nobody else's business on a shared machine.
    pub fn with_directory(data_dir: PathBuf) -> Result<Self, LocalStoreError> {
        if !data_dir.exists() {
            fs::create_dir_all(&data_dir)?;
            restrict_dir(&data_dir)?;
        }
        Ok(Self { data_dir: Some(data_dir) })
    }

    /// A store with nowhere to write. Reads are empty, writes are dropped.
    pub fn disabled() -> Self {
        Self { data_dir: None }
    }

    /// Whether this store actually persists anything.
    pub fn is_enabled(&self) -> bool {
        self.data_dir.is_some()
    }

    fn history_path(&self) -> Option<PathBuf> {
        self.data_dir.as_ref().map(|dir| dir.join(HISTORY_FILE))
    }

    fn contacts_path(&self) -> Option<PathBuf> {
        self.data_dir.as_ref().map(|dir| dir.join(CONTACTS_FILE))
    }

    /// Loads all recorded local call records.
    pub fn load_calls(&self) -> Result<Vec<CallRecord>, LocalStoreError> {
        let Some(path) = self.history_path() else {
            return Ok(Vec::new());
        };
        Ok(read_json::<HistoryPayload>(&path)?.calls)
    }

    /// Appends a new call record to local history.
    pub fn record_call(&self, call: CallRecord) -> Result<(), LocalStoreError> {
        let Some(path) = self.history_path() else {
            return Ok(());
        };
        let mut calls = self.load_calls().unwrap_or_default();
        // Most recent first.
        calls.insert(0, call);
        calls.truncate(MAX_HISTORY_RECORDS);
        write_json(&path, &HistoryPayload { calls })
    }

    /// Loads custom local contacts.
    pub fn load_contacts(&self) -> Result<Vec<Contact>, LocalStoreError> {
        let Some(path) = self.contacts_path() else {
            return Ok(Vec::new());
        };
        Ok(read_json::<ContactsPayload>(&path)?.contacts)
    }

    /// Saves custom local contacts.
    pub fn save_contacts(&self, contacts: &[Contact]) -> Result<(), LocalStoreError> {
        let Some(path) = self.contacts_path() else {
            return Ok(());
        };
        write_json(&path, &ContactsPayload { contacts: contacts.to_vec() })
    }

    /// Adds or updates a local contact.
    pub fn upsert_contact(&self, contact: Contact) -> Result<(), LocalStoreError> {
        let mut contacts = self.load_contacts().unwrap_or_default();
        if let Some(idx) = contacts.iter().position(|c| c.id == contact.id) {
            contacts[idx] = contact;
        } else {
            contacts.push(contact);
        }
        self.save_contacts(&contacts)
    }

    /// Deletes a local contact by ID.
    pub fn delete_contact(&self, contact_id: &str) -> Result<(), LocalStoreError> {
        let mut contacts = self.load_contacts().unwrap_or_default();
        contacts.retain(|c| c.id != contact_id);
        self.save_contacts(&contacts)
    }

    /// Clears all stored local call history.
    pub fn clear_calls(&self) -> Result<(), LocalStoreError> {
        let Some(path) = self.history_path() else {
            return Ok(());
        };
        write_json(&path, &HistoryPayload { calls: Vec::new() })
    }
}

/// Reads a JSON payload, treating a missing file as empty.
///
/// A corrupt file is reported rather than silently defaulting: the previous
/// `unwrap_or_default()` meant damaged history was quietly replaced by an
/// empty list on the next write, destroying whatever was recoverable.
fn read_json<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> Result<T, LocalStoreError> {
    match File::open(path) {
        Ok(file) => Ok(serde_json::from_reader(BufReader::new(file))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.into()),
    }
}

/// Writes a JSON payload atomically, owner-readable only.
///
/// Both properties were missing: `File::create` truncates in place, so a crash
/// mid-write left a half-written history file, and the default mode made call
/// history and contacts world-readable.
fn write_json<T: Serialize>(path: &Path, payload: &T) -> Result<(), LocalStoreError> {
    let temp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(payload)?;
    fs::write(&temp, json)?;
    restrict_file(&temp)?;
    fs::rename(&temp, path)?;
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<(), LocalStoreError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(unix)]
fn restrict_dir(path: &Path) -> Result<(), LocalStoreError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<(), LocalStoreError> {
    Ok(())
}

#[cfg(not(unix))]
fn restrict_dir(_path: &Path) -> Result<(), LocalStoreError> {
    Ok(())
}

/// XDG data directory resolution, without pulling in a crate for two lookups.
fn data_directory() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        if !dir.trim().is_empty() {
            return Some(Path::new(&dir).join("sipster"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return Some(Path::new(&home).join(".local/share/sipster"));
    }
    None
}
