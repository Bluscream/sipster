//! Local persistent storage for Sipster in-app call history and local contacts.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{CallRecord, Contact};

/// File name for local call history and contacts.
const HISTORY_FILE: &str = "history.json";
const CONTACTS_FILE: &str = "contacts.json";

/// Error type for local storage operations.
#[derive(Debug, thiserror::Error)]
pub enum LocalStoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Local store managing on-disk JSON storage in ~/.local/share/sipster (or platform equivalent).
#[derive(Debug, Clone)]
pub struct LocalStore {
    data_dir: PathBuf,
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
        let dir = dirs_next().unwrap_or_else(|| PathBuf::from("."));
        Self::with_directory(dir)
    }

    /// Initializes local storage in a specified directory.
    pub fn with_directory(data_dir: PathBuf) -> Result<Self, LocalStoreError> {
        if !data_dir.exists() {
            fs::create_dir_all(&data_dir)?;
        }
        Ok(Self { data_dir })
    }

    fn history_path(&self) -> PathBuf {
        self.data_dir.join(HISTORY_FILE)
    }

    fn contacts_path(&self) -> PathBuf {
        self.data_dir.join(CONTACTS_FILE)
    }

    /// Loads all recorded local call records.
    pub fn load_calls(&self) -> Result<Vec<CallRecord>, LocalStoreError> {
        let path = self.history_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let payload: HistoryPayload = serde_json::from_reader(reader).unwrap_or_default();
        Ok(payload.calls)
    }

    /// Appends a new call record to local history.
    pub fn record_call(&self, call: CallRecord) -> Result<(), LocalStoreError> {
        let mut calls = self.load_calls().unwrap_or_default();
        // Insert at the front (most recent first)
        calls.insert(0, call);
        // Cap to 500 records
        if calls.len() > 500 {
            calls.truncate(500);
        }
        let file = File::create(self.history_path())?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, &HistoryPayload { calls })?;
        Ok(())
    }

    /// Loads custom local contacts.
    pub fn load_contacts(&self) -> Result<Vec<Contact>, LocalStoreError> {
        let path = self.contacts_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let payload: ContactsPayload = serde_json::from_reader(reader).unwrap_or_default();
        Ok(payload.contacts)
    }

    /// Saves custom local contacts.
    pub fn save_contacts(&self, contacts: &[Contact]) -> Result<(), LocalStoreError> {
        let file = File::create(self.contacts_path())?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, &ContactsPayload { contacts: contacts.to_vec() })?;
        Ok(())
    }
}

/// Fallback or XDG data directory resolution without external dependency.
fn dirs_next() -> Option<PathBuf> {
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
