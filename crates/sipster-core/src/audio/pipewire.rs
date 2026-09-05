//! Selecting a specific `PipeWire` sink or source for a call.
//!
//! # Why this exists
//!
//! ALSA publishes exactly one `PipeWire` PCM — `pipewire` — and nothing per
//! device. `aplay -L` on a normal desktop lists `null`, `sysdefault`,
//! `pipewire`, `default`, and then only raw cards. cpal enumerates that same
//! list and `rvoip-audio-device` picks from it by substring, so through that
//! path "which microphone" can only ever be answered "the default one".
//!
//! # How it works instead
//!
//! Two supported `PipeWire` mechanisms, neither needing the native library (so
//! no libpipewire headers or clang in the build, and nothing to gate for the
//! Windows and armv7 targets):
//!
//! - `pw-dump` reports every node as JSON, which is where the real device list
//!   and its human names come from.
//! - Setting `target.object` on a stream node moves that stream to another
//!   device. This is what a desktop volume mixer does when you drag a stream
//!   between outputs, and the session manager honours it at runtime.
//!
//! Two details here were found the hard way, and both fail *silently* — the
//! metadata is accepted and the stream simply does not move:
//!
//! - The value must be the target's `object.serial`, not its `node.name`.
//!   A name is accepted and ignored.
//! - Our own stream cannot be found by `application.process.id`: `PipeWire`
//!   leaves that unset for ALSA-plugin clients, so every node reports no pid.
//!   The ALSA plugin does name them `alsa_playback.<program>` and
//!   `alsa_capture.<program>`, which is what they are matched on instead.
//!
//! So the call still opens the plain `pipewire` PCM through cpal, and the
//! stream it creates is then moved onto the chosen device.

use std::process::Command;

use super::Device;

/// Marks a device id as a `PipeWire` node rather than an ALSA PCM name.
///
/// The two share one `Option<String>` in the config and one picker in the UI,
/// so they have to be told apart on the way back out.
pub const PREFIX: &str = "pw:";

/// The PCM cpal is asked for when the real target is a `PipeWire` node: the
/// server's default. The stream is moved afterwards.
pub const SERVER_PCM: &str = "pipewire";

/// Whether `id` names a `PipeWire` node.
#[must_use]
pub fn is_node(id: &str) -> bool {
    id.starts_with(PREFIX)
}

/// The bare `node.name` behind a `pw:`-prefixed id.
#[must_use]
pub fn node_name(id: &str) -> Option<&str> {
    id.strip_prefix(PREFIX)
}

/// Which side of a stream a device sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// A microphone: `Audio/Source`.
    Capture,
    /// A speaker: `Audio/Sink`.
    Playback,
}

impl Direction {
    /// The `media.class` a *device* of this direction carries.
    fn device_class(self) -> &'static [&'static str] {
        match self {
            Self::Capture => &["Audio/Source", "Audio/Source/Virtual"],
            Self::Playback => &["Audio/Sink"],
        }
    }

    /// How the ALSA plugin prefixes our stream's node name.
    ///
    /// Reads the opposite way round to `stream_class`: capture is an *input*
    /// stream but the plugin calls the node `alsa_capture`.
    fn stream_prefix(self) -> &'static str {
        match self {
            Self::Capture => "alsa_capture",
            Self::Playback => "alsa_playback",
        }
    }

    /// The `media.class` our own *stream* carries for this direction.
    ///
    /// These read backwards on purpose: a stream that captures is an input to
    /// the application, and one that plays back is an output from it.
    fn stream_class(self) -> &'static str {
        match self {
            Self::Capture => "Stream/Input/Audio",
            Self::Playback => "Stream/Output/Audio",
        }
    }
}

/// One node as `pw-dump` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    id: u32,
    name: String,
    description: String,
    class: String,
    /// `PipeWire`'s per-object serial. This, not the name, is what
    /// `target.object` has to be set to.
    serial: Option<u64>,
    /// Unset for ALSA-plugin clients, which is every stream we create — kept
    /// only because a native client would have it.
    pid: Option<u32>,
}

