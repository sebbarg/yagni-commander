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
/// item's plain text (what a text field or a terminal pastes). Returns the
/// pasteboard's change count after the write, for `change_count` to compare
/// with; `None`, leaving the pasteboard as it was, when a path is not UTF-8
/// or AppKit refuses.
pub fn write_files(paths: &[&Path], text: &str) -> Option<isize> {
    write_files_to(&NSPasteboard::generalPasteboard(), paths, text)
}

/// The general pasteboard's change count, which moves on whenever any app
/// puts something new there: while it is the one `write_files` returned,
/// the files are still there. Never read them back to find out: reading a
/// file URL makes AppKit try to consume the sandbox extension the
/// pasteboard attached to it, which fails in an app without a sandbox and
/// logs `sandbox_extension_consume failed: 1 (Operation not permitted)` to
/// stderr, once per file and read.
pub fn change_count() -> isize {
    NSPasteboard::generalPasteboard().changeCount()
}

fn write_files_to(pasteboard: &NSPasteboard, paths: &[&Path], text: &str) -> Option<isize> {
    let file_url = NSString::from_str(FILE_URL);
    let plain_text = NSString::from_str(PLAIN_TEXT);
    let mut items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
        Vec::with_capacity(paths.len());
    for (ix, path) in paths.iter().enumerate() {
        let path = path.to_str()?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(path)).absoluteString()?;
        let item = NSPasteboardItem::new();
        if !item.setString_forType(&url, &file_url) {
            return None;
        }
        if ix == 0 && !item.setString_forType(&NSString::from_str(text), &plain_text) {
            return None;
        }
        items.push(ProtocolObject::from_retained(item));
    }
    if items.is_empty() {
        return None;
    }
    pasteboard.clearContents();
    // Read after the write, whether or not writing moves the count on.
    pasteboard
        .writeObjects(&NSArray::from_retained_slice(&items))
        .then(|| pasteboard.changeCount())
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
        // Also the change count: one test, since only one writes.
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a b.txt");
        std::fs::write(&file, "x").unwrap();
        let pasteboard = private();
        let paths = [file.as_path(), tmp.path()];
        let count = write_files_to(&pasteboard, &paths, "the text").unwrap();
        assert_eq!(pasteboard.changeCount(), count);
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
        // Reading moved nothing on; new contents do.
        assert_eq!(pasteboard.changeCount(), count);
        pasteboard.clearContents();
        assert_ne!(pasteboard.changeCount(), count);
    }

    #[test]
    fn nothing_to_write_leaves_the_pasteboard_alone() {
        let pasteboard = private();
        assert_eq!(write_files_to(&pasteboard, &[], "text"), None);
    }
}
