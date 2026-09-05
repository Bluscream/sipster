//! System tray icon for Sipster.
//!
//! Uses `ksni` which speaks `StatusNotifierItem` directly — this is what KDE
//! Plasma 6 under Wayland actually listens for. The `tray-icon` crate's
//! libayatana-appindicator backend creates items on the bus that Plasma 6
//! never shows; ksni avoids that whole detour.
//!
//! The tray lives as long as the process. Its icon and menu state are updated
//! via a shared atomic; tray actions are polled by the Iced app each tick and
//! dispatched through `SipsterApp::handle_tray`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// State the tray reads to decide which menu items to show.
///
/// Stored as a `u8` in an atomic for lock-free access from the tray thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CallState {
    Idle = 0,
    Ringing = 1,
    InCall = 2,
}

impl CallState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Ringing,
            2 => Self::InCall,
            _ => Self::Idle,
        }
    }
}

/// What the tray icon asks the application to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Show,
    OpenSettings,
    OpenCallList,
    OpenContacts,
    Answer,
    Hangup,
    Quit,
}

/// The application-side handle to the tray icon.
///
/// Drop this to remove the icon from the tray.
pub struct Handle {
    pub requests: std::sync::mpsc::Receiver<Request>,
    call_state: Arc<AtomicU8>,
    /// Also kept alive deliberately: dropping it removes the icon.
    service: ksni::blocking::Handle<Icon>,
}

impl Handle {
    /// Check for requests from the tray menu/click without blocking.
    pub fn poll(&self) -> Option<Request> {
        self.requests.try_recv().ok()
    }

    /// Update the call state the tray reflects.
    ///
    /// The `update` call is not optional bookkeeping: `StatusNotifierItem` hosts
    /// cache our properties and only re-read them when we signal a change.
    /// Storing the atomic alone would leave Plasma showing the previous status
    /// until something else happened to invalidate it.
    pub fn set_call_state(&self, state: CallState) {
        let previous = self.call_state.swap(state as u8, Ordering::Relaxed);
        if previous != state as u8 {
            self.service.update(|_| {});
        }
    }
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("tray::Handle").finish_non_exhaustive()
    }
}

// ── icon implementation ──────────────────────────────────────────────────────

struct Icon {
    tx: std::sync::mpsc::Sender<Request>,
    call_state: Arc<AtomicU8>,
    pixmap: Vec<ksni::Icon>,
}

impl ksni::Tray for Icon {
    fn id(&self) -> String {
        "sipster".into()
    }

    fn title(&self) -> String {
        "Sipster".into()
    }

    /// A softphone belongs with the chat and mail clients, not under the
    /// default `ApplicationStatus`. Plasma groups the tray by category.
    fn category(&self) -> ksni::Category {
        ksni::Category::Communications
    }

    /// `NeedsAttention` makes Plasma highlight the icon, which is the whole
    /// point of having a tray entry for an inbound call you may have missed.
    fn status(&self) -> ksni::Status {
        if CallState::from_u8(self.call_state.load(Ordering::Relaxed)) == CallState::Ringing {
            ksni::Status::NeedsAttention
        } else {
            ksni::Status::Active
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.pixmap.clone()
    }

    /// Left-click → bring the window to the front.
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(Request::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        let state = CallState::from_u8(self.call_state.load(Ordering::Relaxed));
        let mut items: Vec<ksni::MenuItem<Self>> = vec![
            StandardItem {
                label: "Open Sipster".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(Request::Show);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open Contacts".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(Request::OpenContacts);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open Call List".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(Request::OpenCallList);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open Settings".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(Request::OpenSettings);
                }),
                ..Default::default()
            }
            .into(),
        ];

        if state == CallState::Ringing {
            items.push(
                StandardItem {
                    label: "Answer".into(),
                    activate: Box::new(|this: &mut Self| {
                        let _ = this.tx.send(Request::Answer);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }

        if state != CallState::Idle {
            items.push(
                StandardItem {
                    label: "Hang up".into(),
                    activate: Box::new(|this: &mut Self| {
                        let _ = this.tx.send(Request::Hangup);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Quit Sipster".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(Request::Quit);
                }),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

// ── icon pixmaps ─────────────────────────────────────────────────────────────

/// Decode embedded PNGs into the ARGB format ksni requires.
fn pixmaps() -> Vec<ksni::Icon> {
    const SIZES: [(&[u8], u32); 4] = [
        (include_bytes!("../../../assets/icons/sipster-22.png"), 22),
        (include_bytes!("../../../assets/icons/sipster-32.png"), 32),
        (include_bytes!("../../../assets/icons/sipster-48.png"), 48),
        (include_bytes!("../../../assets/icons/sipster-64.png"), 64),
    ];
    SIZES
        .iter()
        .filter_map(|(bytes, size)| {
            let decoded = image::load_from_memory(bytes).ok()?.to_rgba8();
            let mut argb = Vec::with_capacity(decoded.as_raw().len());
            for pixel in decoded.pixels() {
                let [r, g, b, a] = pixel.0;
                argb.extend_from_slice(&[a, r, g, b]);
            }
            Some(ksni::Icon {
                width: i32::try_from(*size).ok()?,
                height: i32::try_from(*size).ok()?,
                data: argb,
            })
        })
        .collect()
}

// ── public API ───────────────────────────────────────────────────────────────

/// Spawn the tray icon.
///
/// Returns `None` if there is no `StatusNotifierWatcher` on the session bus —
/// that is not an error; desktops without a tray simply won't have one.
#[must_use]
pub fn spawn() -> Option<Handle> {
    let (tx, rx) = std::sync::mpsc::channel();
    let call_state = Arc::new(AtomicU8::new(CallState::Idle as u8));
    let icon = Icon {
        tx,
        call_state: Arc::clone(&call_state),
        pixmap: pixmaps(),
    };
    let service = ksni::blocking::TrayMethods::spawn(icon)
        .map_err(|e| tracing::warn!("no system tray available: {e}"))
        .ok()?;
    tracing::info!("tray icon registered");
    Some(Handle {
        requests: rx,
        call_state,
        service,
    })
}

/// Decode the 256×256 icon for the Iced window.
#[must_use]
pub fn window_icon() -> Option<iced::window::Icon> {
    let bytes = include_bytes!("../../../assets/icons/sipster-256.png");
    let decoded = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = (decoded.width(), decoded.height());
    iced::window::icon::from_rgba(decoded.into_raw(), width, height).ok()
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::pixmaps;

    #[test]
    fn every_tray_size_decodes() {
        let icons = pixmaps();
        assert_eq!(icons.len(), 4, "one of the tray icon sizes failed to decode");
    }

    #[test]
    fn pixmaps_are_argb_and_correct_length() {
        for icon in pixmaps() {
            let pixels = usize::try_from(icon.width * icon.height).expect("not negative");
            assert_eq!(
                icon.data.len(),
                pixels * 4,
                "{}x{} should be 4 bytes per pixel",
                icon.width,
                icon.height
            );
        }
    }

    #[test]
    fn pixmap_sizes_match_files() {
        let sizes: Vec<i32> = pixmaps().iter().map(|i| i.width).collect();
        assert_eq!(sizes, vec![22, 32, 48, 64]);
    }
}
