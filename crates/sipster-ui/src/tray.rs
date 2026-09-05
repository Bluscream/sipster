//! System tray icon for Sipster.
//!
//! Uses `ksni` which speaks `StatusNotifierItem` directly — this is what KDE
//! Plasma 6 under Wayland actually listens for. The `tray-icon` crate's
//! libayatana-appindicator backend creates items on the bus that Plasma 6
//! never shows; ksni avoids that whole detour.
//!
//! The tray lives as long as the process. Its icon and menu state are updated
//! via a shared atomic; tray actions arrive on a channel the Iced app
//! subscribes to, and are dispatched through `SipsterApp::handle_tray`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use tokio::sync::mpsc as tokio_mpsc;

/// The receiving end of the tray's request channel.
pub type Requests = tokio_mpsc::UnboundedReceiver<Request>;

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
    /// Taken once, by the subscription that streams tray requests into the
    /// application. `None` afterwards.
    requests: Option<Requests>,
    call_state: Arc<AtomicU8>,
    /// Also kept alive deliberately: dropping it removes the icon.
    service: ksni::blocking::Handle<Icon>,
}

impl Handle {
    /// Takes the request stream, which only one subscription may own.
    ///
    /// Returns `None` on any call after the first.
    pub fn take_requests(&mut self) -> Option<Requests> {
        self.requests.take()
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
    tx: tokio_mpsc::UnboundedSender<Request>,
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
                label: rust_i18n::t!("open_sipster").into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(Request::Show);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: format!("Open {}", rust_i18n::t!("contacts")),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(Request::OpenContacts);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: format!("Open {}", rust_i18n::t!("history")),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(Request::OpenCallList);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: format!("Open {}", rust_i18n::t!("settings")),
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
                    label: rust_i18n::t!("answer").into(),
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
                    label: rust_i18n::t!("hangup").into(),
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
                label: rust_i18n::t!("quit").into(),
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
            // These are compiled into the binary, so a failure here means a
            // broken asset rather than anything the user did — but silently
            // dropping a size leaves the tray with a subtly wrong icon and no
            // clue why.
            let decoded = image::load_from_memory(bytes)
                .inspect_err(|e| tracing::warn!(size, error = %e, "could not decode a tray icon"))
                .ok()?
                .to_rgba8();
            let mut argb = Vec::with_capacity(decoded.as_raw().len());
            for pixel in decoded.pixels() {
                let [r, g, b, a] = pixel.0;
                argb.extend_from_slice(&[a, r, g, b]);
            }
            Some(ksni::Icon {
                // Sizes are literals above, so these cannot fail; written as
                // conversions rather than casts so that stays true if the
                // list changes.
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
    // Unbounded and non-blocking to send: ksni calls these menu callbacks on
    // its own thread, which must not be parked waiting for the UI.
    let (tx, rx) = tokio_mpsc::unbounded_channel();
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
        requests: Some(rx),
        call_state,
        service,
    })
}

/// Decode the 256×256 icon for the Iced window.
#[must_use]
pub fn window_icon() -> Option<iced::window::Icon> {
    let bytes = include_bytes!("../../../assets/icons/sipster-256.png");
    let decoded = image::load_from_memory(bytes)
        .inspect_err(|e| tracing::warn!(error = %e, "could not decode the window icon"))
        .ok()?
        .to_rgba8();
    let (width, height) = (decoded.width(), decoded.height());
    iced::window::icon::from_rgba(decoded.into_raw(), width, height)
        .inspect_err(|e| tracing::warn!(error = %e, "could not build the window icon"))
        .ok()
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
