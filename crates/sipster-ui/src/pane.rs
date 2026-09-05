//! Where an auxiliary list lives: nowhere, beside the dialer, or in its own
//! window.
//!
//! Contacts and history used to be window-only, which suits a second monitor
//! but is heavy for a glance at who just called — the dialer is 320 px wide
//! and there is plenty of screen next to it. Rather than adding a second
//! control per list, the existing toolbar button cycles through the three
//! placements, so one button covers "show me", "give it room", and "put it
//! away".
//!
//! Docking deliberately does *not* resize the window. `window::resize` reaches
//! the compositor — the window really does change size — but iced goes on
//! laying out at the old width, so the dialer renders at 320 px and the
//! compositor stretches it to fit. Rather than ship that, a docked pane fits
//! itself to whatever width the window already has: side by side once there
//! is room for both, and in place of the dialpad until then. Widening the
//! window by hand switches between the two.

/// The width the dialer column keeps when a pane is docked beside it.
pub const DIALER_WIDTH: f32 = 320.0;

/// The narrowest a docked pane is worth showing beside the dialer.
///
/// Below this the pane takes the dialpad's place instead, which keeps it
/// readable in a window the width of the dialer alone.
pub const SIDE_BY_SIDE_WIDTH: f32 = 620.0;

/// Where one auxiliary list is currently shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    /// Not shown at all.
    #[default]
    Hidden,
    /// Beside the dialer, inside the main window.
    Docked,
    /// In a window of its own.
    Window,
}

impl Placement {
    /// The next placement when the list's toolbar button is pressed.
    ///
    /// Docked comes first because it is the cheapest to dismiss and the most
    /// likely thing wanted mid-call; a separate window is the deliberate step
    /// past it.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Hidden => Self::Docked,
            Self::Docked => Self::Window,
            Self::Window => Self::Hidden,
        }
    }

    #[must_use]
    pub fn is_docked(self) -> bool {
        matches!(self, Self::Docked)
    }

    #[must_use]
    pub fn is_window(self) -> bool {
        matches!(self, Self::Window)
    }

    /// A localized label for the status line.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Hidden => rust_i18n::t!("app.placement_hidden").to_string(),
            Self::Docked => rust_i18n::t!("app.placement_docked").to_string(),
            Self::Window => rust_i18n::t!("app.placement_window").to_string(),
        }
    }
}

/// Whether a docked pane fits beside the dialer at this window width.
#[must_use]
pub fn fits_beside_dialer(window_width: f32) -> bool {
    window_width >= SIDE_BY_SIDE_WIDTH
}

#[cfg(test)]
mod tests {
    use super::{fits_beside_dialer, Placement, DIALER_WIDTH};

    #[test]
    fn a_dialer_width_window_is_too_narrow_for_two_columns() {
        assert!(!fits_beside_dialer(DIALER_WIDTH));
        assert!(!fits_beside_dialer(600.0));
        assert!(fits_beside_dialer(620.0));
        assert!(fits_beside_dialer(1200.0));
    }

    #[test]
    fn the_button_cycles_through_every_placement_and_back() {
        let mut seen = vec![Placement::Hidden];
        let mut at = Placement::Hidden;
        for _ in 0..3 {
            at = at.next();
            seen.push(at);
        }
        assert_eq!(
            seen,
            vec![
                Placement::Hidden,
                Placement::Docked,
                Placement::Window,
                Placement::Hidden
            ],
            "three presses must return to where it started"
        );
    }

    #[test]
    fn hidden_is_the_default() {
        assert_eq!(Placement::default(), Placement::Hidden);
        assert!(!Placement::default().is_docked());
        assert!(!Placement::default().is_window());
    }
}
