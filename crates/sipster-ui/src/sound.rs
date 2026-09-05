//! Local audio feedback: dialpad DTMF, call chimes and the incoming ringtone.
//!
//! This is *UI* sound — what the user hears from their own speakers as
//! feedback. It is deliberately not in `sipster-core`: nothing here touches the
//! call's media stream, and a headless frontend would want none of it.
//!
//! Everything is synthesized into a small in-memory WAV and piped to `pw-play`
//! (`PipeWire`) with `paplay` (`PulseAudio`) as a fallback, so there are no audio
//! assets to ship and no extra audio dependency in the GUI crate.

// Synthesis is float maths written into 16-bit PCM; the casts are the point.
// Amplitudes are clamped to [-1.0, 1.0] before scaling, so the `as i16`
// conversion cannot actually truncate.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::io::AsyncWriteExt;

/// Sample rate of every generated clip. 44.1 kHz is universally accepted and
/// keeps the DTMF harmonics well below Nyquist.
const SAMPLE_RATE: u32 = 44_100;
/// Samples spent fading a tone in and out. Without this the abrupt start and
/// end of a sine burst is audible as a click.
const FADE_SAMPLES: u32 = 150;

/// One segment of a generated clip.
#[derive(Debug, Clone, Copy)]
struct Tone {
    /// Primary frequency in Hz.
    low: f32,
    /// Secondary frequency in Hz, or `None` for a single sine.
    high: Option<f32>,
    /// Duration in seconds.
    secs: f32,
    /// Peak amplitude, 0.0–1.0.
    gain: f32,
}

impl Tone {
    const fn single(freq: f32, secs: f32, gain: f32) -> Self {
        Self { low: freq, high: None, secs, gain }
    }

    const fn dual(low: f32, high: f32, secs: f32, gain: f32) -> Self {
        Self { low, high: Some(high), secs, gain }
    }

    fn sample_count(self) -> u32 {
        (SAMPLE_RATE as f32 * self.secs) as u32
    }

    /// Amplitude envelope: linear fade in over the first [`FADE_SAMPLES`] and
    /// out over the last, flat in between.
    fn envelope(self, index: u32) -> f32 {
        let total = self.sample_count();
        let fade = (total / 2).clamp(1, FADE_SAMPLES);
        if index < fade {
            index as f32 / fade as f32
        } else if index + fade >= total {
            (total.saturating_sub(index)) as f32 / fade as f32
        } else {
            1.0
        }
    }
}

/// Renders `tones` back to back into a complete mono 16-bit WAV file.
fn render(tones: &[Tone]) -> Vec<u8> {
    let total_samples: u32 = tones.iter().map(|tone| tone.sample_count()).sum();
    let data_len = total_samples * 2;

    let mut wav = Vec::with_capacity(44 + data_len as usize);
    // Canonical 44-byte RIFF/WAVE header for mono PCM16.
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());

    for tone in tones {
        for index in 0..tone.sample_count() {
            let t = index as f32 / SAMPLE_RATE as f32;
            let mut value = sine(tone.low, t);
            if let Some(high) = tone.high {
                // Two summed sines would clip at ±2.0; halve to stay in range.
                value = (value + sine(high, t)) * 0.5;
            }
            let amplitude = (value * tone.gain * tone.envelope(index)).clamp(-1.0, 1.0);
            wav.extend_from_slice(&((amplitude * f32::from(i16::MAX)) as i16).to_le_bytes());
        }
    }
    wav
}

fn sine(freq: f32, t: f32) -> f32 {
    (2.0 * std::f32::consts::PI * freq * t).sin()
}

/// Feeds one clip to one player. `false` when that player is not installed.
async fn pipe_to_player(player: &str, wav: &[u8]) -> bool {
    let Ok(mut child) = tokio::process::Command::new(player)
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(wav).await {
            // A player that exits early closes the pipe; the tone is lost but
            // nothing else is affected.
            tracing::debug!(player, error = %e, "could not feed audio to the player");
        }
        // Dropping stdin signals EOF; without it the player waits forever.
        drop(stdin);
    }
    if let Err(e) = child.wait().await {
        tracing::debug!(player, error = %e, "audio player did not exit cleanly");
    }
    true
}

