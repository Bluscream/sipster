//! Headless registration smoke test against a real PBX.
//!
//! Reads the same config file the GUI does, so there is one place credentials
//! live and no environment variables to leak into a shell history or a chat
//! transcript:
//!
//! ```bash
//! cargo run -p sipster-core --example register
//! cargo run -p sipster-core --example register -- --config-file /tmp/test.toml
//! ```
//!
//! Optionally place a call by passing a target:
//! `cargo run -p sipster-core --example register -- '**9'`

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

    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = Config::path_from(&args);
    println!("config: {}", path.display());

    let config = Config::load(&path)?;
    if config.needs_setup() {
        return Err(format!(
            "no usable account in {} — run the GUI and fill in Settings",
            path.display()
        )
        .into());
    }
    let account = config.account;

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

    // The first positional argument that is not a flag or a flag's value.
    if let Some(target) = dial_target(&args) {
        println!("dialing {target}…");
        let id = engine.dial(target).await?;
        println!("call {id} placed — listen now; hanging up in 30s (Ctrl-C to stop)");
        tokio::time::sleep(Duration::from_secs(30)).await;
        println!("hanging up");
        engine.hangup(id).await?;
        // Let the BYE reach the peer before the runtime tears the sockets down.
        tokio::time::sleep(Duration::from_millis(500)).await;
    } else {
        println!("registered; idling 60s to observe refresh / inbound calls");
        tokio::time::sleep(Duration::from_secs(60)).await;
    }

    engine.unregister().await?;
    Ok(())
}

/// Picks the dial target out of argv, skipping `--config-file <PATH>` and its
/// value so the path is never mistaken for a number to call.
fn dial_target(args: &[String]) -> Option<&str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config-file" | "--config" => {
                let _ = iter.next();
            }
            other if other.starts_with('-') => {}
            other => return Some(other),
        }
    }
    None
}
