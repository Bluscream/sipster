//! OS audio device binding for active calls.
//!
//! Bridges the microphone and speaker to a call's media stream via
//! `rvoip-audio-device` (cpal underneath). Kept in core, not the UI, so any
//! frontend gets working audio for free.

use rvoip_audio_device::{AudioDirection, DeviceBridge, DeviceOptions, RunningAudio};
use rvoip_sip::EndpointCall;
use tracing::{info, warn};

use crate::error::{Error, Result};

/// Holds the OS audio streams for one call. Dropping it stops capture and
/// playback, so it is stored alongside the call and released on hangup.
pub struct CallAudio {
    _running: RunningAudio,
}

impl std::fmt::Debug for CallAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CallAudio")
    }
}

/// Attaches the configured (or default) input/output devices to `call`.
///
/// Returns `Ok(None)` when the call has no media stream yet — the caller can
/// retry once the call reaches an established state.
pub async fn attach(call: &EndpointCall, devices: &DeviceSelection) -> Result<CallAudio> {
    let stream = call
        .as_session_handle()
        .audio()
        .await
        .map_err(|e| Error::Audio(format!("no media stream for call: {e}")))?;

    let mut opts = DeviceOptions::new();
    if let Some(input) = &devices.input {
        opts = opts.with_input_device(input.clone());
    }
    if let Some(output) = &devices.output {
        opts = opts.with_output_device(output.clone());
    }

    let running = DeviceBridge::start(stream, opts)
        .map_err(|e| Error::Audio(format!("could not open audio devices: {e}")))?;
    info!("audio devices attached to call");
    Ok(CallAudio { _running: running })
}

/// Which devices to use. `None` means "system default".
#[derive(Debug, Clone, Default)]
pub struct DeviceSelection {
    pub input: Option<String>,
    pub output: Option<String>,
}

/// A selectable audio device: `(id, human readable name)`.
pub type Device = (String, String);

/// Lists available capture devices.
pub fn input_devices() -> Vec<Device> {
    rvoip_audio_device::list_devices(AudioDirection::Input)
}

/// Lists available playback devices.
pub fn output_devices() -> Vec<Device> {
    rvoip_audio_device::list_devices(AudioDirection::Output)
}

/// Logs a warning when audio could not be attached, without failing the call —
/// a call with no local audio is still better than a dropped call.
pub(crate) fn warn_on_failure(result: Result<CallAudio>) -> Option<CallAudio> {
    match result {
        Ok(audio) => Some(audio),
        Err(e) => {
            warn!(error = %e, "call established but audio devices unavailable");
            None
        }
    }
}
