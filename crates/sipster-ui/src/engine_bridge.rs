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
use sipster_core::{Config, SipAccount, SipEngine};
use tokio::sync::mpsc as tokio_mpsc;

use crate::app::Message;

/// Sender for account changes, published once the stream is running.
static RECONFIGURE: OnceLock<tokio_mpsc::UnboundedSender<SipAccount>> = OnceLock::new();

/// Asks the bridge to rebuild the engine for `account`.
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

        let config = match load_config() {
            Ok(config) => config,
            Err(err) => {
                let _ = output.send(Message::EngineFailed(err)).await;
                return;
            }
        };
        let devices = DeviceSelection {
            input: config.audio.input.clone(),
            output: config.audio.output.clone(),
        };
        let Some(mut account) = config.accounts.into_iter().next() else {
            let _ = output
                .send(Message::EngineFailed(
                    "no SIP account configured — open Settings, or set SIPSTER_* in the environment"
                        .into(),
                ))
                .await;
            return;
        };

        // Outer loop: one iteration per engine. A reconfigure drops the engine
        // — which aborts its event pump and releases the SIP port — and starts
        // the next iteration with the new account.
        loop {
            let engine = match Box::pin(connect(account.clone(), devices.clone())).await {
                Ok(engine) => Arc::new(engine),
                Err(err) => {
                    if output.send(Message::EngineFailed(err)).await.is_err() {
                        return;
                    }
                    // Stay alive so the user can correct the account in
                    // Settings; wait for the next attempt rather than exiting.
                    match reconfigure_rx.recv().await {
                        Some(next) => {
                            account = next;
                            continue;
                        }
                        None => return,
                    }
                }
            };

            let mut events = engine.subscribe();
            if output.send(Message::EngineReady(engine.clone())).await.is_err() {
                return;
            }

            // Kick off registration in the background; its result surfaces as a
            // Registration CallEvent through the same stream.
            {
                let engine = engine.clone();
                tokio::spawn(async move {
                    let _ = engine.register().await;
                });
            }

            let next_account = pump(&mut output, &mut events, &mut ipc_rx, &mut reconfigure_rx).await;

            // Unregister politely before dropping the endpoint, so the PBX
            // stops sending us calls immediately instead of waiting out the
            // registration expiry.
            let _ = engine.unregister().await;
            drop(events);
            drop(engine);

            match next_account {
                Some(next) => account = next,
                None => return,
            }
        }
    })
}

/// Forwards engine events and IPC commands until an account change arrives.
///
/// Returns the new account to rebuild with, or `None` when the app is shutting
/// down and the loop should end.
async fn pump(
    output: &mut mpsc::Sender<Message>,
    events: &mut tokio::sync::broadcast::Receiver<sipster_core::CallEvent>,
    ipc_rx: &mut tokio_mpsc::UnboundedReceiver<sipster_core::ipc::Command>,
    reconfigure_rx: &mut tokio_mpsc::UnboundedReceiver<SipAccount>,
) -> Option<SipAccount> {
    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        if output.send(Message::Call(event)).await.is_err() {
                            return None;
                        }
                    }
                    // Lagged: the UI fell behind; skip missed events and continue.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
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
    let _ = engine.set_devices(devices).await;
    Ok(engine)
}

/// Loads the config file, falling back to the environment for the account.
fn load_config() -> Result<Config, String> {
    Config::load_or_env(Config::default_path()).map_err(|e| e.to_string())
}

/// Shared engine handle used by `app` to drive calls.
pub type EngineHandle = Arc<SipEngine>;