/// Pipes a rendered clip to the system mixer, preferring `PipeWire`.
///
/// Returns `false` when no player is available, so callers that loop (the
/// ringtone) can stop instead of spinning.
async fn play(wav: &[u8]) -> bool {
    for player in ["pw-play", "paplay"] {
        if pipe_to_player(player, wav).await {
            return true;
        }
    }
    tracing::debug!("no audio player (pw-play/paplay) available for feedback sounds");
    false
}

/// Renders and plays `tones` without blocking the UI thread.
fn play_detached(tones: &'static [Tone]) {
    tokio::spawn(async move {
        play(&render(tones)).await;
    });
}

// ── clips ────────────────────────────────────────────────────────────────────

/// ITU-T Q.23 DTMF row/column frequency pairs.
///
/// `None` for anything without an assignment, so a character that is not a
/// dialpad key stays silent instead of borrowing some other key's tone.
const fn dtmf_pair(digit: char) -> Option<(f32, f32)> {
    Some(match digit {
        '1' => (697.0, 1209.0),
        '2' => (697.0, 1336.0),
        '3' => (697.0, 1477.0),
        '4' => (770.0, 1209.0),
        '5' => (770.0, 1336.0),
        '6' => (770.0, 1477.0),
        '7' => (852.0, 1209.0),
        '8' => (852.0, 1336.0),
        '9' => (852.0, 1477.0),
        '*' => (941.0, 1209.0),
        '0' => (941.0, 1336.0),
        '#' => (941.0, 1477.0),
        // '+' is not a DTMF key; borrow the otherwise unused 'D' column tone
        // so the dialpad's own '+' button still gives feedback.
        '+' => (941.0, 1633.0),
        _ => return None,
    })
}

/// Keypress feedback for a dialpad digit.
pub fn dtmf(digit: char) {
    let Some((low, high)) = dtmf_pair(digit) else {
        return;
    };
    let tone = Tone::dual(low, high, 0.065, 0.5);
    tokio::spawn(async move {
        play(&render(&[tone])).await;
    });
}

/// Rising two-tone chime played when placing a call (C5 → E5).
pub fn call_started() {
    static CLIP: [Tone; 2] = [
        Tone::single(523.25, 0.09, 0.35),
        Tone::single(659.25, 0.09, 0.35),
    ];
    play_detached(&CLIP);
}

/// Falling two-tone chime played when a call ends (D5 → A4).
pub fn call_ended() {
    static CLIP: [Tone; 2] = [
        Tone::single(587.33, 0.09, 0.35),
        Tone::single(440.0, 0.09, 0.35),
    ];
    play_detached(&CLIP);
}

// ── ringtone ─────────────────────────────────────────────────────────────────

/// A ringtone that keeps ringing until it is stopped or dropped.
///
/// The previous implementation played a single burst, so an incoming call was
/// announced once and then rang in silence. Holding this in the app state and
/// dropping it on answer/decline/terminate keeps the ring and the call in sync.
#[derive(Debug)]
pub struct Ringtone {
    stop: Arc<AtomicBool>,
}

impl Drop for Ringtone {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Desktop ringtone sound files, best first. Falls back to a synthesized
/// double-ring when the desktop ships none of them.
const RINGTONE_FILES: [&str; 3] = [
    "/usr/share/sounds/ocean/stereo/phone-incoming-call.oga",
    "/usr/share/sounds/freedesktop/stereo/phone-incoming-call.oga",
    "/usr/share/sounds/freedesktop/stereo/bell.oga",
];

/// Classic double-ring: two short bursts, then the caller-side silence gap.
static RING_BURST: [Tone; 4] = [
    Tone::dual(440.0, 480.0, 0.4, 0.35),
    Tone::single(0.0, 0.2, 0.0),
    Tone::dual(440.0, 480.0, 0.4, 0.35),
    Tone::single(0.0, 1.6, 0.0),
];

/// Starts ringing. Ringing stops when the returned handle is dropped.
#[must_use]
pub fn start_ringing() -> Ringtone {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);

