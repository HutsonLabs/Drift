//! UI-windows Red: the real app's window model (ADR UI-windows-gallery decisions 1–5).
//!
//! Launches Drift (temporary config, in-memory secrets) and checks on real windows:
//!
//! * the Connections window exists at launch; its close button only hides it and
//!   `show_connections` brings it back as the key window,
//! * connecting two profiles gives two session windows — three independent windows, none of
//!   them tabbed (`tabbingMode` disallowed, `tabbedWindows` nil),
//! * connecting a profile that already has a window focuses it instead of opening another,
//! * each session window has its title-bar webview, and its traffic lights sit at the standard
//!   inset, centred in the 52-point title bar,
//! * full screen covers the whole screen with the picture and hides the title-bar webview.
//!
//! The profiles point at a closed local port, so the sessions fail quickly; a failed window
//! stays until it is dismissed, which is all this test needs.
//!
//! A Tauri event loop must own the process main thread, so this is a `harness = false` binary
//! that runs one test and exits. It needs a logged-in window server:
//! `cargo test -p drift-app --features macos-ui-tests --test windows_ui`.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use drift_app::RunOptions;
use drift_app::commands::AppState;
use drift_app::connections::CONNECTIONS_WINDOW;
use drift_app::present::TITLEBAR_HEIGHT;
use drift_app::profiles::{LinuxPasswordUpdate, SecretsUpdate};
use drift_app::windows::{
    close_session_window, connect_profile, session_labels, show_connections, titlebar_label,
};
use drift_core::{ConnectMode, ConnectionProfile};
use objc2::Message as _;
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow, NSWindowButton, NSWindowStyleMask, NSWindowTabbingMode};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

const NAME: &str = "connections_and_session_windows_are_independent_windows";

fn on_main<T: Send + 'static>(app: &AppHandle, f: impl FnOnce(&AppHandle) -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let a = app.clone();
    // Like the app's own `windows::on_main`: after tao's queued messages, then on the main
    // dispatch queue, outside tao's event handler.
    app.run_on_main_thread(move || {
        drift_macos::dispatch_main(move || {
            let _ = tx.send(f(&a));
        });
    })
    .unwrap();
    rx.recv_timeout(Duration::from_secs(10)).expect("main thread responds")
}

