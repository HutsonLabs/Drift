//! Reading and pressing the frontmost app's menu bar through Accessibility (task **EXT-3**).
//!
//! Humble object: [`read_frontmost`] turns the live `AXMenuBar` into an [`AxNode`] tree and hands
//! it to the pure [`snapshot_from_ax`]; [`AxMenus`] keeps a snapshot current on a worker thread
//! and presses items there. Needs the Accessibility permission, like input injection (HOST-6).
//!
//! Each element is read with one `AXUIElementCopyMultipleAttributeValues` call, and the app's
//! messaging timeout is short, so a busy app cannot stall the host for long. The reader is lazy:
//! it reads when the frontmost app changes, and otherwise at most every [`REREAD`] while a
//! client is asking ([`MenuSource::snapshot`]).

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use drift_ext::menus::{MAX_DEPTH, MAX_ITEMS, MenuSnapshot};
use objc2_app_kit::NSWorkspace;
use objc2_core_foundation::{CFArray, CFBoolean, CFNumber, CFNumberType, CFRetained, CFString, CFType};

use super::{AxNode, AxPath, MenuSource, snapshot_from_ax};

/// How often the worker checks which app is frontmost.
pub const FRONTMOST_CHECK: Duration = Duration::from_millis(250);

/// The longest the same app's menu is trusted before it is read again (while a client asks).
pub const REREAD: Duration = Duration::from_secs(2);

/// How long a client poll keeps the reader awake.
const AWAKE_AFTER_POLL: Duration = Duration::from_secs(3);

/// The app's Accessibility messaging timeout (seconds).
const MESSAGING_TIMEOUT: f32 = 0.5;

/// The deepest AX tree read: a menu level is two AX levels (item, then its `AXMenu`).
const MAX_AX_DEPTH: usize = 2 * MAX_DEPTH + 1;

/// The most elements read for one menu bar (items plus their `AXMenu` containers).
const MAX_AX_NODES: usize = 2 * MAX_ITEMS;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
    fn AXUIElementCopyAttributeValue(
        element: *const c_void,
        attribute: *const c_void,
        value: *mut *const c_void,
    ) -> i32;
    fn AXUIElementCopyMultipleAttributeValues(
        element: *const c_void,
        attributes: *const c_void,
        options: u32,
        values: *mut *const c_void,
    ) -> i32;
    fn AXUIElementPerformAction(element: *const c_void, action: *const c_void) -> i32;
    fn AXUIElementSetMessagingTimeout(element: *const c_void, timeout: f32) -> i32;
}

/// `kAXErrorSuccess`.
const AX_SUCCESS: i32 = 0;

/// The attributes read from every element, in this order.
const ATTRIBUTES: [&str; 7] = [
    "AXRole",
    "AXTitle",
    "AXEnabled",
    "AXMenuItemMarkChar",
    "AXMenuItemCmdChar",
    "AXMenuItemCmdModifiers",
    "AXChildren",
];

/// Whether this process may use Accessibility.
pub fn is_trusted() -> bool {
    // SAFETY: a plain query with no arguments.
    unsafe { AXIsProcessTrusted() }
}

/// An owned `AXUIElementRef`.
struct Element(CFRetained<CFType>);

impl Element {
    fn ptr(&self) -> *const c_void {
        let r: &CFType = &self.0;
        std::ptr::from_ref(r).cast()
    }

    fn application(pid: i32) -> Option<Self> {
        // SAFETY: returns a +1 AXUIElementRef (never null for a pid, but checked).
        let raw = unsafe { AXUIElementCreateApplication(pid) };
        // SAFETY: a +1 CF object we now own.
        let element = Self(unsafe { CFRetained::from_raw(NonNull::new(raw.cast::<CFType>())?) });
        // SAFETY: a live element.
        unsafe { AXUIElementSetMessagingTimeout(element.ptr(), MESSAGING_TIMEOUT) };
        Some(element)
    }

    fn attribute(&self, name: &str) -> Option<CFRetained<CFType>> {
        let name = CFString::from_str(name);
        let mut value: *const c_void = std::ptr::null();
        // SAFETY: a live element and attribute name; on success `value` is +1.
        let err = unsafe {
            AXUIElementCopyAttributeValue(
                self.ptr(),
                std::ptr::from_ref::<CFString>(&name).cast(),
                &raw mut value,
            )
        };
        if err != AX_SUCCESS {
            return None;
        }
        // SAFETY: a +1 CF object on success.
        NonNull::new(value.cast_mut().cast::<CFType>()).map(|p| unsafe { CFRetained::from_raw(p) })
    }

