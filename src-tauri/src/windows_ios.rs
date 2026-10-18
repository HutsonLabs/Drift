//! The Connections window and session windows on iPhone and iPad (tasks **IOS-4**, **IOS-5**;
//! ADR `IOS-5-ios-app-shell`).
//!
//! The iOS twin of `windows.rs`, with the same functions for the rest of the app to call.
//! Humble object: every decision comes from [`crate::present`], [`crate::connections`] or the
//! [`SessionManager`](crate::manager::SessionManager); this module only talks to Tauri and UIKit.
//!
//! * **Connections** (label [`CONNECTIONS_WINDOW`]) is created at launch and never closed.
//! * Every connected profile has one **session window** (`session-<n>`), built from the
//!   `session` template. On iPad each window is its own scene (IOS-1); on iPhone the windows
//!   stack in the one scene and the front one is shown. A `RemoteView` sits below the page's
//!   `WKWebView`; the page is hidden while a live picture is on screen, made transparent over it
//!   for the reconnect overlay, or shrunk to a corner panel for a HUD.
//! * There is no menu bar, Dock menu, title-bar webview or saved window frame on iOS.
//! * Per-window UIKit state (views, render thread) lives in main-thread-only maps.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use drift_clipboard::ClipboardContents;
use drift_clipboard::poll::PasteboardPort as _;
use drift_clipboard::uipasteboard::UiPasteboard;
use drift_core::{ClipboardPrefs, ConnectionProfile, InputEvent, Size, ViewGeometry};
use drift_gfx::FrameSink;
use drift_ios::hardware::{self, HardwareInput};
use drift_ios::lifecycle::AppObserver;
use drift_ios::pacing::FramePacing;
use drift_ios::paste::{PasteButton, Pasted};
use drift_ios::switcher::{MenuEntry, SwitcherButton};
use drift_ios::view::{RemoteView, RemoteViewHandler};
use drift_rdp::{SessionCommand, SessionHandle};
use drift_render::{Compositor, Gpu, LayerTarget, RenderThread};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_ui_kit::UIView;
use tauri::{AppHandle, EventTarget, Manager as _, Runtime, WebviewWindow, WebviewWindowBuilder};
use tauri_specta::Event as _;
use uuid::Uuid;

use crate::commands::AppState;
pub use crate::connections::CONNECTIONS_WINDOW;
use crate::connections::{
    ConnectionStatus, Connections, ConnectionsChanged, ConnectionsIntent, ConnectionsIntentRequested,
    THUMBNAIL_MAX, Thumbnail, ThumbnailThrottle, ThumbnailUpdated, WindowIdentity,
};
use crate::manager::{ConnectPlan, Reconnect};
use crate::present::{self, CloseAction, Surface};
use crate::profiles::CommandError;
use crate::view::{SessionView, SessionViewChanged};

#[path = "windows_ios_phone.rs"]
mod phone_shell;
pub(crate) use phone_shell::{controls_used, phone_controls, set_touch_mode};

/// Label of the Connections window template in tauri.conf.json (it is also the window's label).
pub const CONNECTIONS_TEMPLATE: &str = CONNECTIONS_WINDOW;

/// Label of the session window template in tauri.conf.json.
pub const SESSION_TEMPLATE: &str = "session";

/// Tauri label of the `n`th session window.
pub fn window_label(n: u64) -> String {
    format!("session-{n}")
}

/// The opening number of a session window label (`session-<n>`).
fn window_number(label: &str) -> u64 {
    label.strip_prefix("session-").and_then(|n| n.parse().ok()).unwrap_or(u64::MAX)
}

// ---- per-window UIKit state (main thread only) ---------------------------------------------

type RenderSink = drift_render::RenderSink<Compositor<LayerTarget>>;

struct WindowPlatform {
    view: Retained<RemoteView>,
    /// The page's `WKWebView`.
    web: Retained<UIView>,
    /// What the window shows; drives [`layout`].
    surface: Surface,
    link: Rc<SessionLink>,
    render: Option<RenderThread<Compositor<LayerTarget>>>,
    /// The switcher button and its menu of windows (IOS-11).
    switcher: SwitcherButton,
    /// The paste button over the picture (IOS-10); `None` if it could not be added.
    paste: Option<PasteButton>,
    /// The profile's clipboard preference: no paste button when it is off.
    clipboard: ClipboardPrefs,
}

