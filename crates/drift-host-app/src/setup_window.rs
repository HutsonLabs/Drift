//! The "Set Up a Device" window (task **HOST-15**): how to add a device from Drift, this Mac's
//! name, address and account, and the pairing code with its QR code as the fallback. Humble
//! object: the words come from [`crate::model::device_setup`], and the buttons report a
//! [`SetupAction`].

use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSBitmapImageRep, NSButton, NSCalibratedWhiteColorSpace, NSControl, NSFont,
    NSFontWeightRegular, NSImage, NSImageScaling, NSImageView, NSLayoutAttribute, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSEdgeInsets, NSPoint, NSRect, NSSize, NSString};

use crate::model::DeviceSetup;

/// What a button of the window asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupAction {
    /// Copy the `drift-pair:` link.
    CopyLink,
    /// Show a fresh code.
    NewCode,
    /// Close the window.
    Done,
}

const ACTIONS: [SetupAction; 3] = [SetupAction::CopyLink, SetupAction::NewCode, SetupAction::Done];

/// Instance variables of the buttons' target.
pub struct SetupTargetIvars {
    handler: Rc<dyn Fn(SetupAction)>,
}

define_class!(
    // The target of the window's buttons: maps a button's tag to its action.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DriftHostSetupTarget"]
    #[ivars = SetupTargetIvars]
    struct SetupTarget;

    impl SetupTarget {
        #[unsafe(method(press:))]
        fn press(&self, sender: Option<&AnyObject>) {
            let Some(control) = sender.and_then(|s| s.downcast_ref::<NSControl>()) else { return };
            if let Some(action) = usize::try_from(control.tag()).ok().and_then(|i| ACTIONS.get(i)) {
                // Cloned out first: the handler may rebuild the window (and this target).
                let handler = Rc::clone(&self.ivars().handler);
                handler(*action);
            }
        }
    }
);

/// The window; closed (not released) when dropped.
pub struct SetupWindow {
    window: Retained<NSWindow>,
    _target: Retained<SetupTarget>,
}

impl std::fmt::Debug for SetupWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupWindow").field("visible", &self.is_visible()).finish()
    }
}

const WIDTH: f64 = 440.0;

