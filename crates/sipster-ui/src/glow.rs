//! Afterglow for dialpad keys.
//!
//! Typing into the number field is invisible on the dialpad: the digit appears
//! in the text box and the pad below it does nothing, so the two read as
//! unrelated controls. Lighting the matching key for a moment ties them
//! together, and doubles as feedback that a keystroke was actually taken.
//!
//! Kept as its own module because the fade has to be driven by frames rather
//! than by the 100 ms timer the rest of the UI runs on — at ten frames a
//! second a fade looks like a stutter.

use std::time::{Duration, Instant};

/// How long a key stays lit. Long enough to notice, short enough not to smear
/// when someone types a number quickly.
const FADE: Duration = Duration::from_millis(420);

/// Which keys are lit, and since when.
///
/// A `Vec` rather than a map: there are fifteen keys at most and entries are
/// dropped as they expire, so this holds a handful of items and iterating it
/// beats hashing.
#[derive(Debug, Default, Clone)]
pub struct Glow {
    lit: Vec<(char, Instant)>,
}

impl Glow {
    /// Lights `key`, restarting the fade if it is already lit.
    ///
    /// Restarting matters for a held key: auto-repeat would otherwise leave
    /// the first press to fade out under the ones following it.
    pub fn strike(&mut self, key: char) {
        let now = Instant::now();
        match self.lit.iter_mut().find(|(k, _)| *k == key) {
            Some((_, at)) => *at = now,
            None => self.lit.push((key, now)),
        }
    }

    /// How lit `key` is, from 1.0 just struck to 0.0 faded out.
    ///
    /// Eased rather than linear: a straight ramp reads as a light switch, and
    /// the tail is what makes it look like a glow.
    #[must_use]
    pub fn amount(&self, key: char) -> f32 {
        self.lit
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(0.0, |(_, at)| {
                let elapsed = at.elapsed().as_secs_f32() / FADE.as_secs_f32();
                if elapsed >= 1.0 {
                    return 0.0;
                }
                let remaining = 1.0 - elapsed;
                remaining * remaining
            })
    }

    /// Drops keys that have finished fading. Returns whether anything is still
    /// lit, which is what decides if frames need to keep coming.
    pub fn tick(&mut self) -> bool {
        self.lit.retain(|(_, at)| at.elapsed() < FADE);
        !self.lit.is_empty()
    }

    /// Whether anything is lit. Drives the frame subscription, so that the
    /// app is not woken sixty times a second while nobody is typing.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.lit.iter().any(|(_, at)| at.elapsed() < FADE)
    }
}

#[cfg(test)]
mod tests {
    use super::{Glow, FADE};
    use std::time::{Duration, Instant};

    /// An instant `ago` in the past, so a fade can be tested without sleeping
    /// through it.
    fn rewind(ago: Duration) -> Instant {
        Instant::now().checked_sub(ago).expect("recent enough to rewind")
    }

    #[test]
    fn a_struck_key_starts_fully_lit_and_others_stay_dark() {
        let mut glow = Glow::default();
        glow.strike('5');
        assert!(glow.amount('5') > 0.9, "just struck");
        assert!(glow.amount('6').abs() < f32::EPSILON, "untouched keys stay dark");
        assert!(glow.is_active());
    }

    #[test]
    fn a_key_fades_out_and_is_dropped() {
        let mut glow = Glow::default();
        glow.strike('5');
        // Rewind the strike past the fade rather than sleeping for it.
        glow.lit[0].1 = rewind(FADE);
        assert!(glow.amount('5').abs() < f32::EPSILON, "faded out");
        assert!(!glow.is_active());
        assert!(!glow.tick(), "nothing left to animate");
        assert!(glow.lit.is_empty(), "expired keys are dropped");
    }

    /// Auto-repeat, or simply typing the same digit twice.
    #[test]
    fn striking_again_restarts_the_fade_without_duplicating_the_key() {
        let mut glow = Glow::default();
        glow.strike('7');
        glow.lit[0].1 = rewind(FADE / 2);
        let faded = glow.amount('7');
        assert!(faded < 0.9 && faded > 0.0, "half way through: {faded}");

        glow.strike('7');
        assert_eq!(glow.lit.len(), 1, "one entry per key");
        assert!(glow.amount('7') > 0.9, "the fade restarted");
    }

    #[test]
    fn several_keys_glow_independently() {
        let mut glow = Glow::default();
        glow.strike('1');
        glow.lit[0].1 = rewind(FADE / 2);
        glow.strike('2');
        assert!(glow.amount('2') > glow.amount('1'));
        assert!(glow.tick(), "both still within the fade");
    }
}
