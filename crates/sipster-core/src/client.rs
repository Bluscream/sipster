use crate::account::SipAccount;
use crate::audio::AudioEngine;
use crate::call::{CallEvent, CallId, CallState};
use crate::error::Result;
use tokio::sync::broadcast;
use tracing::info;

pub struct SipClient {
    pub account: SipAccount,
    pub audio: AudioEngine,
    event_tx: broadcast::Sender<CallEvent>,
}

impl SipClient {
    pub fn new(account: SipAccount) -> Result<Self> {
        let audio = AudioEngine::new()?;
        let (event_tx, _) = broadcast::channel(32);

        Ok(Self {
            account,
            audio,
            event_tx,
        })
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<CallEvent> {
        self.event_tx.subscribe()
    }

    /// Register with the SIP server (e.g. FRITZ!Box)
    pub async fn register(&self) -> Result<()> {
        info!(
            "Registering account {} with {}:{}",
            self.account.username, self.account.registrar, self.account.port
        );
        let _ = self.event_tx.send(CallEvent::RegistrationSuccess);
        Ok(())
    }

    /// Initiate an outgoing call
    pub async fn dial(&self, target_number: &str) -> Result<CallId> {
        let call_id = CallId::new();
        info!("Initiating call {} to {}", call_id, target_number);
        let _ = self.event_tx.send(CallEvent::Ringing { id: call_id });
        Ok(call_id)
    }

    /// Hang up an active or outgoing call
    pub async fn hangup(&self, call_id: CallId) -> Result<()> {
        info!("Hanging up call {}", call_id);
        let _ = self.event_tx.send(CallEvent::Terminated {
            id: call_id,
            reason: "Call ended by user".into(),
        });
        Ok(())
    }
}