/// App-wide main-thread state.
#[derive(Default)]
struct Shell {
    connections: Option<Connections>,
    throttle: ThumbnailThrottle,
    live: HashSet<String>,
    /// Up to 120 Hz while any live picture is on screen (ProMotion, IOS-4).
    pacing: Option<FramePacing>,
    /// Foreground and background observers (visibility of every session) and the scene
    /// observer (an iPad window closed from the app switcher).
    observers: Vec<AppObserver>,
    /// The Connections page, its scene, and whether iPadOS has disconnected it (IOS-11).
    connections_web: Option<Retained<UIView>>,
    connections_scene: Option<usize>,
    connections_gone: bool,
    /// `GCKeyboard` / `GCMouse` hooks feeding the front session (IOS-6).
    hardware: Option<HardwareInput>,
}

thread_local! {
    static PLATFORM: RefCell<HashMap<String, WindowPlatform>> = RefCell::new(HashMap::new());
    static SHELL: RefCell<Shell> = RefCell::new(Shell::default());
    /// The shared Metal device and pipelines (plan §2: one per process).
    static GPU: RefCell<Option<Result<Gpu, drift_render::RenderError>>> = const { RefCell::new(None) };
}

/// Forwards `RemoteView` output to the window's session actor.
struct SessionLink {
    handle: RefCell<Option<SessionHandle>>,
}

impl SessionLink {
    fn new() -> Rc<Self> {
        Rc::new(Self { handle: RefCell::new(None) })
    }

    fn set(&self, handle: Option<SessionHandle>) {
        *self.handle.borrow_mut() = handle;
    }

    fn send(&self, cmd: SessionCommand) {
        if let Some(handle) = self.handle.borrow().as_ref() {
            let _ = handle.send(cmd);
        }
    }
}

impl RemoteViewHandler for SessionLink {
    fn input(&self, events: &[InputEvent]) {
        for event in events {
            self.send(SessionCommand::Input(*event));
        }
    }

    fn geometry_changed(&self, geometry: ViewGeometry) {
        self.send(SessionCommand::Resize(geometry));
    }
}

fn with_platform<T>(label: &str, f: impl FnOnce(&mut WindowPlatform) -> T) -> Option<T> {
    PLATFORM.with(|p| p.try_borrow_mut().ok()?.get_mut(label).map(f))
}

/// Runs `f` on the main thread (inline when already there) and returns its result.
///
/// From another thread the work takes two hops, as on macOS: tao's `run_on_main_thread` first
/// (so a window built just before exists), then the main dispatch queue (so UIKit work runs
/// outside tao's event handler).
pub(crate) fn on_main<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    f: impl FnOnce(MainThreadMarker) -> T + Send + 'static,
) -> Result<T, CommandError> {
    if let Some(mtm) = MainThreadMarker::new() {
        return Ok(f(mtm));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        drift_ios::webview::dispatch_main(move || {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let _ = tx.send(f(mtm));
        });
    })
    .map_err(|e| CommandError::Platform { message: e.to_string() })?;
    rx.recv().map_err(|_| CommandError::Platform { message: "the main thread went away".into() })
}

/// Runs `f` on the main thread **later**, with the same two hops as [`on_main`].
pub(crate) fn defer_main<R: Runtime>(app: &AppHandle<R>, f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    let result = app.run_on_main_thread(move || {
        drift_ios::webview::dispatch_main(move || {
            if let Some(mtm) = MainThreadMarker::new() {
                f(mtm);
            }
        });
    });
    if let Err(e) = result {
        tracing::warn!(error = %e, "could not reach the main thread");
    }
}

fn platform_error(message: impl Into<String>) -> CommandError {
    CommandError::Platform { message: message.into() }
}

/// Template `template` of tauri.conf.json, relabelled `label`.
fn template<R: Runtime>(
    app: &AppHandle<R>,
    template: &str,
    label: &str,
) -> tauri::Result<tauri::utils::config::WindowConfig> {
    let mut config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == template)
        .cloned()
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    label.clone_into(&mut config.label);
    Ok(config)
}

/// The page's `WKWebView` of window `label`, read on the main thread through Tauri (call from a
/// worker thread: `with_webview` waits for the event loop).
fn page_view_of<R: Runtime>(window: &WebviewWindow<R>) -> Result<Retained<UIView>, CommandError> {
    let (tx, rx) = std::sync::mpsc::channel();
    window
        .with_webview(move |platform| {
            let Some(mtm) = MainThreadMarker::new() else { return };
            // SAFETY: Tauri passes its live `WKWebView*` on the main thread.
            let _ = tx.send(unsafe { drift_ios::webview::page_view(mtm, platform.inner()) }.map(SendView));
        })
        .map_err(|e| platform_error(e.to_string()))?;
    let view = rx.recv().map_err(|_| platform_error("the page's webview is gone"))?;
    view.map(SendView::into_inner).ok_or_else(|| platform_error("the window has no webview"))
}

