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

    fn with_owner_and_mode(label: &str, owner: &str, mode: u32) -> Entry {
        Entry {
            owner: Some(owner.into()),
            mode: Some(mode),
            ..entry(label, EntryKind::File, Some(1), 0)
        }
    }

    fn sorted_by(entries: &mut [Entry], sort: Sort) -> Vec<String> {
        sort_entries(entries, sort);
        entries.iter().map(|e| e.label.clone()).collect()
    }

    #[test]
    fn default_sort_is_name_ascending() {
        assert_eq!(
            Sort::default(),
            Sort {
                key: SortKey::Name,
                descending: false
            }
        );
    }

    #[test]
    fn toggling_other_key_uses_its_starting_direction() {
        let by_size = Sort::default().toggled(SortKey::Size);
        assert!(by_size.descending);
        let by_owner = by_size.toggled(SortKey::Owner);
        assert_eq!(by_owner.key, SortKey::Owner);
        assert!(!by_owner.descending);
        assert!(by_owner.toggled(SortKey::Owner).descending);
        assert!(!by_owner.toggled(SortKey::Permissions).descending);
        assert!(by_owner.toggled(SortKey::Modified).descending);
    }

    #[test]
    fn sorts_by_owner_with_name_tie_break() {
        let mut entries = vec![
            with_owner_and_mode("c", "root:root", 0o644),
            with_owner_and_mode("a", "seb:seb", 0o644),
            with_owner_and_mode("b", "root:root", 0o644),
        ];
        let asc = Sort::default().toggled(SortKey::Owner);
        assert_eq!(sorted_by(&mut entries, asc), ["b", "c", "a"]);
        // Descending flips the key but keeps names ascending within a group.
        assert_eq!(
            sorted_by(&mut entries, asc.toggled(SortKey::Owner)),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn sorts_by_permissions_mode_value() {
        let mut entries = vec![
            with_owner_and_mode("x", "u:g", 0o100755),
            with_owner_and_mode("r", "u:g", 0o100444),
            with_owner_and_mode("w", "u:g", 0o100644),
        ];
        let asc = Sort::default().toggled(SortKey::Permissions);
        assert_eq!(sorted_by(&mut entries, asc), ["r", "w", "x"]);
    }

    #[test]
    fn missing_values_sort_first_ascending() {
        let mut entries = vec![
            entry("known", EntryKind::File, Some(5), 0),
            entry("unknown", EntryKind::File, None, 0),
        ];
        let asc = Sort::default()
            .toggled(SortKey::Size)
            .toggled(SortKey::Size);
        assert_eq!(sorted_by(&mut entries, asc), ["unknown", "known"]);
    }

    #[test]
    fn identical_labels_are_ordered_by_raw_name() {
        let mut upper = entry("same", EntryKind::File, None, 0);
        upper.name = "SAME".into();
        upper.label = "SAME".into();
        let mut entries = vec![entry("same", EntryKind::File, None, 0), upper];
        // Case-insensitive keys tie, so the raw name decides ("S" < "s").
        assert_eq!(sorted_by(&mut entries, Sort::default()), ["SAME", "same"]);
    }
}
