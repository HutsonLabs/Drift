//! The iPhone session layout (task **IOS-8**; ADR `IOS-8-iphone-layout`), a part of
//! `windows_ios.rs`.
//!
//! While a live picture is on screen, [`crate::phone::phone_layout`] decides: in portrait the
//! picture sits at the top, a native [`TrackpadView`] fills the middle and the page becomes the
//! key-row strip at the bottom; in landscape the picture fills the screen and the page floats as
//! the key row over its bottom edge until [`AutoHide`] hides it. A touch on the picture's
//! bottom strip brings the controls back. Every other surface (forms, overlays, HUDs) uses the
//! shared layout with the picture full screen. A [`ResizeWatcher`] re-runs the layout when the
//! phone rotates.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use drift_core::{Size, TouchMode};
use drift_ios::layout::ResizeWatcher;
use drift_ios::view::{RemoteView, TrackpadView};
use drift_ios::{CGPoint, CGRect, CGSize};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_ui_kit::UIView;
use tauri::{AppHandle, EventTarget, Runtime};
use tauri_specta::Event as _;

use super::{Surface, WindowPlatform, defer_main, with_platform};
use crate::phone::{
    AUTO_HIDE_SECS, AutoHide, Frame, Insets, PhoneControls, PhoneControlsChanged, phone_layout,
};

/// Height of the picture's bottom strip that reveals the landscape controls.
const REVEAL_STRIP: f64 = 44.0;

/// Per-window iPhone state (main thread only).
struct Phone {
    pad: Retained<TrackpadView>,
    _watcher: Retained<ResizeWatcher>,
    auto_hide: AutoHide,
    /// The last controls state sent to the page.
    sent: Option<PhoneControls>,
    /// The orientation of the last live layout (`true`: landscape).
    landscape: Option<bool>,
}

thread_local! {
    static PHONES: RefCell<HashMap<String, Phone>> = RefCell::new(HashMap::new());
}

/// Seconds on a process-wide monotonic clock.
fn now() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn rect(f: Frame) -> CGRect {
    CGRect { origin: CGPoint { x: f.x, y: f.y }, size: CGSize { width: f.width, height: f.height } }
}

/// Places a view that may carry a zoom transform: bounds and centre stay valid under one,
/// unlike the frame.
fn place(view: &UIView, f: Frame) {
    view.setBounds(CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: f.width, height: f.height },
    });
    view.setCenter(CGPoint { x: f.x + f.width / 2.0, y: f.y + f.height / 2.0 });
}

/// On an iPhone, adds the trackpad and the resize watcher for session window `label`.
pub(super) fn attach<R: Runtime>(
    mtm: MainThreadMarker,
    app: &AppHandle<R>,
    label: &str,
    view: &RemoteView,
    web: &UIView,
) {
    if !drift_ios::view::is_phone(mtm) {
        return;
    }
    let Some(container) = web.superview() else { return };
    // iPhone windows stack in one scene: nothing of the window below may show around the
    // picture, the trackpad or the key row.
    container.setBackgroundColor(Some(&objc2_ui_kit::UIColor::systemBackgroundColor()));
    let pad = TrackpadView::new(mtm, CGRect::default(), view);
    pad.setHidden(true);
    container.insertSubview_aboveSubview(&pad, view);
    let (app2, label2) = (app.clone(), label.to_owned());
    let watcher = ResizeWatcher::new(mtm, container.bounds(), Box::new(move |_, _| relayout(&app2, &label2)));
    watcher.install(&container);
    let (app2, label2) = (app.clone(), label.to_owned());
    view.set_reveal_strip(Some(REVEAL_STRIP), Box::new(move || reveal(&app2, &label2)));
    // Simulator smoke runs cannot rotate the device; this turns the scene instead.
    #[cfg(feature = "smoke-env")]
    if std::env::var("DRIFT_SMOKE_ORIENTATION").is_ok_and(|v| v == "landscape") {
        let (app2, label2) = (app.clone(), label.to_owned());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(8));
            defer_main(&app2, move |_| {
                with_platform(&label2, |plat| drift_ios::layout::request_orientation(&plat.view, true));
            });
        });
    }
    PHONES.with(|p| {
        p.borrow_mut().insert(
            label.to_owned(),
            Phone { pad, _watcher: watcher, auto_hide: AutoHide::default(), sent: None, landscape: None },
        );
    });
}

/// Forgets window `label` (closed).
pub(super) fn remove(label: &str) {
    PHONES.with(|p| {
        if let Some(phone) = p.borrow_mut().remove(label) {
            phone.pad.removeFromSuperview();
        }
    });
}