/// Carries a retained view across the channel from the main thread back to the main thread.
struct SendView(Retained<UIView>);

impl SendView {
    /// The view (a method, so a closure captures the whole `Send` wrapper, not the field).
    fn into_inner(self) -> Retained<UIView> {
        self.0
    }
}
// SAFETY: the view is only ever *used* on the main thread; the worker thread that receives it
// moves it straight into `on_main`.
unsafe impl Send for SendView {}

// ---- the Connections window -------------------------------------------------------------------

/// Creates the Connections window (at launch; returns immediately) and then connects
/// `autoconnect`, if any. Also starts the app-wide visibility observers.
pub(crate) fn open_connections_window<R: Runtime>(app: &AppHandle<R>, autoconnect: Option<Uuid>) {
    install_visibility_observers(app);
    install_scene_observer(app);
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = build_connections_window(&app) {
            tracing::error!(error = %e, "could not create the Connections window");
            return;
        }
        if let Some(profile) = autoconnect
            && let Err(e) = connect_profile(&app, profile)
        {
            tracing::error!(error = %e, "autoconnect failed");
        }
    });
}

/// Builds the Connections window and remembers its scene (worker thread).
fn build_connections_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), CommandError> {
    let tauri_error = |e: tauri::Error| platform_error(e.to_string());
    let config = template(app, CONNECTIONS_TEMPLATE, CONNECTIONS_WINDOW).map_err(tauri_error)?;
    let window =
        WebviewWindowBuilder::from_config(app, &config).map_err(tauri_error)?.build().map_err(tauri_error)?;
    let web = SendView(page_view_of(&window)?);
    on_main(app, move |_| {
        let web = web.into_inner();
        let scene = drift_ios::webview::scene_id(&web);
        SHELL.with(|s| {
            let mut shell = s.borrow_mut();
            shell.connections_web = Some(web);
            shell.connections_scene = scene;
            shell.connections_gone = false;
        });
    })
}

/// iPadOS disconnects a scene when the user closes it from the app switcher. A session window's
/// scene closing closes its session; the Connections window is rebuilt the next time it is
/// asked for (IOS-11). tao does not report either.
fn install_scene_observer<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    let observer = AppObserver::scene_disconnected(move |scene| {
        let app2 = app.clone();
        defer_main(&app, move |_| {
            let closed: Vec<String> = PLATFORM.with(|p| {
                p.borrow()
                    .iter()
                    .filter(|(_, plat)| drift_ios::webview::scene_id(&plat.web) == Some(scene))
                    .map(|(label, _)| label.clone())
                    .collect()
            });
            for label in closed {
                tracing::info!(%label, "its scene was closed; closing the session");
                close_session_window(&app2, &label);
            }
            SHELL.with(|s| {
                let mut shell = s.borrow_mut();
                if shell.connections_scene == Some(scene) {
                    shell.connections_gone = true;
                }
            });
        });
    });
    SHELL.with(|s| s.borrow_mut().observers.push(observer));
}

/// Every session is visible while the app is in the foreground and hidden once iOS has moved it
/// to the background (frames stop being drawn; the actor suppresses output, M6-3).
fn install_visibility_observers<R: Runtime>(app: &AppHandle<R>) {
    let set_all = |app: AppHandle<R>, visible: bool| {
        move || {
            let app = app.clone();
            defer_main(&app.clone(), move |_| {
                PLATFORM.with(|p| {
                    if let Ok(map) = p.try_borrow() {
                        for plat in map.values() {
                            if !visible {
                                // Nothing stays pressed on the remote while iOS suspends Drift.
                                plat.view.release_all();
                            }
                            plat.link.send(SessionCommand::SetVisible(visible));
                        }
                    }
                });
                update_pacing();
            });
        }
    };
    let observers = vec![
        AppObserver::foreground(set_all(app.clone(), true)),
        AppObserver::background(set_all(app.clone(), false)),
    ];
    let hardware = MainThreadMarker::new().map(HardwareInput::start);
    SHELL.with(|s| {
        let mut shell = s.borrow_mut();
        shell.observers.extend(observers);
        shell.hardware = hardware;
    });
}

