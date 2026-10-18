//! The explicit paste on iOS (task **IOS-10**).
//!
//! iOS prompts "Drift would like to paste from …" whenever an app reads the pasteboard on its
//! own. A `UIPasteControl` is the system's paste button: tapping it is the user's consent, so it
//! pastes without the prompt. [`PasteButton`] puts one in the corner of the picture and points
//! it at a [`PasteTarget`], which loads the pasted text and image once and hands them to the app
//! as [`Pasted`]. The app turns that into the session's clipboard plus a remote Ctrl+V
//! (`drift_app::paste`).

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use block2::RcBlock;
use dispatch2::MainThreadBound;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSData, NSError, NSItemProvider, NSObjectProtocol, NSString};
use objc2_ui_kit::{
    UIImage, UIPasteConfiguration, UIPasteConfigurationSupporting, UIPasteControl,
    UIPasteControlConfiguration, UIPasteControlDisplayMode, UIView, UIViewAutoresizing,
};

/// What one paste delivered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pasted {
    /// Plain text, if the pasted item had any.
    pub text: Option<String>,
    /// The image as PNG, if the pasted item had one.
    pub png: Option<Vec<u8>>,
}

/// The type identifiers the paste target accepts: any text, any image.
pub fn accepted_type_identifiers() -> [&'static str; 2] {
    ["public.plain-text", "public.image"]
}

/// Instance variables of [`PasteTarget`].
pub struct Ivars {
    handler: RefCell<Rc<dyn Fn(Pasted)>>,
}

define_class!(
    /// A zero-size view that receives the paste of a `UIPasteControl`.
    #[unsafe(super(UIView))]
    #[thread_kind = MainThreadOnly]
    #[name = "DriftPasteTarget"]
    #[ivars = Ivars]
    pub struct PasteTarget;

    unsafe impl NSObjectProtocol for PasteTarget {}

    // `UIPasteConfigurationSupporting` is what a `UIPasteControl` target must be; UIResponder
    // already conforms, and these override its two methods.
    unsafe impl UIPasteConfigurationSupporting for PasteTarget {
        #[unsafe(method(pasteItemProviders:))]
        fn paste_item_providers(&self, providers: &NSArray<NSItemProvider>) {
            self.load(providers);
        }

        #[unsafe(method(canPasteItemProviders:))]
        fn can_paste_item_providers(&self, providers: &NSArray<NSItemProvider>) -> bool {
            providers.iter().any(|p| {
                accepted_type_identifiers()
                    .iter()
                    .any(|t| p.hasItemConformingToTypeIdentifier(&NSString::from_str(t)))
            })
        }
    }
);

/// Collects the asynchronous loads of one paste and delivers them once all have finished.
struct Pending {
    left: usize,
    pasted: Pasted,
}

impl PasteTarget {
    /// A paste target calling `handler` (on the main thread) once per paste.
    pub fn new(mtm: MainThreadMarker, handler: impl Fn(Pasted) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm)
            .set_ivars(Ivars { handler: RefCell::new(Rc::new(handler) as Rc<dyn Fn(Pasted)>) });
        let zero = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 0.0, height: 0.0 } };
        // SAFETY: `initWithFrame:` is UIView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: zero] };
        let ids: Vec<Retained<NSString>> =
            accepted_type_identifiers().iter().map(|s| NSString::from_str(s)).collect();
        let ids = NSArray::from_retained_slice(&ids);
        let config =
            UIPasteConfiguration::initWithAcceptableTypeIdentifiers(UIPasteConfiguration::alloc(mtm), &ids);
        this.setPasteConfiguration(Some(&config));
        this
    }

    fn load(&self, providers: &NSArray<NSItemProvider>) {
        let Some(provider) = providers.firstObject() else { return };
        let text_type = Some("public.plain-text")
            .filter(|t| provider.hasItemConformingToTypeIdentifier(&NSString::from_str(t)));
        let image = provider.hasItemConformingToTypeIdentifier(&NSString::from_str("public.image"));
        let left = usize::from(text_type.is_some()) + usize::from(image);
        if left == 0 {
            return;
        }
        let pending = Arc::new(Mutex::new(Pending { left, pasted: Pasted::default() }));
        let target = Arc::new(MainThreadBound::new(self.retain_target(), self.mtm()));
        let finish = move |update: Box<dyn FnOnce(&mut Pasted) + Send>| {
            let mut p = pending.lock().unwrap_or_else(PoisonError::into_inner);
            update(&mut p.pasted);
            p.left -= 1;
            if p.left > 0 {
                return;
            }
            let pasted = std::mem::take(&mut p.pasted);
            let target = Arc::clone(&target);
            dispatch2::DispatchQueue::main().exec_async(move || {
                let Some(mtm) = MainThreadMarker::new() else { return };
                let handler = target.get(mtm).ivars().handler.borrow().clone();
                handler(pasted);
            });
        };
        let finish = Arc::new(finish);
        if text_type.is_some() {
            // `loadObjectOfClass:` (not bound by objc2-foundation 0.3) decodes whichever text
            // representation the provider has: raw UTF-8 from a pasteboard, or an archived string.
            let f = Arc::clone(&finish);
            let block = RcBlock::new(move |object: *mut AnyObject, _error: *mut NSError| {
                // SAFETY: NSItemProvider passes an autoreleased NSString (the class asked for) or
                // nil for the duration of the call.
                let text = unsafe { object.cast::<NSString>().as_ref() }.map(NSString::to_string);
                f(Box::new(move |p| p.text = text));
            });
            // SAFETY: `NSString` conforms to `NSItemProviderReading`; the block matches the
            // `(id, NSError *)` completion signature and is copied by NSItemProvider.
            let _progress: Option<Retained<AnyObject>> = unsafe {
                msg_send![&*provider, loadObjectOfClass: NSString::class(), completionHandler: &*block]
            };
        }
        if image {
            let f = Arc::clone(&finish);
            let block = RcBlock::new(move |data: *mut NSData, _error: *mut NSError| {
                // SAFETY: as above.
                let png = unsafe { data.as_ref() }.and_then(to_png);
                f(Box::new(move |p| p.png = png));
            });
            // SAFETY: as above.
            let _ = unsafe {
                provider.loadDataRepresentationForTypeIdentifier_completionHandler(
                    &NSString::from_str("public.image"),
                    &block,
                )
            };
        }
    }

    fn retain_target(&self) -> Retained<Self> {
        // SAFETY: `self` is a live, main-thread object; retaining it keeps it alive until the
        // handler has run.
        unsafe { Retained::retain(NonNull::from(self).as_ptr()) }.expect("self is not null")
    }
}

