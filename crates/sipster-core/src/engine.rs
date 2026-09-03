//! The Sipster calling engine: a safe, UI-agnostic wrapper over rvoip's
//! softphone `Endpoint`.
//!
//! Responsibilities that belong to *any* frontend live here — registration,
//! placing/answering/ending calls, and translating rvoip's event stream into
//! Sipster's own [`CallEvent`]s on a broadcast channel. A UI subscribes and
//! renders; it never touches rvoip types directly.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use rvoip_sip::{EndpointCall, EndpointControl, EndpointEvent, EndpointEvents, EndpointIncomingCall};

use crate::audio::{self, CallAudio, DeviceSelection};
use crate::call::{CallEvent, CallId, CallState, RegistrationState};
use crate::config::SipAccount;
use crate::error::{Error, Result};

/// A call handle tracked by the engine, keyed by our own [`CallId`].
///
/// `Incoming` is boxed: `EndpointIncomingCall` is substantially larger than
/// `EndpointCall`, and every map entry would otherwise be sized for it.
enum Tracked {
    /// Inbound call ringing, awaiting a local answer/decline decision.
    Incoming(Box<EndpointIncomingCall>),
    /// Established (or outbound-in-progress) call we can control.
    Active(EndpointCall),
}

/// Maps our stable [`CallId`]s to rvoip handles and back.
///
/// Generic over the handle type purely for testability: rvoip's call handles
/// cannot be constructed without a live stack, so tests substitute a stand-in
/// and still exercise the real bookkeeping.
struct Registry<T = Tracked> {
    tracked: HashMap<CallId, T>,
    /// rvoip `Call-ID` string -> our [`CallId`], for correlating inbound events.
    by_rvoip: HashMap<String, CallId>,
    /// Live OS audio bindings; dropping an entry stops that call's audio.
    audio: HashMap<CallId, CallAudio>,
}

// Hand-written so `T` needs no `Default` bound of its own.
impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self {
            tracked: HashMap::new(),
            by_rvoip: HashMap::new(),
            audio: HashMap::new(),
        }
    }
}

impl<T> Registry<T> {
    fn insert(&mut self, rvoip_id: String, tracked: T) -> CallId {
        let id = CallId::new();
        self.by_rvoip.insert(rvoip_id, id);
        self.tracked.insert(id, tracked);
        id
    }

    fn resolve(&self, rvoip_id: &str) -> Option<CallId> {
        self.by_rvoip.get(rvoip_id).copied()
    }

    /// Forgets a call and returns its handle so the caller can still act on it
    /// (send BYE/CANCEL) after the bookkeeping is gone.
    ///
    /// Returning the handle is the whole point: an earlier version cleared the
    /// entry first and then tried to read it, silently skipping the BYE while
    /// still stopping audio — the call stayed up with no sound.
    fn take(&mut self, id: CallId) -> Option<T> {
        let tracked = self.tracked.remove(&id);
        self.by_rvoip.retain(|_, v| *v != id);
        // Dropping the CallAudio stops capture/playback for this call.
        self.audio.remove(&id);
        tracked
    }
}

/// The engine. Clone-cheap parts live behind `Arc`; drop stops the event pump.
pub struct SipEngine {
    account: SipAccount,
    control: Arc<EndpointControl>,
    event_tx: broadcast::Sender<CallEvent>,
    registry: Arc<Mutex<Registry>>,
    devices: Arc<DeviceSelection>,
    pump: JoinHandle<()>,
}

impl SipEngine {
    /// Builds an endpoint for `account` and starts translating its events.
    ///
    /// Does not register yet — call [`register`](Self::register) once a
    /// subscriber is listening, so no registration event is missed.
    pub async fn connect(account: SipAccount) -> Result<Self> {
        let endpoint = crate::build_endpoint(&account).await?;
        let events = endpoint
            .events()
            .await
            .map_err(|e| Error::Config(format!("event subscribe failed: {e}")))?;
        let (control, _events_owned) = endpoint.split();

        let (event_tx, _) = broadcast::channel(64);
        let registry = Arc::new(Mutex::new(Registry::default()));
        let devices = Arc::new(DeviceSelection::default());
        let pump = spawn_pump(events, registry.clone(), event_tx.clone(), devices.clone());

        Ok(Self {
            account,
            control: Arc::new(control),
            event_tx,
            registry,
            devices,
            pump,
        })
    }

    /// Subscribe to engine events. Multiple subscribers are supported.
    pub fn subscribe(&self) -> broadcast::Receiver<CallEvent> {
        self.event_tx.subscribe()
    }

    pub fn account(&self) -> &SipAccount {
        &self.account
    }

    /// Register with the configured registrar, emitting registration state.
    pub async fn register(&self) -> Result<()> {
        let _ = self
            .event_tx
            .send(CallEvent::Registration(RegistrationState::Registering));
        match self.control.register().await {
            Ok(()) => {
                info!(account = %self.account.label, "registered");
                let _ = self
                    .event_tx
                    .send(CallEvent::Registration(RegistrationState::Registered));
                Ok(())
            }
            Err(e) => {
                let reason = e.to_string();
                warn!(account = %self.account.label, %reason, "registration failed");
                let _ = self.event_tx.send(CallEvent::Registration(
                    RegistrationState::Failed(reason.clone()),
                ));
                Err(Error::RegistrationRejected {
                    registrar: self.account.registrar.clone(),
                    status: 0,
                    reason,
                })
            }
        }
    }

