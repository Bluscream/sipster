//! Main entry point for `sipster-integrations`.
//!
//! Provides unified contacts and call history synchronized across local storage,
//! FRITZ!Box TR-064, and `CardDAV` sources.

pub mod carddav;
pub mod fritzbox;
pub mod google;
pub mod local;
pub mod model;
pub mod vcard;
pub mod akonadi;
pub mod pinned_tls;
#[cfg(target_os = "linux")]
pub mod eds;
pub mod vdir;

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

pub use carddav::{CardDavClient, CardDavConfig};
pub use fritzbox::{take_learned_fingerprint, FritzBoxClient, FritzConfig, FritzError};
pub use google::{cancel_pending_auth, GoogleContactsClient, GoogleTokenResponse};
pub use local::{LocalStore, LocalStoreError};
pub use vdir::VdirStore;
pub use model::{
    caller_number, normalize_number, number_contains, number_matches, timestamp_key, CallRecord,
    CallType, Contact, NumberType,
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
    // One shared agent for the whole process. `ureq::Agent` is a cheap
    // clonable handle around a connection pool, so building a new one per
    // request — as every call site used to — threw the pool away and paid a
    // fresh TCP handshake every time.
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(build_agent).clone()
}

/// An agent that trusts one self-signed certificate, pinned by fingerprint.
///
/// Used for the router, whose certificate no authority can vouch for. See
/// [`pinned_tls`]. The learned fingerprint is reported back through `seen` so
/// the caller can store it after a successful first connection.
#[must_use]
pub fn pinned_agent(
    fingerprint: String,
) -> (ureq::Agent, std::sync::Arc<std::sync::Mutex<Option<String>>>) {
    // rustls refuses to pick a crypto provider when more than one could be
    // compiled in, and panics inside the worker rather than returning an
    // error. Installed once here, before any TLS config is built.
    static PROVIDER: std::sync::Once = std::sync::Once::new();
    PROVIDER.call_once(|| {
        if rustls::crypto::ring::default_provider().install_default().is_err() {
            // Already installed by someone else, which is equally fine.
            tracing::debug!("a rustls crypto provider was already installed");
        }
    });

    let (verifier, seen) = pinned_tls::PinnedCert::new(fingerprint);
    let tls = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    // Our own connector rather than `tls_config`, so that a router closing the
    // socket without `close_notify` reads as end of stream — see
    // `pinned_tls::PinnedConnector`.
    let connector = pinned_tls::PinnedConnector::new(std::sync::Arc::new(tls));
    let agent = agent_builder().tls_connector(std::sync::Arc::new(connector)).build();
    (agent, seen)
}

fn build_agent() -> ureq::Agent {
    agent_builder().build()
}

fn agent_builder() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new()
        // Connect fast so an unreachable host fails quickly, but read
        // patiently: a FRITZ!Box generates calllist.lua on demand and can take
        // the better part of a minute to answer for a long call list. 20s was
        // not enough and the sync failed against a working router.
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(60))
        .timeout_write(std::time::Duration::from_secs(15))
}

/// Central manager coordinating multi-source contact and call history synchronization.
#[derive(Debug, Clone)]
pub struct SyncManager {
    local_store: LocalStore,
    fritz_client: Option<FritzBoxClient>,
    google_clients: Vec<GoogleContactsClient>,
    carddav_clients: Vec<CardDavClient>,
    vdir_stores: Vec<VdirStore>,
    eds_enabled: bool,
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
            vdir_stores: Vec::new(),
            eds_enabled: false,
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

    /// Turns the Evolution Data Server provider on or off.
    pub fn set_eds(&mut self, enabled: bool) {
        self.eds_enabled = enabled;
    }