/// PNG bytes stay as they are; any other image is re-encoded through `UIImage`.
fn to_png(data: &NSData) -> Option<Vec<u8>> {
    let bytes = data.to_vec();
    if bytes.starts_with(b"\x89PNG") {
        return Some(bytes);
    }
    let image = UIImage::imageWithData(data)?;
    Some(image.png_representation()?.to_vec())
}

/// Side of the paste button, in points, and its inset from the picture's corner.
const BUTTON: f64 = 44.0;
const INSET: f64 = 16.0;

/// A `UIPasteControl` in the bottom-trailing corner of the picture, with its target.
pub struct PasteButton {
    control: Retained<UIPasteControl>,
    target: Retained<PasteTarget>,
}

impl std::fmt::Debug for PasteButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasteButton").field("hidden", &self.control.isHidden()).finish_non_exhaustive()
    }
}

impl PasteButton {
    /// Adds the button (and its zero-size target) to `picture`'s superview, above `picture`.
    /// Returns `None` when `picture` is not in a view hierarchy.
    pub fn install(
        mtm: MainThreadMarker,
        picture: &UIView,
        handler: impl Fn(Pasted) + 'static,
    ) -> Option<Self> {
        let parent = picture.superview()?;
        let target = PasteTarget::new(mtm, handler);
        parent.addSubview(&target);
        let config = UIPasteControlConfiguration::new(mtm);
        config.setDisplayMode(UIPasteControlDisplayMode::IconOnly);
        let control = UIPasteControl::initWithConfiguration(UIPasteControl::alloc(mtm), &config);
        let frame = picture.frame();
        control.setFrame(CGRect {
            origin: CGPoint {
                x: (frame.origin.x + frame.size.width - BUTTON - INSET).max(0.0),
                y: (frame.origin.y + frame.size.height - BUTTON - INSET).max(0.0),
            },
            size: CGSize { width: BUTTON, height: BUTTON },
        });
        control.setAutoresizingMask(
            UIViewAutoresizing::FlexibleLeftMargin | UIViewAutoresizing::FlexibleTopMargin,
        );
        control.setTarget(Some(ProtocolObject::from_ref(&*target)));
        parent.addSubview(&control);
        Some(Self { control, target })
    }

    /// Puts the button in the bottom-trailing corner of a `width` × `height` picture at the
    /// origin of its superview (the window lays the picture out after the button is added).
    pub fn place(&self, width: f64, height: f64) {
        self.control.setFrame(CGRect {
            origin: CGPoint { x: (width - BUTTON - INSET).max(0.0), y: (height - BUTTON - INSET).max(0.0) },
            size: CGSize { width: BUTTON, height: BUTTON },
        });
    }

    /// Shows or hides the button (hidden while the page covers the picture or the clipboard is
    /// off).
    pub fn set_visible(&self, visible: bool) {
        self.control.setHidden(!visible);
    }

    /// The target (to paste programmatically in tests).
    pub fn target(&self) -> &PasteTarget {
        &self.target
    }
}
