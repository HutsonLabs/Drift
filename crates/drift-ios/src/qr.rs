//! Scanning a pairing QR code with the camera (task **IOS-9**, ADR `IOS-9-pairing`).
//!
//! [`scan`] presents a full-screen camera view over the calling window with a Cancel button,
//! and reports the first QR code's text (or why there is none). AVFoundation's metadata output
//! does the decoding; VisionKit's `DataScannerViewController` would too, but it is Swift-only.
//! The simulator has no camera, so there [`ScanError::NoCamera`] comes back at once.

use std::cell::RefCell;
use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, MainThreadMarker, define_class, msg_send, sel};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput,
    AVCaptureMetadataOutput, AVCaptureMetadataOutputObjectsDelegate, AVCaptureOutput, AVCaptureSession,
    AVCaptureVideoPreviewLayer, AVLayerVideoGravityResizeAspectFill, AVMediaTypeVideo,
    AVMetadataMachineReadableCodeObject, AVMetadataObject, AVMetadataObjectTypeQRCode,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSString};
use objc2_ui_kit::{
    UIButton, UIButtonType, UIColor, UIControlEvents, UIControlState, UIModalPresentationStyle,
    UIViewController,
};

/// Why no code came back.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    /// The device has no camera (or none Drift may use).
    #[error("This device has no camera. Type the code shown on the Mac instead.")]
    NoCamera,
    /// The user denied camera access.
    #[error("Drift may not use the camera. Allow it in Settings, or type the code shown on the Mac.")]
    CameraDenied,
    /// The user tapped Cancel.
    #[error("Scanning was cancelled.")]
    Cancelled,
    /// AVFoundation failed.
    #[error("The camera could not start: {0}")]
    Failed(String),
}

type Done = Box<dyn FnOnce(Result<String, ScanError>) + Send>;

/// Instance variables of the scanner.
pub struct Ivars {
    done: RefCell<Option<Done>>,
    session: RefCell<Option<Retained<AVCaptureSession>>>,
    controller: RefCell<Option<Retained<UIViewController>>>,
}

define_class!(
    /// The metadata delegate and the Cancel button's target.
    #[unsafe(super(NSObject))]
    #[name = "DriftQrScanner"]
    #[ivars = Ivars]
    struct Scanner;

    unsafe impl NSObjectProtocol for Scanner {}

    unsafe impl AVCaptureMetadataOutputObjectsDelegate for Scanner {
        #[unsafe(method(captureOutput:didOutputMetadataObjects:fromConnection:))]
        fn did_output(
            &self,
            _output: &AVCaptureOutput,
            objects: &NSArray<AVMetadataObject>,
            _connection: &AVCaptureConnection,
        ) {
            for object in objects.iter() {
                let Ok(code) = object.downcast::<AVMetadataMachineReadableCodeObject>() else { continue };
                // SAFETY: a plain getter on a delivered metadata object.
                if let Some(text) = unsafe { code.stringValue() } {
                    self.finish(Ok(text.to_string()));
                    return;
                }
            }
        }
    }

    impl Scanner {
        #[unsafe(method(cancel:))]
        fn cancel(&self, _sender: &NSObject) {
            self.finish(Err(ScanError::Cancelled));
        }
    }
);

thread_local! {
    /// The scanner on screen (kept alive until it finishes).
    static ACTIVE: RefCell<Option<Retained<Scanner>>> = const { RefCell::new(None) };
}

impl Scanner {
    /// Stops the camera, dismisses the view and reports once.
    fn finish(&self, result: Result<String, ScanError>) {
        if let Some(session) = self.ivars().session.borrow_mut().take() {
            // SAFETY: stopping a session we started.
            unsafe { session.stopRunning() };
        }
        if let Some(controller) = self.ivars().controller.borrow_mut().take() {
            controller.dismissViewControllerAnimated_completion(true, None);
        }
        let done = self.ivars().done.borrow_mut().take();
        if let Some(done) = done {
            done(result);
        }
        ACTIVE.with(|a| a.borrow_mut().take());
    }
}

/// Scans one QR code over `presenter` and calls `done` with its text (on the main thread).
///
/// # Safety
/// `presenter` must be a live `UIViewController*` (Tauri's `PlatformWebview::view_controller`)
/// and the caller must be on the main thread (`mtm`).
pub unsafe fn scan(
    mtm: MainThreadMarker,
    presenter: *mut c_void,
    done: impl FnOnce(Result<String, ScanError>) + Send + 'static,
) {
    let done: Done = Box::new(done);
    // SAFETY: a constant exported by AVFoundation.
    let Some(video) = (unsafe { AVMediaTypeVideo }) else { return done(Err(ScanError::NoCamera)) };
    // SAFETY: a class-method query.
    if unsafe { AVCaptureDevice::defaultDeviceWithMediaType(video) }.is_none() {
        return done(Err(ScanError::NoCamera));
    }
    let presenter = presenter as usize;
    // SAFETY: a class-method query.
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(video) } {
        AVAuthorizationStatus::Authorized => present(mtm, presenter, done),
        AVAuthorizationStatus::NotDetermined => {
            let done = std::sync::Mutex::new(Some(done));
            let block = block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
                let Some(done) = done.lock().ok().and_then(|mut d| d.take()) else { return };
                let granted = granted.as_bool();
                crate::webview::dispatch_main(move || {
                    let Some(mtm) = MainThreadMarker::new() else { return };
                    if granted { present(mtm, presenter, done) } else { done(Err(ScanError::CameraDenied)) }
                });
            });
            // SAFETY: the completion handler is copied by AVFoundation and captures Send state.
            unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(video, &block) };
        }
        _ => done(Err(ScanError::CameraDenied)),
    }
}