    /// Unregister from the registrar.
    pub async fn unregister(&self) -> Result<()> {
        self.control
            .unregister()
            .await
            .map_err(|e| Error::Config(format!("unregister failed: {e}")))?;
        let _ = self
            .event_tx
            .send(CallEvent::Registration(RegistrationState::Unregistered));
        Ok(())
    }

    /// Place an outbound call to `target` (an extension or full SIP URI).
    ///
    /// Returns immediately with a [`CallId`]; progress and answer arrive as
    /// [`CallEvent`]s. The call is tracked so it can be hung up later.
    pub async fn dial(&self, target: &str) -> Result<CallId> {
        let rvoip_id = self
            .control
            .invite(target)
            .map_err(|e| Error::Config(format!("invite build failed: {e}")))?
            .send()
            .await
            .map_err(|e| Error::CallRejected {
                status: 0,
                reason: e.to_string(),
            })?;
        let call = self.control.wrap_call(rvoip_id.clone());
        let id = {
            let mut reg = self.registry.lock().await;
            reg.insert(call.id().to_string(), Tracked::Active(call))
        };
        info!(%id, target, "dialing");
        let _ = self.event_tx.send(CallEvent::StateChanged {
            id,
            state: CallState::Dialing,
        });
        Ok(id)
    }

    /// Answer a ringing inbound call.
    pub async fn answer(&self, id: CallId) -> Result<()> {
        let tracked = {
            let mut reg = self.registry.lock().await;
            reg.tracked.remove(&id)
        };
        match tracked {
            Some(Tracked::Incoming(incoming)) => {
                let call = incoming
                    .answer()
                    .await
                    .map_err(|e| Error::Config(format!("answer failed: {e}")))?;
                // Bind mic/speaker before announcing the call as active, so the
                // user is not told they are connected while still silent.
                let bound = audio::warn_on_failure(audio::attach(&call, &self.devices).await);
                let mut reg = self.registry.lock().await;
                reg.by_rvoip.insert(call.id().to_string(), id);
                reg.tracked.insert(id, Tracked::Active(call));
                if let Some(bound) = bound {
                    reg.audio.insert(id, bound);
                }
                drop(reg);
                let _ = self.event_tx.send(CallEvent::StateChanged {
                    id,
                    state: CallState::Active,
                });
                Ok(())
            }
            Some(other) => {
                let mut reg = self.registry.lock().await;
                reg.tracked.insert(id, other);
                Err(Error::UnknownCall(id))
            }
            None => Err(Error::UnknownCall(id)),
        }
    }

    /// Hang up (or decline) a call by our id.
    pub async fn hangup(&self, id: CallId) -> Result<()> {
        let tracked = self.registry.lock().await.take(id);
        match tracked {
            Some(Tracked::Active(call)) => call
                .hangup()
                .await
                .map_err(|e| Error::Sip(format!("hangup failed: {e}"))),
            Some(Tracked::Incoming(incoming)) => incoming
                .decline()
                .await
                .map_err(|e| Error::Sip(format!("decline failed: {e}"))),
            None => Err(Error::UnknownCall(id)),
        }
    }
}

impl std::fmt::Debug for SipEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SipEngine")
            .field("account", &self.account.label)
            .finish_non_exhaustive()
    }
}

impl Drop for SipEngine {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

/// Spawns the task that drains rvoip events and emits [`CallEvent`]s.
fn spawn_pump(
    mut events: EndpointEvents,
    registry: Arc<Mutex<Registry>>,
    tx: broadcast::Sender<CallEvent>,
    devices: Arc<DeviceSelection>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match events.next().await {
                Ok(Some(event)) => translate(event, &registry, &tx, &devices).await,
                Ok(None) => {
                    debug!("rvoip event stream closed");
                    break;
                }
                Err(e) => {
                    warn!(error = %e, "rvoip event stream error");
                    break;
                }
            }
        }
    })
}

