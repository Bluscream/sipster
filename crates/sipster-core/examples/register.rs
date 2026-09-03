//! Headless registration smoke test against a real PBX.
//!
//! Credentials come from the environment so they never enter a repo file or a
//! chat transcript:
//!
//! ```bash
//! export SIPSTER_REGISTRAR=fritz.box
//! export SIPSTER_USERNAME=620
//! export SIPSTER_PASSWORD='...'      # note the leading space to skip shell history
//! cargo run -p sipster-core --example register
//! ```
//!
//! Optionally place a call by passing a target:
//! `cargo run -p sipster-core --example register -- **9`

use std::time::Duration;

use sipster_core::{CallEvent, Config, SipEngine};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sipster_core=debug".into()),
        )
        .init();

    let account = Config::from_env()?
        .accounts
        .into_iter()
        .next()
        .ok_or("no account configured")?;

    println!("account: {account:?}"); // password is redacted by Debug
    println!("connecting…");

    let engine = Box::pin(SipEngine::connect(account)).await?;
    let mut events = engine.subscribe();

    // Drain events in the background so we see everything the engine reports.
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            match event {
                CallEvent::Registration(state) => println!("  [registration] {state:?}"),
                CallEvent::IncomingCall { remote_uri, .. } => {
                    println!("  [incoming] from {remote_uri}");
                }
                CallEvent::StateChanged { id, state } => {
                    println!("  [call {id}] {state:?}");
                }
                CallEvent::Terminated { id, reason } => {
                    println!("  [call {id}] terminated: {reason}");
                }
            }
        }
    });

    match engine.register().await {
        Ok(()) => println!("REGISTER succeeded"),
        Err(e) => {
            eprintln!("REGISTER failed: {e}");
            return Err(e.into());
        }
    }

    if let Some(target) = std::env::args().nth(1) {
        println!("dialing {target}…");
        let id = engine.dial(&target).await?;
        println!("call {id} placed; waiting 30s (Ctrl-C to stop)");
        tokio::time::sleep(Duration::from_secs(30)).await;
        engine.hangup(id).await?;
    } else {
        println!("registered; idling 60s to observe refresh / inbound calls");
        tokio::time::sleep(Duration::from_mins(1)).await;
    }

    engine.unregister().await?;
    Ok(())
}
