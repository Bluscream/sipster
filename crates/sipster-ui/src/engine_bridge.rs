//! Bridges the async [`SipEngine`] into Iced's subscription model.
//!
//! Iced subscriptions are built from a bare `fn` pointer and cannot capture
//! state, so the subscription itself owns the engine: it loads config, builds
//! the engine, registers, and streams events. The engine handle is handed back
//! to the app as the first [`Message::EngineReady`] so `update` can drive calls.
//!
//! Because the stream owns the engine, changing the account cannot be done by
//! swapping the handle in `update` — the old endpoint would keep its sockets
//! and its registration. [`reconfigure`] instead asks this loop to tear the
//! engine down and build a new one, which then arrives as a fresh
//! `EngineReady`.

use std::sync::{Arc, OnceLock};

use iced::futures::channel::mpsc;
use iced::futures::SinkExt;
use iced::stream;
use sipster_core::audio::DeviceSelection;
use sipster_core::{SipAccount, SipEngine};
use tokio::sync::mpsc as tokio_mpsc;

use crate::app::Message;

/// Sender for account changes, published once the stream is running.
static RECONFIGURE: OnceLock<tokio_mpsc::UnboundedSender<Vec<SipAccount>>> = OnceLock::new();

/// Asks the bridge to rebuild its engines for `accounts`.
///
/// Silently does nothing before the stream has started, which can only happen
/// in the moments before the first `EngineReady` — there is nothing to
/// reconfigure yet, and the account will be read from the config file anyway.
pub fn reconfigure(accounts: Vec<SipAccount>) {
    if let Some(tx) = RECONFIGURE.get() {
        let _ = tx.send(accounts);
    }
}

