//! The Connections window, session windows, the menu bar and the Dock menu (tasks **M6-2**,
//! **M1-6**, **M2-4**, **M8-3**, **UI-windows**).
//!
//! Humble object: every decision comes from [`crate::present`], [`crate::menu`],
//! [`crate::dock`], [`crate::frames`], [`crate::connections`] or the
//! [`SessionManager`](crate::manager::SessionManager); this module only talks to Tauri and
//! AppKit (ADR `UI-windows-gallery`).
//!
//! * **Connections** (label [`CONNECTIONS_WINDOW`]) is created at launch and only ever hidden;
//!   its page draws its own toolbar in the transparent title bar.
//! * Every connected profile has one **session window** (`session-<n>`), built hidden from the
//!   `session` template with its remembered frame, then shown. A `RemoteView` sits below the
//!   page's `WKWebView`; a transparent child webview `<label>-titlebar` draws the identity
//!   capsule in the 52-point title bar. The page is hidden while a live picture is on screen,
//!   made transparent over it for the reconnect overlay, or shrunk to a corner panel for a HUD
//!   ([`crate::present::surface_for`], [`layout`]).
//! * No window ever joins a tab group (`tabbingMode = Disallowed`).
//! * Per-window AppKit state (views, observer, render thread) lives in main-thread-only maps.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use drift_core::{ConnectionProfile, InputEvent, KeyboardPrefs, Size, ViewGeometry};
use drift_gfx::FrameSink;
use drift_input::ScaleMode;
use drift_macos::alert::{self, AlertText};
use drift_macos::dock::DockMenuItem;
use drift_macos::{RemoteView, RemoteViewHandler, WindowEvent as MacWindowEvent, WindowObserver, tauri_glue};
use drift_rdp::{SessionCommand, SessionHandle};
use drift_render::{Compositor, Gpu, LayerTarget, RenderThread};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSScreen, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use tauri::menu::{CheckMenuItemBuilder, Menu, MenuItemBuilder, PredefinedMenuItem, Submenu};
use tauri::webview::WebviewBuilder;
use tauri::{
    AppHandle, EventTarget, LogicalPosition, LogicalSize, Manager as _, Runtime, WebviewUrl,
    WebviewWindowBuilder, Window,
};
use tauri_specta::Event as _;
use uuid::Uuid;

use crate::commands::AppState;
use crate::connections::{
    CONNECTIONS_WINDOW, ConnectionStatus, Connections, ConnectionsChanged, ConnectionsIntent,
    ConnectionsIntentRequested, THUMBNAIL_MAX, Thumbnail, ThumbnailThrottle, ThumbnailUpdated,
    WindowIdentity, WindowIdentityChanged,
};
use crate::dock::{DockAction, DockItem, dock_menu};
use crate::frames::{self, CONNECTIONS_KEY, Frame, Template, profile_key};
use crate::manager::{ConnectPlan, Reconnect};
use crate::menu::{MenuAction, MenuEntry, SessionItem, Standard, SubmenuSpec, accelerator, menu_spec};
use crate::present::{self, CloseAction, Confirmation, Focus, Surface};
use crate::profiles::CommandError;
use crate::view::{SessionView, SessionViewChanged};

/// Label of the Connections window template in tauri.conf.json (it is also the window's label).
pub const CONNECTIONS_TEMPLATE: &str = CONNECTIONS_WINDOW;

/// Label of the session window template in tauri.conf.json.
pub const SESSION_TEMPLATE: &str = "session";

/// The page of every session window's title-bar webview (built by `ui/build.ts`).
pub const TITLEBAR_PAGE: &str = "titlebar.html";

/// Tauri label of the `n`th session window.
pub fn window_label(n: u64) -> String {
    format!("session-{n}")
}

/// Label of session window `window`'s title-bar webview. It starts with the window's label, so
/// the `session-*` capability covers it.
pub fn titlebar_label(window: &str) -> String {
    format!("{window}-titlebar")
}

/// The opening number of a session window label (`session-<n>`).
fn window_number(label: &str) -> u64 {
    label.strip_prefix("session-").and_then(|n| n.parse().ok()).unwrap_or(u64::MAX)
}

// ---- per-window AppKit state (main thread only) --------------------------------------------

type RenderSink = drift_render::RenderSink<Compositor<LayerTarget>>;

struct WindowPlatform {
    view: Retained<RemoteView>,
    window: Retained<NSWindow>,
    /// The page's `WKWebView` (connecting stages, prompts, overlays, HUDs).
    web: Retained<NSView>,
    /// The title bar's `WKWebView`, once it has been added.
    titlebar: Option<Retained<NSView>>,
    /// What the window shows; drives [`layout`] and keyboard focus.
    surface: Surface,
    /// The last identity pushed to the title bar (identical pushes are skipped).
    identity: Option<WindowIdentity>,
    link: Rc<SessionLink>,
    _observer: WindowObserver,
    render: Option<RenderThread<Compositor<LayerTarget>>>,
    #[cfg(feature = "recording")]
    recording: Option<crate::recording::Recording>,
}

struct ConnectionsPlatform {
    window: Retained<NSWindow>,
    web: Option<Retained<NSView>>,
    _observer: WindowObserver,
}

/// App-wide main-thread state: the pushed menu and gallery state, and the thumbnail throttle.
#[derive(Default)]
struct Shell {
    menu: Option<Vec<SubmenuSpec>>,
    connections: Option<Connections>,
    throttle: ThumbnailThrottle,
    live: HashSet<String>,
}

thread_local! {
    static PLATFORM: RefCell<HashMap<String, WindowPlatform>> = RefCell::new(HashMap::new());
    static CONNECTIONS: RefCell<Option<ConnectionsPlatform>> = const { RefCell::new(None) };
    static SHELL: RefCell<Shell> = RefCell::new(Shell::default());
    /// The shared Metal device and pipelines (plan §2: one per process).
    static GPU: RefCell<Option<Result<Gpu, drift_render::RenderError>>> = const { RefCell::new(None) };
}

/// Forwards `RemoteView` output to the window's session actor (M1-6 wiring).
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