impl SetupWindow {
    /// Builds the window for `setup` and `qr` (modules from
    /// `HostAnnouncement::pairing_qr_modules`) and brings it to the front. `on_action` runs on
    /// the main thread.
    pub fn show(
        mtm: MainThreadMarker,
        setup: &DeviceSetup,
        code: Option<&str>,
        qr: Option<&(usize, Vec<bool>)>,
        on_action: impl Fn(SetupAction) + 'static,
    ) -> Self {
        let target = SetupTarget::alloc(mtm).set_ivars(SetupTargetIvars { handler: Rc::new(on_action) });
        // SAFETY: `init` on a fresh NSObject subclass.
        let target: Retained<SetupTarget> = unsafe { msg_send![super(target), init] };

        let stack = column(mtm, 14.0);
        stack.setEdgeInsets(NSEdgeInsets { top: 22.0, left: 24.0, bottom: 20.0, right: 24.0 });
        let title = NSTextField::labelWithString(&NSString::from_str(&setup.title), mtm);
        title.setFont(Some(&NSFont::boldSystemFontOfSize(17.0)));
        stack.addArrangedSubview(&title);

        let steps = column(mtm, 6.0);
        for (i, step) in setup.steps.iter().enumerate() {
            steps.addArrangedSubview(&wrapping(mtm, &format!("{}. {step}", i + 1)));
        }
        stack.addArrangedSubview(&steps);

        let details = column(mtm, 3.0);
        for (label, value) in &setup.details {
            let line = NSTextField::labelWithString(&NSString::from_str(&format!("{label}:  {value}")), mtm);
            line.setSelectable(true);
            details.addArrangedSubview(&line);
        }
        stack.addArrangedSubview(&details);

        stack.addArrangedSubview(&wrapping(mtm, &setup.code_note));
        if let (Some(code), Some(image)) = (code, qr.and_then(|(w, dark)| qr_image(*w, dark))) {
            let row = NSStackView::new(mtm);
            row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            row.setSpacing(18.0);
            let view = NSImageView::imageViewWithImage(&image, mtm);
            view.setImageScaling(NSImageScaling::ScaleNone);
            row.addArrangedSubview(&view);
            let digits = NSTextField::labelWithString(&NSString::from_str(code), mtm);
            digits.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(28.0, unsafe {
                // SAFETY: an immutable constant exported by AppKit.
                NSFontWeightRegular
            })));
            digits.setSelectable(true);
            row.addArrangedSubview(&digits);
            stack.addArrangedSubview(&row);
        }

        let buttons = NSStackView::new(mtm);
        buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        for (i, title) in ["Copy Pairing Link", "New Code", "Done"].into_iter().enumerate() {
            // SAFETY: `press:` takes the sender and is implemented by `SetupTarget`, which the
            // window's owner keeps alive as long as the buttons.
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title),
                    Some(&target),
                    Some(sel!(press:)),
                    mtm,
                )
            };
            button.setTag(isize::try_from(i).unwrap_or(0));
            if ACTIONS.get(i) == Some(&SetupAction::Done) {
                button.setKeyEquivalent(&NSString::from_str("\r"));
            }
            buttons.addArrangedSubview(&button);
        }
        stack.addArrangedSubview(&buttons);

        // SAFETY: a titled, closable window; `setReleasedWhenClosed(false)` because it is owned by
        // its `Retained`.
        let window = unsafe {
            let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, 400.0)),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            );
            window.setReleasedWhenClosed(false);
            window
        };
        window.setTitle(&NSString::from_str("Drift Host"));
        window.setContentView(Some(&stack));
        window.center();
        objc2_app_kit::NSApplication::sharedApplication(mtm).activate();
        window.makeKeyAndOrderFront(None);
        Self { window, _target: target }
    }

    /// Whether the window is on screen (the user may have closed it).
    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    /// Closes the window.
    pub fn close(&self) {
        self.window.orderOut(None);
    }
}

impl Drop for SetupWindow {
    fn drop(&mut self) {
        self.close();
    }
}

fn column(mtm: MainThreadMarker, spacing: f64) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(NSLayoutAttribute::Leading);
    stack.setSpacing(spacing);
    stack
}

fn wrapping(mtm: MainThreadMarker, text: &str) -> Retained<NSView> {
    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    label.setPreferredMaxLayoutWidth(WIDTH - 48.0);
    Retained::into_super(Retained::into_super(label))
}

/// Pixels per QR module, and the quiet zone around the code in modules.
const SCALE: usize = 6;
const QUIET: usize = 2;

/// The QR code as a crisp grayscale image (drawn at two pixels per point).
fn qr_image(width: usize, dark: &[bool]) -> Option<Retained<NSImage>> {
    let side = (width + 2 * QUIET) * SCALE;
    let side_i = isize::try_from(side).ok()?;
    // SAFETY: null planes ask AppKit to allocate the buffer; 8-bit gray, one byte per pixel,
    // `side` bytes per row.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            side_i,
            side_i,
            8,
            1,
            false,
            false,
            NSCalibratedWhiteColorSpace,
            side_i,
            8,
        )
    }?;
    let data = rep.bitmapData();
    if data.is_null() {
        return None;
    }
    // SAFETY: the rep owns `side * side` bytes at `data` (allocated above, one byte per pixel).
    let pixels = unsafe { std::slice::from_raw_parts_mut(data, side * side) };
    for (i, p) in pixels.iter_mut().enumerate() {
        let (x, y) = ((i % side) / SCALE, (i / side) / SCALE);
        let module = x.checked_sub(QUIET).zip(y.checked_sub(QUIET)).filter(|(x, y)| *x < width && *y < width);
        *p = match module {
            Some((x, y)) if dark.get(y * width + x).copied().unwrap_or(false) => 0,
            _ => 255,
        };
    }
    #[allow(clippy::cast_precision_loss)]
    let points = side as f64 / 2.0;
    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(points, points));
    image.addRepresentation(&rep);
    Some(image)
}