    tokio::spawn(async move {
        let file = RINGTONE_FILES.iter().find(|p| Path::new(p).exists());
        let synthesized = file.is_none().then(|| render(&RING_BURST));

        while !flag.load(Ordering::Relaxed) {
            let played = match (file, synthesized.as_deref()) {
                (Some(path), _) => play_file(path).await,
                (None, Some(wav)) => play(wav).await,
                (None, None) => false,
            };
            if !played {
                break; // no player available; do not spin
            }
        }
    });

    Ringtone { stop }
}

async fn play_file(path: &str) -> bool {
    for player in ["pw-play", "paplay"] {
        if let Ok(mut child) = tokio::process::Command::new(player)
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            if let Err(e) = child.wait().await {
                tracing::debug!(player, error = %e, "audio player did not exit cleanly");
            }
            return true;
        }
    }
    false
}

// ── desktop notification ─────────────────────────────────────────────────────

/// Raises a desktop notification for an incoming call.
///
/// Best effort: a desktop without `notify-send` simply gets no popup.
pub fn notify_incoming(caller: &str) {
    let caller = caller.to_owned();
    tokio::spawn(async move {
        let notified = tokio::process::Command::new("notify-send")
            .args([
                "-a", "Sipster",
                "-i", "call-start",
                "-u", "critical",
                "Incoming Call",
                &caller,
            ])
            .status()
            .await;
        // A desktop without notify-send simply gets no popup, but if it is
        // there and failed, that is worth a line rather than silence.
        match notified {
            Ok(status) if !status.success() => {
                tracing::debug!(%status, "notify-send reported a failure");
            }
            Err(e) => tracing::debug!(error = %e, "no desktop notification (notify-send)"),
            Ok(_) => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{render, Tone, SAMPLE_RATE};

    /// A WAV whose header disagrees with its payload is silently rejected by
    /// some players and plays as noise in others.
    #[test]
    fn header_lengths_match_the_payload() {
        let wav = render(&[Tone::single(440.0, 0.1, 0.5)]);
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
        assert_eq!(wav.len(), 44 + data_len, "data chunk size must match payload");

        let riff_len = u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff_len, wav.len() - 8, "RIFF size counts everything after itself");
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
    }

    #[test]
    fn duration_determines_sample_count() {
        let wav = render(&[Tone::single(440.0, 0.5, 0.5)]);
        let samples = (wav.len() - 44) / 2;
        assert_eq!(samples, SAMPLE_RATE as usize / 2);
    }

    #[test]
    fn segments_are_concatenated() {
        let one = render(&[Tone::single(440.0, 0.1, 0.5)]).len();
        let two = render(&[Tone::single(440.0, 0.1, 0.5), Tone::single(880.0, 0.1, 0.5)]).len();
        assert_eq!(two - 44, (one - 44) * 2, "two tones are twice the payload");
    }

    /// Clipping is audible as harsh distortion; the summed dual tone plus the
    /// envelope must stay inside the 16-bit range.
    #[test]
    fn samples_never_clip() {
        let wav = render(&[Tone::dual(697.0, 1209.0, 0.05, 1.0)]);
        for chunk in wav[44..].chunks_exact(2) {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            assert!(sample > i16::MIN, "sample wrapped past the negative rail");
        }
    }

    /// Clicks at the edges were the reason the fade exists at all.
    #[test]
    fn tones_fade_in_from_silence() {
        let wav = render(&[Tone::single(440.0, 0.2, 1.0)]);
        let first = i16::from_le_bytes([wav[44], wav[45]]);
        assert_eq!(first, 0, "a tone must start at zero amplitude");
    }

    /// A tone shorter than twice the fade length would otherwise compute a
    /// negative flat section and panic on the underflow.
    #[test]
    fn very_short_tones_do_not_panic() {
        let wav = render(&[Tone::single(440.0, 0.001, 0.5)]);
        assert!(wav.len() > 44);
    }
}
