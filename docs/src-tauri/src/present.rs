//! Pure presentation rules for a session window (tasks **M1-6**, **M6-2**, **M3-2**).
//!
//! The humble window glue (`crate::windows`) applies these decisions to AppKit/Tauri:
//! which surface is in front (webview or RemoteView), where the title bar, the page and the
//! picture sit, who owns the keyboard, the window title, the identity capsule, whether closing
//! asks first, and the remote pointer shape.

use std::sync::Arc;

use drift_core::{ConnectionProfile, SessionState, Size};
use drift_macos::CursorShape;
use drift_macos::cursor::CursorImage;
use drift_rdp::CursorUpdate;

use crate::connections::{CONNECTIONS_WINDOW, ConnectionStatus, WindowIdentity};
use crate::menu::SessionItem;
use crate::view::{Screen, SessionView};

/// Which view is in front of a session window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The webview fills the window and there is nothing to see behind it (form, certificate
    /// prompt, progress, error). The RemoteView is hidden, so the page's glass panels sit on
    /// the window's native vibrancy.
    Webview,
    /// The RemoteView with the live picture; the webview is hidden and the view is first
    /// responder.
    Remote,
    /// The webview fills the window but paints only a dimming wash and a card, so the last
    /// frame stays visible underneath (M7-3). It takes keyboard focus: the only thing the user
    /// can do is answer the overlay.
    Overlay,
    /// A small panel of the webview floats over a **live** picture. The webview is shrunk to
    /// [`hud_frame`] so every event outside that rectangle still reaches the RemoteView, which
    /// keeps first responder.
    Hud(Hud),
}

/// Which panel a [`Surface::Hud`] draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hud {
    /// The greeter-wait hint, centred under the title bar (M3-2, M7-3).
    Banner,
    /// The statistics readouts in the bottom-right corner (M1 "Done (manual M1)", M9-1).
    Stats,
}

/// Which HUD (if any) belongs over the picture of `view`.
pub fn hud_for(view: &SessionView) -> Option<Hud> {
    match view.screen {
        Screen::GreeterHint => Some(Hud::Banner),
        Screen::Live if view.show_stats && view.stats.is_some() => Some(Hud::Stats),
        _ => None,
    }
}

/// The surface for a view.
///
/// The webview is shown whenever there is no live picture (plan §2 decision 6). Where the plan
/// asks for something drawn *over* the picture — the reconnect overlay's dimmed last frame, the
/// greeter hint, the statistics line — the webview stays in front of the RemoteView and is
/// transparent instead (see `docs/adr/M7-3-overlays-over-the-live-picture.md`).
pub fn surface_for(view: &SessionView) -> Surface {
    if let Some(hud) = hud_for(view) {
        return Surface::Hud(hud);
    }
    match view.screen {
        Screen::Live | Screen::GreeterHint => Surface::Remote,
        Screen::Reconnecting => Surface::Overlay,
        Screen::Profiles | Screen::Connecting | Screen::Certificate | Screen::Error => Surface::Webview,
    }
}

/// Edges a [`HudFrame`] may move away from when the window resizes (the other two pin it to the
/// window edge). These are screen directions; `crate::windows` maps them to an AppKit
/// autoresizing mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flexible {
    /// The space on the left may grow.
    pub left: bool,
    /// The space on the right may grow.
    pub right: bool,
    /// The space above may grow.
    pub top: bool,
    /// The space below may grow.
    pub bottom: bool,
}

/// Where a HUD panel sits inside the web view's superview, in that view's own coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudFrame {
    /// Distance from the left edge.
    pub x: f64,
    /// Distance from the coordinate-space origin (top edge when flipped, bottom edge otherwise).
    pub y: f64,
    /// Width in points.
    pub width: f64,
    /// Height in points.
    pub height: f64,
    /// How the panel follows a window resize.
    pub flexible: Flexible,
}

/// Margin between a HUD panel and the window edges, in points.
const HUD_INSET: f64 = 16.0;
/// Widest the greeter banner gets.
const BANNER_WIDTH: f64 = 560.0;
/// Height of the greeter banner (two lines plus its top margin).
const BANNER_HEIGHT: f64 = 76.0;
/// Width of the statistics panel (four labelled readouts).
const STATS_WIDTH: f64 = 300.0;
/// Height of the statistics panel (value over unit).
const STATS_HEIGHT: f64 = 56.0;

/// Height of a session window's transparent title bar, in points (UI-windows decision 5). The
/// traffic lights, the identity capsule and the two buttons sit in it; the page and the live
/// picture start below it.
pub const TITLEBAR_HEIGHT: f64 = 52.0;

/// How a session window is laid out from the top, in points (task UI-windows).
///
/// The title bar is transparent and has no title; a transparent title-bar webview draws the
/// identity capsule and buttons in it. In full screen it is hidden entirely — also when the
/// menu bar slides down — and the picture fills the screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chrome {
    /// Height of the title-bar webview (0 = hidden).
    pub titlebar: f64,
    /// Where the page and the live picture start.
    pub content_top: f64,
}

