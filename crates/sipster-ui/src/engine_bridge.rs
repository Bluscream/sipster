//! Bridges the async [`SipEngine`] into Iced's subscription model.
//!
//! Iced subscriptions are built from a bare `fn` pointer and cannot capture
//! state, so the subscription itself owns the engine: it loads config, builds
//! the engine, registers, and streams events. The engine handle is handed back
//! to the app as the first [`Message::EngineReady`] so `update` can drive calls.

use std::sync::Arc;

use iced::futures::channel::mpsc;
use iced::futures::SinkExt;
use iced::stream;
use sipster_core::{Config, SipEngine};

use crate::app::Message;

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
        let (ipc_tx, mut ipc_rx) = tokio::sync::mpsc::unbounded_channel();

        if let Some(primary) = primary_state {
            if let Some(initial_cmd) = primary.initial_command {
                let _ = ipc_tx.send(initial_cmd);
            }
            tokio::spawn(sipster_core::ipc::serve(primary.listener, ipc_tx));
        }

        // Boxed: building the whole rvoip endpoint makes this future large
        // enough that clippy (rightly) does not want it on the stack.
        let engine = match Box::pin(bootstrap()).await {
            Ok(engine) => Arc::new(engine),
            Err(err) => {
                let _ = output.send(Message::EngineFailed(err)).await;
                return;
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

        loop {
            tokio::select! {
                event = events.recv() => {
                    match event {
                        Ok(event) => {
                            if output.send(Message::Call(event)).await.is_err() {
                                break;
                            }
                        }
                        // Lagged: the UI fell behind; skip missed events and continue.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                cmd = ipc_rx.recv() => {
                    if let Some(cmd) = cmd {
                        if output.send(Message::Ipc(cmd)).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    })
}


/// Loads config (env first, then `sipster.toml`) and connects the engine.
async fn bootstrap() -> Result<SipEngine, String> {
    let config = load_config()?;
    let account = config
        .accounts
        .into_iter()
        .next()
        .ok_or_else(|| "no SIP account configured (set SIPSTER_* env or sipster.toml)".to_string())?;
    Box::pin(SipEngine::connect(account))
        .await
        .map_err(|e| e.to_string())
}

fn load_config() -> Result<Config, String> {
    if let Ok(config) = Config::from_env() {
        return Ok(config);
    }
    let path = config_path();
    Config::load(&path).map_err(|e| e.to_string())
}

/// `$XDG_CONFIG_HOME/sipster/sipster.toml`, falling back to `./sipster.toml`.
fn config_path() -> std::path::PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return std::path::Path::new(&xdg).join("sipster/sipster.toml");
    }
    if let Ok(home) = std::env::var("HOME") {
        return std::path::Path::new(&home).join(".config/sipster/sipster.toml");
    }
    std::path::PathBuf::from("sipster.toml")
}

/// Shared engine handle used by `app` to drive calls.
pub type EngineHandle = Arc<SipEngine>;
