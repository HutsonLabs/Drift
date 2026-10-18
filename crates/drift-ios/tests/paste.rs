//! IOS-10 Red: the explicit paste path on iOS. A `UIPasteControl` over the picture pastes into a
//! `PasteTarget` without the system's paste prompt; the target loads the pasted item providers
//! (text, image) and hands them to the app once.
//!
//! Runs on the main thread of an iOS simulator process (`support/harness.rs`).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

#[cfg(target_os = "ios")]
#[path = "support/harness.rs"]
mod harness;

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() -> std::process::ExitCode {
    tests::run()
}

#[cfg(target_os = "ios")]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use drift_ios::paste::{PasteButton, PasteTarget, Pasted, accepted_type_identifiers};
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_foundation::{NSArray, NSDate, NSItemProvider, NSRunLoop, NSString};
    use objc2_ui_kit::{UIPasteConfigurationSupporting as _, UIView};

    use super::harness;

    pub fn run() -> std::process::ExitCode {
        harness::run(&[
            ("target_accepts_text_and_images", target_accepts_text_and_images),
            ("pasted_text_reaches_the_handler_once", pasted_text_reaches_the_handler_once),
            ("paste_button_sits_over_the_picture", paste_button_sits_over_the_picture),
        ])
    }

    fn mtm() -> MainThreadMarker {
        MainThreadMarker::new().expect("main thread")
    }

    fn spin_until(done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() && Instant::now() < deadline {
            NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.02));
        }
    }

    fn target_accepts_text_and_images() {
        let ids = accepted_type_identifiers();
        assert!(ids.contains(&"public.plain-text"));
        assert!(ids.contains(&"public.image"));
        let target = PasteTarget::new(mtm(), |_| {});
        let config = target.pasteConfiguration().expect("a paste configuration");
        let accepted: Vec<String> =
            config.acceptableTypeIdentifiers().iter().map(|s| s.to_string()).collect();
        assert!(accepted.iter().any(|s| s == "public.plain-text"), "{accepted:?}");
        assert!(accepted.iter().any(|s| s == "public.image"), "{accepted:?}");
    }

    fn pasted_text_reaches_the_handler_once() {
        let seen: Rc<RefCell<Vec<Pasted>>> = Rc::default();
        let s = seen.clone();
        let target = PasteTarget::new(mtm(), move |p| s.borrow_mut().push(p));
        let text = NSString::from_str("fake pasted text");
        // SAFETY: an NSString is NSSecureCoding; the type identifier matches it.
        let provider = unsafe {
            NSItemProvider::initWithItem_typeIdentifier(
                NSItemProvider::alloc(),
                Some(ProtocolObject::from_ref(&*text)),
                Some(&NSString::from_str("public.utf8-plain-text")),
            )
        };
        target.pasteItemProviders(&NSArray::from_retained_slice(&[provider]));
        spin_until(|| !seen.borrow().is_empty());
        let seen = seen.borrow();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].text.as_deref(), Some("fake pasted text"));
        assert_eq!(seen[0].png, None);
    }

    fn paste_button_sits_over_the_picture() {
        let mtm = mtm();
        let rect =
            CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 400.0, height: 300.0 } };
        let root = UIView::initWithFrame(UIView::alloc(mtm), rect);
        let picture = UIView::initWithFrame(UIView::alloc(mtm), rect);
        root.addSubview(&picture);
        let button = PasteButton::install(mtm, &picture, |_| {}).expect("picture has a superview");
        let subviews = root.subviews();
        assert!(subviews.len() >= 2, "the control is added next to the picture");
        let last = subviews.lastObject().unwrap();
        assert_eq!(last.class().name().to_str().unwrap(), "UIPasteControl");
        let frame = last.frame();
        assert!(frame.origin.x + frame.size.width <= 400.0 && frame.origin.y + frame.size.height <= 300.0);
        button.set_visible(false);
        assert!(last.isHidden());
        button.set_visible(true);
        assert!(!last.isHidden());
        let _: Retained<UIView> = last;
    }
}
