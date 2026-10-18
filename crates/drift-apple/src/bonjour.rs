//! Finding Drift Hosts on the local network (task **IOS-9**, ADR `IOS-9-pairing`).
//!
//! * [`HostBrowser`] browses [`SERVICE_TYPE`] with Network.framework's `nw_browser`, reads each
//!   service's TXT record and reports the valid [`HostAdvert`]s after every batch of changes.
//!   iOS only lets an app browse service types listed in `NSBonjourServices`, and asks for the
//!   Local Network permission on first use.
//! * [`Advertisement`] registers a service with `DNSServiceRegister`: what Drift Host will do
//!   (HOST-8), and what the smoke test uses to have something to find.
//!
//! The TXT entries are handed to `drift_core::pairing::HostAdvert::from_txt`; records it
//! rejects (another version, missing fields) are logged and left out.

use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use block2::{DynBlock, RcBlock};
use dispatch2::{DispatchQueue, DispatchRetained};
use drift_core::pairing::{HostAdvert, SERVICE_TYPE};
use objc2::runtime::Bool;

#[link(name = "Network", kind = "framework")]
unsafe extern "C" {
    fn nw_browse_descriptor_create_bonjour_service(kind: *const c_char, domain: *const c_char)
    -> *mut c_void;
    fn nw_browse_descriptor_set_include_txt_record(descriptor: *mut c_void, include: bool);
    fn nw_parameters_create() -> *mut c_void;
    fn nw_browser_create(descriptor: *mut c_void, parameters: *mut c_void) -> *mut c_void;
    fn nw_browser_set_queue(browser: *mut c_void, queue: NonNull<DispatchQueue>);
    fn nw_browser_set_browse_results_changed_handler(
        browser: *mut c_void,
        handler: &DynBlock<dyn Fn(*mut c_void, *mut c_void, Bool)>,
    );
    fn nw_browser_start(browser: *mut c_void);
    fn nw_browser_cancel(browser: *mut c_void);
    fn nw_browse_result_copy_endpoint(result: *mut c_void) -> *mut c_void;
    fn nw_browse_result_copy_txt_record_object(result: *mut c_void) -> *mut c_void;
    fn nw_endpoint_get_bonjour_service_name(endpoint: *mut c_void) -> *const c_char;
    fn nw_txt_record_apply(
        txt: *mut c_void,
        applier: &DynBlock<dyn Fn(*const c_char, i32, *const u8, usize) -> Bool>,
    ) -> Bool;
    fn nw_release(object: *mut c_void);
}

// dns_sd (libSystem).
type DnsServiceRef = *mut c_void;
unsafe extern "C" {
    fn DNSServiceRegister(
        sd_ref: *mut DnsServiceRef,
        flags: u32,
        interface_index: u32,
        name: *const c_char,
        regtype: *const c_char,
        domain: *const c_char,
        host: *const c_char,
        port_network_order: u16,
        txt_len: u16,
        txt_record: *const c_void,
        callback: Option<unsafe extern "C" fn()>,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceRefDeallocate(sd_ref: DnsServiceRef);
}

/// The `nw_txt_record_find_key_t` value of a key with a value.
const TXT_KEY_HAS_VALUE: i32 = 4;

/// A Network.framework object we own one reference to; released on drop.
struct NwObject(NonNull<c_void>);

impl NwObject {
    fn new(ptr: *mut c_void) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

impl Drop for NwObject {
    fn drop(&mut self) {
        // SAFETY: we own one reference (a `create`/`copy` result).
        unsafe { nw_release(self.0.as_ptr()) };
    }
}

/// The service name and TXT entries of a browse result.
fn read_result(result: *mut c_void) -> Option<(String, Vec<(String, String)>)> {
    // SAFETY: `result` is a live `nw_browse_result_t` for the duration of the handler; the copy
    // functions return +1 objects (or null) that `NwObject` releases.
    let endpoint = NwObject::new(unsafe { nw_browse_result_copy_endpoint(result) })?;
    // SAFETY: a live endpoint; the name stays valid while the endpoint lives.
    let name = unsafe { nw_endpoint_get_bonjour_service_name(endpoint.as_ptr()) };
    if name.is_null() {
        return None;
    }
    // SAFETY: non-null C string owned by the endpoint.
    let name = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
    let entries = Arc::new(Mutex::new(Vec::new()));
    // SAFETY: as above.
    if let Some(txt) = NwObject::new(unsafe { nw_browse_result_copy_txt_record_object(result) }) {
        let sink = entries.clone();
        let applier =
            RcBlock::new(move |key: *const c_char, found: i32, value: *const u8, len: usize| -> Bool {
                if key.is_null() || found != TXT_KEY_HAS_VALUE || value.is_null() {
                    return Bool::YES;
                }
                // SAFETY: Network.framework passes a NUL-terminated key and `len` value bytes, valid
                // for the duration of the call.
                let key = unsafe { CStr::from_ptr(key) }.to_string_lossy().into_owned();
                // SAFETY: as above.
                let value = unsafe { std::slice::from_raw_parts(value, len) };
                if let (Ok(mut e), Ok(value)) = (sink.lock(), std::str::from_utf8(value)) {
                    e.push((key, value.to_owned()));
                }
                Bool::YES
            });
        // SAFETY: a live TXT record; the applier runs synchronously.
        unsafe { nw_txt_record_apply(txt.as_ptr(), &applier) };
    }
    let entries = entries.lock().map(|e| e.clone()).unwrap_or_default();
    Some((name, entries))
}

/// A running `nw_browser` for Drift Hosts. Cancelled on drop.
pub struct HostBrowser {
    browser: NwObject,
    _queue: DispatchRetained<DispatchQueue>,
}

impl std::fmt::Debug for HostBrowser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostBrowser").finish_non_exhaustive()
    }
}