/// Builds the camera session and its view and presents it.
fn present(mtm: MainThreadMarker, presenter: usize, done: Done) {
    match build(mtm, presenter, done) {
        Ok(()) => {}
        Err((done, e)) => done(Err(e)),
    }
}

fn build(mtm: MainThreadMarker, presenter: usize, done: Done) -> Result<(), (Done, ScanError)> {
    let fail = |done: Done, why: &str| Err((done, ScanError::Failed(why.to_owned())));
    // SAFETY: the caller of `scan` guaranteed a live view controller; we only use it here, on
    // the main thread.
    let Some(presenter) = (unsafe { (presenter as *mut UIViewController).as_ref() }) else {
        return fail(done, "no window to show the camera over");
    };
    // SAFETY: constants and constructors of AVFoundation, used on the main thread.
    let (session, output) = unsafe {
        let Some(video) = AVMediaTypeVideo else { return Err((done, ScanError::NoCamera)) };
        let Some(device) = AVCaptureDevice::defaultDeviceWithMediaType(video) else {
            return Err((done, ScanError::NoCamera));
        };
        let input = match AVCaptureDeviceInput::deviceInputWithDevice_error(&device) {
            Ok(input) => input,
            Err(e) => return fail(done, &e.localizedDescription().to_string()),
        };
        let session = AVCaptureSession::new();
        let output = AVCaptureMetadataOutput::new();
        if !session.canAddInput(&input) || !session.canAddOutput(&output) {
            return fail(done, "the camera is busy");
        }
        session.addInput(&input);
        session.addOutput(&output);
        (session, output)
    };
    let scanner: Retained<Scanner> = {
        let this = Scanner::alloc().set_ivars(Ivars {
            done: RefCell::new(Some(done)),
            session: RefCell::new(Some(session.clone())),
            controller: RefCell::new(None),
        });
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    };
    // SAFETY: the QR type is available once the output is attached; delegate callbacks go to the
    // main queue, where the scanner lives.
    unsafe {
        output.setMetadataObjectTypes(Some(&NSArray::from_slice(&[AVMetadataObjectTypeQRCode])));
        output.setMetadataObjectsDelegate_queue(
            Some(ProtocolObject::from_ref(&*scanner)),
            Some(dispatch2::DispatchQueue::main()),
        );
    }

    let controller = UIViewController::new(mtm);
    let view = controller.view().unwrap_or_else(|| objc2_ui_kit::UIView::new(mtm));
    view.setBackgroundColor(Some(&UIColor::blackColor()));
    let bounds = presenter.view().map_or(
        CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 390.0, height: 844.0 } },
        |v| v.bounds(),
    );
    // SAFETY: a preview layer for our session, added to our own view's layer.
    unsafe {
        let preview = AVCaptureVideoPreviewLayer::layerWithSession(&session);
        if let Some(gravity) = AVLayerVideoGravityResizeAspectFill {
            preview.setVideoGravity(gravity);
        }
        preview.setFrame(bounds);
        view.layer().addSublayer(&preview);
    }
    let cancel = UIButton::buttonWithType(UIButtonType::System, mtm);
    cancel.setTitle_forState(Some(&NSString::from_str("Cancel")), UIControlState::Normal);
    // SAFETY: setting a colour on our own button.
    unsafe { cancel.setTintColor(Some(&UIColor::whiteColor())) };
    cancel.setFrame(CGRect {
        origin: CGPoint { x: 16.0, y: 48.0 },
        size: CGSize { width: 100.0, height: 44.0 },
    });
    // SAFETY: `cancel:` takes the sender and returns nothing; the scanner outlives the button
    // (both are released when scanning ends).
    unsafe {
        cancel.addTarget_action_forControlEvents(
            Some(&scanner),
            sel!(cancel:),
            UIControlEvents::TouchUpInside,
        )
    };
    view.addSubview(&cancel);
    controller.setModalPresentationStyle(UIModalPresentationStyle::FullScreen);
    *scanner.ivars().controller.borrow_mut() = Some(controller.clone());
    ACTIVE.with(|a| *a.borrow_mut() = Some(scanner));
    presenter.presentViewController_animated_completion(&controller, true, None);

    // `startRunning` blocks until the camera is up; keep it off the main thread.
    let session = SendSession(session);
    dispatch2::DispatchQueue::new("com.hutsonlabs.drift.qr-camera", None).exec_async(move || {
        let session = session;
        // SAFETY: AVCaptureSession may be started from any queue.
        unsafe { session.0.startRunning() };
    });
    Ok(())
}

/// `AVCaptureSession` is thread-safe; this carries it to the camera queue.
struct SendSession(Retained<AVCaptureSession>);
// SAFETY: see above; AVFoundation documents starting a session off the main thread.
unsafe impl Send for SendSession {}
