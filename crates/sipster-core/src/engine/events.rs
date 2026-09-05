//! Translating rvoip's event stream into Sipster's own.
//!
//! The engine next door decides what to do; this decides what happened. Every
//! rvoip event is matched by name, so a new one cannot be added upstream
//! without a decision being made here — a catch-all once hid inbound REFER
//! entirely.

use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use rvoip_sip::{
    EndpointCall, EndpointEvent, EndpointEvents, EndpointIncomingCall,
    EndpointRegistrationStatus,
};

use crate::audio;
use crate::call::{CallEvent, CallState, RegistrationState};

use super::{current, Devices, Registry, Tracked};

/// Spawns the task that drains rvoip events and emits [`CallEvent`]s.
pub(super) fn spawn_pump(
    mut events: EndpointEvents,
    registry: Arc<Mutex<Registry>>,
    tx: broadcast::Sender<CallEvent>,
    devices: Devices,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match events.next().await {
                Ok(Some(event)) => {
                    debug!("received rvoip endpoint event");
                    translate(event, &registry, &tx, &devices).await;
                }
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
    devices: &Devices,
) {
    match event {
        EndpointEvent::IncomingCall(incoming) => {
            on_incoming_call(incoming, registry, tx).await;
        }
        EndpointEvent::CallProgress { call_id, has_sdp, .. } => {
            on_call_progress(call_id.to_string(), has_sdp, registry, tx, devices).await;
        }
        EndpointEvent::CallAnswered { call, .. } => {
            on_call_answered(call, registry, tx, devices).await;
        }
        EndpointEvent::CallEnded { call_id, reason }
        | EndpointEvent::CallFailed { call_id, reason, .. } => {
            terminate(call_id.to_string(), reason, registry, tx).await;
        }
        EndpointEvent::CallCancelled { call_id } => {
            terminate(call_id.to_string(), "cancelled".into(), registry, tx).await;
        }
        EndpointEvent::RegistrationChanged(info) => on_registration_changed(&info, tx),

        // Everything else is informational and does not change Sipster's own
        // call state. Logged one variant at a time all the same: a bare
        // catch-all hid which event had arrived, which is how incoming REFER
        // stayed invisible for so long.
        other => log_informational(other),
    }
}

/// Logs an rvoip event that Sipster does not otherwise act on.
///
/// Hold is not tracked here because the UI reads it back from the session when
/// it needs it, so these arms exist to make the traffic visible rather than to
/// drive state.
// One flat arm per event, each of which just logs. Clippy scores a wide match
// as complexity, but splitting a dispatch table across functions to satisfy it
// would make it harder to check that every variant is covered, not easier.
#[allow(clippy::cognitive_complexity)]
fn log_informational(event: EndpointEvent) {
    match event {
        EndpointEvent::LocalHold { call_id } => {
            debug!(%call_id, "the local side put the call on hold");
        }
        EndpointEvent::LocalResume { call_id } => {
            debug!(%call_id, "the local side took the call off hold");
        }
        EndpointEvent::RemoteHold { call_id } => {
            info!(%call_id, "the remote side put us on hold");
        }
        EndpointEvent::RemoteResume { call_id } => {
            info!(%call_id, "the remote side took us off hold");
        }
        EndpointEvent::DtmfReceived { call_id, digit } => {
            info!(%call_id, %digit, "received a DTMF digit");
        }
        EndpointEvent::NetworkError { call_id, error } => {
            warn!(
                call_id = call_id.map(|id| id.to_string()).unwrap_or_default(),
                %error,
                "the SIP engine reported a network error"
            );
        }
        // Every SIP message crossing the transport. Far too noisy for anything
        // but `trace`, and invaluable there — this is the only place the raw
        // signalling is visible.
        EndpointEvent::SipTrace(trace) => {
            tracing::trace!(
                direction = ?trace.direction,
                transport = %trace.transport,
                local = %trace.local_addr,
                remote = %trace.remote_addr,
                start_line = %trace.start_line,
                "SIP"
            );
        }
        // rvoip's endpoint facade models only part of its own event set and
        // funnels the rest through `Info` as a debug string. Inbound REFER
        // arrives this way, so it is worth saying so plainly rather than
        // leaving a transfer to look like nothing happened.
        EndpointEvent::Info { call_id, message } => {
            let call_id = call_id.map(|id| id.to_string()).unwrap_or_default();
            if message.contains("ReferReceived") {
                info!(
                    %call_id,
                    %message,
                    "a transfer was requested of us; rvoip accepts it on our behalf"
                );
            } else {
                debug!(%call_id, %message, "SIP engine event");
            }
        }
        // Handled by the caller; listed so that a new rvoip event cannot be
        // added without this match failing to compile.
        EndpointEvent::IncomingCall(_)
        | EndpointEvent::CallProgress { .. }
        | EndpointEvent::CallAnswered { .. }
        | EndpointEvent::CallEnded { .. }
        | EndpointEvent::CallFailed { .. }
        | EndpointEvent::CallCancelled { .. }
        | EndpointEvent::RegistrationChanged(_) => {}
    }
}

/// Binds OS audio for an already-tracked call, unless it is already bound.
///
/// Used for early media, where audio starts flowing before the call is
/// answered. Answering later is a no-op because the entry already exists.
async fn attach_audio_if_missing(
    rvoip_id: &str,
    registry: &Arc<Mutex<Registry>>,
    devices: &Devices,
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
    let selection = current(devices);
    if let Some(bound) = audio::warn_on_failure(audio::attach(&call, &selection).await) {
        registry.lock().await.audio.insert(id, bound);
    }
}

/// Handles a provisional response (180/183) for an outbound call.
async fn on_call_progress(
    rvoip_id: String,
    has_sdp: bool,
    registry: &Arc<Mutex<Registry>>,
    tx: &broadcast::Sender<CallEvent>,
    devices: &Devices,
) {
    // A 183 with SDP means early media: ringback tones, announcements and IVR
    // prompts arrive before any 200 OK. Bind the speaker now, or the user
    // hears silence through the whole announcement.
    if has_sdp {
        attach_audio_if_missing(&rvoip_id, registry, devices).await;
    }
    emit_state(rvoip_id, CallState::Ringing, registry, tx).await;
}

/// Records a ringing inbound call and announces it.
async fn on_incoming_call(
    incoming: EndpointIncomingCall,
    registry: &Arc<Mutex<Registry>>,
    tx: &broadcast::Sender<CallEvent>,
) {
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

/// Promotes an answered outbound call to active and binds its audio.
async fn on_call_answered(
    call: EndpointCall,
    registry: &Arc<Mutex<Registry>>,
    tx: &broadcast::Sender<CallEvent>,
    devices: &Devices,
) {
    let rvoip_id = call.id().to_string();
    let Some(id) = registry.lock().await.resolve(&rvoip_id) else {
        return;
    };
    registry.lock().await.tracked.insert(id, Tracked::Active(call));
    // No-op when early media already bound the devices.
    attach_audio_if_missing(&rvoip_id, registry, devices).await;
    let _ = tx.send(CallEvent::StateChanged {
        id,
        state: CallState::Active,
    });
}

/// Publishes a registration change. This — not the return of `register()` —
/// is what the UI's status line reflects.
fn on_registration_changed(
    info: &rvoip_sip::EndpointRegistrationInfo,
    tx: &broadcast::Sender<CallEvent>,
) {
    let state = registration_state(info);
    info!(?state, "registration state changed");
    let _ = tx.send(CallEvent::Registration(state));
}

/// Translates rvoip's registration snapshot into our own state.
///
/// `Failed` carries the registrar's reason so the status line can show why —
/// a wrong password reads as a 401 loop, which is worth saying out loud.
fn registration_state(info: &rvoip_sip::EndpointRegistrationInfo) -> RegistrationState {
    match info.status {
        EndpointRegistrationStatus::Registered => RegistrationState::Registered,
        EndpointRegistrationStatus::Registering | EndpointRegistrationStatus::Unregistering => {
            RegistrationState::Registering
        }
        EndpointRegistrationStatus::Unregistered => RegistrationState::Unregistered,
        EndpointRegistrationStatus::Failed => RegistrationState::Failed(
            info.last_failure
                .clone()
                .unwrap_or_else(|| "registration rejected".into()),
        ),
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
