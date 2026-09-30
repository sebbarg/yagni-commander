use std::cmp::Ordering;

use crate::entry::{Entry, EntryKind};

/// Column a panel is sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Owner,
    Permissions,
}

impl SortKey {
    /// Direction used when first sorting by this column. Like TC, size and date
    /// start with the largest/newest first.
    fn starts_descending(self) -> bool {
        matches!(self, SortKey::Size | SortKey::Modified)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            key: SortKey::Name,
            descending: false,
        }
    }
}

impl Sort {
    /// The sort that results from clicking column `key`: reverses the direction
    /// if already sorted by it, otherwise sorts by it in its default direction.
    pub fn toggled(self, key: SortKey) -> Self {
        if self.key == key {
            Self {
                key,
                descending: !self.descending,
            }
        } else {
            Self {
                key,
                descending: key.starts_descending(),
            }
        }
    }
}

/// Sorts TC-style: ".." first, then directories, then files. Within each group
/// by `sort`, with ties broken by case-insensitive name.
pub(crate) fn sort_entries(entries: &mut [Entry], sort: Sort) {
    entries.sort_by(|a, b| compare(a, b, sort));
}

fn rank(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Parent => 0,
        EntryKind::Dir => 1,
        EntryKind::File => 2,
    }
}

fn compare(a: &Entry, b: &Entry, sort: Sort) -> Ordering {
    let by_key = match sort.key {
        SortKey::Name => Ordering::Equal,
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Modified => a.modified.cmp(&b.modified),
        SortKey::Owner => a.owner.cmp(&b.owner),
        SortKey::Permissions => a.mode.cmp(&b.mode),
    };
    let by_name = a
        .sort_name
        .cmp(&b.sort_name)
        .then_with(|| a.name.cmp(&b.name));
    let (by_key, by_name) = match (sort.descending, sort.key) {
        (false, _) => (by_key, by_name),
        (true, SortKey::Name) => (by_key, by_name.reverse()),
        (true, _) => (by_key.reverse(), by_name),
    };
    rank(a.kind).cmp(&rank(b.kind)).then(by_key).then(by_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn entry(label: &str, kind: EntryKind, size: Option<u64>, age: u64) -> Entry {
        Entry {
            name: label.into(),
            label: label.into(),
            sort_name: label.to_lowercase(),
            kind,
            size,
            modified: Some(UNIX_EPOCH + Duration::from_secs(1000 - age)),
            ..Entry::parent()
        }
    }

    fn fixture() -> Vec<Entry> {
        vec![
            entry("b.txt", EntryKind::File, Some(10), 1),
            entry("zdir", EntryKind::Dir, None, 5),
            entry("A.txt", EntryKind::File, Some(300), 3),
            Entry::parent(),
            entry("Adir", EntryKind::Dir, None, 2),
            entry("c.txt", EntryKind::File, Some(10), 2),
        ]
    }

    fn sorted(sort: Sort) -> Vec<String> {
        let mut entries = fixture();
        sort_entries(&mut entries, sort);
        entries.into_iter().map(|e| e.label).collect()
    }

    #[test]
    fn name_ascending_is_case_insensitive_with_dirs_first() {
        assert_eq!(
            sorted(Sort::default()),
            ["..", "Adir", "zdir", "A.txt", "b.txt", "c.txt"]
        );
    }

    #[test]
    fn name_descending_keeps_parent_and_dirs_first() {
        let sort = Sort::default().toggled(SortKey::Name);
        assert!(sort.descending);
        assert_eq!(
            sorted(sort),
            ["..", "zdir", "Adir", "c.txt", "b.txt", "A.txt"]
        );
    }

    #[test]
    fn size_starts_descending_and_ties_break_by_name() {
        let sort = Sort::default().toggled(SortKey::Size);
        assert!(sort.descending);
        // Directories have no size, so they stay in name order.
        assert_eq!(
            sorted(sort),
            ["..", "Adir", "zdir", "A.txt", "b.txt", "c.txt"]
        );
        let sort = sort.toggled(SortKey::Size);
        assert_eq!(
            sorted(sort),
            ["..", "Adir", "zdir", "b.txt", "c.txt", "A.txt"]
        );
    }

    #[test]
    fn modified_starts_newest_first() {
        let sort = Sort::default().toggled(SortKey::Modified);
        assert_eq!(
            sorted(sort),
            ["..", "Adir", "zdir", "b.txt", "c.txt", "A.txt"]
        );
    }
}