/// The [`Chrome`] of a session window, in or out of full screen.
pub fn chrome(full_screen: bool) -> Chrome {
    if full_screen {
        Chrome { titlebar: 0.0, content_top: 0.0 }
    } else {
        Chrome { titlebar: TITLEBAR_HEIGHT, content_top: TITLEBAR_HEIGHT }
    }
}

/// The origin `y` of a horizontal band `top` points below the top edge and `height` points tall,
/// in a `parent_height`-high view (`flipped` = the view's `isFlipped`; AppKit's default origin is
/// bottom-left).
pub fn band_y(parent_height: f64, top: f64, height: f64, flipped: bool) -> f64 {
    if flipped { top } else { parent_height - top - height }
}

/// Which view gets the keyboard in a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The `RemoteView` (the remote desktop gets every key).
    Remote,
    /// The page's webview (forms, prompts, overlays).
    Page,
}

/// Who owns the keyboard for `surface`. The title-bar webview never keeps it: when it gains
/// focus it hands the keyboard straight back to this (UI-windows decision 11).
pub fn focus_for(surface: Surface) -> Focus {
    match surface {
        Surface::Remote | Surface::Hud(_) => Focus::Remote,
        Surface::Webview | Surface::Overlay => Focus::Page,
    }
}

/// The rectangle a HUD panel occupies over a `parent`-sized picture.
///
/// `flipped` is the superview's `isFlipped`: AppKit's default coordinate space has its origin in
/// the bottom-left, so the same panel is mirrored vertically there. The panel is always fully
/// inside `parent` and never covers more than a corner of it, so the user keeps clicking and
/// typing into the remote desktop everywhere else.
pub fn hud_frame(parent: Size<f64>, hud: Hud, flipped: bool) -> HudFrame {
    let (width, height, flexible) = match hud {
        Hud::Banner => (
            BANNER_WIDTH.min(parent.width - 2.0 * HUD_INSET).max(1.0),
            BANNER_HEIGHT.min(parent.height).max(1.0),
            Flexible { left: true, right: true, top: false, bottom: true },
        ),
        Hud::Stats => (
            STATS_WIDTH.min(parent.width).max(1.0),
            STATS_HEIGHT.min(parent.height).max(1.0),
            Flexible { left: true, right: false, top: true, bottom: false },
        ),
    };
    let (x, top) = match hud {
        Hud::Banner => ((parent.width - width) / 2.0, 0.0),
        Hud::Stats => (parent.width - width - HUD_INSET, parent.height - height - HUD_INSET),
    };
    let x = x.clamp(0.0, (parent.width - width).max(0.0));
    let top = top.clamp(0.0, (parent.height - height).max(0.0));
    let y = if flipped { top } else { parent.height - top - height };
    HudFrame { x, y, width, height, flexible }
}

/// A profile name as windows and menus show it: control characters become spaces, surrounding
/// whitespace is trimmed and a blank name reads "Untitled".
pub fn display_name(profile_name: &str) -> String {
    let cleaned: String = profile_name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let name = cleaned.trim();
    if name.is_empty() { "Untitled".to_owned() } else { name.to_owned() }
}

/// The session window's title: the plain profile name, which Mission Control, Cmd+` and the
/// Window menu show (UI-windows decision 5).
pub fn window_title(view: &SessionView) -> String {
    display_name(&view.profile_name)
}

/// Where the connection shown by `view` stands (UI-windows decision 6). A window that has not
/// heard from its actor yet (`Profiles`) is connecting.
pub fn status_for(view: &SessionView) -> ConnectionStatus {
    match view.screen {
        Screen::Profiles | Screen::Connecting | Screen::Certificate => ConnectionStatus::Connecting,
        Screen::Live | Screen::GreeterHint => ConnectionStatus::Live,
        Screen::Reconnecting => ConnectionStatus::Reconnecting,
        Screen::Error => ConnectionStatus::Failed,
    }
}

/// The text stand-in for a session's status in the Window and Dock menus (UI-tabs decision 11,
/// UI-windows decision 9), following [`status_for`]:
///
/// | Status | Glyph |
/// |---|---|
/// | live picture | `●` |
/// | live GNOME login screen | `◐` |
/// | connecting | `◌` |
/// | reconnecting | `↻` |
/// | failed (also a disconnect with an explanation) | `⚠` |
pub fn status_glyph(view: &SessionView) -> char {
    match status_for(view) {
        ConnectionStatus::Live if view.screen == Screen::GreeterHint => '◐',
        ConnectionStatus::Live => '●',
        ConnectionStatus::Connecting => '◌',
        ConnectionStatus::Reconnecting => '↻',
        ConnectionStatus::Failed => '⚠',
        ConnectionStatus::Idle => '○',
    }
}

/// Session window `window` as the Window and Dock menus list it.
pub fn session_item(window: &str, view: &SessionView) -> SessionItem {
    SessionItem {
        window: window.to_owned(),
        name: display_name(&view.profile_name),
        glyph: status_glyph(view),
        status: status_for(view),
    }
}