fn wait_until(what: &str, secs: u64, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The `NSWindow` of the window whose page webview is `label` (main thread).
fn ns_window(app: &AppHandle, label: &str) -> Option<Retained<NSWindow>> {
    let w = app.get_webview(label)?.window();
    let ptr = w.ns_window().ok()?.cast::<NSWindow>();
    // SAFETY: tao's live NSWindow; we are on the main thread and retain it.
    unsafe { ptr.as_ref() }.map(|w| w.retain())
}

fn save_profile(app: &AppHandle, name: &str) -> Uuid {
    let mut p = ConnectionProfile::new(name, "127.0.0.1", ConnectMode::Headless);
    p.port = 9; // discard: closed on a Mac, so the session fails at once
    p.rdp_username = "fake-user".into();
    let secrets =
        SecretsUpdate { rdp_password: Some("fake-pass".into()), linux_password: LinuxPasswordUpdate::Keep };
    app.state::<AppState>().profiles.save(p, secrets).unwrap().profile.id
}

/// What one window looks like, read on the main thread (asserted on the test thread: a panic
/// inside a main-queue block would abort the process).
#[derive(Debug)]
struct Facts {
    label: String,
    visible: bool,
    key: bool,
    tabbing: NSWindowTabbingMode,
    tabbed: bool,
    /// `(close button centre from the top, close button x)`.
    lights: (f64, f64),
    has_titlebar: bool,
}

fn facts(a: &AppHandle, label: &str) -> Option<Facts> {
    let ns = ns_window(a, label)?;
    let close = ns.standardWindowButton(NSWindowButton::CloseButton)?;
    let rect = close.convertRect_toView(close.bounds(), None);
    let height = ns.contentView()?.bounds().size.height;
    // A test launched from a terminal may not be allowed to activate (macOS activation is
    // cooperative); then "key" means "front-most of Drift's windows".
    let mtm = objc2::MainThreadMarker::new()?;
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let front = app.orderedWindows().firstObject().is_some_and(|w| std::ptr::eq(&*w, &*ns));
    Some(Facts {
        label: label.to_owned(),
        visible: ns.isVisible(),
        key: if app.isActive() { ns.isKeyWindow() } else { front },
        tabbing: ns.tabbingMode(),
        tabbed: ns.tabbedWindows().is_some(),
        lights: (height - rect.origin.y - rect.size.height / 2.0, rect.origin.x),
        has_titlebar: a.get_webview(&titlebar_label(label)).is_some(),
    })
}

/// `(full screen, window frame vs screen frame, picture frame height, content height,
/// title bar webview hidden)` of session window `label` (main thread).
fn full_screen_facts(a: &AppHandle, label: &str) -> (bool, Option<String>, f64, f64, bool) {
    let ns = ns_window(a, label).unwrap();
    let full = ns.styleMask().contains(NSWindowStyleMask::FullScreen);
    let covers = ns.screen().and_then(|s| {
        let (w, f) = (ns.frame(), s.frame());
        let same = (w.size.width - f.size.width).abs() < 0.5 && (w.size.height - f.size.height).abs() < 0.5;
        (!same).then(|| format!("window {w:?} on screen {f:?}, full screen: {full}"))
    });
    let content: Retained<NSView> = ns.contentView().unwrap();
    let picture = drift_macos::webview::find_subview_of_class(&content, "DriftRemoteView").unwrap();
    let titlebar_hidden = drift_macos::webview::find_webviews(&content).iter().skip(1).all(|v| v.isHidden());
    (full, covers, picture.frame().size.height, content.bounds().size.height, titlebar_hidden)
}

/// The Dock menu's item titles, as AppKit gets them from the application delegate (main thread).
fn dock_menu_titles() -> Vec<String> {
    let mtm = objc2::MainThreadMarker::new().unwrap();
    let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let delegate = ns_app.delegate().expect("tao's application delegate");
    let delegate: &objc2::runtime::AnyObject = delegate.as_ref();
    // SAFETY: Drift added `applicationDockMenu:` (`@@:@`) to the delegate's class.
    let menu: Option<Retained<objc2_app_kit::NSMenu>> =
        unsafe { objc2::msg_send![delegate, applicationDockMenu: &*ns_app] };
    let menu = menu.expect("a Dock menu");
    (0..menu.numberOfItems()).filter_map(|i| menu.itemAtIndex(i)).map(|i| i.title().to_string()).collect()
}

/// The test's temporary config directory.
static CONFIG_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// `window-frames.json` in the test's config directory.
fn frames_path() -> std::path::PathBuf {
    CONFIG_DIR.get().unwrap().join(drift_app::frames::FRAMES_FILE)
}

/// The Window menu's visible item titles and whether it is `NSApp.windowsMenu` (main thread).
fn window_menu() -> (Vec<String>, bool) {
    let mtm = objc2::MainThreadMarker::new().unwrap();
    let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let Some(main) = ns_app.mainMenu() else { return (Vec::new(), false) };
    let Some(window) =
        main.itemWithTitle(&objc2_foundation::NSString::from_str("Window")).and_then(|i| i.submenu())
    else {
        return (Vec::new(), false);
    };
    let titles = (0..window.numberOfItems())
        .filter_map(|i| window.itemAtIndex(i))
        // AppKit keeps a hidden "Enter Full Screen" of its own next to Drift's.
        .filter(|i| !i.isHidden())
        .map(|i| i.title().to_string())
        .collect();
    let is_windows_menu = ns_app.windowsMenu().is_some_and(|m| std::ptr::eq(&*m, &*window));
    (titles, is_windows_menu)
}

fn scenario(app: AppHandle) {
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // 1. Connections exists at launch; closing it only hides it.
            wait_until("the Connections window", 20, || {
                on_main(&app, |a| facts(a, CONNECTIONS_WINDOW)).is_some_and(|f| f.visible)
            });
            on_main(&app, |a| a.get_webview_window(CONNECTIONS_WINDOW).unwrap().close().unwrap());
            wait_until("Connections hidden", 10, || {
                on_main(&app, |a| facts(a, CONNECTIONS_WINDOW)).is_some_and(|f| !f.visible)
            });
            show_connections(&app, None);
            wait_until("Connections shown and key", 10, || {
                on_main(&app, |a| facts(a, CONNECTIONS_WINDOW)).is_some_and(|f| f.visible && f.key)
            });
            println!("Connections: hidden by its close button, back with show_connections");

            // 2. Two connections, two more windows.
            let alpha = save_profile(&app, "Alpha");
            let bravo = save_profile(&app, "Bravo");
            connect_profile(&app, alpha).unwrap();
            connect_profile(&app, bravo).unwrap();
            wait_until("two session windows", 20, || on_main(&app, |_| session_labels()).len() == 2);
            let sessions = on_main(&app, |_| session_labels());
            let mut labels = vec![CONNECTIONS_WINDOW.to_owned()];
            labels.extend(sessions.iter().cloned());
            wait_until("every window visible", 10, || {
                let labels = labels.clone();
                on_main(&app, move |a| labels.iter().all(|l| facts(a, l).is_some_and(|f| f.visible)))
            });
            // tao moves the traffic lights when a window first draws.
            std::thread::sleep(Duration::from_millis(500));
            let all: Vec<Facts> = {
                let labels = labels.clone();
                on_main(&app, move |a| labels.iter().filter_map(|l| facts(a, l)).collect())
            };
            assert_eq!(all.len(), 3, "Connections + two session windows: {all:?}");
            for f in &all {
                assert_eq!(f.tabbing, NSWindowTabbingMode::Disallowed, "{}", f.label);
                assert!(!f.tabbed, "{} is not in a tab group", f.label);
            }
            for f in &all[1..] {
                assert!(f.has_titlebar, "{} has its title-bar webview", f.label);
                let (centre, x) = f.lights;
                assert!(
                    (centre - TITLEBAR_HEIGHT / 2.0).abs() <= 3.0,
                    "{}: close button centre {centre} pt from the top",
                    f.label
                );
                assert!((x - 20.0).abs() <= 3.0, "{}: close button at x {x}", f.label);
            }
            println!("three independent windows, none tabbed; traffic lights centred in the 52-pt bar");

            // The Window menu lists the sessions itself and is not AppKit's windows menu.
            wait_until("the Sessions section", 10, || {
                on_main(&app, |_| {
                    window_menu().0.iter().filter(|t| t.contains("Alpha") || t.contains("Bravo")).count() == 2
                })
            });
            let (titles, is_windows_menu) = on_main(&app, |_| window_menu());
            assert!(titles.iter().any(|t| t == "Sessions"), "{titles:?}");
            assert!(titles.iter().any(|t| t == "Connections"), "{titles:?}");
            assert!(!is_windows_menu, "Drift owns the list; AppKit adds none: {titles:?}");
            assert!(!titles.iter().any(|t| t.contains("Tab")), "no tab items: {titles:?}");
            assert_eq!(titles.iter().filter(|t| t.contains("Full Screen")).count(), 1, "{titles:?}");
            println!("Window menu: {titles:?}");

            // 3. Connecting Alpha again focuses its window; no third session window.
            let first = app.state::<AppState>().sessions.window_for(alpha).expect("Alpha's window");
            connect_profile(&app, alpha).unwrap();
            wait_until("Alpha's window is key", 10, || {
                let first = first.clone();
                on_main(&app, move |a| facts(a, &first)).is_some_and(|f| f.key)
            });
            std::thread::sleep(Duration::from_millis(300));
            assert_eq!(on_main(&app, |_| session_labels()).len(), 2, "no second window for Alpha");
            println!("connecting an open profile focuses its window");

            // 4. Full screen: the picture covers the whole screen, no title bar chrome. macOS only
            // lets the active app enter full screen, and activation is cooperative: a test binary
            // started from a background shell may never become active. Then this part is
            // reported as skipped (the pure `present::chrome(true)` rule is tested either way).
            let active = on_main(&app, |_| {
                let mtm = objc2::MainThreadMarker::new().unwrap();
                let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                ns_app.activate();
                ns_app.isActive()
            });
            if !active {
                println!("full screen: SKIPPED (Drift could not become the active app in this session)");
            } else {
                let second = sessions[1].clone();
                {
                    let second = second.clone();
                    on_main(&app, move |a| ns_window(a, &second).unwrap().toggleFullScreen(None));
                }
                wait_until("full screen covering the screen", 20, || {
                    let second = second.clone();
                    on_main(&app, move |a| {
                        let facts = full_screen_facts(a, &second);
                        facts.0 && facts.1.is_none()
                    })
                });
                std::thread::sleep(Duration::from_millis(500));
                let (full, covers, picture, content, titlebar_hidden) = {
                    let second = second.clone();
                    on_main(&app, move |a| full_screen_facts(a, &second))
                };
                assert!(full, "still in full screen");
                assert_eq!(covers, None, "the full-screen window covers its screen");
                assert!(
                    (picture - content).abs() < 0.5,
                    "the picture fills the screen: {picture} of {content}"
                );
                assert!(titlebar_hidden, "no title-bar webview in full screen");
                {
                    let second = second.clone();
                    on_main(&app, move |a| ns_window(a, &second).unwrap().toggleFullScreen(None));
                }
                wait_until("left full screen", 20, || {
                    let second = second.clone();
                    on_main(&app, move |a| !full_screen_facts(a, &second).0)
                });
                println!("full screen: the picture covers the screen, the title bar is hidden");
            }

            // 5. The Dock menu lists both sessions, then Connections and New Connection….
            let dock = on_main(&app, |_| dock_menu_titles());
            assert_eq!(dock.len(), 6, "{dock:?}");
            assert_eq!(dock[0], "Sessions");
            assert!(
                dock[1..3].iter().any(|t| t.ends_with("Alpha"))
                    && dock[1..3].iter().any(|t| t.ends_with("Bravo"))
            );
            assert_eq!(dock[4..], ["Connections".to_owned(), "New Connection…".to_owned()]);
            println!("Dock menu: {dock:?}");

            // 6. A closed window's frame comes back with its profile.
            let bravo_window = app.state::<AppState>().sessions.window_for(bravo).unwrap();
            let moved = {
                let bravo_window = bravo_window.clone();
                on_main(&app, move |a| {
                    let ns = ns_window(a, &bravo_window).unwrap();
                    let mut frame = ns.frame();
                    frame.origin.x += 37.0;
                    frame.size.width -= 101.0;
                    ns.setFrame_display(frame, true);
                    ns.frame()
                })
            };
            close_session_window(&app, &bravo_window);
            wait_until("Bravo's window closed", 10, || on_main(&app, |_| session_labels()).len() == 1);
            let saved = std::fs::read_to_string(frames_path()).unwrap();
            assert!(saved.contains(&bravo.to_string()), "{saved}");
            connect_profile(&app, bravo).unwrap();
            wait_until("Bravo's new window", 20, || on_main(&app, |_| session_labels()).len() == 2);
            wait_until("Bravo's session registered", 10, || {
                app.state::<AppState>().sessions.window_for(bravo).is_some()
            });
            let again = app.state::<AppState>().sessions.window_for(bravo).unwrap();
            assert_ne!(again, bravo_window, "a new window");
            let restored = on_main(&app, move |a| ns_window(a, &again).unwrap().frame());
            assert_eq!(restored, moved, "the frame is restored");
            println!("frames: saved on close, restored on the next connect");
        }));
        let ok = result.is_ok();
        println!("test {NAME} ... {}", if ok { "ok" } else { "FAILED" });
        let (status, passed, failed) = if ok { ("ok", 1, 0) } else { ("FAILED", 0, 1) };
        println!("\ntest result: {status}. {passed} passed; {failed} failed; 0 ignored");
        std::process::exit(if ok { 0 } else { 1 });
    });
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {
        if !args.iter().any(|a| a == "--ignored") {
            println!("{NAME}: test");
        }
        return;
    }
    let filters: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if !filters.is_empty() && !filters.iter().any(|f| NAME.contains(f.as_str())) {
        println!("\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored");
        return;
    }
    println!("\nrunning 1 test");
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(120));
        println!("test {NAME} ... FAILED (timeout)");
        std::process::exit(2);
    });
    let dir = tempfile::tempdir().unwrap();
    CONFIG_DIR.set(dir.path().to_path_buf()).unwrap();
    // `..Default::default()` on purpose: a new `RunOptions` field must not break this test
    // binary, which only `cargo xtask ci`'s `--all-features` lint pass compiles.
    let options = RunOptions {
        config_dir: Some(dir.path().to_path_buf()),
        memory_secrets: true,
        on_ready: Some(Box::new(scenario)),
        ..Default::default()
    };
    drift_app::run_with(options);
}