    /// The [`ATTRIBUTES`] of this element, as one array (missing ones are AXValue errors).
    fn attributes(&self, names: &CFArray<CFString>) -> Option<CFRetained<CFArray<CFType>>> {
        let mut values: *const c_void = std::ptr::null();
        // SAFETY: a live element and a CFArray of CFStrings; on success `values` is a +1 CFArray.
        let err = unsafe {
            AXUIElementCopyMultipleAttributeValues(
                self.ptr(),
                std::ptr::from_ref::<CFArray<CFString>>(names).cast(),
                0,
                &raw mut values,
            )
        };
        if err != AX_SUCCESS {
            return None;
        }
        // SAFETY: a +1 CFArray of CF objects.
        NonNull::new(values.cast_mut().cast::<CFArray<CFType>>()).map(|p| unsafe { CFRetained::from_raw(p) })
    }

    fn press(&self) -> bool {
        let action = CFString::from_static_str("AXPress");
        // SAFETY: a live element and a constant action name.
        unsafe {
            AXUIElementPerformAction(self.ptr(), std::ptr::from_ref::<CFString>(&action).cast()) == AX_SUCCESS
        }
    }
}

/// The elements behind an [`AxNode`] tree, same shape.
struct ElementTree {
    element: Element,
    children: Vec<ElementTree>,
}

impl ElementTree {
    fn at(&self, path: &[usize]) -> Option<&Element> {
        match path.split_first() {
            None => Some(&self.element),
            Some((i, rest)) => self.children.get(*i)?.at(rest),
        }
    }
}

fn string(value: Option<&CFRetained<CFType>>) -> Option<String> {
    value.and_then(|v| v.downcast_ref::<CFString>()).map(ToString::to_string)
}

fn number(value: Option<&CFRetained<CFType>>) -> Option<i64> {
    let n = value?.downcast_ref::<CFNumber>()?;
    let mut out: i64 = 0;
    // SAFETY: a valid CFNumber and an i64 out-pointer of the requested type.
    unsafe { n.value(CFNumberType::SInt64Type, std::ptr::from_mut(&mut out).cast()) }.then_some(out)
}

fn boolean(value: Option<&CFRetained<CFType>>) -> Option<bool> {
    value.and_then(|v| v.downcast_ref::<CFBoolean>()).map(CFBoolean::as_bool)
}

/// Reads `element` and its children into plain values, bounded in depth and size.
fn read(
    element: Element,
    names: &CFArray<CFString>,
    depth: usize,
    budget: &mut usize,
) -> Option<(AxNode, ElementTree)> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let values = element.attributes(names)?;
    let get = |i: usize| values.get(i);
    let mut node = AxNode {
        role: string(get(0).as_ref()).unwrap_or_default(),
        title: string(get(1).as_ref()).unwrap_or_default(),
        enabled: boolean(get(2).as_ref()).unwrap_or(false),
        marked: string(get(3).as_ref()).is_some_and(|m| !m.trim().is_empty()),
        cmd_char: string(get(4).as_ref()).filter(|c| !c.is_empty()),
        cmd_modifiers: number(get(5).as_ref()),
        children: Vec::new(),
    };
    let mut tree = ElementTree { element, children: Vec::new() };
    if depth < MAX_AX_DEPTH
        && let Some(children) = get(6).and_then(|c| c.downcast::<CFArray>().ok())
    {
        // SAFETY: `AXChildren` is an array of AXUIElementRefs.
        let children: CFRetained<CFArray<CFType>> = unsafe { CFRetained::cast_unchecked(children) };
        for i in 0..children.len() {
            let Some(child) = children.get(i) else { continue };
            let Some((child_node, child_tree)) = read(Element(child), names, depth + 1, budget) else {
                break;
            };
            node.children.push(child_node);
            tree.children.push(child_tree);
        }
    }
    Some((node, tree))
}

/// The frontmost app's pid and name.
fn frontmost() -> Option<(i32, String)> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    let name = app.localizedName().map(|n| n.to_string()).unwrap_or_default();
    Some((app.processIdentifier(), name))
}

/// One reading of the frontmost app's menu bar.
pub struct AxReading {
    /// The snapshot.
    pub snapshot: MenuSnapshot,
    /// The app it belongs to.
    pub pid: i32,
    paths: HashMap<u32, AxPath>,
    elements: ElementTree,
}

impl std::fmt::Debug for AxReading {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AxReading")
            .field("pid", &self.pid)
            .field("app", &self.snapshot.app)
            .finish_non_exhaustive()
    }
}

impl AxReading {
    /// Presses item `id` (`AXPress` on its element).
    pub fn press(&self, id: u32) -> bool {
        self.paths.get(&id).and_then(|p| self.elements.at(p)).is_some_and(Element::press)
    }
}