/// Brings Connections forward; with `intent`, asks its page to open a sheet. Returns at once.
pub fn show_connections<R: Runtime>(app: &AppHandle<R>, intent: Option<ConnectionsIntent>) {
    let gone = SHELL.with(|s| s.try_borrow().map(|s| s.connections_gone).unwrap_or(false));
    if gone {
        // Its scene was closed: rebuild the window, which brings a new scene forward.
        let app = app.clone();
        std::thread::spawn(move || {
            if let Some(window) = app.get_webview_window(CONNECTIONS_WINDOW) {
                let _ = window.destroy();
            }
            if let Err(e) = build_connections_window(&app) {
                tracing::error!(error = %e, "could not rebuild the Connections window");
            }
            let _ = on_main(&app.clone(), move |_| push_connections_forced(&app));
        });
    } else {
        focus_window(app, CONNECTIONS_WINDOW);
    }
    if let Some(intent) = intent {
        let _ = ConnectionsIntentRequested(intent)
            .emit_to(app, EventTarget::webview_window(CONNECTIONS_WINDOW))
            .inspect_err(|e| tracing::warn!(error = %e, "could not emit a Connections intent"));
    }
}

/// Pushes the gallery's "Open" section to the Connections page if it changed (main thread).
fn push_connections<R: Runtime>(app: &AppHandle<R>) {
    let connections = app.state::<AppState>().sessions.connections();
    let mut key = connections.clone();
    for open in &mut key.open {
        open.live_secs = open.live_secs.map(|_| 0);
    }
    let changed = SHELL.with(|s| {
        let Ok(mut shell) = s.try_borrow_mut() else { return true };
        let changed = shell.connections.as_ref() != Some(&key);
        shell.connections = Some(key);
        changed
    });
    if changed {
        let _ = ConnectionsChanged(connections)
            .emit_to(app, EventTarget::webview_window(CONNECTIONS_WINDOW))
            .inspect_err(|e| tracing::warn!(error = %e, "could not emit the connections"));
    }
}

// ---- live thumbnails --------------------------------------------------------------------------

/// Starts the thumbnail clock: once a second the throttle says which live sessions are due.
pub(crate) fn start_thumbnails<R: Runtime>(app: &AppHandle<R>) {
    SHELL.with(|s| {
        if let Ok(mut shell) = s.try_borrow_mut() {
            shell.throttle.set_visible(true);
        }
    });
    let app = app.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let app2 = app.clone();
            if on_main(&app, move |_| sample_thumbnails(&app2)).is_err() {
                return;
            }
        }
    });
}

/// Samples every due live session (main thread); the read-back and PNG encoding run on a worker.
fn sample_thumbnails<R: Runtime>(app: &AppHandle<R>) {
    let sessions = app.state::<AppState>().sessions.sessions();
    let live: Vec<String> = sessions
        .iter()
        .filter(|s| present::status_for(&s.view) == ConnectionStatus::Live)
        .map(|s| s.window.clone())
        .collect();
    let due = SHELL
        .with(|s| s.try_borrow_mut().map(|mut s| s.throttle.due(Instant::now(), &live)).unwrap_or_default());
    for window in due {
        let Some(profile_id) = sessions.iter().find(|s| s.window == window).map(|s| s.profile.id) else {
            continue;
        };
        let Some(Some(sink)) = with_platform(&window, |plat| plat.render.as_ref().map(RenderThread::sink))
        else {
            continue;
        };
        let app = app.clone();
        std::thread::spawn(move || {
            let sink: RenderSink = sink;
            let Some(Some(frame)) = sink.with(Compositor::read_output) else { return };
            let (rgba, size) = crate::connections::downscale(&frame.data, frame.size, THUMBNAIL_MAX);
            drop(frame);
            let Some(image) = crate::connections::png_data_url(&rgba, size) else { return };
            emit_thumbnail(&app, profile_id, Some(image));
        });
    }
}

fn emit_thumbnail<R: Runtime>(app: &AppHandle<R>, profile_id: Uuid, image: Option<String>) {
    let _ = ThumbnailUpdated(Thumbnail { profile_id, image })
        .emit_to(app, EventTarget::webview_window(CONNECTIONS_WINDOW))
        .inspect_err(|e| tracing::warn!(error = %e, "could not emit a thumbnail"));
}

// ---- session windows ---------------------------------------------------------------------------