fn ns_window_of(label: &str) -> Option<Retained<NSWindow>> {
    if label == CONNECTIONS_WINDOW {
        CONNECTIONS.with(|c| c.try_borrow().ok()?.as_ref().map(|c| c.window.clone()))
    } else {
        PLATFORM.with(|p| p.try_borrow().ok()?.get(label).map(|plat| plat.window.clone()))
    }
}

/// Runs `f` on the main thread (inline when already there) and returns its result.
///
/// From another thread the work takes two hops: tao's `run_on_main_thread` first, so it runs
/// after every message already queued for the event loop (a window built with
/// `WebviewWindowBuilder::build` only exists once tao has handled its creation message), then
/// the main dispatch queue ([`drift_macos::dispatch_main`]), so it runs **outside** tao's event
/// handler: AppKit calls that draw synchronously would otherwise re-enter tao's handler and
/// deadlock on its lock (UI-tabs decision 12).
pub(crate) fn on_main<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    f: impl FnOnce(MainThreadMarker) -> T + Send + 'static,
) -> Result<T, CommandError> {
    if let Some(mtm) = MainThreadMarker::new() {
        return Ok(f(mtm));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        drift_macos::dispatch_main(move || {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let _ = tx.send(f(mtm));
        });
    })
    .map_err(|e| CommandError::Platform { message: e.to_string() })?;
    rx.recv().map_err(|_| CommandError::Platform { message: "the main thread went away".into() })
}

