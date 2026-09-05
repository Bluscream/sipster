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
static RECONFIGURE: OnceLock<tokio_mpsc::UnboundedSender<SipAccount>> = OnceLock::new();

/// Asks the bridge to rebuild its engine for `account`.
///
/// Silently does nothing before the stream has started, which can only happen
/// in the moments before the first `EngineReady` — there is nothing to
/// reconfigure yet, and the account will be read from the config file anyway.
pub fn reconfigure(account: SipAccount) {
    if let Some(tx) = RECONFIGURE.get() {
        let _ = tx.send(account);
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

        let mut account = config.account.clone();
        while !account.enabled || account.validate().is_err() {
            // Nothing usable configured yet. Report it and wait — returning
            // here would end the stream and close the reconfigure channel, so
            // entering an account in Settings could never start anything and a
            // fresh install would be stuck until the next launch.
            let _ = output
                .send(Message::EngineFailed(
                    "no SIP account configured — open Settings to add one".into(),
                ))
                .await;
            let Some(next) = reconfigure_rx.recv().await else {
                return;
            };
            account = next;
        }

        // Outer loop: one iteration per engine. A reconfigure drops it — which
        // aborts its event pump and releases the SIP port — and starts again
        // with the new account.
        loop {
            let (event_tx, mut event_rx) = tokio_mpsc::unbounded_channel();
            let mut engine = None;

            match Box::pin(connect(account.clone(), devices.clone())).await {
                Ok(connected) => {
                    let connected = Arc::new(connected);
                    forward_events(&connected, event_tx.clone());
                    if output
                        .send(Message::EngineReady(connected.clone()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    {
                        // The registration result arrives as an event.
                        let connected = connected.clone();
                        tokio::spawn(async move {
                            let _ = connected.register().await;
                        });
                    }
                    engine = Some(connected);
                }
                Err(err) => {
                    // Report and keep the loop alive, so the settings window
                    // can still fix a wrong password without a restart.
                    if output.send(Message::EngineFailed(err)).await.is_err() {
                        return;
                    }
                }
            }
            drop(event_tx);

            let next = pump(&mut output, &mut event_rx, &mut ipc_rx, &mut reconfigure_rx).await;

            // Unregister politely before dropping the endpoint, so the PBX
            // stops sending us calls immediately instead of waiting out the
            // registration expiry.
            if let Some(engine) = &engine {
                if let Err(e) = engine.unregister().await {
                    // Not fatal, but the PBX will keep sending us calls until
                    // the registration expires — worth knowing about.
                    tracing::warn!(error = %e, "could not unregister cleanly");
                }
            }
            drop(engine);

            match next {
                Some(next) => account = next,
                None => return,
            }
        }
    })
}

/// Spawns a task that forwards one engine's events to the application.
fn forward_events(
    engine: &Arc<SipEngine>,
    tx: tokio_mpsc::UnboundedSender<sipster_core::CallEvent>,
) {
    let mut events = engine.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    if tx.send(event).is_err() {
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
    events: &mut tokio_mpsc::UnboundedReceiver<sipster_core::CallEvent>,
    ipc_rx: &mut tokio_mpsc::UnboundedReceiver<sipster_core::ipc::Command>,
    reconfigure_rx: &mut tokio_mpsc::UnboundedReceiver<SipAccount>,
) -> Option<SipAccount> {
    loop {
        tokio::select! {
            event = events.recv() => {
                if output.send(Message::Call(event?)).await.is_err() {
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
            account = reconfigure_rx.recv() => {
                return account;
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