/// Connects `profile`: if it already has a session window, that window is brought forward;
/// otherwise a new window is opened and the session starts in it. Returns once decided.
pub fn connect_profile<R: Runtime>(app: &AppHandle<R>, profile: Uuid) -> Result<(), CommandError> {
    let state = app.state::<AppState>();
    if let ConnectPlan::Focus(label) = state.sessions.connect_plan(profile) {
        focus_window(app, &label);
        return Ok(());
    }
    if let Some(label) = state.window_of_profile(profile) {
        focus_window(app, &label);
        return Ok(());
    }
    let entry = state.profiles.get(profile)?;
    open_session_window(app, entry.profile);
    Ok(())
}

/// Opens a session window for `profile` and its session. Returns immediately: the window is
/// built off the main thread (building and `with_webview` wait for the event loop).
fn open_session_window<R: Runtime>(app: &AppHandle<R>, profile: ConnectionProfile) {
    let label = window_label(app.state::<AppState>().next_window());
    app.state::<AppState>().set_window_profile(&label, Some(profile.clone()));
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = build_session_window(&app, &label, &profile) {
            tracing::error!(%label, error = %e, "could not open the session window");
            discard_window(&app, &label);
        }
    });
}

fn build_session_window<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    profile: &ConnectionProfile,
) -> Result<(), CommandError> {
    let tauri_error = |e: tauri::Error| platform_error(e.to_string());
    let config = template(app, SESSION_TEMPLATE, label).map_err(tauri_error)?;
    let window = WebviewWindowBuilder::from_config(app, &config)
        .map_err(tauri_error)?
        .title(present::display_name(&profile.name))
        // The page paints over the live picture: overlays and HUDs (ADR M7-3).
        .transparent(true)
        .build()
        .map_err(tauri_error)?;
    let web = SendView(page_view_of(&window)?);
    let (cmd_as, touch) = (profile.keyboard.cmd_as, profile.touch);
    let label2 = label.to_owned();
    let clipboard = profile.clipboard;
    let app3 = app.clone();
    let app2 = app.clone();
    on_main(app, move |mtm| -> Result<(), CommandError> {
        let web = web.into_inner();
        let view = drift_ios::webview::attach_below(mtm, &web).map_err(|e| platform_error(e.to_string()))?;
        let link = SessionLink::new();
        view.set_handler(link.clone());
        view.set_cmd_as(cmd_as);
        view.set_touch(touch, drift_ios::view::is_phone(mtm));
        view.setHidden(true);
        let paste = {
            let link = link.clone();
            PasteButton::install(mtm, &view, move |pasted: Pasted| {
                let contents = ClipboardContents::from_paste(pasted.text, pasted.png, clipboard);
                for cmd in crate::paste::paste_commands(contents, clipboard) {
                    link.send(cmd);
                }
            })
        };
        if paste.is_none() {
            tracing::warn!("could not add the paste button");
        }
        let parent = web.superview().ok_or_else(|| platform_error("the page is not in a window"))?;
        let switcher = SwitcherButton::install(mtm, &parent, move |id| pick(&app3, &id));
        let plat = WindowPlatform {
            view,
            web,
            surface: Surface::Webview,
            link,
            render: None,
            switcher,
            paste,
            clipboard,
        };
        let app4 = app2.clone();
        phone_shell::attach(mtm, &app4, &label2, &plat.view, &plat.web);
        layout(&plat);
        PLATFORM.with(|p| {
            p.borrow_mut().insert(label2, plat);
        });
        Ok(())
    })??;
    app.state::<AppState>().sessions.open(label, profile.clone())?;
    let app2 = app.clone();
    let _ = on_main(app, move |_| refresh_shell(&app2));
    Ok(())
}

/// Throws away a window that could not be opened (closing its session, if it started).
fn discard_window<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let closing = app.state::<AppState>().sessions.close(label);
    tauri::async_runtime::spawn(closing);
    let label = label.to_owned();
    let app2 = app.clone();
    let _ = on_main(app, move |_| {
        PLATFORM.with(|p| p.borrow_mut().remove(&label));
        if let Some(window) = app2.get_webview_window(&label) {
            let _ = window.destroy();
        }
        app2.state::<AppState>().set_window_profile(&label, None);
        refresh_shell(&app2);
    });
}

/// Session window labels in opening order (main thread only).
pub fn session_labels() -> Vec<String> {
    let mut labels: Vec<String> =
        PLATFORM.with(|p| p.try_borrow().map(|m| m.keys().cloned().collect()).unwrap_or_default());
    labels.sort_by_key(|l| window_number(l));
    labels
}