/// Lays an iPhone session window out. Returns `false` when the shared layout should run (not a
/// phone, or no live picture), after putting the picture back to full screen.
pub(super) fn layout<R: Runtime>(app: &AppHandle<R>, label: &str, plat: &WindowPlatform) -> bool {
    let Some(container) = plat.web.superview() else { return false };
    let bounds = container.bounds();
    let size = Size::new(bounds.size.width, bounds.size.height);
    let insets = container.safeAreaInsets();
    let safe = Insets { top: insets.top, left: insets.left, bottom: insets.bottom, right: insets.right };
    let full = Frame { x: 0.0, y: 0.0, width: size.width, height: size.height };
    let touch = plat.view.touch_mode();
    let controls = PHONES.with(|p| {
        let mut phones = p.borrow_mut();
        let phone = phones.get_mut(label)?;
        let live = plat.surface == Surface::Remote;
        if !live {
            phone.pad.setHidden(true);
            place(&plat.view, full);
            return Some((false, PhoneControls { visible: false, touch }));
        }
        let landscape = size.width > size.height;
        // Turning to landscape (or going live in it) shows the controls for a moment.
        let entered = landscape && phone.landscape.replace(landscape) != Some(true);
        if !landscape {
            phone.landscape = Some(false);
        }
        if entered {
            phone.auto_hide.touch(now());
            schedule_hide(app, label);
        }
        let shown = !landscape || phone.auto_hide.visible(now());
        let desktop = plat.view.viewport().map(|v| v.desktop);
        let l = phone_layout(size, safe, desktop, shown);
        plat.view.setHidden(false);
        place(&plat.view, l.picture);
        match l.pad {
            Some(pad) => {
                phone.pad.setHidden(false);
                phone.pad.setFrame(rect(pad));
            }
            None => phone.pad.setHidden(true),
        }
        match l.keys {
            Some(keys) => {
                plat.web.setHidden(false);
                drift_ios::webview::place_page(&plat.web, keys.x, keys.y, keys.width, keys.height, false);
            }
            None => plat.web.setHidden(true),
        }
        Some((true, PhoneControls { visible: l.keys.is_some(), touch }))
    });
    let Some((handled, controls)) = controls else { return false };
    send_controls(app, label, controls);
    handled
}

/// Tells the page whether its key row shows (only on change).
fn send_controls<R: Runtime>(app: &AppHandle<R>, label: &str, controls: PhoneControls) {
    let changed = PHONES.with(|p| {
        p.borrow_mut().get_mut(label).is_some_and(|phone| phone.sent.replace(controls) != Some(controls))
    });
    if changed {
        let _ = PhoneControlsChanged(controls)
            .emit_to(app, EventTarget::webview_window(label))
            .inspect_err(|e| tracing::warn!(error = %e, "could not emit the phone controls"));
    }
}

/// What window `label`'s page should show of the controls (the page asks when it loads, in
/// case it missed the event).
pub(crate) fn phone_controls(label: &str) -> Option<PhoneControls> {
    PHONES.with(|p| p.borrow().get(label).and_then(|phone| phone.sent))
}

/// Lays window `label` out again (rotation, auto-hide, reveal; main thread).
fn relayout<R: Runtime>(app: &AppHandle<R>, label: &str) {
    with_platform(label, |plat| {
        if !layout(app, label, plat) {
            super::layout(plat);
        }
    });
}

/// Shows the landscape controls now and hides them after [`AUTO_HIDE_SECS`] without use.
fn reveal<R: Runtime>(app: &AppHandle<R>, label: &str) {
    PHONES.with(|p| {
        if let Some(phone) = p.borrow_mut().get_mut(label) {
            phone.auto_hide.touch(now());
        }
    });
    relayout(app, label);
    schedule_hide(app, label);
}

/// Lays window `label` out again once the controls' time is up.
fn schedule_hide<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let (app2, label2) = (app.clone(), label.to_owned());
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs_f64(AUTO_HIDE_SECS + 0.05));
        let app3 = app2.clone();
        defer_main(&app2, move |_| relayout(&app3, &label2));
    });
}

/// The key row was used: it stays up a while longer in landscape.
pub(crate) fn controls_used<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let (app2, label) = (app.clone(), label.to_owned());
    defer_main(app, move |_| {
        if PHONES.with(|p| p.borrow().contains_key(&label)) {
            reveal(&app2, &label);
        }
    });
}

/// Switches window `label`'s touch mode for this session (the key row's switch).
pub(crate) fn set_touch_mode<R: Runtime>(app: &AppHandle<R>, label: &str, mode: TouchMode) {
    let (app2, label) = (app.clone(), label.to_owned());
    defer_main(app, move |mtm| {
        with_platform(&label, |plat| plat.view.set_touch(mode, drift_ios::view::is_phone(mtm)));
        relayout(&app2, &label);
    });
}