/// Runs `f` on the main thread **later**, with the same two hops as [`on_main`], also when
/// called on the main thread: showing windows, sheets and modal alerts must never run inside
/// tao's event handler (a menu click, a command).
pub(crate) fn defer_main<R: Runtime>(app: &AppHandle<R>, f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    let result = app.run_on_main_thread(move || {
        drift_macos::dispatch_main(move || {
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

// ---- templates and frames ---------------------------------------------------------------------

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
        .ok_or(tauri::Error::WindowNotFound)?;
    config.label = label.to_owned();
    config.create = true;
    Ok(config)
}

fn limits(config: &tauri::utils::config::WindowConfig) -> Template {
    Template {
        width: config.width,
        height: config.height,
        min_width: config.min_width.unwrap_or(0.0),
        min_height: config.min_height.unwrap_or(0.0),
    }
}

/// Height of the primary screen (the one with the menu bar), which anchors AppKit's
/// bottom-left screen coordinates.
fn primary_height(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm).firstObject().map_or(0.0, |s| s.frame().size.height)
}

fn top_left(rect: NSRect, primary: f64) -> Frame {
    Frame {
        x: rect.origin.x,
        y: primary - rect.origin.y - rect.size.height,
        width: rect.size.width,
        height: rect.size.height,
    }
}

fn appkit_rect(frame: Frame, primary: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(frame.x, primary - frame.y - frame.height),
        NSSize::new(frame.width, frame.height),
    )
}

/// The usable area of every screen, top-left origin, the screen with the key window first.
fn screen_frames(mtm: MainThreadMarker) -> Vec<Frame> {
    let primary = primary_height(mtm);
    let mut screens: Vec<Frame> =
        NSScreen::screens(mtm).iter().map(|s| top_left(s.visibleFrame(), primary)).collect();
    if let Some(main) = NSScreen::mainScreen(mtm) {
        let main = top_left(main.visibleFrame(), primary);
        if let Some(i) = screens.iter().position(|s| *s == main) {
            screens.swap(0, i);
        }
    }
    screens
}

/// Places `window` at its restored frame (main thread).
fn place(mtm: MainThreadMarker, window: &NSWindow, saved: Option<Frame>, template: Template, cascade: usize) {
    let frame = frames::restore(saved, &screen_frames(mtm), template, cascade);
    window.setFrame_display(appkit_rect(frame, primary_height(mtm)), false);
}

/// Saves window `label`'s frame under its key (main thread; not in full screen).
fn save_frame<R: Runtime>(app: &AppHandle<R>, mtm: MainThreadMarker, label: &str) {
    let state = app.state::<AppState>();
    let key = if label == CONNECTIONS_WINDOW {
        CONNECTIONS_KEY.to_owned()
    } else {
        match state.sessions.profile_id(label).or_else(|| state.window_profile(label).map(|p| p.id)) {
            // A deleted profile keeps no frame.
            Some(id) if state.profiles.get(id).is_ok() => profile_key(id),
            _ => return,
        }
    };
    let Some(window) = ns_window_of(label) else { return };
    if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return;
    }
    let frame = top_left(window.frame(), primary_height(mtm));
    if let Err(e) = state.frames.set(&key, frame) {
        tracing::warn!(error = %e, "could not save a window frame");
    }
}

/// Saves the frame of every Drift window (quit).
fn save_all_frames<R: Runtime>(app: &AppHandle<R>) {
    let app2 = app.clone();
    let _ = on_main(app, move |mtm| {
        for label in session_labels().into_iter().chain([CONNECTIONS_WINDOW.to_owned()]) {
            save_frame(&app2, mtm, &label);
        }
    });
}

// ---- the Connections window -------------------------------------------------------------------

/// Creates the Connections window (at launch; returns immediately — the window is built off the
/// main thread and finished on it) and then connects `autoconnect`, if any.
pub(crate) fn open_connections_window<R: Runtime>(app: &AppHandle<R>, autoconnect: Option<Uuid>) {
    let app = app.clone();
    std::thread::spawn(move || {
        let built = template(&app, CONNECTIONS_TEMPLATE, CONNECTIONS_WINDOW).and_then(|config| {
            let limits = limits(&config);
            WebviewWindowBuilder::from_config(&app, &config)?.visible(false).build().map(|w| (w, limits))
        });
        let limits = match built {
            Ok((_, limits)) => limits,
            Err(e) => {
                tracing::error!(error = %e, "could not create the Connections window");
                return;
            }
        };
        let app2 = app.clone();
        let prepared = on_main(&app, move |mtm| -> Result<(), CommandError> {
            let window2 = window_of(&app2, CONNECTIONS_WINDOW)
                .ok_or_else(|| platform_error("the Connections window is gone"))?;
            let ns = tauri_glue::ns_window(&window2).map_err(|e| platform_error(e.to_string()))?;
            drift_macos::window::disallow_tabbing(&ns);
            let saved = app2.state::<AppState>().frames.get(CONNECTIONS_KEY);
            place(mtm, &ns, saved, limits, 0);
            let content = tauri_glue::content_view(&window2).map_err(|e| platform_error(e.to_string()))?;
            let web = drift_macos::webview::find_webview(&content);
            let handle = app2.clone();
            let observer = WindowObserver::new(&ns, move |event| match event {
                MacWindowEvent::Occlusion { visible } => connections_visible(&handle, visible),
                MacWindowEvent::Key(_) => request_menu_refresh(&handle),
                MacWindowEvent::FullScreen(_) | MacWindowEvent::Resized => {}
            });
            CONNECTIONS.with(|c| {
                *c.borrow_mut() = Some(ConnectionsPlatform { window: ns.clone(), web, _observer: observer });
            });
            window2.show().map_err(|e| platform_error(e.to_string()))?;
            ns.makeKeyAndOrderFront(None);
            Ok(())
        });
        if let Err(e) = prepared.and_then(|r| r) {
            tracing::error!(error = %e, "could not show the Connections window");
        }
        if let Some(profile) = autoconnect
            && let Err(e) = connect_profile(&app, profile)
        {
            tracing::error!(error = %e, "autoconnect failed");
        }
    });
}

/// Shows the Connections window and makes it key (Cmd+0, File ▸ Show Connections, the Dock
/// menu, the title bar's grid button, reopening the app); with `intent`, asks its page to open
/// a sheet. Returns at once.
pub fn show_connections<R: Runtime>(app: &AppHandle<R>, intent: Option<ConnectionsIntent>) {
    let app2 = app.clone();
    defer_main(app, move |mtm| {
        let Some((window, web)) =
            CONNECTIONS.with(|c| c.borrow().as_ref().map(|c| (c.window.clone(), c.web.clone())))
        else {
            return;
        };
        if window.isMiniaturized() {
            window.deminiaturize(None);
        }
        NSApplication::sharedApplication(mtm).activate();
        window.makeKeyAndOrderFront(None);
        if let Some(web) = web {
            drift_macos::webview::focus_webview(&window, &web);
        }
        if let Some(intent) = intent {
            let _ = ConnectionsIntentRequested(intent)
                .emit_to(&app2, EventTarget::webview_window(CONNECTIONS_WINDOW))
                .inspect_err(|e| tracing::warn!(error = %e, "could not emit a Connections intent"));
        }
    });
}

/// Hides the Connections window (its close button; it is only destroyed on quit).
fn hide_connections<R: Runtime>(app: &AppHandle<R>) {
    let app2 = app.clone();
    defer_main(app, move |mtm| {
        save_frame(&app2, mtm, CONNECTIONS_WINDOW);
        if let Some(window) = ns_window_of(CONNECTIONS_WINDOW) {
            window.orderOut(None);
        }
    });
}

/// The Connections window became visible or hidden: thumbnails only flow while it shows.
fn connections_visible<R: Runtime>(app: &AppHandle<R>, visible: bool) {
    let became_visible = SHELL.with(|s| {
        let Ok(mut shell) = s.try_borrow_mut() else { return false };
        let was = shell.throttle.visible();
        shell.throttle.set_visible(visible);
        visible && !was
    });
    if became_visible {
        sample_thumbnails(app);
    }
}

/// Pushes the gallery's "Open" section to the Connections page if it changed (main thread).
fn push_connections<R: Runtime>(app: &AppHandle<R>) {
    let connections = app.state::<AppState>().sessions.connections();
    // The uptime ticks locally in the page; a new number alone is not worth a push.
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

// ---- live thumbnails (ADR decision 8) ----------------------------------------------------------

/// Starts the thumbnail clock: once a second the throttle says which live sessions are due.
pub(crate) fn start_thumbnails<R: Runtime>(app: &AppHandle<R>) {
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

/// Samples every due live session (main thread); the read-back, downscale and PNG encoding run
/// on a worker thread, and the result goes to the Connections page only.
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

/// Connects `profile` (commands, menus, Dock, autoconnect; any thread): if the profile already
/// has a session window, that window is brought forward; otherwise a new window is opened and
/// the session starts in it (ADR decision 3). Returns once the decision is made.
pub fn connect_profile<R: Runtime>(app: &AppHandle<R>, profile: Uuid) -> Result<(), CommandError> {
    let state = app.state::<AppState>();
    if let ConnectPlan::Focus(label) = state.sessions.connect_plan(profile) {
        focus_window(app, &label);
        return Ok(());
    }
    if let Some(label) = state.window_of_profile(profile) {
        // Still being built.
        focus_window(app, &label);
        return Ok(());
    }
    let entry = state.profiles.get(profile)?;
    open_session_window(app, entry.profile, true);
    Ok(())
}

/// Builds a session window for `profile` without starting a session, for the `overlays_ui`
/// test (which drives the window's view directly).
#[cfg(feature = "macos-ui-tests")]
pub fn open_window_for_tests<R: Runtime>(app: &AppHandle<R>, profile: ConnectionProfile) {
    open_session_window(app, profile, false);
}

/// Opens a session window for `profile` and (with `start`) its session. Returns immediately:
/// the window is built off the main thread, because `WebviewWindowBuilder::build` and
/// `Window::add_child` dispatch to the event loop, and finished on the main thread.
fn open_session_window<R: Runtime>(app: &AppHandle<R>, profile: ConnectionProfile, start: bool) {
    let label = window_label(app.state::<AppState>().next_window());
    app.state::<AppState>().set_window_profile(&label, Some(profile.clone()));
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = build_session_window(&app, &label, &profile, start) {
            tracing::error!(%label, error = %e, "could not open the session window");
            discard_window(&app, &label);
        }
    });
}

fn build_session_window<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    profile: &ConnectionProfile,
    start: bool,
) -> Result<(), CommandError> {
    let tauri_error = |e: tauri::Error| platform_error(e.to_string());
    // Size, title bar, traffic lights and window material come from the `session` window
    // template in tauri.conf.json (`create: false`), so they can be tuned without a rebuild.
    let config = template(app, SESSION_TEMPLATE, label).map_err(tauri_error)?;
    let limits = limits(&config);
    WebviewWindowBuilder::from_config(app, &config)
        .map_err(tauri_error)?
        .title(present::display_name(&profile.name))
        // The page must be able to paint over the live picture: the reconnect overlay dims the
        // last frame and the greeter/statistics HUDs float on top of it (M7-3, M1). Needs Tauri's
        // `macos-private-api`; see docs/adr/M7-3-overlays-over-the-live-picture.md.
        .transparent(true)
        .visible(false)
        .build()
        .map_err(tauri_error)?;
    prepare_window(app, label.to_owned(), profile.id, limits)?;
    // The title bar is a webview of its own: the page is hidden or shrunk to a HUD over the live
    // picture, but the identity capsule must always be there (outside full screen).
    add_titlebar(app, label)?;
    if start {
        app.state::<AppState>().sessions.open(label, profile.clone())?;
    }
    show_session_window(app, label.to_owned())?;
    // tao moves the traffic lights when the window first draws: lay out again after that.
    std::thread::sleep(Duration::from_millis(250));
    let label = label.to_owned();
    let _ = on_main(app, move |_| sync_chrome(&label));
    Ok(())
}