/// Brings window `label` forward: its scene on iPad, the window itself on iPhone. Returns at
/// once.
pub(crate) fn focus_window<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let label = label.to_owned();
    defer_main(app, move |mtm| {
        let page = if label == CONNECTIONS_WINDOW {
            SHELL.with(|s| s.borrow().connections_web.clone())
        } else {
            with_platform(&label, |plat| plat.web.clone())
        };
        if let Some(page) = page {
            drift_ios::webview::bring_to_front(mtm, &page);
        }
    });
}

// ---- session wiring (called by the host) -----------------------------------------------------

/// Creates the window's render thread over its `RemoteView`'s `CAMetalLayer` and returns the
/// [`FrameSink`] for the session actor.
pub(crate) fn create_render_sink<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
) -> Result<Box<dyn FrameSink>, CommandError> {
    let label = label.to_owned();
    on_main(app, move |_| {
        let gpu = GPU.with(|gpu| gpu.borrow_mut().get_or_insert_with(Gpu::system_default).clone());
        let gpu = gpu.map_err(|e| CommandError::Platform { message: e.to_string() })?;
        with_platform(&label, |plat| {
            let target = LayerTarget::new(&gpu, plat.view.metal_layer());
            let thread = RenderThread::spawn(&label, move || Compositor::new(gpu, target))
                .map_err(|e| CommandError::Platform { message: e.to_string() })?;
            let sink: RenderSink = thread.sink();
            plat.render = Some(thread);
            Ok(Box::new(sink) as Box<dyn FrameSink>)
        })
        .unwrap_or_else(|| Err(CommandError::Platform { message: format!("window {label} is gone") }))
    })?
}

/// Points the window's input at its new session actor and reports the current geometry.
pub(crate) fn attach_session<R: Runtime>(app: &AppHandle<R>, label: &str, handle: SessionHandle) {
    let label = label.to_owned();
    let _ = on_main(app, move |_| {
        with_platform(&label, |plat| {
            plat.link.set(Some(handle));
            plat.view.sync_drawable_size();
            plat.link.send(SessionCommand::Resize(plat.view.geometry()));
            plat.link.send(SessionCommand::SetVisible(true));
            plat.link.send(SessionCommand::Focus(true));
        });
    });
}

/// Applies a new [`SessionView`] to the window: emits it to the page, updates the gallery and
/// switches between the page and the live picture.
pub(crate) fn apply_view<R: Runtime>(app: &AppHandle<R>, label: &str, view: &SessionView) {
    let _ = SessionViewChanged(view.clone())
        .emit_to(app, EventTarget::webview_window(label))
        .inspect_err(|e| tracing::warn!(error = %e, "could not emit the session view"));
    let accessibility_label = present::accessibility_label(Some(view));
    let surface = present::surface_for(view);
    let live = present::status_for(view) == ConnectionStatus::Live;
    let desktop = match view.state {
        drift_core::SessionState::Connected { desktop, .. } => Some(desktop),
        _ => None,
    };
    let label = label.to_owned();
    let app2 = app.clone();
    let _ = on_main(app, move |mtm| {
        with_platform(&label, |plat| {
            plat.view.set_accessibility_label(&accessibility_label);
            match desktop {
                Some(size) => plat.view.set_desktop(size),
                None => plat.view.clear_desktop(),
            }
            plat.surface = surface;
            if !phone_shell::layout(&app2, &label, plat) {
                layout(plat);
            }
            route_input(mtm, plat);
        });
        let went_live = SHELL.with(|s| {
            let Ok(mut shell) = s.try_borrow_mut() else { return false };
            if !live {
                shell.live.remove(&label);
                return false;
            }
            let first = shell.live.insert(label.clone());
            if first {
                shell.throttle.went_live(&label);
            }
            first
        });
        update_pacing();
        refresh_shell(&app2);
        if went_live {
            sample_thumbnails(&app2);
        }
    });
}

/// Shows the picture and the page as `plat.surface` says ([`present::surface_for`]): the page
/// fills the window, floats transparent over the picture, shrinks to a HUD corner panel, or is
/// hidden so every touch reaches the picture.
fn layout(plat: &WindowPlatform) {
    let Some((width, height)) = drift_ios::webview::container_size(&plat.web) else { return };
    plat.view.setHidden(plat.surface == Surface::Webview);
    plat.web.setHidden(plat.surface == Surface::Remote);
    let (top, left) = drift_ios::webview::safe_area(&plat.web);
    plat.switcher.place(top, left);
    plat.switcher.bring_to_front();
    if let Some(paste) = &plat.paste {
        paste.place(width, height);
        let picture = matches!(plat.surface, Surface::Remote | Surface::Hud(_));
        paste.set_visible(picture && plat.clipboard != ClipboardPrefs::Off);
    }
    match plat.surface {
        Surface::Webview | Surface::Overlay | Surface::Remote => {
            drift_ios::webview::place_page(&plat.web, 0.0, 0.0, width, height, true);
        }
        Surface::Hud(hud) => {
            let frame = present::hud_frame(Size::new(width, height), hud, true);
            drift_ios::webview::place_page(&plat.web, frame.x, frame.y, frame.width, frame.height, false);
        }
    }
}