/// Translates a single rvoip event into Sipster's event vocabulary.
async fn translate(
    event: EndpointEvent,
    registry: &Arc<Mutex<Registry>>,
    tx: &broadcast::Sender<CallEvent>,
    devices: &DeviceSelection,
) {
    match event {
        EndpointEvent::IncomingCall(incoming) => {
            let (from, rvoip_id) = (incoming.from().to_string(), incoming.id().to_string());
            let id = {
                let mut reg = registry.lock().await;
                reg.insert(rvoip_id, Tracked::Incoming(Box::new(incoming)))
            };
            let _ = tx.send(CallEvent::IncomingCall {
                id,
                remote_uri: from,
                display_name: None,
            });
        }
        EndpointEvent::CallProgress { call_id, has_sdp, .. } => {
            let rvoip_id = call_id.to_string();
            // A 183 with SDP means early media: ringback tones, announcements
            // and IVR prompts arrive before any 200 OK. Bind the speaker now,
            // or the user hears silence through the whole announcement.
            if has_sdp {
                attach_audio_if_missing(&rvoip_id, registry, devices).await;
            }
            emit_state(rvoip_id, CallState::Ringing, registry, tx).await;
        }
        EndpointEvent::CallAnswered { call, .. } => {
            let rvoip_id = call.id().to_string();
            let id = {
                let reg = registry.lock().await;
                reg.resolve(&rvoip_id)
            };
            if let Some(id) = id {
                registry.lock().await.tracked.insert(id, Tracked::Active(call));
                // No-op when early media already bound the devices.
                attach_audio_if_missing(&rvoip_id, registry, devices).await;
                let _ = tx.send(CallEvent::StateChanged {
                    id,
                    state: CallState::Active,
                });
            }
        }
        EndpointEvent::CallEnded { call_id, reason }
        | EndpointEvent::CallFailed { call_id, reason, .. } => {
            terminate(call_id.to_string(), reason, registry, tx).await;
        }
        EndpointEvent::CallCancelled { call_id } => {
            terminate(call_id.to_string(), "cancelled".into(), registry, tx).await;
        }
        _ => debug!("unhandled rvoip event"),
    }
}

/// Binds OS audio for an already-tracked call, unless it is already bound.
///
/// Used for early media, where audio starts flowing before the call is
/// answered. Answering later is a no-op because the entry already exists.
async fn attach_audio_if_missing(
    rvoip_id: &str,
    registry: &Arc<Mutex<Registry>>,
    devices: &DeviceSelection,
) {
    // Clone the handle and release the lock before awaiting the device open,
    // which is slow and must not block the event pump's registry.
    let call = {
        let reg = registry.lock().await;
        let Some(id) = reg.resolve(rvoip_id) else {
            return;
        };
        if reg.audio.contains_key(&id) {
            return;
        }
        match reg.tracked.get(&id) {
            Some(Tracked::Active(call)) => Some((id, call.clone())),
            _ => None,
        }
    };

    let Some((id, call)) = call else {
        return;
    };
    if let Some(bound) = audio::warn_on_failure(audio::attach(&call, devices).await) {
        registry.lock().await.audio.insert(id, bound);
    }
}

async fn emit_state(
    rvoip_id: String,
    state: CallState,
    registry: &Arc<Mutex<Registry>>,
    tx: &broadcast::Sender<CallEvent>,
) {
    if let Some(id) = registry.lock().await.resolve(&rvoip_id) {
        let _ = tx.send(CallEvent::StateChanged { id, state });
    }
}

async fn terminate(
    rvoip_id: String,
    reason: String,
    registry: &Arc<Mutex<Registry>>,
    tx: &broadcast::Sender<CallEvent>,
) {
    let id = {
        let mut reg = registry.lock().await;
        let id = reg.resolve(&rvoip_id);
        if let Some(id) = id {
            reg.take(id);
        }
        id
    };
    if let Some(id) = id {
        let _ = tx.send(CallEvent::Terminated { id, reason });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-in for an rvoip call handle, which cannot be built without a
    /// live stack.
    type TestRegistry = Registry<&'static str>;

    #[test]
    fn resolve_maps_rvoip_id_to_our_call_id() {
        let mut reg = TestRegistry::default();
        let id = reg.insert("rvoip-call-1".into(), "handle");
        assert_eq!(reg.resolve("rvoip-call-1"), Some(id));
        assert_eq!(reg.resolve("unknown"), None);
    }

    /// The regression behind "hanging up doesn't work but audio stopped":
    /// `take` must hand back the call handle it removes, or the BYE is never
    /// sent while the audio binding is dropped anyway.
    #[test]
    fn take_returns_the_handle_it_removes() {
        let mut reg = TestRegistry::default();
        let id = reg.insert("rvoip-call-1".into(), "handle");
        assert_eq!(reg.take(id), Some("handle"), "hangup needs the handle back");
    }

    #[test]
    fn take_clears_every_index_for_the_call() {
        let mut reg = TestRegistry::default();
        let id = reg.insert("rvoip-call-1".into(), "handle");
        reg.take(id);
        assert_eq!(reg.resolve("rvoip-call-1"), None, "rvoip id must be forgotten");
        assert!(reg.tracked.is_empty());
        assert!(reg.audio.is_empty());
    }

    #[test]
    fn taking_an_unknown_call_is_not_an_error() {
        let mut reg = TestRegistry::default();
        assert_eq!(reg.take(CallId::new()), None);
    }

    #[test]
    fn calls_are_tracked_independently() {
        let mut reg = TestRegistry::default();
        let first = reg.insert("rvoip-1".into(), "a");
        let second = reg.insert("rvoip-2".into(), "b");
        reg.take(first);
        assert_eq!(reg.resolve("rvoip-2"), Some(second), "unrelated call survives");
        assert_eq!(reg.take(second), Some("b"));
    }
}
