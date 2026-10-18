//! The page's `WKWebView` and the picture under it (IOS-5; ADR M7-3 layering).
//!
//! Tauri hands out the page as a raw `WKWebView*` (`PlatformWebview::inner`); these helpers take
//! it from there. wry adds the `WKWebView` as a subview of tao's root view, so the picture can be
//! inserted below it in the same superview, as on macOS. All functions run on the main thread.

use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, Message as _};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_ui_kit::{UIColor, UIView, UIViewAutoresizing};

use crate::view::{AttachError, RemoteView, insert_below};

/// The page view behind a raw `WKWebView*`, retained.
///
/// # Safety
/// `web` must be null or a live `WKWebView` (a `UIView`), and the caller must be on the main
/// thread (`_mtm`).
pub unsafe fn page_view(_mtm: MainThreadMarker, web: *mut c_void) -> Option<Retained<UIView>> {
    // SAFETY: the caller guarantees a live UIView or null; we retain it before returning.
    unsafe { web.cast::<UIView>().as_ref() }.map(|v| v.retain())
}

/// Creates a `RemoteView` under the page (same frame, autoresizing) and makes the page
/// transparent, so overlays and HUDs draw over the picture.
pub fn attach_below(mtm: MainThreadMarker, page: &UIView) -> Result<Retained<RemoteView>, AttachError> {
    let view = RemoteView::new(mtm, page.frame());
    insert_below(page, &view)?;
    // The page's scroll view paints its own background; clear it too.
    page.setBackgroundColor(Some(&UIColor::clearColor()));
    Ok(view)
}

/// The size of the page's superview (the window's root view), in points.
pub fn container_size(page: &UIView) -> Option<(f64, f64)> {
    page.superview().map(|s| {
        let b = s.bounds();
        (b.size.width, b.size.height)
    })
}

/// Places the page at `(x, y, width, height)` points in its superview (top-left origin). With
/// `fill`, the page also follows the superview's size; otherwise it keeps its frame (a HUD
/// corner panel).
pub fn place_page(page: &UIView, x: f64, y: f64, width: f64, height: f64, fill: bool) {
    page.setFrame(CGRect { origin: CGPoint { x, y }, size: CGSize { width, height } });
    page.setAutoresizingMask(if fill {
        UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight
    } else {
        UIViewAutoresizing::empty()
    });
}

/// Runs `work` asynchronously on the main dispatch queue (outside tao's event handler).
pub fn dispatch_main(work: impl FnOnce() + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(work);
}

/// Whether the page's window is the one in front: its scene is foreground-active and the window
/// is the scene's key window, or the scene's only window. On iPad every session window is its
/// own scene and may never become "key" in the iPhone sense; on iPhone the windows share one
/// scene and the front one is key (IOS-10, IOS-11).
pub fn is_front(page: &UIView) -> bool {
    use objc2_ui_kit::UISceneActivationState;
    let Some(window) = page.window() else { return false };
    let Some(scene) = window.windowScene() else { return window.isKeyWindow() };
    if scene.activationState() != UISceneActivationState::ForegroundActive {
        return false;
    }
    let scene_key = scene.keyWindow().is_some_and(|k| std::ptr::eq(&*k, &*window));
    window.isKeyWindow() || scene_key || scene.windows().len() == 1
}

/// The address of the scene the page's window belongs to, as reported by
/// `AppObserver::scene_disconnected` (IOS-11); `None` without a window or scene.
pub fn scene_id(page: &UIView) -> Option<usize> {
    let scene = page.window()?.windowScene()?;
    Some(Retained::as_ptr(&scene) as usize)
}

/// The top and leading safe-area insets of the page's superview (the status bar, the camera
/// housing in landscape), in points.
pub fn safe_area(page: &UIView) -> (f64, f64) {
    page.superview().map_or((0.0, 0.0), |s| {
        let insets = s.safeAreaInsets();
        (insets.top, insets.left)
    })
}

/// Brings the page's window to the front (IOS-11).
///
/// On iPad (multiple scenes) this activates the window's **own** scene session. tao 0.35's
/// `set_focus` sends an activation request without a session, which makes iPadOS open a new,
/// empty scene instead. On iPhone the windows share one scene, and the window is made key and
/// visible above the others.
pub fn bring_to_front(mtm: MainThreadMarker, page: &UIView) {
    use objc2_ui_kit::{UIApplication, UISceneSessionActivationRequest};
    let Some(window) = page.window() else { return };
    let app = UIApplication::sharedApplication(mtm);
    match window.windowScene() {
        Some(scene) if app.supportsMultipleScenes() => {
            let request = UISceneSessionActivationRequest::requestWithSession(&scene.session());
            let on_error = block2::RcBlock::new(|error: std::ptr::NonNull<objc2_foundation::NSError>| {
                // SAFETY: UIKit passes a live error for the duration of the call.
                let error = unsafe { error.as_ref() };
                tracing::warn!(error = %error.localizedDescription(), "could not activate the window's scene");
            });
            app.activateSceneSessionForRequest_errorHandler(&request, Some(&on_error));
        }
        _ => window.makeKeyAndVisible(),
    }
}
