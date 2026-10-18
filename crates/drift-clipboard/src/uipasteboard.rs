//! UIPasteboard adapter (task **IOS-10**, iOS only). A humble object: decisions live in
//! [`crate::poll`], [`crate::sync`] and [`crate::formats`].
//!
//! iOS shows "Drift would like to paste from …" whenever an app reads another app's pasteboard
//! contents on its own, so Drift never polls: [`UiPasteboard::read`] is only for tests and named
//! pasteboards, the local → remote direction uses an explicit paste (`drift-ios::paste`), and
//! [`UiPasteboard::has_contents`] asks only `hasStrings`/`hasImages`, which never prompt.
//! Writing (remote → local) is allowed without a prompt.
//!
//! Representations: text is `public.utf8-plain-text`; images are `public.png` (any other image
//! is re-encoded to PNG through `UIImage`).

use drift_core::ClipboardPrefs;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_ui_kit::{UIImage, UIPasteboard};

use crate::poll::PasteboardPort;
use crate::{ClipboardContents, ClipboardItem};

const TEXT_TYPE: &str = "public.utf8-plain-text";
const PNG_TYPE: &str = "public.png";

/// A `UIPasteboard` implementing [`PasteboardPort`].
#[derive(Debug)]
pub struct UiPasteboard {
    pb: Retained<UIPasteboard>,
    unique: bool,
}

/// Re-encodes image `data` (any format `UIImage` reads) as PNG.
fn to_png(data: &NSData) -> Option<Vec<u8>> {
    let image = UIImage::imageWithData(data)?;
    Some(image.png_representation()?.to_vec())
}

impl UiPasteboard {
    /// The system general pasteboard.
    pub fn general() -> Self {
        Self { pb: UIPasteboard::generalPasteboard(), unique: false }
    }

    /// A private pasteboard with a unique name (tests); removed on drop.
    pub fn unique() -> Self {
        Self { pb: UIPasteboard::pasteboardWithUniqueName(), unique: true }
    }

    /// Whether the pasteboard holds something `level` would pass on, **without reading it**
    /// (`hasStrings` / `hasImages` never show the paste prompt).
    pub fn has_contents(&self, level: ClipboardPrefs) -> bool {
        // SAFETY: plain BOOL getters on a live pasteboard.
        let (strings, images) = unsafe { (self.pb.hasStrings(), self.pb.hasImages()) };
        match level {
            ClipboardPrefs::Off => false,
            ClipboardPrefs::Text => strings,
            ClipboardPrefs::TextAndImages => strings || images,
        }
    }
}

impl PasteboardPort for UiPasteboard {
    fn change_count(&self) -> i64 {
        // SAFETY: a plain NSInteger getter on a live pasteboard.
        i64::try_from(unsafe { self.pb.changeCount() }).unwrap_or(i64::MAX)
    }

    fn read(&self, level: ClipboardPrefs) -> ClipboardContents {
        let mut items = Vec::new();
        if level == ClipboardPrefs::Off {
            return ClipboardContents { items };
        }
        // SAFETY: a getter returning an autoreleased copy of the first string, or nil.
        if let Some(text) = unsafe { self.pb.string() } {
            items.push(ClipboardItem::Text(text.to_string()));
        }
        if level == ClipboardPrefs::TextAndImages {
            if let Some(png) = self.pb.dataForPasteboardType(&NSString::from_str(PNG_TYPE)) {
                items.push(ClipboardItem::Png(png.to_vec()));
            } else if let Some(image) = unsafe {
                // SAFETY: a getter returning the first image, or nil.
                self.pb.image()
            } && let Some(png) = image.png_representation()
            {
                items.push(ClipboardItem::Png(png.to_vec()));
            }
        }
        ClipboardContents { items }
    }

    fn write(&self, contents: &ClipboardContents) -> i64 {
        let mut keys: Vec<Retained<NSString>> = Vec::new();
        let mut values: Vec<Retained<AnyObject>> = Vec::new();
        for item in &contents.items {
            match item {
                ClipboardItem::Text(t) => {
                    keys.push(NSString::from_str(TEXT_TYPE));
                    values.push(Retained::into_super(Retained::into_super(NSString::from_str(t))));
                }
                ClipboardItem::Png(p) => {
                    keys.push(NSString::from_str(PNG_TYPE));
                    values.push(Retained::into_super(Retained::into_super(NSData::with_bytes(p))));
                }
                ClipboardItem::Tiff(t) => {
                    if let Some(png) = to_png(&NSData::with_bytes(t)) {
                        keys.push(NSString::from_str(PNG_TYPE));
                        values.push(Retained::into_super(Retained::into_super(NSData::with_bytes(&png))));
                    }
                }
            }
        }
        let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let item = NSDictionary::from_retained_objects(&key_refs, &values);
        let items =
            if contents.items.is_empty() { NSArray::new() } else { NSArray::from_retained_slice(&[item]) };
        // SAFETY: one dictionary of UTI → NSString/NSData values, the types UIPasteboard accepts.
        unsafe { self.pb.setItems(&items) };
        self.change_count()
    }
}

impl Drop for UiPasteboard {
    fn drop(&mut self) {
        if self.unique {
            // SAFETY: the name of a live pasteboard; removing an app-private named pasteboard.
            let name = unsafe { self.pb.name() };
            UIPasteboard::removePasteboardWithName(&name);
        }
    }
}
