//! `CGVirtualDisplay`, a private CoreGraphics API (task **HOST-10**, ADR `IOS-0` decision 5,
//! ADR `HOST-10-virtual-display`). Everything private lives in this one module and goes through
//! the Objective-C runtime; [`self_test`] checks at launch that the classes and selectors still
//! exist, so a macOS release that changes them makes the host fall back to the physical display
//! instead of crashing.
//!
//! The API, as used here: a `CGVirtualDisplayDescriptor` (name, maximum pixels, physical size,
//! vendor/product/serial ids, a dispatch queue) creates a `CGVirtualDisplay`; a
//! `CGVirtualDisplaySettings` with `hiDPI` and one `CGVirtualDisplayMode` (width, height, refresh
//! rate) is applied to it; `displayID` names it to the rest of CoreGraphics. The display exists
//! for as long as the object does.

use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSArray, NSString};

use crate::vdisplay_plan::ModePlan;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}

/// Vendor id reported for Drift's virtual displays (arbitrary, stable).
const VENDOR_ID: u32 = 0x4452; // "DR"
/// Product id.
const PRODUCT_ID: u32 = 0x0001;

const CLASSES: [(&str, &[&str]); 4] = [
    (
        "CGVirtualDisplayDescriptor",
        &[
            "setName:",
            "setMaxPixelsWide:",
            "setMaxPixelsHigh:",
            "setSizeInMillimeters:",
            "setVendorID:",
            "setProductID:",
            "setSerialNum:",
            "setQueue:",
        ],
    ),
    ("CGVirtualDisplay", &["initWithDescriptor:", "applySettings:", "displayID"]),
    ("CGVirtualDisplaySettings", &["setHiDPI:", "setModes:"]),
    ("CGVirtualDisplayMode", &["initWithWidth:height:refreshRate:"]),
];

fn class(name: &str) -> Result<&'static AnyClass, String> {
    let c = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
    AnyClass::get(&c).ok_or_else(|| format!("{name} is missing"))
}

/// Checks that every class and selector this module uses exists. Creates nothing.
pub fn self_test() -> Result<(), String> {
    for (name, selectors) in CLASSES {
        let c = class(name)?;
        for s in selectors {
            let c_sel = std::ffi::CString::new(*s).map_err(|e| e.to_string())?;
            let sel = Sel::register(&c_sel);
            if !c.responds_to(sel) {
                return Err(format!("{name} does not respond to {s}"));
            }
        }
    }
    Ok(())
}

/// A virtual display. Removed when dropped.
pub struct VirtualDisplay {
    display: Retained<AnyObject>,
    _queue: dispatch2::DispatchRetained<dispatch2::DispatchQueue>,
    plan: ModePlan,
}

impl std::fmt::Debug for VirtualDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtualDisplay").field("id", &self.display_id()).field("plan", &self.plan).finish()
    }
}

fn new_object(name: &str) -> Result<Retained<AnyObject>, String> {
    let c = class(name)?;
    // SAFETY: `+new` on an NSObject subclass returns a +1 instance or nil.
    let object: Option<Retained<AnyObject>> = unsafe { msg_send![c, new] };
    object.ok_or_else(|| format!("{name} new returned nil"))
}

