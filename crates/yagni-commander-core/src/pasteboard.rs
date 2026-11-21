//! macOS: Ctrl-C's files on the general pasteboard, as Finder's Cmd-C
//! puts them (one item per file, its `public.file-url`), so Finder and
//! other apps paste the files. gpui's own clipboard writes text only.

use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSString, NSURL};

/// The pasteboard types, by their UTIs: AppKit's constants for them are
/// extern statics, which only `unsafe` code may read.
const FILE_URL: &str = "public.file-url";
const PLAIN_TEXT: &str = "public.utf8-plain-text";

/// Puts `paths` on the general pasteboard as files, and `text` as the first
/// item's plain text (what a text field or a terminal pastes, and what gpui
/// reads back as the clipboard's text). Returns false, leaving the
/// pasteboard as it was, when a path is not UTF-8 or AppKit refuses.
pub fn write_files(paths: &[&Path], text: &str) -> bool {
    write_files_to(&NSPasteboard::generalPasteboard(), paths, text)
}

fn write_files_to(pasteboard: &NSPasteboard, paths: &[&Path], text: &str) -> bool {
    let file_url = NSString::from_str(FILE_URL);
    let plain_text = NSString::from_str(PLAIN_TEXT);
    let mut items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
        Vec::with_capacity(paths.len());
    for (ix, path) in paths.iter().enumerate() {
        let Some(path) = path.to_str() else {
            return false;
        };
        let Some(url) = NSURL::fileURLWithPath(&NSString::from_str(path)).absoluteString() else {
            return false;
        };
        let item = NSPasteboardItem::new();
        if !item.setString_forType(&url, &file_url) {
            return false;
        }
        if ix == 0 && !item.setString_forType(&NSString::from_str(text), &plain_text) {
            return false;
        }
        items.push(ProtocolObject::from_retained(item));
    }
    if items.is_empty() {
        return false;
    }
    pasteboard.clearContents();
    pasteboard.writeObjects(&NSArray::from_retained_slice(&items))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private pasteboard, never the general one (the owner's clipboard).
    /// One name, so runs don't pile up pasteboards in the system's server
    /// (objc2 offers no `releaseGlobally`); only one test writes to it.
    fn private() -> Retained<NSPasteboard> {
        NSPasteboard::pasteboardWithName(&NSString::from_str("dk.yagni.yagni-commander.test"))
    }

    fn strings(pasteboard: &NSPasteboard, kind: &str) -> Vec<Option<String>> {
        let kind = NSString::from_str(kind);
        pasteboard
            .pasteboardItems()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.stringForType(&kind).map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn each_file_is_an_item_with_its_url_and_the_first_holds_the_text() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a b.txt");
        std::fs::write(&file, "x").unwrap();
        let pasteboard = private();
        let paths = [file.as_path(), tmp.path()];
        assert!(write_files_to(&pasteboard, &paths, "the text"));
        let urls: Vec<_> = strings(&pasteboard, FILE_URL)
            .into_iter()
            .map(Option::unwrap)
            .collect();
        assert_eq!(urls.len(), 2);
        assert!(urls[0].starts_with("file:///") && urls[0].ends_with("/a%20b.txt"));
        assert!(urls[1].ends_with('/'), "a folder's URL: {}", urls[1]);
        assert_eq!(
            strings(&pasteboard, PLAIN_TEXT),
            [Some("the text".to_owned()), None]
        );
        pasteboard.clearContents();
    }

    #[test]
    fn nothing_to_write_leaves_the_pasteboard_alone() {
        let pasteboard = private();
        assert!(!write_files_to(&pasteboard, &[], "text"));
    }
}
