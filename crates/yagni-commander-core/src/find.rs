//! Alt-F7: finding files below a folder by name masks and, optionally, by
//! content. The app runs [`search`] on a thread of its own.

pub mod masks;
pub mod text;
mod walk;

pub use walk::{Progress, Query, SearchSummary, search};

/// The folders the find dialog's "Skip folders" offers.
pub const SKIP_FOLDERS: [&str; 9] = [
    ".git",
    "node_modules",
    "bin",
    "obj",
    "target",
    ".venv",
    "__pycache__",
    "dist",
    "build",
];

/// Those skipped until the user changes it: the ones that are never
/// anything but tooling.
pub const DEFAULT_SKIP: [&str; 2] = [".git", "node_modules"];

/// Why a command that writes a name is refused in a results listing.
pub const IN_RESULTS: &str = "Not available in search results.";

/// A finished search, as a panel shows it ("Feed to panel").
#[derive(Debug, Clone)]
pub struct Results {
    pub root: std::path::PathBuf,
    /// The masks as typed, for the header.
    pub masks: String,
    /// Named by their path relative to `root`. Shared with the find
    /// dialog, which keeps them for the next Alt-F7.
    pub entries: std::sync::Arc<Vec<crate::Entry>>,
}

/// Same root, masks and entry names (navigations compare by this).
impl PartialEq for Results {
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root
            && self.masks == other.masks
            && self.entries.len() == other.entries.len()
            && self
                .entries
                .iter()
                .zip(other.entries.iter())
                .all(|(a, b)| a.name == b.name)
    }
}

impl Eq for Results {}

impl Results {
    pub fn new(
        root: std::path::PathBuf,
        masks: String,
        entries: std::sync::Arc<Vec<crate::Entry>>,
    ) -> Self {
        Self {
            root,
            masks,
            entries,
        }
    }

    /// The panel's path header: `Results: *.rs in /x/proj`.
    pub fn header(&self) -> String {
        format!("{}{}", self.header_prefix(), self.root.display())
    }

    /// The header before the folder, which a narrow panel never cuts.
    pub fn header_prefix(&self) -> String {
        match self.masks.trim() {
            "" => "Results in ".to_owned(),
            masks => format!("Results: {masks} in "),
        }
    }

    /// The panel listing: ".." (unless the root is `/`) and the results.
    pub(crate) fn listing(self: &std::sync::Arc<Self>) -> crate::Listing {
        let mut entries = Vec::with_capacity(self.entries.len() + 1);
        if self.root.parent().is_some() {
            entries.push(crate::Entry::parent());
        }
        entries.extend(self.entries.iter().cloned());
        crate::Listing {
            path: self.root.clone(),
            entries,
            archive: None,
            results: Some(self.clone()),
        }
    }

    /// The same results, re-stat'ed: entries that are gone are dropped.
    pub(crate) fn recheck(&self) -> Self {
        let mut owners = crate::entry::OwnerCache::default();
        let entries = self
            .entries
            .iter()
            .filter_map(|e| {
                let path = self.root.join(&e.name);
                let meta = std::fs::symlink_metadata(&path).ok()?;
                Some(crate::entry::stat_entry(
                    e.name.clone(),
                    &path,
                    Some(meta),
                    &mut owners,
                ))
            })
            .collect();
        Self {
            root: self.root.clone(),
            masks: self.masks.clone(),
            entries: std::sync::Arc::new(entries),
        }
    }
}
