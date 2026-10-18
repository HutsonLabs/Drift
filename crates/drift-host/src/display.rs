//! Where the host captures from (task **HOST-10**, ADR `HOST-10-virtual-display`): a virtual
//! display sized to the client's layout, or the physical display when that is not possible
//! (the client then letterboxes, as with `ScaleMode::Fit`).

use drift_core::Size;

use crate::vdisplay_plan::ModePlan;

/// A client's layout as Display Control sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutRequest {
    /// The monitor's size in pixels.
    pub pixels: Size<u32>,
    /// Its desktop scale in percent (MS-RDPEDISP `DesktopScaleFactor`), if sent.
    pub desktop_scale: Option<u32>,
    /// Its physical size in millimetres, if sent.
    pub physical_mm: Option<(u32, u32)>,
}

/// The display the host captures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayTarget {
    /// The Mac's own (main) display.
    Physical,
    /// A virtual display made for the client.
    Virtual {
        /// Its `CGDirectDisplayID`.
        display_id: u32,
        /// Its mode.
        plan: ModePlan,
    },
}

/// Turns client layouts into a display to capture.
pub trait DisplayManager: Send {
    /// A client asked for `request`; returns the display to capture from now on.
    fn apply(&mut self, request: &LayoutRequest) -> DisplayTarget;
    /// The session ended: give the display back.
    fn release(&mut self) {}
}

/// Always the physical display (no virtual displays: disabled, unavailable, or tests).
#[derive(Debug, Clone, Copy, Default)]
pub struct PhysicalOnly;

impl DisplayManager for PhysicalOnly {
    fn apply(&mut self, _request: &LayoutRequest) -> DisplayTarget {
        DisplayTarget::Physical
    }
}

/// Virtual displays through `CGVirtualDisplay`, owned by a thread of their own (the objects are
/// not `Send`). Any failure falls back to the physical display.
#[cfg(target_os = "macos")]
pub struct VirtualDisplays {
    tx: std::sync::mpsc::Sender<Command>,
}

#[cfg(target_os = "macos")]
enum Command {
    Apply(LayoutRequest, std::sync::mpsc::Sender<DisplayTarget>),
    Release,
}

#[cfg(target_os = "macos")]
impl std::fmt::Debug for VirtualDisplays {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtualDisplays").finish_non_exhaustive()
    }
}

#[cfg(target_os = "macos")]
impl VirtualDisplays {
    /// Starts the owner thread; `name` is what macOS shows in Displays settings.
    pub fn start(name: &str) -> std::io::Result<Self> {
        use crate::cg_virtual::VirtualDisplay;
        use crate::vdisplay_plan::plan;

        let (tx, rx) = std::sync::mpsc::channel::<Command>();
        let name = name.to_owned();
        std::thread::Builder::new().name("drift-host-display".into()).spawn(move || {
            let mut current: Option<VirtualDisplay> = None;
            for command in rx {
                match command {
                    Command::Apply(request, reply) => {
                        let next = plan(&request);
                        let result = match current.as_mut() {
                            Some(display) if !ModePlan::needs_change(Some(display.plan()), &next) => Ok(()),
                            Some(display) => display.apply(&next),
                            None => VirtualDisplay::create(&name, &next).map(|d| current = Some(d)),
                        };
                        let target = match (result, current.as_ref()) {
                            (Ok(()), Some(d)) => {
                                DisplayTarget::Virtual { display_id: d.display_id(), plan: d.plan().clone() }
                            }
                            (Err(e), _) => {
                                tracing::warn!(error = %e, "no virtual display; capturing the physical display");
                                current = None;
                                DisplayTarget::Physical
                            }
                            (Ok(()), None) => DisplayTarget::Physical,
                        };
                        let _ = reply.send(target);
                    }
                    Command::Release => current = None,
                }
            }
        })?;
        Ok(Self { tx })
    }
}

#[cfg(target_os = "macos")]
impl DisplayManager for VirtualDisplays {
    fn apply(&mut self, request: &LayoutRequest) -> DisplayTarget {
        let (reply, answer) = std::sync::mpsc::channel();
        if self.tx.send(Command::Apply(request.clone(), reply)).is_err() {
            return DisplayTarget::Physical;
        }
        answer.recv_timeout(std::time::Duration::from_secs(5)).unwrap_or(DisplayTarget::Physical)
    }

    fn release(&mut self) {
        let _ = self.tx.send(Command::Release);
    }
}