/// Hardware input goes to the picture while it is on screen (live or under a HUD); the pointer
/// is locked only for the live picture (IOS-6). Otherwise the page keeps the keyboard.
fn route_input(mtm: MainThreadMarker, plat: &WindowPlatform) {
    let picture = matches!(plat.surface, Surface::Remote | Surface::Hud(_));
    if picture {
        hardware::set_active(Some(&plat.view));
        plat.view.becomeFirstResponder();
    } else if hardware::active().is_some_and(|a| std::ptr::eq(&*a, &*plat.view)) {
        hardware::set_active(None);
    }
    drift_ios::pointer_lock::set_locked_for_view(mtm, &plat.view, plat.surface == Surface::Remote);
}

/// Runs the 120 Hz display link while any live picture is on screen.
fn update_pacing() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    SHELL.with(|s| {
        let Ok(mut shell) = s.try_borrow_mut() else { return };
        let active = !shell.live.is_empty();
        shell.pacing.get_or_insert_with(|| FramePacing::new(mtm)).set_active(active);
    });
}

/// Session window `label`'s identity: from its session, or from its profile while the session
/// is still starting.
pub(crate) fn window_identity<R: Runtime>(app: &AppHandle<R>, label: &str) -> Option<WindowIdentity> {
    let state = app.state::<AppState>();
    let profile = state.sessions.profile(label).or_else(|| state.window_profile(label))?;
    let view = state.sessions.view(label).unwrap_or_else(|| SessionView::new(&profile));
    Some(present::identity(&profile, &view))
}

/// The remote pointer shape. Touch has no pointer to draw; the iPad pointer arrives with IOS-6.
pub(crate) fn apply_cursor<R: Runtime>(
    _app: &AppHandle<R>,
    _label: &str,
    _shape: drift_apple::CursorShape,
    _scale: u32,
) {
}

/// Releases the window's session resources (render thread, handle) and drops its thumbnail.
pub(crate) fn release_session<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let label = label.to_owned();
    let app2 = app.clone();
    let _ = on_main(app, move |_| {
        with_platform(&label, |plat| {
            plat.view.release_all();
            plat.view.clear_desktop();
            plat.link.set(None);
            if let Some(render) = plat.render.take() {
                render.shutdown();
            }
        });
        SHELL.with(|s| {
            if let Ok(mut shell) = s.try_borrow_mut() {
                shell.live.remove(&label);
                shell.throttle.ended(&label);
            }
        });
        update_pacing();
        if let Some(profile) = app2.state::<AppState>().sessions.profile_id(&label) {
            emit_thumbnail(&app2, profile, None);
        }
    });
}

/// Keyboard focus follows the surface on macOS; on iOS the page keeps it until IOS-6 gives the
/// picture hardware keyboard input.
pub(crate) fn focus_content<R: Runtime>(_app: &AppHandle<R>, _label: &str) -> Result<(), CommandError> {
    Ok(())
}

// ---- closing ------------------------------------------------------------------------------------

/// iOS has no close button; the page's Disconnect calls this. Connections is never closed.
pub(crate) fn request_close<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let view = app.state::<AppState>().sessions.view(label);
    match present::close_action(label, view.as_ref()) {
        CloseAction::Hide => {}
        CloseAction::CloseNow | CloseAction::Confirm => close_session_window(app, label),
    }
}