// SAFETY: Network.framework objects are thread-safe reference-counted objects; after start we
// only cancel and release the browser.
unsafe impl Send for HostBrowser {}
// SAFETY: as above.
unsafe impl Sync for HostBrowser {}

impl HostBrowser {
    /// Starts browsing; `on_change` gets every valid host, sorted by name, after each batch of
    /// changes (on a private queue). `None` if Network.framework cannot create a browser.
    pub fn start(on_change: impl Fn(Vec<HostAdvert>) + Send + Sync + 'static) -> Option<Self> {
        let kind = CString::new(SERVICE_TYPE).ok()?;
        let domain = CString::new("local.").ok()?;
        // SAFETY: plain constructors with valid C strings; +1 objects or null.
        let descriptor = NwObject::new(unsafe {
            nw_browse_descriptor_create_bonjour_service(kind.as_ptr(), domain.as_ptr())
        })?;
        // SAFETY: a live descriptor.
        unsafe { nw_browse_descriptor_set_include_txt_record(descriptor.as_ptr(), true) };
        // SAFETY: as above.
        let parameters = NwObject::new(unsafe { nw_parameters_create() })?;
        // SAFETY: live descriptor and parameters; the browser retains both.
        let browser = NwObject::new(unsafe { nw_browser_create(descriptor.as_ptr(), parameters.as_ptr()) })?;
        let queue = DispatchQueue::new("com.hutsonlabs.drift.host-browser", None);
        let found: Mutex<BTreeMap<String, Vec<(String, String)>>> = Mutex::new(BTreeMap::new());
        let handler = RcBlock::new(move |old: *mut c_void, new: *mut c_void, batch_complete: Bool| {
            let Ok(mut found) = found.lock() else { return };
            if !old.is_null()
                && let Some((name, _)) = read_result(old)
            {
                found.remove(&name);
            }
            if !new.is_null()
                && let Some((name, txt)) = read_result(new)
            {
                found.insert(name, txt);
            }
            if batch_complete.as_bool() {
                let mut hosts: Vec<HostAdvert> = found
                    .iter()
                    .filter_map(|(name, txt)| {
                        HostAdvert::from_txt(name, txt)
                            .inspect_err(|e| tracing::debug!(service = %name, error = %e, "ignoring a Drift Host advert"))
                            .ok()
                    })
                    .collect();
                hosts.sort_by_key(|h| h.name.to_lowercase());
                drop(found);
                on_change(hosts);
            }
        });
        // SAFETY: a live browser; the queue and the block are retained by Network.framework, and
        // the block only captures Send + Sync state.
        unsafe {
            nw_browser_set_queue(browser.as_ptr(), DispatchRetained::as_ptr(&queue));
            nw_browser_set_browse_results_changed_handler(browser.as_ptr(), &handler);
            nw_browser_start(browser.as_ptr());
        }
        Some(Self { browser, _queue: queue })
    }
}

impl Drop for HostBrowser {
    fn drop(&mut self) {
        // SAFETY: a live browser; cancel stops future callbacks (the release follows in NwObject).
        unsafe { nw_browser_cancel(self.browser.as_ptr()) };
    }
}

/// A registered Bonjour service. Deregistered on drop.
#[derive(Debug)]
pub struct Advertisement {
    sd_ref: NonNull<c_void>,
}

// SAFETY: the DNSServiceRef is only deallocated, once, in Drop.
unsafe impl Send for Advertisement {}

/// The TXT record wire format: each entry a length byte and `key=value`.
fn txt_record(entries: &[(String, String)]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for (key, value) in entries {
        let entry = format!("{key}={value}");
        out.push(u8::try_from(entry.len()).ok()?);
        out.extend_from_slice(entry.as_bytes());
    }
    Some(out)
}

impl Advertisement {
    /// Registers `name` of `kind` on `port` with `txt`. `None` if a field does not fit DNS-SD
    /// (an entry over 255 bytes, a NUL) or the daemon refuses.
    pub fn register(name: &str, kind: &str, port: u16, txt: &[(String, String)]) -> Option<Self> {
        let name = CString::new(name).ok()?;
        let kind = CString::new(kind).ok()?;
        let record = txt_record(txt)?;
        let len = u16::try_from(record.len()).ok()?;
        let mut sd_ref: DnsServiceRef = std::ptr::null_mut();
        // SAFETY: valid C strings and TXT bytes for the duration of the call; no callback, so no
        // context is ever read; `sd_ref` receives the new reference.
        let error = unsafe {
            DNSServiceRegister(
                &raw mut sd_ref,
                0,
                0,
                name.as_ptr(),
                kind.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                port.to_be(),
                len,
                record.as_ptr().cast(),
                None,
                std::ptr::null_mut(),
            )
        };
        if error != 0 {
            tracing::warn!(error, "DNSServiceRegister failed");
            return None;
        }
        NonNull::new(sd_ref).map(|sd_ref| Self { sd_ref })
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        // SAFETY: we own the reference returned by DNSServiceRegister.
        unsafe { DNSServiceRefDeallocate(self.sd_ref.as_ptr()) };
    }
}
