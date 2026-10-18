//! Clipboard payloads exchanged between the platform layer and the session actor.

use serde::{Deserialize, Serialize};

/// One representation of the clipboard contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardItem {
    /// Plain text (LF line endings, no trailing NUL).
    Text(String),
    /// PNG-encoded image bytes (`public.png` / `"image/png"`).
    Png(Vec<u8>),
    /// TIFF-encoded image bytes (`public.tiff` / `CF_TIFF`).
    Tiff(Vec<u8>),
}

/// A clipboard snapshot: every representation available, most preferred first.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ClipboardContents {
    /// Available representations.
    pub items: Vec<ClipboardItem>,
}

impl ClipboardContents {
    /// An empty clipboard.
    pub fn empty() -> Self {
        Self::default()
    }

    /// `true` when there is nothing to offer.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// What one explicit paste on iOS offers the session (IOS-10): the pasted text and/or PNG,
    /// filtered by the profile's clipboard preference as `PasteboardPort::read` does on macOS.
    /// Empty representations are dropped and CRLF becomes LF.
    pub fn from_paste(text: Option<String>, png: Option<Vec<u8>>, level: drift_core::ClipboardPrefs) -> Self {
        use drift_core::ClipboardPrefs;
        let mut items = Vec::new();
        if level == ClipboardPrefs::Off {
            return Self { items };
        }
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            items.push(ClipboardItem::Text(text.replace("\r\n", "\n")));
        }
        if level == ClipboardPrefs::TextAndImages
            && let Some(png) = png.filter(|p| !p.is_empty())
        {
            items.push(ClipboardItem::Png(png));
        }
        Self { items }
    }

    /// The text representation, if any.
    pub fn text(&self) -> Option<&str> {
        self.items.iter().find_map(|i| match i {
            ClipboardItem::Text(t) => Some(t.as_str()),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_lookup() {
        assert!(ClipboardContents::empty().is_empty());
        let c =
            ClipboardContents { items: vec![ClipboardItem::Png(vec![1]), ClipboardItem::Text("hi".into())] };
        assert_eq!(c.text(), Some("hi"));
        assert!(!c.is_empty());
        assert_eq!(ClipboardContents { items: vec![ClipboardItem::Tiff(vec![])] }.text(), None);
    }
}