fn attribute_names() -> CFRetained<CFArray<CFString>> {
    let names: Vec<CFRetained<CFString>> = ATTRIBUTES.iter().map(|a| CFString::from_static_str(a)).collect();
    let refs: Vec<&CFString> = names.iter().map(|n| &**n).collect();
    CFArray::from_objects(&refs)
}

/// Reads the frontmost app's menu bar at `revision`; `None` without a frontmost app, an
/// Accessibility grant or a readable menu bar.
pub fn read_frontmost(revision: u32) -> Option<AxReading> {
    let (pid, name) = frontmost()?;
    read_app(pid, &name, revision)
}

fn read_app(pid: i32, name: &str, revision: u32) -> Option<AxReading> {
    if !is_trusted() {
        return None;
    }
    let app = Element::application(pid)?;
    let bar = app.attribute("AXMenuBar")?;
    let names = attribute_names();
    let mut budget = MAX_AX_NODES;
    let (node, elements) = read(Element(bar), &names, 0, &mut budget)?;
    let (snapshot, paths) = snapshot_from_ax(name, &node, revision);
    Some(AxReading { snapshot, pid, paths, elements })
}

/// What the worker shares with the responder.
#[derive(Default)]
struct Shared {
    current: Option<Arc<MenuSnapshot>>,
    last_poll: Option<Instant>,
}

enum Job {
    Press { revision: u32, id: u32 },
}

/// The frontmost app's menu, kept current on a worker thread.
pub struct AxMenus {
    shared: Arc<Mutex<Shared>>,
    jobs: Mutex<Sender<Job>>,
}

impl std::fmt::Debug for AxMenus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AxMenus").finish_non_exhaustive()
    }
}

impl AxMenus {
    /// Starts the reader; it stops when the last `Arc` is dropped.
    pub fn start() -> std::io::Result<Arc<Self>> {
        let (tx, rx) = channel();
        let menus = Arc::new(Self { shared: Arc::default(), jobs: Mutex::new(tx) });
        let weak = Arc::downgrade(&menus.shared);
        std::thread::Builder::new().name("drift-host-menus".into()).spawn(move || worker(&weak, &rx))?;
        Ok(menus)
    }
}

impl MenuSource for AxMenus {
    fn snapshot(&self) -> Option<Arc<MenuSnapshot>> {
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        shared.last_poll = Some(Instant::now());
        shared.current.clone()
    }

    fn press(&self, revision: u32, id: u32) -> bool {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner).send(Job::Press { revision, id }).is_ok()
    }
}

fn worker(shared: &Weak<Mutex<Shared>>, jobs: &Receiver<Job>) {
    let mut reading: Option<AxReading> = None;
    let mut read_at: Option<Instant> = None;
    let mut revision = 0u32;
    let mut hash = None;
    loop {
        match jobs.recv_timeout(FRONTMOST_CHECK) {
            Ok(Job::Press { revision: wanted, id }) => {
                let pressed = reading.as_ref().is_some_and(|r| r.snapshot.revision == wanted && r.press(id));
                tracing::debug!(id, pressed, "menu item chosen from a client");
                // The app's menu may have changed (a checkmark): read again soon.
                read_at = None;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let Some(shared) = shared.upgrade() else { return };
        let awake = shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last_poll
            .is_some_and(|t| t.elapsed() < AWAKE_AFTER_POLL);
        if !awake {
            continue;
        }
        let front = frontmost();
        let app_changed = front.as_ref().map(|f| f.0) != reading.as_ref().map(|r| r.pid);
        let stale = read_at.is_none_or(|t| t.elapsed() >= REREAD);
        if !app_changed && !stale {
            continue;
        }
        read_at = Some(Instant::now());
        let next = front.and_then(|(pid, name)| read_app(pid, &name, revision + 1));
        let next_hash = next.as_ref().map(|r| r.snapshot.content_hash());
        if next_hash == hash && reading.as_ref().map(|r| r.pid) == next.as_ref().map(|r| r.pid) {
            // Same menu: keep its revision, but press through the freshly read elements.
            if let Some(mut r) = next {
                r.snapshot.revision = revision;
                reading = Some(r);
            }
            continue;
        }
        hash = next_hash;
        let snapshot = match next {
            Some(r) => {
                revision += 1;
                let s = Arc::new(r.snapshot.clone());
                reading = Some(r);
                Some(s)
            }
            None => {
                reading = None;
                None
            }
        };
        tracing::debug!(revision, app = snapshot.as_ref().map(|s| s.app.as_str()), "menu bar changed");
        shared.lock().unwrap_or_else(PoisonError::into_inner).current = snapshot;
    }
}