    /// Sets the local vCard directories, replacing any previous ones.
    ///
    /// Plural because a machine can have several: the user's own directory
    /// plus each address book Akonadi is configured with.
    pub fn set_vdir(&mut self, stores: Vec<VdirStore>) {
        self.vdir_stores = stores;
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

    /// Streams contacts as each provider returns, rather than after all of
    /// them have.
    ///
    /// The local store answers immediately and the router can take ten
    /// seconds, so waiting for everything meant staring at "Syncing…" while
    /// results were already in hand. Each batch is sent as it lands; the
    /// receiver merges and re-sorts.
    pub fn sync_contacts_streaming(&self, tx: tokio::sync::mpsc::UnboundedSender<Vec<Contact>>) {
        let fritz = self.fritz_client.clone();
        let google = self.google_clients.clone();
        let carddav = self.carddav_clients.clone();
        let vdir = self.vdir_stores.clone();
        let eds = self.eds_enabled;
        let cache = Arc::clone(&self.cached_contacts);

        tokio::spawn(async move {
            let mut everything = Vec::new();



            let mut tasks: Vec<
                std::pin::Pin<Box<dyn std::future::Future<Output = Vec<Contact>> + Send>>,
            > = Vec::new();
            if let Some(client) = fritz {
                tasks.push(Box::pin(run_provider_owned("FRITZ!Box".to_string(), move || {
                    client.fetch_contacts()
                })));
            }
            for client in google {
                let label = format!("Google ({})", client.email);
                tasks.push(Box::pin(run_provider_owned(label, move || {
                    client.fetch_contacts()
                })));
            }
            for client in carddav {
                let label = format!("CardDAV ({})", client.config.url);
                tasks.push(Box::pin(run_provider_owned(label, move || {
                    client.fetch_contacts()
                })));
            }
            for store in vdir {
                let label = format!("vCards ({})", store.root().display());
                tasks.push(Box::pin(run_provider_owned(label, move || store.load())));
            }
            if eds {
                tasks.push(Box::pin(run_provider_owned("Evolution".to_string(), eds_contacts)));
            }

            // Emit each provider the moment it finishes, in completion order.
            let mut pending: futures_util::stream::FuturesUnordered<_> =
                tasks.into_iter().collect();
            while let Some(batch) = futures_util::StreamExt::next(&mut pending).await {
                if batch.is_empty() {
                    continue;
                }
                everything.extend(batch.clone());
                if tx.send(batch).is_err() {
                    return; // window closed
                }
            }

            *cache.write().await = everything;
        });
    }

    /// Streams call records as each source returns. See
    /// [`sync_contacts_streaming`](Self::sync_contacts_streaming).
    pub fn sync_calls_streaming(&self, tx: tokio::sync::mpsc::UnboundedSender<Vec<CallRecord>>) {
        let local = self.local_store.clone();
        let fritz = self.fritz_client.clone();
        let cache = Arc::clone(&self.cached_calls);

        tokio::spawn(async move {
            let mut everything = Vec::new();

            match local.load_calls() {
                Ok(calls) if !calls.is_empty() => {
                    everything.extend(calls.clone());
                    let _ = tx.send(calls);
                }
                Ok(_) => {}
                Err(e) => warn!(error = %e, "could not read local call history"),
            }

            if let Some(client) = fritz {
                let batch =
                    run_provider("FRITZ!Box call list", move || client.fetch_calls()).await;
                if !batch.is_empty() {
                    everything.extend(batch.clone());
                    if tx.send(batch).is_err() {
                        return;
                    }
                }
            }

            *cache.write().await = everything;
        });
    }

    /// Refreshes contacts from all active providers and merges them.
    pub async fn sync_contacts(&self) -> Vec<Contact> {
        let mut merged = Vec::new();



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

    /// Fetches from every configured provider at once.
    ///
    /// These were awaited one after another, so the total was the sum of every
    /// provider's latency — a slow router delayed the Google and `CardDAV`
    /// results behind it for no reason. They are independent, so the total is
    /// now the slowest one rather than the sum.
    async fn fetch_remote_contacts(&self) -> Vec<Contact> {
        let mut tasks: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = Vec<Contact>> + Send>>> =
            Vec::new();

        if let Some(client) = self.fritz_client.clone() {
            tasks.push(Box::pin(run_provider_owned(
                "FRITZ!Box".to_string(),
                move || client.fetch_contacts(),
            )));
        }
        for client in self.google_clients.clone() {
            let label = format!("Google ({})", client.email);
            tasks.push(Box::pin(run_provider_owned(label, move || {
                client.fetch_contacts()
            })));
        }
        for client in self.carddav_clients.clone() {
            let label = format!("CardDAV ({})", client.config.url);
            tasks.push(Box::pin(run_provider_owned(label, move || {
                client.fetch_contacts()
            })));
        }
        for store in self.vdir_stores.clone() {
            let label = format!("vCards ({})", store.root().display());
            tasks.push(Box::pin(run_provider_owned(label, move || store.load())));
        }
        if self.eds_enabled {
            tasks.push(Box::pin(run_provider_owned("Evolution".to_string(), eds_contacts)));
        }

        futures_util::future::join_all(tasks)
            .await
            .into_iter()
            .flatten()
            .collect()
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
        // Newest first, by parsed date. Comparing the raw strings does not
        // work: the router writes DD.MM.YY, so lexicographic order put every
        // 31st ahead of every 30th regardless of month.
        merged.sort_by(|a, b| {
            model::timestamp_key(&b.timestamp)
                .cmp(&model::timestamp_key(&a.timestamp))
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
/// [`run_provider`] with an owned label, so it can be held across an `await`
/// in a joined set.
async fn run_provider_owned<T, E, F>(label: String, fetch: F) -> Vec<T>
where
    F: FnOnce() -> Result<Vec<T>, E> + Send + 'static,
    T: Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    run_provider(&label, fetch).await
}

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

/// The Evolution Data Server provider, as the sync tasks want it.
///
/// EDS is a GNOME component; everywhere else this is simply an empty address
/// book rather than a missing feature.
#[cfg(target_os = "linux")]
fn eds_contacts() -> Result<Vec<Contact>, String> {
    eds::fetch_contacts().map_err(|e| e.to_string())
}

#[cfg(not(target_os = "linux"))]
fn eds_contacts() -> Result<Vec<Contact>, String> {
    Ok(Vec::new())
}

/// Whether Evolution Data Server is reachable on this machine.
///
/// Cross-platform so callers — the settings panel especially — need no `cfg`
/// of their own. EDS is a GNOME component, so this is simply `false` off
/// Linux. Not having this is what broke the Windows build: the UI called into
/// the Linux-only module directly, and `build.sh check` only covers the host
/// target, so nothing caught it until the cross build ran.
#[must_use]
pub fn eds_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        eds::available()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}
