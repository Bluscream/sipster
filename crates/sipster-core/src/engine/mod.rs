//! The Sipster calling engine: a safe, UI-agnostic wrapper over rvoip's
//! softphone `Endpoint`.
//!
//! Responsibilities that belong to *any* frontend live here — registration,
//! placing/answering/ending calls, and translating rvoip's event stream into
//! Sipster's own [`CallEvent`]s on a broadcast channel. A UI subscribes and
//! renders; it never touches rvoip types directly.

mod events;

use events::spawn_pump;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use rvoip_sip::{EndpointCall, EndpointControl, EndpointIncomingCall};

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
    /// Swappable so the settings window can change devices without a restart.
    /// Read-and-cloned at each attach; never held across an await.
    devices: Devices,
    pump: JoinHandle<()>,
}

/// The device selection shared between the engine and its event pump.
pub(crate) type Devices = Arc<std::sync::RwLock<DeviceSelection>>;

/// Reads the current selection. Recovers from a poisoned lock rather than
/// panicking: a failed audio attach elsewhere must not disable audio forever.
pub(crate) fn current(devices: &Devices) -> DeviceSelection {
    devices
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

impl SipEngine {
    /// Builds an endpoint for `account` and starts translating its events.
    ///
    /// Does not register yet — call [`register`](Self::register) once a
    /// subscriber is listening, so no registration event is missed.
    pub async fn connect(account: SipAccount) -> Result<Self> {
        let endpoint = crate::build_endpoint(&account).await?;
        let (control, events) = endpoint.split();

        let (event_tx, _) = broadcast::channel(64);
        let registry = Arc::new(Mutex::new(Registry::default()));
        let devices: Devices = Arc::new(std::sync::RwLock::new(DeviceSelection::default()));
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

    /// The devices calls currently use.
    pub fn devices(&self) -> DeviceSelection {
        current(&self.devices)
    }

    /// Switches microphone/speaker without restarting.
    ///
    /// Calls started afterwards pick the new devices up automatically. Calls
    /// already up are re-bound here, because a settings change the user cannot
    /// hear until the next call is not a settings change they will believe in.
    ///
    /// # Errors
    ///
    /// Never returns `Err` today; re-binding failures are logged per call and
    /// leave that call on its previous devices rather than dropping it.
    pub async fn set_devices(&self, selection: DeviceSelection) -> Result<()> {
        {
            let mut guard = self
                .devices
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *guard == selection {
                return Ok(());
            }
            *guard = selection;
        }
        info!("audio devices changed; re-binding active calls");
        self.rebind_active_audio().await;
        Ok(())
    }

    /// Drops and re-creates the OS audio binding for every established call.
    async fn rebind_active_audio(&self) {
        let selection = current(&self.devices);

        // Collect handles under the lock, then open devices without holding it.
        let calls: Vec<(CallId, EndpointCall)> = {
            let reg = self.registry.lock().await;
            reg.tracked
                .iter()
                .filter_map(|(id, tracked)| match tracked {
                    Tracked::Active(call) => Some((*id, call.clone())),
                    Tracked::Incoming(_) => None,
                })
                .collect()
        };

        for (id, call) in calls {
            // Drop the old binding first: the previous device must be released
            // before the new one is opened, or an exclusive device would fail.
            self.registry.lock().await.audio.remove(&id);
            if let Some(bound) = audio::warn_on_failure(audio::attach(&call, &selection).await) {
                self.registry.lock().await.audio.insert(id, bound);
            }
        }
    }

    /// Register with the configured registrar, emitting registration state.
    pub async fn register(&self) -> Result<()> {
        let _ = self
            .event_tx
            .send(CallEvent::Registration(RegistrationState::Registering));
        match self.control.register().await {
            Ok(()) => {
                // Deliberately *not* reporting Registered here. This only says
                // the REGISTER was accepted for sending; the registrar can
                // still reject it, and with a wrong password it answers 401
                // forever. The authoritative answer arrives asynchronously as
                // a RegistrationChanged event, which `translate` forwards.
                // Claiming success here showed "Registered" for an account
                // that never authenticated.
                info!(account = %self.account.label(), "REGISTER sent");
                Ok(())
            }
            Err(e) => {
                let reason = e.to_string();
                warn!(account = %self.account.label(), %reason, "registration failed");
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
                let selection = current(&self.devices);
                let bound = audio::warn_on_failure(audio::attach(&call, &selection).await);
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

    /// Sends one DTMF digit to the far end of an active call.
    ///
    /// This is what drives a phone menu: the local feedback tone the dialpad
    /// plays is audible only here, and reaches nobody.
    ///
    /// Only `0-9`, `*`, `#` and `A-D` are valid RFC 4733 events; anything else
    /// is rejected rather than sent, because a `+` typed into the dial field
    /// is part of a number, not a tone.
    ///
    /// # Errors
    ///
    /// When the id is not an established call, or the digit is not dialable.
    pub async fn send_dtmf(&self, id: CallId, digit: char) -> Result<()> {
        if !is_dtmf_digit(digit) {
            return Err(Error::Config(format!("{digit} is not a DTMF digit")));
        }

        // Borrow rather than take: the call carries on after the tone.
        let reg = self.registry.lock().await;
        match reg.tracked.get(&id) {
            Some(Tracked::Active(call)) => call
                .send_dtmf(digit)
                .await
                .map_err(|e| Error::Sip(format!("could not send DTMF {digit}: {e}"))),
            _ => Err(Error::UnknownCall(id)),
        }
    }

    /// Puts an established call on hold, or takes it off again.
    ///
    /// Hold is a re-INVITE that stops the media flowing; the call stays up.
    ///
    /// # Errors
    ///
    /// When the id is not an established call, or the far end refuses.
    pub async fn set_hold(&self, id: CallId, hold: bool) -> Result<()> {
        let reg = self.registry.lock().await;
        let Some(Tracked::Active(call)) = reg.tracked.get(&id) else {
            return Err(Error::UnknownCall(id));
        };
        let result = if hold { call.hold().await } else { call.resume().await };
        result.map_err(|e| {
            let what = if hold { "hold" } else { "resume" };
            Error::Sip(format!("could not {what} the call: {e}"))
        })
    }

    /// Whether a call is currently held.
    pub async fn is_on_hold(&self, id: CallId) -> bool {
        let reg = self.registry.lock().await;
        match reg.tracked.get(&id) {
            Some(Tracked::Active(call)) => call.as_session_handle().is_on_hold().await,
            _ => false,
        }
    }

    /// Hands an established call to `target` and drops out of it (RFC 5589
    /// blind transfer).
    ///
    /// Blind, so it completes as soon as the far end accepts the REFER —
    /// there is no consultation call and no way back if the target does not
    /// answer. That is the transfer people mean when they say "put them
    /// through".
    ///
    /// # Errors
    ///
    /// When the id is not an established call, the target is empty, or the
    /// far end refuses the REFER.
    pub async fn transfer_blind(&self, id: CallId, target: &str) -> Result<()> {
        let target = transfer_target(target)?;
        // Passed through as given, the same as `dial`: rvoip turns a bare
        // extension into a URI against the registrar.
        let reg = self.registry.lock().await;
        let Some(Tracked::Active(call)) = reg.tracked.get(&id) else {
            return Err(Error::UnknownCall(id));
        };
        // Sent through the REFER builder rather than `transfer()` so a
        // `Referred-By` can be attached. Without it a FRITZ!Box answers
        // 429 "Provide Referrer Identity" (RFC 3892) and the transfer never
        // reaches the other party.
        let referred_by = format!("<sip:{}@{}>", self.account.effective_auth_user(), self.account.registrar);
        call.as_session_handle()
            .refer(target)
            .with_referred_by(referred_by)
            .send()
            .await
            .map_err(|e| Error::Sip(format!("could not transfer to {target}: {}", detail(&e))))
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
            .field("account", &self.account.label())
            .finish_non_exhaustive()
    }
}

impl Drop for SipEngine {
    fn drop(&mut self) {
        self.pump.abort();
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

/// The text of an rvoip error, with its own redaction undone.
///
/// `SessionError` renders every detail as `redacted(bytes=N)` — a policy meant
/// for operator-facing surfaces, applied unconditionally in `Display` with no
/// way to switch it off. It left a local log saying only that something had
/// failed and how long the explanation was, which is what "the transfer button
/// does nothing" looked like from outside.
///
/// The variants carry their text in public fields, so it is read from there
/// instead. Nothing in it is secret: these are dialog and media states, and
/// the credentials that would matter are redacted where they are stored.
fn detail(error: &rvoip_sip::SessionError) -> String {
    use rvoip_sip::SessionError as E;
    match error {
        E::SessionNotFound(text)
        | E::InvalidTransition(text)
        | E::DialogError(text)
        | E::MediaError(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Validates a blind-transfer target.
///
/// Transfer hands the call away irreversibly, so an empty or whitespace-only
/// target is refused here rather than sent — a REFER to nowhere drops the call
/// with no way back.
///
/// # Errors
///
/// When the target is empty once trimmed.
pub fn transfer_target(target: &str) -> Result<&str> {
    let target = target.trim();
    if target.is_empty() {
        return Err(Error::Config("no transfer target given".into()));
    }
    Ok(target)
}

/// Whether `digit` is a sendable DTMF event.
///
/// RFC 4733 defines events for the twelve keys of a phone plus the A-D tones;
/// `+` and anything else on the dialpad is dial-string syntax, not a tone.
#[must_use]
pub fn is_dtmf_digit(digit: char) -> bool {
    digit.is_ascii_digit() || matches!(digit, '*' | '#' | 'A'..='D' | 'a'..='d')
}

#[cfg(test)]
mod dtmf_tests {
    use super::{is_dtmf_digit, transfer_target};

    /// A blind transfer cannot be undone, so a blank target must never reach
    /// the wire.
    #[test]
    fn a_blank_transfer_target_is_refused() {
        assert!(transfer_target("").is_err());
        assert!(transfer_target("   ").is_err());
        assert!(transfer_target("\t\n").is_err());
    }

    #[test]
    fn a_transfer_target_is_trimmed_but_otherwise_passed_through() {
        assert_eq!(transfer_target("  **610 ").expect("valid"), "**610");
        assert_eq!(
            transfer_target("sip:bob@example.com").expect("valid"),
            "sip:bob@example.com"
        );
    }

    #[test]
    fn the_twelve_phone_keys_and_the_abcd_tones_are_sendable() {
        for digit in "0123456789*#".chars() {
            assert!(is_dtmf_digit(digit), "{digit} is a phone key");
        }
        for digit in "ABCDabcd".chars() {
            assert!(is_dtmf_digit(digit), "{digit} is an RFC 4733 tone");
        }
    }

    /// `+` is on our dialpad but is dial-string syntax, not a tone; sending it
    /// would be a protocol error rather than a keypress the far end hears.
    #[test]
    fn dial_string_characters_are_not_tones() {
        for digit in ['+', ' ', '-', '(', 'x', 'Z', '\n'] {
            assert!(!is_dtmf_digit(digit), "{digit:?} is not a DTMF event");
        }
    }
}