/// The Tauri window of session window `label`.
fn window_of<R: Runtime>(app: &AppHandle<R>, label: &str) -> Option<Window<R>> {
    app.get_webview(label).map(|page| page.window())
}

/// Attaches the RemoteView below the page, restores the frame and registers the window (main
/// thread; the window is still hidden and has only its page webview).
fn prepare_window<R: Runtime>(
    app: &AppHandle<R>,
    label: String,
    profile: Uuid,
    limits: Template,
) -> Result<(), CommandError> {
    let app2 = app.clone();
    on_main(app, move |mtm| -> Result<(), CommandError> {
        let window =
            window_of(&app2, &label).ok_or_else(|| platform_error(format!("window {label} is gone")))?;
        let (view, web) = tauri_glue::attach(&window, KeyboardPrefs::default())
            .map_err(|e| platform_error(e.to_string()))?;
        let link = SessionLink::new();
        view.set_handler(link.clone());
        let ns = tauri_glue::ns_window(&window).map_err(|e| platform_error(e.to_string()))?;
        drift_macos::window::disallow_tabbing(&ns);
        let cascade = PLATFORM.with(|p| p.borrow().len());
        let saved = app2.state::<AppState>().frames.get(&profile_key(profile));
        place(mtm, &ns, saved, limits, cascade);
        let observer = {
            let link = link.clone();
            let app = app2.clone();
            let label = label.clone();
            // Full screen, resizes and key changes all change the layout or the menus.
            WindowObserver::new(&ns, move |event| {
                match event {
                    MacWindowEvent::Occlusion { visible } => link.send(SessionCommand::SetVisible(visible)),
                    MacWindowEvent::Key(focused) => {
                        link.send(SessionCommand::Focus(focused));
                        request_menu_refresh(&app);
                    }
                    MacWindowEvent::FullScreen(_) | MacWindowEvent::Resized => {}
                }
                sync_chrome(&label);
            })
        };
        drift_macos::view::install_key_up_monitor(mtm);
        PLATFORM.with(|p| {
            p.borrow_mut().insert(
                label.clone(),
                WindowPlatform {
                    view,
                    window: ns,
                    web,
                    titlebar: None,
                    surface: Surface::Webview,
                    identity: None,
                    link,
                    _observer: observer,
                    render: None,
                    #[cfg(feature = "recording")]
                    recording: None,
                },
            );
        });
        Ok(())
    })?
}

/// Adds the title-bar webview to window `label` (off the main thread: `add_child` waits for
/// the event loop) and remembers its `WKWebView`.
fn add_titlebar<R: Runtime>(app: &AppHandle<R>, label: &str) -> Result<(), CommandError> {
    let window = window_of(app, label).ok_or_else(|| platform_error(format!("window {label} is gone")))?;
    let builder =
        WebviewBuilder::new(titlebar_label(label), WebviewUrl::App(TITLEBAR_PAGE.into())).transparent(true);
    // `layout` owns the frame from here on; this is only the initial size.
    window
        .add_child(
            builder,
            LogicalPosition::new(0.0, 0.0),
            LogicalSize::new(1280.0, present::TITLEBAR_HEIGHT),
        )
        .map_err(|e| platform_error(e.to_string()))?;
    let label = label.to_owned();
    on_main(app, move |_| -> Result<(), CommandError> {
        let content = tauri_glue::content_view(&window).map_err(|e| platform_error(e.to_string()))?;
        with_platform(&label, |plat| {
            plat.titlebar = drift_macos::webview::find_webviews(&content)
                .into_iter()
                .find(|v| !std::ptr::eq(Retained::as_ptr(v), Retained::as_ptr(&plat.web)));
        })
        .ok_or_else(|| platform_error(format!("window {label} is not registered")))
    })?
}

/// Shows a prepared session window and makes it key (main thread).
fn show_session_window<R: Runtime>(app: &AppHandle<R>, label: String) -> Result<(), CommandError> {
    let app2 = app.clone();
    on_main(app, move |mtm| -> Result<(), CommandError> {
        let window =
            window_of(&app2, &label).ok_or_else(|| platform_error(format!("window {label} is gone")))?;
        let ns = with_platform(&label, |plat| plat.window.clone())
            .ok_or_else(|| platform_error(format!("window {label} is not registered")))?;
        window.show().map_err(|e| platform_error(e.to_string()))?;
        NSApplication::sharedApplication(mtm).activate();
        ns.makeKeyAndOrderFront(None);
        sync_chrome(&label);
        focus_surface(&label);
        push_identity(&app2, &label);
        refresh_shell(&app2);
        Ok(())
    })?
}

/// Throws away a window that could not be opened (closing its session, if it started).
fn discard_window<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let closing = app.state::<AppState>().sessions.close(label);
    tauri::async_runtime::spawn(closing);
    let label = label.to_owned();
    let app2 = app.clone();
    let _ = on_main(app, move |_| {
        PLATFORM.with(|p| p.borrow_mut().remove(&label));
        if let Some(window) = window_of(&app2, &label) {
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

/// Brings session window `label` forward and gives the keyboard to its surface (returns at
/// once).
pub(crate) fn focus_window<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let label = label.to_owned();
    defer_main(app, move |mtm| {
        let Some(window) = ns_window_of(&label) else { return };
        if window.isMiniaturized() {
            window.deminiaturize(None);
        }
        NSApplication::sharedApplication(mtm).activate();
        window.makeKeyAndOrderFront(None);
        focus_surface(&label);
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
        // One Gpu (device + compiled shaders) for the whole process, as the plan requires.
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
            plat.link.send(SessionCommand::Resize(plat.view.geometry()));
            plat.link.send(SessionCommand::SetVisible(drift_macos::window::is_visible(&plat.window)));
            plat.link.send(SessionCommand::Focus(plat.window.isKeyWindow()));
        });
    });
}