/// Runs a `PipeWire` tool, returning `None` when it is missing or fails.
///
/// A machine without `PipeWire` is an ordinary case, not an error: the ALSA
/// device list still works, so nothing here should be load-bearing.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| tracing::debug!(program, error = %e, "PipeWire tool unavailable"))
        .ok()?;
    if !output.status.success() {
        tracing::debug!(program, status = ?output.status, "PipeWire tool failed");
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Every node `pw-dump` currently reports.
fn dump() -> Vec<Node> {
    run("pw-dump", &[]).map(|json| parse_dump(&json)).unwrap_or_default()
}

/// Pulls the nodes out of a `pw-dump` document.
///
/// Written against the shape rather than a schema: `pw-dump` emits every
/// global, most of which are not nodes and have no `info.props` at all.
fn parse_dump(json: &str) -> Vec<Node> {
    let Ok(objects) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        tracing::debug!("could not parse pw-dump output");
        return Vec::new();
    };

    objects
        .iter()
        .filter_map(|object| {
            let props = object.get("info")?.get("props")?;
            let name = props.get("node.name")?.as_str()?.to_owned();
            let class = props.get("media.class")?.as_str()?.to_owned();
            // `node.nick` is often the friendlier of the two; the raw name is
            // a last resort so a device is never listed as nothing.
            let description = props
                .get("node.description")
                .or_else(|| props.get("node.nick"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&name)
                .to_owned();
            Some(Node {
                id: u32::try_from(object.get("id")?.as_u64()?).ok()?,
                name,
                description,
                class,
                serial: props.get("object.serial").and_then(serde_json::Value::as_u64),
                pid: props
                    .get("application.process.id")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|pid| u32::try_from(pid).ok()),
            })
        })
        .collect()
}

/// The selectable devices for `direction`, newest duplicates removed.
#[must_use]
pub fn devices(direction: Direction) -> Vec<Device> {
    let mut devices: Vec<Device> = dump()
        .into_iter()
        .filter(|node| direction.device_class().contains(&node.class.as_str()))
        .map(|node| (format!("{PREFIX}{}", node.name), node.description))
        .collect();

    // A device can appear twice — two cards with one description, or a node
    // re-announced — and the picker should not show the same word twice.
    devices.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    devices.dedup_by(|a, b| a.0 == b.0);
    devices
}

/// Whether this process has any audio streams yet.
///
/// The stream nodes appear a moment after the PCM opens, so [`route`] has
/// nothing to move until this is true.
#[must_use]
pub fn streams_exist() -> bool {
    let nodes = dump();
    [Direction::Capture, Direction::Playback]
        .iter()
        .any(|direction| our_stream(&nodes, *direction).is_some())
}

/// How the ALSA plugin names this process's streams.
///
/// It builds them as `alsa_playback.<program>` / `alsa_capture.<program>`,
/// where `<program>` is the executable's file name.
fn program_name() -> Option<String> {
    Some(
        std::env::current_exe()
            .ok()?
            .file_name()?
            .to_string_lossy()
            .into_owned(),
    )
}

/// Moves this process's audio streams onto the chosen devices.
///
/// Called after the call's audio is open, because the stream nodes do not
/// exist until then. Each is looked up by our own pid and moved with
/// `target.object`.
///
/// Failure is logged, never returned: the call already has working audio on
/// the default device, and losing the call over a routing preference would be
/// the worse outcome.
pub fn route(capture: Option<&str>, playback: Option<&str>) {
    if capture.is_none() && playback.is_none() {
        return;
    }
    let nodes = dump();
    if let Some(target) = capture {
        move_stream(&nodes, Direction::Capture, target);
    }
    if let Some(target) = playback {
        move_stream(&nodes, Direction::Playback, target);
    }
}

/// Points our stream for `direction` at the device named `target`.
fn move_stream(nodes: &[Node], direction: Direction, target: &str) {
    match try_move(nodes, direction, target) {
        Ok(serial) => tracing::info!(target, serial, ?direction, "moved call audio"),
        Err(why) => tracing::warn!(target, ?direction, why, "could not move call audio"),
    }
}

/// The move itself, with each way it can fail named.
fn try_move(nodes: &[Node], direction: Direction, target: &str) -> Result<u64, &'static str> {
    let stream = our_stream(nodes, direction).ok_or("no stream of ours to move")?;
    // The session manager matches the target by serial. Given a name it
    // accepts the metadata and quietly leaves the stream where it was.
    let serial = device_serial(nodes, direction, target).ok_or("no such device")?;

    run("pw-metadata", &[&stream.to_string(), "target.object", &serial.to_string()])
        .ok_or("pw-metadata failed")?;
    Ok(serial)
}

