//! OS audio device binding for active calls.
//!
//! Bridges the microphone and speaker to a call's media stream via
//! `rvoip-audio-device` (cpal underneath). Kept in core, not the UI, so any
//! frontend gets working audio for free.

pub mod pipewire;

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

    // A PipeWire node is not an ALSA PCM and cpal cannot open one, so those
    // selections open the server's default PCM here and are moved onto the
    // chosen device once the stream exists. See [`pipewire`].
    let mut opts = DeviceOptions::new();
    if let Some(input) = &devices.input {
        opts = opts.with_input_device(cpal_selector(input));
    }
    if let Some(output) = &devices.output {
        opts = opts.with_output_device(cpal_selector(output));
    }

    let running = DeviceBridge::start(stream, opts)
        .map_err(|e| Error::Audio(format!("could not open audio devices: {e}")))?;
    info!("audio devices attached to call");

    route_pipewire(devices);
    Ok(CallAudio { _running: running })
}

/// The PCM name cpal should open for a selection.
fn cpal_selector(id: &str) -> String {
    if pipewire::is_node(id) {
        pipewire::SERVER_PCM.to_string()
    } else {
        id.to_string()
    }
}

/// Moves the call's streams onto the selected `PipeWire` devices, if any.
///
/// Spawned rather than awaited: the call already has audio on the default
/// device, so the move is a preference, not a prerequisite. Awaiting it held
/// up call setup by up to a second.
fn route_pipewire(devices: &DeviceSelection) {
    let capture = devices.input.as_deref().and_then(pipewire::node_name).map(str::to_owned);
    let playback = devices.output.as_deref().and_then(pipewire::node_name).map(str::to_owned);
    if capture.is_none() && playback.is_none() {
        return;
    }
    tokio::task::spawn_blocking(move || {
        pipewire::route_when_ready(capture.as_deref(), playback.as_deref());
    });
}

/// Which devices to use. `None` means "system default".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceSelection {
    pub input: Option<String>,
    pub output: Option<String>,
}

/// A selectable audio device: `(id, human readable name)`.
pub type Device = (String, String);

/// Lists available capture devices, filtered and named for humans.
///
/// `PipeWire`'s own nodes come first when it is running, because they are the
/// only entries that name a real microphone rather than "the default one".
pub fn input_devices() -> Vec<Device> {
    let mut devices = pipewire::devices(pipewire::Direction::Capture);
    devices.extend(usable_devices(rvoip_audio_device::list_devices(
        AudioDirection::Input,
    )));
    devices
}

/// Lists available playback devices, filtered and named for humans.
pub fn output_devices() -> Vec<Device> {
    let mut devices = pipewire::devices(pipewire::Direction::Playback);
    devices.extend(usable_devices(rvoip_audio_device::list_devices(
        AudioDirection::Output,
    )));
    devices
}

/// Reduces raw ALSA enumeration to devices worth offering, and gives them
/// readable names.
///
/// cpal enumerates every ALSA plugin permutation, which on a normal desktop is
/// dozens of entries like `surround71:CARD=PCH,DEV=0` and `dmix:CARD=HDMI,DEV=9`
/// — and, worse, `hw:` entries that take exclusive access to the card. On a
/// `PipeWire` or `PulseAudio` system the sound server already holds the hardware,
/// so picking one of those fails with "device busy". Offering them is offering
/// a broken choice.
///
/// What survives: the sound server's own device, the ALSA default, and the
/// format-converting `plughw:` wrappers, which are the ones that actually work.
fn usable_devices(raw: Vec<Device>) -> Vec<Device> {
    let mut devices: Vec<Device> = raw
        .into_iter()
        .filter(|(id, _)| is_usable(id))
        .map(|(id, _)| {
            let label = friendly_name(&id);
            (id, label)
        })
        .collect();

    // Stable, useful order: the sound server first, then the rest by name.
    devices.sort_by(|a, b| {
        let rank = |id: &str| u8::from(!is_sound_server(id));
        rank(&a.0).cmp(&rank(&b.0)).then_with(|| a.1.cmp(&b.1))
    });
    devices.dedup_by(|a, b| a.1 == b.1);
    devices
}