/// The title bar's identity capsule for `profile`'s window showing `view`.
pub fn identity(profile: &ConnectionProfile, view: &SessionView) -> WindowIdentity {
    WindowIdentity {
        profile_id: profile.id,
        name: display_name(&view.profile_name),
        host: profile.host.clone(),
        mode: view.mode,
        status: status_for(view),
        hint: greeter_hint(Some(view)),
        show_stats: view.show_stats,
    }
}

/// Whether closing a window showing `view` asks first: only while a desktop is held (live,
/// greeter, reconnecting). Connecting, a certificate prompt and an error close at once.
pub fn close_needs_confirmation(view: &SessionView) -> bool {
    matches!(view.screen, Screen::Live | Screen::GreeterHint | Screen::Reconnecting)
}

/// What the close button (or Cmd+W) does to a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// The Connections window: hide it (it is only destroyed on quit).
    Hide,
    /// Ask first, then close the session and the window.
    Confirm,
    /// Close the session (if any) and the window at once.
    CloseNow,
}

/// The close decision for window `label`, whose session (if any) shows `view`.
pub fn close_action(label: &str, view: Option<&SessionView>) -> CloseAction {
    if label == CONNECTIONS_WINDOW {
        CloseAction::Hide
    } else if view.is_some_and(close_needs_confirmation) {
        CloseAction::Confirm
    } else {
        CloseAction::CloseNow
    }
}

/// The words of a two-button confirmation alert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    /// Bold first line.
    pub message: String,
    /// Explanation.
    pub informative: String,
    /// Default button.
    pub confirm: String,
    /// Cancel button.
    pub cancel: String,
}

/// The sheet shown before closing a live session window.
pub fn close_confirmation(profile_name: &str) -> Confirmation {
    Confirmation {
        message: format!("Disconnect “{}”?", display_name(profile_name)),
        informative: "The remote session keeps running on the host.".to_owned(),
        confirm: "Disconnect".to_owned(),
        cancel: "Cancel".to_owned(),
    }
}

/// The alert shown before quitting with `sessions` open; `None` = quit at once.
pub fn quit_confirmation(sessions: usize) -> Option<Confirmation> {
    (sessions > 0).then(|| Confirmation {
        message: "Quit Drift?".to_owned(),
        informative: if sessions == 1 {
            "1 session will be disconnected.".to_owned()
        } else {
            format!("{sessions} sessions will be disconnected.")
        },
        confirm: "Quit".to_owned(),
        cancel: "Cancel".to_owned(),
    })
}

/// The greeter hint while the GNOME login screen is live, else `None`.
///
/// The title bar shows no title (UI-tabs, UI-windows), so this is no longer the window subtitle:
/// the same words are the [`Hud::Banner`] floating over the login screen and the identity
/// capsule's tooltip.
pub fn greeter_hint(view: Option<&SessionView>) -> Option<String> {
    let view = view.filter(|v| v.screen == Screen::GreeterHint)?;
    Some(match (&view.linux_username, view.resuming) {
        (Some(user), true) => format!("Session is still running — log in as “{user}” to resume"),
        (Some(user), false) => format!("Log in as “{user}” to start your session"),
        (None, true) => "Session is still running — log in to resume".to_owned(),
        (None, false) => "Log in to start your session".to_owned(),
    })
}

/// What VoiceOver announces for the live picture (task M9-4).
///
/// The `RemoteView` has an image role and this label; naming the connection and the desktop
/// size is the only way a VoiceOver user can tell two session windows apart, because the picture
/// itself is pixels from another computer.
pub fn accessibility_label(view: Option<&SessionView>) -> String {
    let Some(view) = view else { return drift_macos::view::DEFAULT_ACCESSIBILITY_LABEL.to_owned() };
    match view.state {
        SessionState::Connected { desktop, .. } => {
            format!("{} — remote desktop, {} by {} pixels", view.profile_name, desktop.width, desktop.height)
        }
        _ => format!("{} — remote desktop", view.profile_name),
    }
}

/// Converts an actor pointer update into a RemoteView cursor and its desktop scale (percent).
/// Position updates do not change the shape (`None`); the Mac pointer is never warped.
pub fn cursor_shape(update: &CursorUpdate) -> Option<(CursorShape, u32)> {
    match update {
        CursorUpdate::Hidden => Some((CursorShape::Hidden, 100)),
        CursorUpdate::Default => Some((CursorShape::Default, 100)),
        CursorUpdate::Bitmap(bitmap) => Some((
            CursorShape::Image(Arc::new(CursorImage {
                size: bitmap.size,
                hotspot: bitmap.hotspot,
                bgra: bitmap.bgra.clone(),
            })),
            bitmap.scale,
        )),
        CursorUpdate::Position(_) => None,
    }
}

/// The profile `DRIFT_AUTOCONNECT` names: an exact name match, else a unique case-insensitive
/// match (surrounding whitespace ignored).
pub fn find_autoconnect<'a>(profiles: &'a [ConnectionProfile], name: &str) -> Option<&'a ConnectionProfile> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if let Some(exact) = profiles.iter().find(|p| p.name == name) {
        return Some(exact);
    }
    let mut matches = profiles.iter().filter(|p| p.name.eq_ignore_ascii_case(name));
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}