impl VirtualDisplay {
    /// Creates a display named `name` in `plan`'s mode.
    pub fn create(name: &str, plan: &ModePlan) -> Result<Self, String> {
        self_test()?;
        let queue = dispatch2::DispatchQueue::new("com.hutsonlabs.drift.host.display", None);
        let descriptor = new_object("CGVirtualDisplayDescriptor")?;
        let size = CGSize { width: f64::from(plan.size_mm.0), height: f64::from(plan.size_mm.1) };
        // SAFETY: setters of CGVirtualDisplayDescriptor with the argument types the private
        // header declares (NSString, unsigned int, CGSize, dispatch_queue_t); checked by
        // `self_test` to exist.
        unsafe {
            let () = msg_send![&*descriptor, setName: &*NSString::from_str(name)];
            let () = msg_send![&*descriptor, setMaxPixelsWide: plan.pixels.width];
            let () = msg_send![&*descriptor, setMaxPixelsHigh: plan.pixels.height];
            let () = msg_send![&*descriptor, setSizeInMillimeters: size];
            let () = msg_send![&*descriptor, setVendorID: VENDOR_ID];
            let () = msg_send![&*descriptor, setProductID: PRODUCT_ID];
            let () = msg_send![&*descriptor, setSerialNum: std::process::id()];
            let queue_ptr: *const AnyObject = dispatch2::DispatchRetained::as_ptr(&queue).as_ptr().cast();
            let () = msg_send![&*descriptor, setQueue: queue_ptr];
        }
        let c = class("CGVirtualDisplay")?;
        // SAFETY: `alloc` + `initWithDescriptor:` returns a +1 display or nil.
        let display: Option<Retained<AnyObject>> = unsafe {
            let allocated: Allocated<AnyObject> = msg_send![c, alloc];
            msg_send![allocated, initWithDescriptor: &*descriptor]
        };
        let display = display.ok_or("CGVirtualDisplay initWithDescriptor: returned nil")?;
        let mut this = Self { display, _queue: queue, plan: plan.clone() };
        this.apply(plan)?;
        if this.display_id() == 0 {
            return Err("the virtual display has no display id".into());
        }
        Ok(this)
    }

    /// Applies `plan`'s mode (a resize keeps the same display).
    pub fn apply(&mut self, plan: &ModePlan) -> Result<(), String> {
        let settings = new_object("CGVirtualDisplaySettings")?;
        let mode_class = class("CGVirtualDisplayMode")?;
        // SAFETY: `alloc` + `initWithWidth:height:refreshRate:` (unsigned int, unsigned int, double;
        // objc2 checks the encoding in debug builds).
        let mode: Option<Retained<AnyObject>> = unsafe {
            let allocated: Allocated<AnyObject> = msg_send![mode_class, alloc];
            msg_send![
                allocated,
                initWithWidth: plan.points.width,
                height: plan.points.height,
                refreshRate: f64::from(plan.refresh_hz)
            ]
        };
        let mode = mode.ok_or("CGVirtualDisplayMode init returned nil")?;
        let modes = NSArray::from_retained_slice(&[mode]);
        // SAFETY: `hiDPI` is an unsigned int, `modes` an NSArray of modes; `applySettings:`
        // returns BOOL.
        let ok: bool = unsafe {
            let () = msg_send![&*settings, setHiDPI: u32::from(plan.hidpi)];
            let () = msg_send![&*settings, setModes: &*modes];
            msg_send![&*self.display, applySettings: &*settings]
        };
        if !ok {
            return Err(format!("applySettings: refused {}×{}", plan.points.width, plan.points.height));
        }
        self.plan = plan.clone();
        Ok(())
    }

    /// Its `CGDirectDisplayID`.
    pub fn display_id(&self) -> u32 {
        // SAFETY: `displayID` returns a CGDirectDisplayID (uint32_t).
        unsafe { msg_send![&*self.display, displayID] }
    }

    /// Its current mode.
    pub fn plan(&self) -> &ModePlan {
        &self.plan
    }
}

/// The active displays' ids (`CGGetActiveDisplayList`).
pub fn active_displays() -> Vec<u32> {
    let mut ids = [0_u32; 32];
    let mut count = 0_u32;
    // SAFETY: a buffer of 32 ids and its length; CoreGraphics writes at most that many.
    let err = unsafe { objc2_core_graphics::CGGetActiveDisplayList(32, ids.as_mut_ptr(), &mut count) };
    if err != objc2_core_graphics::CGError::Success {
        return Vec::new();
    }
    ids[..count.min(32) as usize].to_vec()
}