/// Whether this id routes through a sound server rather than the raw card.
fn is_sound_server(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.starts_with("pipewire") || id.starts_with("pulse") || id == "default"
}

fn is_usable(id: &str) -> bool {
    if is_sound_server(id) {
        return true;
    }
    // `plughw:` converts sample formats and can share a card; `hw:` cannot.
    // Everything else is an ALSA routing plugin the user has no reason to pick.
    id.starts_with("plughw:") || id.starts_with("sysdefault:")
}

/// Turns `plughw:CARD=PCH,DEV=0` into `PCH (device 0)`.
fn friendly_name(id: &str) -> String {
    if is_sound_server(id) {
        return match id.split(':').next().unwrap_or(id) {
            "pipewire" => "PipeWire".to_string(),
            "pulse" => "PulseAudio".to_string(),
            _ => "ALSA default".to_string(),
        };
    }

    let card = id
        .split("CARD=")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .unwrap_or(id);
    let device = id.split("DEV=").nth(1).and_then(|d| d.parse::<u32>().ok());

    match device {
        Some(0) | None => card.to_string(),
        Some(n) => format!("{card} (device {n})"),
    }
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

#[cfg(test)]
mod tests {
    use super::{friendly_name, usable_devices};

    fn raw(ids: &[&str]) -> Vec<super::Device> {
        ids.iter().map(|id| ((*id).to_string(), (*id).to_string())).collect()
    }

    /// The real enumeration from a desktop running `PipeWire`: mostly ALSA
    /// plugin permutations that nobody would choose on purpose.
    #[test]
    fn drops_the_alsa_plugin_noise() {
        let devices = usable_devices(raw(&[
            "hw:CARD=PCH,DEV=0",
            "plughw:CARD=PCH,DEV=0",
            "surround71:CARD=PCH,DEV=0",
            "dmix:CARD=HDMI,DEV=9",
            "dsnoop:CARD=PCH,DEV=2",
            "hdmi:CARD=HDMI,DEV=1",
            "front:CARD=PCH,DEV=0",
        ]));
        let ids: Vec<&str> = devices.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["plughw:CARD=PCH,DEV=0"]);
    }

    /// `hw:` takes exclusive access to the card, which fails whenever a sound
    /// server holds it — which is always, on a normal desktop.
    #[test]
    fn exclusive_hw_devices_are_not_offered() {
        let devices = usable_devices(raw(&["hw:CARD=PCH,DEV=0"]));
        assert!(devices.is_empty());
    }

    #[test]
    fn the_sound_server_comes_first() {
        let devices = usable_devices(raw(&[
            "plughw:CARD=PCH,DEV=0",
            "pipewire",
            "plughw:CARD=HDMI,DEV=3",
        ]));
        assert_eq!(devices[0].1, "PipeWire");
    }

    #[test]
    fn names_are_readable() {
        assert_eq!(friendly_name("plughw:CARD=PCH,DEV=0"), "PCH");
        assert_eq!(friendly_name("plughw:CARD=HDMI,DEV=3"), "HDMI (device 3)");
        assert_eq!(friendly_name("pipewire"), "PipeWire");
        assert_eq!(friendly_name("pulse"), "PulseAudio");
        assert_eq!(friendly_name("default"), "ALSA default");
    }

    /// Two ids that render to the same label would look like duplicates.
    #[test]
    fn duplicate_labels_are_collapsed() {
        let devices = usable_devices(raw(&["plughw:CARD=PCH,DEV=0", "sysdefault:CARD=PCH"]));
        let labels: Vec<&str> = devices.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(labels, vec!["PCH"], "same card should appear once");
    }

    #[test]
    fn an_empty_enumeration_is_not_an_error() {
        assert!(usable_devices(Vec::new()).is_empty());
    }
}
