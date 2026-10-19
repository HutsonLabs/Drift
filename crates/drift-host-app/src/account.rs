//! The Mac account's password (task **HOST-15**): a prompt with a secure field, and a check
//! against the local directory so a typo is caught here, not as a failed sign-in on the device.
//! Humble object; the words live in [`crate::model::password_prompt`].

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{MainThreadMarker, MainThreadOnly, msg_send};
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSApplication, NSSecureTextField};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::model::PasswordPrompt;

#[link(name = "OpenDirectory", kind = "framework")]
unsafe extern "C" {}

/// `kODNodeTypeLocalNodes`.
const LOCAL_NODES: u32 = 0x2200;
/// `kODRecordTypeUsers`.
const USERS: &str = "dsRecTypeStandard:Users";

/// Whether `password` is `account`'s login password on this Mac; `None` when the local
/// directory cannot tell (no such local account, e.g. a network account).
pub fn verify_password(account: &str, password: &str) -> Option<bool> {
    let session_class = AnyClass::get(c"ODSession")?;
    let node_class = AnyClass::get(c"ODNode")?;
    let no_error = std::ptr::null_mut::<*mut AnyObject>();
    // SAFETY: documented OpenDirectory class methods and selectors; the error out-parameters are
    // null (allowed), and every returned object is checked for nil.
    unsafe {
        let session: Option<Retained<AnyObject>> = msg_send![session_class, defaultSession];
        let session = session?;
        let node: Option<Retained<AnyObject>> =
            msg_send![node_class, nodeWithSession: &*session, type: LOCAL_NODES, error: no_error];
        let node = node?;
        let record: Option<Retained<AnyObject>> = msg_send![
            &*node,
            recordWithRecordType: &*NSString::from_str(USERS),
            name: &*NSString::from_str(account),
            attributes: std::ptr::null::<AnyObject>(),
            error: no_error
        ];
        let record = record?;
        let ok: Bool = msg_send![&*record, verifyPassword: &*NSString::from_str(password), error: no_error];
        Some(ok.as_bool())
    }
}

/// Asks for the password with `prompt`; `None` when the user cancelled.
pub fn ask(mtm: MainThreadMarker, prompt: &PasswordPrompt) -> Option<String> {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&prompt.title));
    alert.setInformativeText(&NSString::from_str(&prompt.body));
    alert.addButtonWithTitle(&NSString::from_str("Save"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    let field = NSSecureTextField::initWithFrame(
        NSSecureTextField::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(260.0, 24.0)),
    );
    field.setPlaceholderString(Some(&NSString::from_str("Password")));
    alert.setAccessoryView(Some(&field));
    alert.window().setInitialFirstResponder(Some(&field));
    NSApplication::sharedApplication(mtm).activate();
    if alert.runModal() != NSAlertFirstButtonReturn {
        return None;
    }
    Some(field.stringValue().to_string())
}