/// Closes session window `label`: the session is closed gracefully (2 s cap), then the window
/// is destroyed and Connections comes forward.
pub fn close_session_window<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let state = app.state::<AppState>();
    if label == CONNECTIONS_WINDOW || !state.begin_closing(label) {
        return;
    }
    let profile = state.sessions.profile_id(label).or_else(|| state.window_profile(label).map(|p| p.id));
    let closing = state.sessions.close(label);
    let app = app.clone();
    let label = label.to_owned();
    tauri::async_runtime::spawn(async move {
        if !closing.await {
            tracing::warn!(%label, "session did not close within the cap; abandoning it");
        }
        let (app2, label2) = (app.clone(), label.clone());
        let _ = on_main(&app, move |_| {
            PLATFORM.with(|p| p.borrow_mut().remove(&label2));
            phone_shell::remove(&label2);
            SHELL.with(|s| {
                if let Ok(mut shell) = s.try_borrow_mut() {
                    shell.live.remove(&label2);
                    shell.throttle.ended(&label2);
                }
            });
            update_pacing();
            if let Some(window) = app2.get_webview_window(&label2) {
                let _ = window.destroy();
            }
        });
        let state = app.state::<AppState>();
        state.set_window_profile(&label, None);
        state.finish_closing(&label);
        if let Some(profile) = profile {
            emit_thumbnail(&app, profile, None);
        }
        show_connections(&app, None);
        let app2 = app.clone();
        let _ = on_main(&app, move |_| refresh_shell(&app2));
    });
}

/// iOS terminates apps without asking; close what can be closed within the cap.
pub(crate) fn shutdown_blocking<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    if !state.begin_quit() || state.sessions.live_sessions() == 0 {
        return;
    }
    let shutdown = state.sessions.shutdown();
    let report = tauri::async_runtime::block_on(shutdown);
    tracing::info!(graceful = report.graceful, abandoned = report.abandoned, "shutdown on exit");
}

// ---- intents ----------------------------------------------------------------------------------

/// "Reconnect": tells a live session to retry now, or starts the window's profile again in the
/// same window.
pub(crate) fn reconnect<R: Runtime>(app: &AppHandle<R>, label: &str) -> Result<(), CommandError> {
    let state = app.state::<AppState>();
    match state.sessions.reconnect_now(label)? {
        Reconnect::Sent => Ok(()),
        Reconnect::Reopen(profile) => state.sessions.open(label, state.profiles.get(profile)?.profile),
    }
}

/// Pushes the gallery state after a change (main thread).
fn refresh_shell<R: Runtime>(app: &AppHandle<R>) {
    push_connections(app);
    refresh_switchers(app);
}

/// Re-sends the gallery state to a rebuilt Connections page.
fn push_connections_forced<R: Runtime>(app: &AppHandle<R>) {
    SHELL.with(|s| {
        if let Ok(mut shell) = s.try_borrow_mut() {
            shell.connections = None;
        }
    });
    push_connections(app);
}

/// Rebuilds every session window's switcher menu from the current sessions (main thread).
fn refresh_switchers<R: Runtime>(app: &AppHandle<R>) {
    let sessions: Vec<(String, String, ConnectionStatus)> = app
        .state::<AppState>()
        .sessions
        .sessions()
        .into_iter()
        .map(|s| (s.window, s.profile.name, present::status_for(&s.view)))
        .collect();
    PLATFORM.with(|p| {
        let Ok(map) = p.try_borrow() else { return };
        for (label, plat) in map.iter() {
            let menu: Vec<MenuEntry> = crate::switcher::entries(label, &sessions)
                .into_iter()
                .map(|e| MenuEntry { id: e.id, title: e.title, subtitle: e.subtitle, checked: e.current })
                .collect();
            plat.switcher.set_entries(&menu);
        }
    });
}

/// A switcher choice: Connections or a session window that still exists.
pub(crate) fn pick<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match crate::switcher::target(id, &session_labels()) {
        Some(crate::switcher::Target::Connections) => show_connections(app, None),
        Some(crate::switcher::Target::Session(label)) => focus_window(app, &label),
        None => {}
    }
    // Which window ended up in front a moment later (iPad activates scenes asynchronously),
    // for the logs (`RUST_LOG=drift_app=debug`).
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        defer_main(&app, |_| {
            let front: Vec<String> = PLATFORM.with(|p| {
                p.borrow()
                    .iter()
                    .filter(|(_, plat)| drift_ios::webview::is_front(&plat.web))
                    .map(|(label, _)| label.clone())
                    .collect()
            });
            tracing::debug!(?front, "switcher: session windows in front");
        });
    });
}

/// Writes remote clipboard contents to the general pasteboard, but only while the window is in
/// front (plan M5-2: only the focused session syncs). Writing never shows a prompt on iOS.
pub(crate) fn write_pasteboard<R: Runtime>(app: &AppHandle<R>, label: &str, contents: ClipboardContents) {
    let label = label.to_owned();
    let _ = on_main(app, move |_| {
        let front = with_platform(&label, |plat| drift_ios::webview::is_front(&plat.web)).unwrap_or(false);
        tracing::debug!(%label, front, "remote clipboard");
        if front {
            UiPasteboard::general().write(&contents);
        }
    });
}