/// Builds the engine and yields `Message`s for the Iced application loop.
///
/// This is a `fn()` pointer (no captures) so it satisfies `Subscription::run`.
/// It takes the primary-instance state exactly once via
/// [`crate::take_primary_state`]; subsequent subscription calls (from Iced
/// re-rendering) get `None`, which is fine because the stream keeps running
/// inside the Iced executor.
pub fn run() -> impl iced::futures::Stream<Item = Message> {
    let primary_state = crate::take_primary_state();

    stream::channel(64, |mut output: mpsc::Sender<Message>| async move {
        let (ipc_tx, mut ipc_rx) = tokio_mpsc::unbounded_channel();
        let (reconfigure_tx, mut reconfigure_rx) = tokio_mpsc::unbounded_channel();
        let _ = RECONFIGURE.set(reconfigure_tx);

        if let Some(primary) = primary_state {
            if let Some(initial_cmd) = primary.initial_command {
                let _ = ipc_tx.send(initial_cmd);
            }
            tokio::spawn(sipster_core::ipc::serve(primary.listener, ipc_tx));
        }

        let (_, config) = crate::startup_config();
        let devices = DeviceSelection {
            input: config.audio.input.clone(),
            output: config.audio.output.clone(),
        };

        let mut accounts = enabled_accounts(config);
        if accounts.is_empty() {
            // Nothing configured yet. Report it and wait — returning here
            // would end the stream and close the reconfigure channel, so
            // entering an account in Settings could never start anything and
            // a fresh install would be stuck until the next launch.
            let _ = output
                .send(Message::EngineFailed(
                    "no SIP account configured — open Settings to add one".into(),
                ))
                .await;
            let Some(next) = reconfigure_rx.recv().await else {
                return;
            };
            accounts = next;
        }

        // Outer loop: one iteration per set of engines. A reconfigure drops
        // them all — which aborts their event pumps and releases their SIP
        // ports — and starts again with the new accounts.
        loop {
            let mut engines = Vec::new();
            // Every account's events funnel into one channel, tagged with the
            // account they came from, so the pump below stays a single select
            // however many accounts there are.
            let (tagged_tx, mut tagged_rx) = tokio_mpsc::unbounded_channel();

            for (index, account) in accounts.iter().enumerate() {
                match Box::pin(connect(account.clone(), devices.clone())).await {
                    Ok(engine) => {
                        let engine = Arc::new(engine);
                        forward_events(index, &engine, tagged_tx.clone());
                        if output
                            .send(Message::EngineReady(index, engine.clone()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                        {
                            // Registration result arrives as a tagged event.
                            let engine = engine.clone();
                            tokio::spawn(async move {
                                let _ = engine.register().await;
                            });
                        }
                        engines.push(engine);
                    }
                    Err(err) => {
                        // One bad account must not stop the others: a wrong
                        // password on a second line should not take the line
                        // that works down with it.
                        if output.send(Message::EngineFailed(err)).await.is_err() {
                            return;
                        }
                    }
                }
            }
            drop(tagged_tx);

            let next = pump(&mut output, &mut tagged_rx, &mut ipc_rx, &mut reconfigure_rx).await;

            // Unregister politely before dropping the endpoints, so the PBX
            // stops sending us calls immediately instead of waiting out the
            // registration expiry.
            for engine in &engines {
                if let Err(e) = engine.unregister().await {
                    // Not fatal, but the PBX will keep sending us calls until
                    // the registration expires — worth knowing about.
                    tracing::warn!(error = %e, "could not unregister cleanly");
                }
            }
            drop(engines);

            match next {
                Some(next) => accounts = next,
                None => return,
            }
        }
    })
}

/// The accounts that should be registered, in config order.
///
/// The index into this list is what tags every event, and what the UI uses to
/// name the account a call belongs to.
fn enabled_accounts(config: &sipster_core::Config) -> Vec<SipAccount> {
    config
        .accounts
        .iter()
        .filter(|account| account.enabled)
        .cloned()
        .collect()
}

/// Spawns a task that tags one engine's events with its account index.
fn forward_events(
    index: usize,
    engine: &Arc<SipEngine>,
    tx: tokio_mpsc::UnboundedSender<(usize, sipster_core::CallEvent)>,
) {
    let mut events = engine.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    if tx.send((index, event)).is_err() {
                        return;
                    }
                }
                // Lagged: the UI fell behind; skip missed events and continue.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

/// Forwards engine events and IPC commands until an account change arrives.
///
/// Returns the new accounts to rebuild with, or `None` when the app is
/// shutting down and the loop should end.
async fn pump(
    output: &mut mpsc::Sender<Message>,
    events: &mut tokio_mpsc::UnboundedReceiver<(usize, sipster_core::CallEvent)>,
    ipc_rx: &mut tokio_mpsc::UnboundedReceiver<sipster_core::ipc::Command>,
    reconfigure_rx: &mut tokio_mpsc::UnboundedReceiver<Vec<SipAccount>>,
) -> Option<Vec<SipAccount>> {
    loop {
        tokio::select! {
            event = events.recv() => {
                let (index, event) = event?;
                if output.send(Message::Call(index, event)).await.is_err() {
                    return None;
                }
            }
            cmd = ipc_rx.recv() => {
                if let Some(cmd) = cmd {
                    if output.send(Message::Ipc(cmd)).await.is_err() {
                        return None;
                    }
                }
            }
            accounts = reconfigure_rx.recv() => {
                return accounts;
            }
        }
    }
}

/// Connects an engine for `account` with `devices` already selected.
async fn connect(account: SipAccount, devices: DeviceSelection) -> Result<SipEngine, String> {
    let engine = Box::pin(SipEngine::connect(account))
        .await
        .map_err(|e| e.to_string())?;
    // Apply the saved devices before any call can arrive, so the first call
    // already uses them rather than the system default.
    if let Err(e) = engine.set_devices(devices).await {
        // The call still works, on the system default rather than the chosen
        // microphone and speaker.
        tracing::warn!(error = %e, "could not apply the saved audio devices");
    }
    Ok(engine)
}

/// Shared engine handle used by `app` to drive calls.
pub type EngineHandle = Arc<SipEngine>;