/// Applies a new [`SessionView`] to the window: emits it to the page, updates the title, the
/// title bar, the gallery and the menus, and switches between the page and the live picture.
pub(crate) fn apply_view<R: Runtime>(app: &AppHandle<R>, label: &str, view: &SessionView) {
    let _ = SessionViewChanged(view.clone())
        .emit_to(app, EventTarget::webview_window(label))
        .inspect_err(|e| tracing::warn!(error = %e, "could not emit the session view"));
    let title = present::window_title(view);
    let accessibility_label = present::accessibility_label(Some(view));
    let surface = present::surface_for(view);
    let live = present::status_for(view) == ConnectionStatus::Live;
    let desktop = match view.state {
        drift_core::SessionState::Connected { desktop, .. } => Some(desktop),
        _ => None,
    };
    let label = label.to_owned();
    let app2 = app.clone();
    let _ = on_main(app, move |_| {
        let (Some(window), Some(page)) = (window_of(&app2, &label), app2.get_webview(&label)) else { return };
        // Mission Control, Cmd+` and the Window menu show it.
        let _ = window.set_title(&title);
        let Some(web) = with_platform(&label, |plat| {
            plat.view.set_accessibility_label(&accessibility_label);
            match desktop {
                Some(size) => plat.view.set_desktop(size, ScaleMode::Fit),
                None => plat.view.clear_desktop(),
            }
            plat.surface = surface;
            plat.view.setHidden(surface == Surface::Webview);
            plat.web.clone()
        }) else {
            return;
        };
        sync_chrome(&label);
        let glue = |e: drift_macos::tauri_glue::GlueError| platform_error(e.to_string());
        let switched: Result<(), CommandError> = match surface {
            Surface::Remote => {
                with_platform(&label, |plat| tauri_glue::show_remote(&window, &page, &plat.view))
                    .unwrap_or(Ok(()))
                    .map_err(glue)
            }
            // With no picture to show, the opaque Metal view would cover the window's vibrancy
            // (hidden above); the overlay keeps it for the dimmed last frame.
            Surface::Webview | Surface::Overlay => {
                tauri_glue::show_webview(&window, &page, &web).map_err(glue)
            }
            // The page is a corner panel (`layout`); everything else still reaches the picture,
            // which keeps the keyboard.
            Surface::Hud(_) => page.show().map_err(|e| platform_error(e.to_string())).map(|()| {
                with_platform(&label, |plat| drift_macos::webview::focus_remote(&plat.window, &plat.view));
            }),
        };
        if let Err(e) = switched {
            tracing::warn!(error = %e, "could not switch the window surface");
        }
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
        push_identity(&app2, &label);
        refresh_shell(&app2);
        if went_live {
            sample_thumbnails(&app2);
        }
    });
}

/// Applies a [`SessionView`] to a real window, for the `overlays_ui` test.
///
/// In production only the `SessionHost` calls [`apply_view`]; the test drives it directly to
/// check what AppKit really does with the overlay surfaces.
#[cfg(feature = "macos-ui-tests")]
pub fn apply_view_for_tests<R: Runtime>(app: &AppHandle<R>, label: &str, view: &SessionView) {
    apply_view(app, label, view);
}

/// Pushes session window `label`'s identity to its title bar if it changed (main thread).
fn push_identity<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let Some(identity) = window_identity(app, label) else { return };
    let changed = with_platform(label, |plat| {
        let changed = plat.identity.as_ref() != Some(&identity);
        plat.identity = Some(identity.clone());
        changed
    });
    if changed == Some(true) {
        let _ = WindowIdentityChanged(identity)
            .emit_to(app, EventTarget::webview(titlebar_label(label)))
            .inspect_err(|e| tracing::warn!(error = %e, "could not emit the window identity"));
    }
}

/// Session window `label`'s identity: from its session, or from its profile while the session
/// is still starting.
pub(crate) fn window_identity<R: Runtime>(app: &AppHandle<R>, label: &str) -> Option<WindowIdentity> {
    let state = app.state::<AppState>();
    let profile = state.sessions.profile(label).or_else(|| state.window_profile(label))?;
    let view = state.sessions.view(label).unwrap_or_else(|| SessionView::new(&profile));
    Some(present::identity(&profile, &view))
}

// ---- layout: title bar, page, picture and HUDs (UI-windows, M7-3, M1) --------------------------

/// Lays session window `label` out ([`layout`], main thread). A no-op while the window's
/// platform state is being set up or borrowed.
fn sync_chrome(label: &str) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    PLATFORM.with(|p| {
        let Ok(map) = p.try_borrow() else { return };
        if let Some(plat) = map.get(label) {
            layout(plat);
        }
    });
}

