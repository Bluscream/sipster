//! Main entry point for `sipster-integrations`.
//!
//! Provides unified contacts and call history synchronized across local storage,
//! FRITZ!Box TR-064, and `CardDAV` sources.

pub mod carddav;
pub mod fritzbox;
pub mod google;
pub mod local;
pub mod model;

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

pub use carddav::{CardDavClient, CardDavConfig};
pub use fritzbox::{FritzBoxClient, FritzConfig, FritzError};
pub use google::{GoogleContactsClient, GoogleTokenResponse, DEFAULT_CLIENT_ID, DEFAULT_CLIENT_SECRET};
pub use local::{LocalStore, LocalStoreError};
pub use model::{CallRecord, CallType, Contact, NumberType, PhoneNumber, RecordSource};

/// Central manager coordinating multi-source contact and call history synchronization.
#[derive(Debug, Clone)]
pub struct SyncManager {
    local_store: LocalStore,
    fritz_client: Option<FritzBoxClient>,
    google_clients: Vec<GoogleContactsClient>,
    carddav_clients: Vec<CardDavClient>,
    cached_contacts: Arc<RwLock<Vec<Contact>>>,
    cached_calls: Arc<RwLock<Vec<CallRecord>>>,
}

impl SyncManager {
    /// Creates a new `SyncManager`, automatically discovering FRITZ!Box credentials
    /// from the environment or local configuration if available.
    ///
    /// # Panics
    ///
    /// Panics only if system temp directory is completely unwritable.
    pub fn new() -> Self {
        let local_store = LocalStore::new().unwrap_or_else(|e| {
            warn!(error = %e, "could not initialize local store in default directory");
            LocalStore::with_directory(std::env::temp_dir().join("sipster"))
                .expect("temp dir writable")
        });

        let fritz_client = FritzConfig::from_env().map(FritzBoxClient::new);

        Self {
            local_store,
            fritz_client,
            google_clients: Vec::new(),
            carddav_clients: Vec::new(),
            cached_contacts: Arc::new(RwLock::new(Vec::new())),
            cached_calls: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Configures the active FRITZ!Box client.
    pub fn set_fritzbox(&mut self, config: Option<FritzConfig>) {
        self.fritz_client = config.map(FritzBoxClient::new);
    }

    /// Sets the list of active Google Contacts clients.
    pub fn set_google_accounts(&mut self, clients: Vec<GoogleContactsClient>) {
        self.google_clients = clients;
    }

    /// Sets the list of active `CardDAV` clients.
    pub fn set_carddav_accounts(&mut self, clients: Vec<CardDavClient>) {
        self.carddav_clients = clients;
    }

    /// Access to the underlying local storage engine.
    pub fn local_store(&self) -> &LocalStore {
        &self.local_store
    }

    /// Records an in-app call event into local history.
    pub fn record_local_call(&self, call: CallRecord) {
        if let Err(e) = self.local_store.record_call(call) {
            warn!(error = %e, "could not persist local call record");
        }
    }

    /// Refreshes contacts from all active providers and merges them.
    pub async fn sync_contacts(&self) -> Vec<Contact> {
        let mut merged = Vec::new();

        // 1. Local contacts
        if let Ok(local_contacts) = self.local_store.load_contacts() {
            merged.extend(local_contacts);
        }

        // 2. FRITZ!Box contacts
        if let Some(client) = self.fritz_client.clone() {
            if let Ok(Ok(fritz_contacts)) = tokio::task::spawn_blocking(move || client.fetch_contacts()).await {
                info!(count = fritz_contacts.len(), "fetched contacts from FRITZ!Box");
                merged.extend(fritz_contacts);
            }
        }

        // 3. Google Contacts accounts
        for google_client in self.google_clients.clone() {
            let g_email = google_client.email.clone();
            if let Ok(Ok(google_contacts)) = tokio::task::spawn_blocking(move || google_client.fetch_contacts()).await {
                info!(count = google_contacts.len(), email = %g_email, "fetched contacts from Google");
                merged.extend(google_contacts);
            }
        }

        // 4. CardDAV accounts
        for carddav_client in self.carddav_clients.clone() {
            let label = carddav_client.config.url.clone();
            if let Ok(Ok(carddav_contacts)) = tokio::task::spawn_blocking(move || carddav_client.fetch_contacts()).await {
                info!(count = carddav_contacts.len(), url = %label, "fetched contacts from CardDAV");
                merged.extend(carddav_contacts);
            }
        }

        merged.sort_by_key(|a| a.name.to_lowercase());
        merged.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name) && a.primary_number() == b.primary_number());

        let mut lock = self.cached_contacts.write().await;
        (*lock).clone_from(&merged);
        merged
    }

    /// Refreshes call history from all active providers and merges them chronologically.
    pub async fn sync_calls(&self) -> Vec<CallRecord> {
        let mut merged = Vec::new();

        if let Ok(local_calls) = self.local_store.load_calls() {
            merged.extend(local_calls);
        }

        if let Some(client) = self.fritz_client.clone() {
            if let Ok(Ok(fritz_calls)) = tokio::task::spawn_blocking(move || client.fetch_calls()).await {
                info!(count = fritz_calls.len(), "fetched call records from FRITZ!Box");
                merged.extend(fritz_calls);
            }
        }

        merged.dedup_by(|a, b| a.timestamp == b.timestamp && a.remote_number == b.remote_number);

        let mut lock = self.cached_calls.write().await;
        (*lock).clone_from(&merged);
        merged
    }

    /// Returns currently cached contacts without blocking.
    pub async fn get_cached_contacts(&self) -> Vec<Contact> {
        self.cached_contacts.read().await.clone()
    }

    /// Returns currently cached call history without blocking.
    pub async fn get_cached_calls(&self) -> Vec<CallRecord> {
        self.cached_calls.read().await.clone()
    }
}

impl Default for SyncManager {
    fn default() -> Self {
        Self::new()
    }
}
