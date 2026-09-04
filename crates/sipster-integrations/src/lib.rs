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
pub use google::{GoogleContactsClient, GoogleTokenResponse};
pub use local::{LocalStore, LocalStoreError};
pub use model::{
    caller_number, normalize_number, number_matches, CallRecord, CallType, Contact, NumberType,
    PhoneNumber, RecordSource,
};

/// Shared HTTP agent for every provider.
///
/// The timeouts are the point. Without them a router or address-book server
/// that accepts the connection and then stops responding pins a
/// `spawn_blocking` worker forever; a few failed syncs would exhaust the
/// blocking pool and every later sync would hang with no error anywhere.
#[must_use]
pub fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(20))
        .timeout_write(std::time::Duration::from_secs(10))
        .build()
}

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
    /// Creates a new `SyncManager` with no providers configured.
    ///
    /// Providers are supplied from the config file via `set_fritzbox`,
    /// `set_google_accounts` and `set_carddav_accounts`. Nothing is read from
    /// the environment.
    ///
    /// Never panics: this runs during app boot, and an unwritable data
    /// directory must degrade to "history does not persist", not take the
    /// whole softphone down.
    pub fn new() -> Self {
        let local_store = LocalStore::new()
            .or_else(|e| {
                warn!(error = %e, "could not initialize local store; trying a temporary directory");
                LocalStore::with_directory(std::env::temp_dir().join("sipster"))
            })
            .unwrap_or_else(|e| {
                warn!(error = %e, "no writable data directory; local history is disabled");
                LocalStore::disabled()
            });

        Self {
            local_store,
            fritz_client: None,
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

        // 2. Remote providers
        merged.extend(self.fetch_remote_contacts().await);

        // Cached: the lowercase key is built once per contact rather than on
        // every comparison, which matters once several address books merge.
        merged.sort_by_cached_key(|c| c.name.to_lowercase());
        merged.dedup_by(|a, b| {
            a.name.eq_ignore_ascii_case(&b.name) && a.primary_number() == b.primary_number()
        });

        let mut lock = self.cached_contacts.write().await;
        (*lock).clone_from(&merged);
        merged
    }

    async fn fetch_remote_contacts(&self) -> Vec<Contact> {
        let mut remote = Vec::new();

        if let Some(client) = self.fritz_client.clone() {
            let fetched = run_provider("FRITZ!Box", move || client.fetch_contacts()).await;
            remote.extend(fetched);
        }

        for client in self.google_clients.clone() {
            let label = format!("Google ({})", client.email);
            let fetched = run_provider(&label, move || client.fetch_contacts()).await;
            remote.extend(fetched);
        }

        for client in self.carddav_clients.clone() {
            let label = format!("CardDAV ({})", client.config.url);
            let fetched = run_provider(&label, move || client.fetch_contacts()).await;
            remote.extend(fetched);
        }

        remote
    }

    /// Refreshes call history from all active providers and merges them,
    /// most recent first.
    pub async fn sync_calls(&self) -> Vec<CallRecord> {
        let mut merged = Vec::new();

        match self.local_store.load_calls() {
            Ok(local_calls) => merged.extend(local_calls),
            Err(e) => warn!(error = %e, "could not read local call history"),
        }

        if let Some(client) = self.fritz_client.clone() {
            merged.extend(run_provider("FRITZ!Box call list", move || client.fetch_calls()).await);
        }

        // `dedup_by` only collapses *adjacent* equal items, so the sort is what
        // makes it work at all. Without it the same call fetched from local
        // history and from the router stayed in the list twice, because the two
        // copies were never neighbours. Sorting also delivers the newest-first
        // order the call list renders.
        merged.sort_by(|a, b| {
            b.timestamp
                .cmp(&a.timestamp)
                .then_with(|| a.remote_number.cmp(&b.remote_number))
        });
        merged.dedup_by(|a, b| a.timestamp == b.timestamp && a.remote_number == b.remote_number);

        let mut lock = self.cached_calls.write().await;
        (*lock).clone_from(&merged);
        merged
    }

    /// Returns the contacts from the last successful sync.
    pub async fn get_cached_contacts(&self) -> Vec<Contact> {
        self.cached_contacts.read().await.clone()
    }

    /// Returns the call history from the last successful sync.
    pub async fn get_cached_calls(&self) -> Vec<CallRecord> {
        self.cached_calls.read().await.clone()
    }
}

/// Runs one blocking provider fetch, reporting failures rather than dropping
/// them.
///
/// Every provider call used to be wrapped in `if let Ok(Ok(..))`, which
/// discarded both the provider's error and a panic in the worker thread — a
/// misconfigured account synced zero contacts and said nothing at all.
async fn run_provider<T, E, F>(label: &str, fetch: F) -> Vec<T>
where
    F: FnOnce() -> Result<Vec<T>, E> + Send + 'static,
    T: Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    // Flatten "the task panicked" and "the provider failed" into one outcome
    // before reporting, so there is a single success and a single failure path.
    let outcome = match tokio::task::spawn_blocking(fetch).await {
        Ok(result) => result.map_err(|e| e.to_string()),
        Err(e) => Err(format!("worker thread panicked: {e}")),
    };

    match outcome {
        Ok(items) => {
            info!(provider = label, count = items.len(), "synced");
            items
        }
        Err(reason) => {
            warn!(provider = label, error = %reason, "sync failed");
            Vec::new()
        }
    }
}

impl Default for SyncManager {
    fn default() -> Self {
        Self::new()
    }
}