/// Places the title bar, the picture and the page ([`present::chrome`]):
///
/// * the title-bar webview is the top [`present::TITLEBAR_HEIGHT`] points, hidden in full
///   screen (also while the menu bar is revealed: nothing observes the reveal),
/// * the `RemoteView` fills the rest,
/// * the page fills the same area for the page-only screens and the reconnect overlay, or is
///   shrunk to [`present::hud_frame`] and pinned to its corner for a HUD — AppKit then hit-tests
///   everything outside the panel straight down to the picture.
fn layout(plat: &WindowPlatform) {
    // SAFETY: `superview` returns the (retained) parent or nil; we are on the main thread.
    let Some(parent) = (unsafe { plat.view.superview() }) else { return };
    let bounds = parent.bounds();
    let flipped = parent.isFlipped();
    let full_screen = plat.window.styleMask().contains(NSWindowStyleMask::FullScreen);
    let chrome = present::chrome(full_screen);
    let (width, height) = (bounds.size.width, bounds.size.height);
    let content_height = (height - chrome.content_top).max(1.0);
    let content = NSRect::new(
        NSPoint::new(
            bounds.origin.x,
            bounds.origin.y + present::band_y(height, chrome.content_top, content_height, flipped),
        ),
        NSSize::new(width, content_height),
    );
    let fill = NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable;
    plat.view.setAutoresizingMask(fill);
    plat.view.setFrame(content);
    if let Some(titlebar) = &plat.titlebar {
        titlebar.setHidden(chrome.titlebar <= 0.0);
        if chrome.titlebar > 0.0 {
            // Pinned to the top edge: the margin below it grows with the window.
            let below = if flipped {
                NSAutoresizingMaskOptions::ViewMaxYMargin
            } else {
                NSAutoresizingMaskOptions::ViewMinYMargin
            };
            titlebar.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | below);
            titlebar.setFrame(NSRect::new(
                NSPoint::new(
                    bounds.origin.x,
                    bounds.origin.y + present::band_y(height, 0.0, chrome.titlebar, flipped),
                ),
                NSSize::new(width, chrome.titlebar),
            ));
        }
    }
    match plat.surface {
        Surface::Webview | Surface::Overlay => {
            plat.web.setAutoresizingMask(fill);
            plat.web.setFrame(content);
        }
        Surface::Hud(hud) => {
            let frame = present::hud_frame(Size::new(width, content_height), hud, flipped);
            plat.web.setAutoresizingMask(autoresize_mask(frame.flexible, flipped));
            plat.web.setFrame(NSRect::new(
                NSPoint::new(content.origin.x + frame.x, content.origin.y + frame.y),
                NSSize::new(frame.width, frame.height),
            ));
        }
        Surface::Remote => {}
    }
}

/// Screen-direction flexibility as an AppKit autoresizing mask.
fn autoresize_mask(flexible: present::Flexible, flipped: bool) -> NSAutoresizingMaskOptions {
    let mut mask = NSAutoresizingMaskOptions::empty();
    if flexible.left {
        mask |= NSAutoresizingMaskOptions::ViewMinXMargin;
    }
    if flexible.right {
        mask |= NSAutoresizingMaskOptions::ViewMaxXMargin;
    }
    // "Min Y" is the bottom edge in AppKit's default space and the top edge in a flipped one.
    let (top, bottom) = if flipped {
        (NSAutoresizingMaskOptions::ViewMinYMargin, NSAutoresizingMaskOptions::ViewMaxYMargin)
    } else {
        (NSAutoresizingMaskOptions::ViewMaxYMargin, NSAutoresizingMaskOptions::ViewMinYMargin)
    };
    if flexible.top {
        mask |= top;
    }
    if flexible.bottom {
        mask |= bottom;
    }
    mask
}

/// Shows a remote pointer shape on the window.
pub(crate) fn apply_cursor<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    shape: drift_macos::CursorShape,
    scale: u32,
) {
    let label = label.to_owned();
    let _ = on_main(app, move |_| {
        with_platform(&label, |plat| plat.view.set_cursor_shape(&shape, scale));
    });
}

/// Releases the window's session resources (render thread, handle, cursor) and drops its
/// thumbnail.
pub(crate) fn release_session<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let label = label.to_owned();
    let app2 = app.clone();
    let _ = on_main(app, move |_| {
        with_platform(&label, |plat| {
            #[cfg(feature = "recording")]
            if let Some(recording) = plat.recording.take() {
                recording.stop();
            }
            plat.view.release_all();
            plat.link.set(None);
            plat.view.clear_desktop();
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
        if let Some(profile) = app2.state::<AppState>().sessions.profile_id(&label) {
            emit_thumbnail(&app2, profile, None);
        }
    });
}

/// Hands the keyboard to whatever session window `label`'s surface says owns it
/// ([`present::focus_for`], main thread).
fn focus_surface(label: &str) {
    with_platform(label, |plat| match present::focus_for(plat.surface) {
        Focus::Remote => drift_macos::webview::focus_remote(&plat.window, &plat.view),
        Focus::Page => drift_macos::webview::focus_webview(&plat.window, &plat.web),
    });
}

/// Hands the keyboard back to window `label`'s page or live picture; the title bar calls this
/// whenever it receives focus, so a click on it never keeps the keyboard (decision 11).
pub(crate) fn focus_content<R: Runtime>(app: &AppHandle<R>, label: &str) -> Result<(), CommandError> {
    let label = label.to_owned();
    on_main(app, move |_| {
        if label == CONNECTIONS_WINDOW {
            CONNECTIONS.with(|c| {
                if let Some(c) = c.borrow().as_ref()
                    && let Some(web) = &c.web
                {
                    drift_macos::webview::focus_webview(&c.window, web);
                }
            });
        } else {
            focus_surface(&label);
        }
    })
}

// ---- closing and quitting --------------------------------------------------------------------

fn alert_text(c: Confirmation) -> AlertText {
    AlertText { message: c.message, informative: c.informative, confirm: c.confirm, cancel: c.cancel }
}

/// The red button or Cmd+W on window `label` (ADR decisions 2 and 4): Connections hides; a
/// session window holding a desktop asks with a sheet first; anything else closes at once.
pub(crate) fn request_close<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let view = app.state::<AppState>().sessions.view(label);
    match present::close_action(label, view.as_ref()) {
        CloseAction::Hide => hide_connections(app),
        CloseAction::CloseNow => close_session_window(app, label),
        CloseAction::Confirm => {
            let name = view.map(|v| v.profile_name).unwrap_or_default();
            let text = alert_text(present::close_confirmation(&name));
            let (app2, label) = (app.clone(), label.to_owned());
            defer_main(app, move |_| {
                let Some(window) = ns_window_of(&label) else { return };
                if window.attachedSheet().is_some() {
                    return;
                }
                alert::confirm_sheet(&window, &text, move |confirmed| {
                    if confirmed {
                        close_session_window(&app2, &label);
                    }
                });
            });
        }
    }
}