/// The `object.serial` of the device called `target`.
fn device_serial(nodes: &[Node], direction: Direction, target: &str) -> Option<u64> {
    nodes
        .iter()
        .find(|node| {
            node.name == target && direction.device_class().contains(&node.class.as_str())
        })
        .and_then(|node| node.serial)
}

/// The id of this process's stream node for `direction`.
///
/// Matched by name, not pid: `PipeWire` leaves `application.process.id` unset
/// for ALSA-plugin clients, so a pid match never succeeded and every move was
/// skipped with "no stream of ours".
fn our_stream(nodes: &[Node], direction: Direction) -> Option<u32> {
    let expected = program_name().map(|name| format!("{}.{name}", direction.stream_prefix()));
    let pid = std::process::id();
    let class = direction.stream_class();
    nodes
        .iter()
        .find(|node| {
            node.class == class
                && (node.pid == Some(pid) || expected.as_deref() == Some(node.name.as_str()))
        })
        .map(|node| node.id)
}

#[cfg(test)]
mod tests {
    use super::{is_node, node_name, parse_dump, Direction, PREFIX};

    /// Trimmed from real `pw-dump` output on a desktop: a sink, a source, a
    /// stream belonging to a process, and two globals that are not nodes.
    const DUMP: &str = r#"[
      {"id": 38, "type": "PipeWire:Interface:Metadata"},
      {"id": 52, "info": {"props": {
        "media.class": "Audio/Sink",
        "node.name": "alsa_output.usb-WG2-00.analog-stereo",
        "node.description": "WG2 Analog Stereo"}}},
      {"id": 60, "info": {"props": {
        "media.class": "Audio/Source",
        "node.name": "alsa_input.pci-0000_00_1f.3.analog-stereo",
        "node.description": "Built-in Audio Analog Stereo"}}},
      {"id": 91, "info": {"props": {
        "media.class": "Stream/Output/Audio",
        "node.name": "sipster",
        "application.process.id": 4242}}},
      {"id": 7, "info": {}}
    ]"#;

    #[test]
    fn reads_devices_and_streams_out_of_a_dump() {
        let nodes = parse_dump(DUMP);
        assert_eq!(nodes.len(), 3, "the metadata global and the propless node are not nodes");

        let sink = nodes.iter().find(|n| n.id == 52).expect("sink");
        assert_eq!(sink.description, "WG2 Analog Stereo");
        assert_eq!(sink.class, "Audio/Sink");
        assert_eq!(sink.pid, None);

        let stream = nodes.iter().find(|n| n.id == 91).expect("stream");
        assert_eq!(stream.pid, Some(4242));
        assert_eq!(stream.class, Direction::Playback.stream_class());
    }

    /// A node with no `node.description` must still be listed under something.
    #[test]
    fn a_nameless_device_falls_back_to_its_node_name() {
        let nodes = parse_dump(
            r#"[{"id": 1, "info": {"props": {
                "media.class": "Audio/Sink", "node.name": "bare_sink"}}}]"#,
        );
        assert_eq!(nodes[0].description, "bare_sink");
    }

    /// `pw-dump` is an external program; malformed output must not panic.
    #[test]
    fn malformed_output_yields_nothing() {
        assert!(parse_dump("").is_empty());
        assert!(parse_dump("not json").is_empty());
        assert!(parse_dump("{}").is_empty());
        assert!(parse_dump("[null, 3, \"x\"]").is_empty());
    }

    /// Capture and playback classes read backwards from the device's, and
    /// mixing them up would move the microphone onto the speaker.
    #[test]
    fn stream_and_device_classes_do_not_get_crossed() {
        assert_eq!(Direction::Capture.stream_class(), "Stream/Input/Audio");
        assert_eq!(Direction::Playback.stream_class(), "Stream/Output/Audio");
        assert!(Direction::Capture.device_class().contains(&"Audio/Source"));
        assert!(Direction::Playback.device_class().contains(&"Audio/Sink"));
    }

    #[test]
    fn node_ids_are_told_apart_from_alsa_pcm_names() {
        assert!(is_node("pw:alsa_output.thing"));
        assert!(!is_node("plughw:CARD=PCH,DEV=0"));
        assert!(!is_node("pipewire"));
        assert_eq!(node_name("pw:thing"), Some("thing"));
        assert_eq!(node_name("plughw:x"), None);
        assert_eq!(format!("{PREFIX}x"), "pw:x");
    }
}