/// Closes session window `label` without asking: its frame is saved, the session is closed
/// gracefully (2 s cap), then the window is destroyed and its card goes back to idle.
pub fn close_session_window<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let state = app.state::<AppState>();
    if label == CONNECTIONS_WINDOW || !state.begin_closing(label) {
        return;
    }
    {
        let (app2, label) = (app.clone(), label.to_owned());
        let _ = on_main(app, move |mtm| save_frame(&app2, mtm, &label));
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
            SHELL.with(|s| {
                if let Ok(mut shell) = s.try_borrow_mut() {
                    shell.live.remove(&label2);
                    shell.throttle.ended(&label2);
                }
            });
            if let Some(window) = window_of(&app2, &label2) {
                let _ = window.destroy();
            }
        });
        let state = app.state::<AppState>();
        state.set_window_profile(&label, None);
        state.finish_closing(&label);
        if let Some(profile) = profile {
            emit_thumbnail(&app, profile, None);
        }
        let app2 = app.clone();
        let _ = on_main(&app, move |_| refresh_shell(&app2));
    });
}

/// Drift ▸ Quit (Cmd+Q): asks once when sessions are open, then saves every frame and shuts the
/// sessions down within the cap before exiting (ADR decision 13).
pub(crate) fn quit<R: Runtime>(app: &AppHandle<R>) {
    // A failed window holds no session any more; it is not "disconnected" by quitting.
    let open = app
        .state::<AppState>()
        .sessions
        .sessions()
        .iter()
        .filter(|s| present::status_for(&s.view) != ConnectionStatus::Failed)
        .count();
    match present::quit_confirmation(open) {
        None => shutdown_and_exit(app),
        Some(text) => {
            let app2 = app.clone();
            defer_main(app, move |mtm| {
                if alert::confirm(mtm, &alert_text(text)) {
                    shutdown_and_exit(&app2);
                }
            });
        }
    }
}

fn shutdown_and_exit<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    if !state.begin_quit() {
        return;
    }
    save_all_frames(app);
    let shutdown = state.sessions.shutdown();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let report = shutdown.await;
        tracing::info!(graceful = report.graceful, abandoned = report.abandoned, "shutdown complete");
        app.exit(0);
    });
}

/// Last-resort shutdown when macOS terminates the app without asking (Dock ▸ Quit).
pub(crate) fn shutdown_blocking<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    if !state.begin_quit() {
        return;
    }
    save_all_frames(app);
    if state.sessions.live_sessions() == 0 {
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
        // Reload the profile, so a freshly pinned certificate is used.
        Reconnect::Reopen(profile) => state.sessions.open(label, state.profiles.get(profile)?.profile),
    }
}

// ---- menus and the Dock (ADR decisions 9 and 10) ---------------------------------------------

/// The key window, as Drift sees it (main thread).
enum KeyWindow {
    Connections,
    Session(String),
    Other(Retained<NSWindow>),
}

fn key_window(mtm: MainThreadMarker) -> Option<KeyWindow> {
    let key = NSApplication::sharedApplication(mtm).keyWindow()?;
    let is = |w: &NSWindow| std::ptr::eq(w, &*key);
    if ns_window_of(CONNECTIONS_WINDOW).is_some_and(|w| is(&w)) {
        return Some(KeyWindow::Connections);
    }
    let session = PLATFORM.with(|p| {
        p.try_borrow().ok()?.iter().find(|(_, plat)| is(&plat.window)).map(|(label, _)| label.clone())
    });
    Some(session.map_or(KeyWindow::Other(key), KeyWindow::Session))
}

fn key_session(mtm: MainThreadMarker) -> Option<String> {
    match key_window(mtm)? {
        KeyWindow::Session(label) => Some(label),
        KeyWindow::Connections | KeyWindow::Other(_) => None,
    }
}

fn session_items<R: Runtime>(app: &AppHandle<R>) -> Vec<SessionItem> {
    app.state::<AppState>()
        .sessions
        .sessions()
        .iter()
        .map(|s| present::session_item(&s.window, &s.view))
        .collect()
}

/// Pushes the gallery state and rebuilds the menus if they changed (main thread).
fn refresh_shell<R: Runtime>(app: &AppHandle<R>) {
    push_connections(app);
    refresh_menus(app);
}

/// Rebuilds the menu bar later (key-window changes arrive inside AppKit calls).
fn request_menu_refresh<R: Runtime>(app: &AppHandle<R>) {
    let app2 = app.clone();
    defer_main(app, move |_| refresh_menus(&app2));
}

/// Rebuilds the menu bar from [`menu_spec`] when it changed (main thread).
fn refresh_menus<R: Runtime>(app: &AppHandle<R>) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let key = match key_window(mtm) {
        Some(KeyWindow::Session(label)) => Some(label),
        Some(KeyWindow::Connections) => Some(CONNECTIONS_WINDOW.to_owned()),
        Some(KeyWindow::Other(_)) | None => None,
    };
    let spec = menu_spec(&session_items(app), key.as_deref());
    let changed = SHELL.with(|s| {
        let Ok(mut shell) = s.try_borrow_mut() else { return false };
        let changed = shell.menu.as_ref() != Some(&spec);
        if changed {
            shell.menu = Some(spec.clone());
        }
        changed
    });
    if !changed {
        return;
    }
    match build_menu(app, &spec).and_then(|menu| app.set_menu(menu)) {
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "could not rebuild the menu bar"),
    }
}

/// Builds the menu bar from `spec`. The Window submenu is an ordinary submenu, **not**
/// `NSApp.windowsMenu`: Drift lists the session windows itself.
pub(crate) fn build_menu<R: Runtime>(app: &AppHandle<R>, spec: &[SubmenuSpec]) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    for spec in spec {
        let submenu = Submenu::new(app, &spec.title, true)?;
        for entry in &spec.entries {
            match entry {
                MenuEntry::Separator => submenu.append(&PredefinedMenuItem::separator(app)?)?,
                MenuEntry::Standard(item) => submenu.append(&predefined(app, *item)?)?,
                MenuEntry::Header(title) => {
                    submenu.append(&MenuItemBuilder::new(title).enabled(false).build(app)?)?;
                }
                MenuEntry::Action { action, title, shortcut, enabled, checked: None } => {
                    let mut item = MenuItemBuilder::with_id(action.id(), title).enabled(*enabled);
                    if let Some(shortcut) = shortcut {
                        item = item.accelerator(accelerator(*shortcut));
                    }
                    submenu.append(&item.build(app)?)?;
                }
                MenuEntry::Action { action, title, shortcut, enabled, checked: Some(checked) } => {
                    let mut item =
                        CheckMenuItemBuilder::with_id(action.id(), title).enabled(*enabled).checked(*checked);
                    if let Some(shortcut) = shortcut {
                        item = item.accelerator(accelerator(*shortcut));
                    }
                    submenu.append(&item.build(app)?)?;
                }
            }
        }
        menu.append(&submenu)?;
    }
    Ok(menu)
}

fn predefined<R: Runtime>(app: &AppHandle<R>, item: Standard) -> tauri::Result<PredefinedMenuItem<R>> {
    match item {
        Standard::About => PredefinedMenuItem::about(app, None, None),
        Standard::Services => PredefinedMenuItem::services(app, None),
        Standard::Hide => PredefinedMenuItem::hide(app, None),
        Standard::HideOthers => PredefinedMenuItem::hide_others(app, None),
        Standard::ShowAll => PredefinedMenuItem::show_all(app, None),
        Standard::Undo => PredefinedMenuItem::undo(app, None),
        Standard::Redo => PredefinedMenuItem::redo(app, None),
        Standard::Cut => PredefinedMenuItem::cut(app, None),
        Standard::Copy => PredefinedMenuItem::copy(app, None),
        Standard::Paste => PredefinedMenuItem::paste(app, None),
        Standard::SelectAll => PredefinedMenuItem::select_all(app, None),
        Standard::Minimize => PredefinedMenuItem::minimize(app, None),
        Standard::Zoom => PredefinedMenuItem::maximize(app, None),
        Standard::Fullscreen => PredefinedMenuItem::fullscreen(app, Some("Enter Full Screen")),
    }
}

/// Runs a menu action (main thread).
pub(crate) fn run_menu_action<R: Runtime>(app: &AppHandle<R>, action: MenuAction) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let session = key_session(mtm);
    let state = app.state::<AppState>();
    match action {
        MenuAction::NewConnection => show_connections(app, Some(ConnectionsIntent::New)),
        MenuAction::ShowConnections => show_connections(app, None),
        MenuAction::EditConnection => {
            if let Some(profile_id) = session.and_then(|label| state.sessions.profile_id(&label)) {
                show_connections(app, Some(ConnectionsIntent::Edit { profile_id }));
            }
        }
        MenuAction::Disconnect => {
            if let Some(label) = session {
                close_session_window(app, &label);
            }
        }
        MenuAction::CloseWindow => match key_window(mtm) {
            Some(KeyWindow::Connections) => request_close(app, CONNECTIONS_WINDOW),
            Some(KeyWindow::Session(label)) => request_close(app, &label),
            Some(KeyWindow::Other(window)) => window.performClose(None),
            None => {}
        },
        MenuAction::SelectSession(n) => {
            let sessions = state.sessions.sessions();
            if let Some(s) = usize::try_from(n).ok().and_then(|n| sessions.get(n.checked_sub(1)?)) {
                focus_window(app, &s.window);
            }
        }
        MenuAction::Quit => quit(app),
        MenuAction::SendCtrlAltDel => {
            if let Some(label) = session {
                with_platform(&label, |plat| plat.view.send_ctrl_alt_del());
            }
        }
        MenuAction::Reconnect => {
            if let Some(label) = session
                && let Err(e) = reconnect(app, &label)
            {
                tracing::info!(error = %e, "menu: reconnect");
            }
        }
        MenuAction::ToggleStats => {
            if let Some(label) = session
                && let Err(e) = state.sessions.toggle_stats(&label)
            {
                tracing::info!(error = %e, "menu: show statistics");
            }
        }
        MenuAction::ToggleRecording => toggle_recording(app, session.as_deref()),
    }
}

/// Installs the Dock menu (main thread, after the application delegate exists).
pub(crate) fn install_dock<R: Runtime>(app: &AppHandle<R>) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let (provider_app, click_app) = (app.clone(), app.clone());
    let installed = drift_macos::dock::install(
        mtm,
        move || {
            dock_menu(&session_items(&provider_app))
                .into_iter()
                .map(|item| match item {
                    DockItem::Header(title) => DockMenuItem::Header(title),
                    DockItem::Separator => DockMenuItem::Separator,
                    DockItem::Action { action, title, key } => DockMenuItem::Item {
                        id: action.id(),
                        title,
                        key: key.map(String::from).unwrap_or_default(),
                    },
                })
                .collect()
        },
        move |id| match DockAction::from_id(id) {
            Some(DockAction::Focus(label)) => focus_window(&click_app, &label),
            Some(DockAction::ShowConnections) => show_connections(&click_app, None),
            Some(DockAction::NewConnection) => show_connections(&click_app, Some(ConnectionsIntent::New)),
            None => {}
        },
    );
    if let Err(e) = installed {
        tracing::warn!(error = %e, "the Dock menu is not available");
    }
}

#[cfg(feature = "recording")]
fn toggle_recording<R: Runtime>(app: &AppHandle<R>, label: Option<&str>) {
    let Some(label) = label else { return };
    let profile_name = app
        .state::<AppState>()
        .sessions
        .view(label)
        .map_or_else(|| "session".to_owned(), |view| view.profile_name);
    with_platform(label, |plat| match plat.recording.take() {
        Some(recording) => {
            if let Some(render) = &plat.render {
                render.with(drift_render::Compositor::stop_recording);
            }
            recording.stop();
        }
        None => {
            let Some(render) = &plat.render else {
                tracing::info!("Record Session: no live session in this window");
                return;
            };
            match crate::recording::start(render, &profile_name) {
                Ok(recording) => plat.recording = Some(recording),
                Err(e) => tracing::error!(error = %e, "could not start recording"),
            }
        }
    });
}

#[cfg(not(feature = "recording"))]
fn toggle_recording<R: Runtime>(app: &AppHandle<R>, label: Option<&str>) {
    let _ = (app, label);
}

/// Writes remote clipboard contents to the general pasteboard, but only while the window is the
/// key one (plan M5-2: only the focused session syncs).
pub(crate) fn write_pasteboard<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    contents: drift_clipboard::ClipboardContents,
) {
    let label = label.to_owned();
    let _ = on_main(app, move |_| {
        let key = with_platform(&label, |plat| plat.window.isKeyWindow()).unwrap_or(false);
        if key {
            use drift_clipboard::poll::PasteboardPort as _;
            drift_clipboard::pasteboard::NsPasteboard::general().write(&contents);
        }
    });
}
