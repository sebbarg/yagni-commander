//! A tab's folder history: Alt-Left/Alt-Right go back and forward, Alt-Down
//! lists it. Kept for the session only, per tab (Ctrl-T copies it).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// How many folders a tab remembers; the oldest go first. Also what the
/// Alt-Down popup can show without scrolling.
pub const MAX_PLACES: usize = 16;

/// A folder a tab showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The panel path (through an archive file inside one, `sftp://` on a
    /// host).
    pub path: PathBuf,
    /// The archive `path` is inside, if any.
    pub archive: Option<PathBuf>,
    /// The entry the cursor was on when the tab left the folder.
    pub select: Option<OsString>,
}

/// The folders in the order they were shown, and where the tab is in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    places: Vec<Place>,
    /// The place shown now; meaningless while `places` is empty.
    current: usize,
}

impl History {
    /// Oldest first.
    pub fn places(&self) -> &[Place] {
        &self.places
    }

    /// The index of the place shown now.
    pub fn current(&self) -> usize {
        self.current
    }

    /// The place `step` away from the current one (-1 back, 1 forward).
    pub fn step(&self, step: isize) -> Option<usize> {
        let ix = self.current.checked_add_signed(step)?;
        (ix < self.places.len()).then_some(ix)
    }

    pub fn get(&self, ix: usize) -> Option<&Place> {
        self.places.get(ix)
    }

    /// The tab leaves the current place with the cursor on `select`.
    pub(crate) fn leave(&mut self, select: Option<OsString>) {
        if let Some(place) = self.places.get_mut(self.current) {
            place.select = select;
        }
    }

    /// The tab now shows `path`. `travel` is the place it went to through
    /// the history; else (or if that place is no longer `path`, say the
    /// folder was gone and an ancestor is shown) it is a new place after
    /// the current one, and the places ahead are dropped, like a browser's.
    pub(crate) fn arrive(&mut self, path: &Path, archive: Option<&Path>, travel: Option<usize>) {
        if let Some(ix) = travel
            && self.places.get(ix).is_some_and(|p| p.path == path)
        {
            self.current = ix;
            return;
        }
        if self
            .places
            .get(self.current)
            .is_some_and(|p| p.path == path)
        {
            return;
        }
        if !self.places.is_empty() {
            self.places.truncate(self.current + 1);
        }
        self.places.push(Place {
            path: path.to_path_buf(),
            archive: archive.map(Path::to_path_buf),
            select: None,
        });
        if self.places.len() > MAX_PLACES {
            self.places.remove(0);
        }
        self.current = self.places.len() - 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(h: &History) -> Vec<&str> {
        h.places()
            .iter()
            .map(|p| p.path.to_str().unwrap())
            .collect()
    }

    fn walk(h: &mut History, path: &str) {
        h.arrive(Path::new(path), None, None);
    }

    #[test]
    fn new_places_go_after_the_current_one() {
        let mut h = History::default();
        assert_eq!(h.step(-1), None);
        walk(&mut h, "/a");
        walk(&mut h, "/b");
        walk(&mut h, "/c");
        assert_eq!(paths(&h), ["/a", "/b", "/c"]);
        assert_eq!(h.current(), 2);
        assert_eq!(h.step(-1), Some(1));
        assert_eq!(h.step(1), None);
        // The same folder again (a reload) is no new place.
        walk(&mut h, "/c");
        assert_eq!(paths(&h), ["/a", "/b", "/c"]);
    }

    #[test]
    fn travelling_moves_within_and_a_new_place_drops_those_ahead() {
        let mut h = History::default();
        for p in ["/a", "/b", "/c"] {
            walk(&mut h, p);
        }
        h.arrive(Path::new("/a"), None, Some(0));
        assert_eq!(h.current(), 0);
        assert_eq!(h.step(1), Some(1));
        walk(&mut h, "/d");
        assert_eq!(paths(&h), ["/a", "/d"]);
        assert_eq!(h.current(), 1);
    }

    #[test]
    fn a_travel_that_lands_elsewhere_is_a_new_place() {
        let mut h = History::default();
        for p in ["/a", "/a/gone", "/b"] {
            walk(&mut h, p);
        }
        // Back to /a/gone, which vanished: the panel shows /a.
        h.arrive(Path::new("/a"), None, Some(1));
        assert_eq!(paths(&h), ["/a", "/a/gone", "/b", "/a"]);
        assert_eq!(h.current(), 3);
    }

    #[test]
    fn leaving_remembers_the_cursor_entry() {
        let mut h = History::default();
        h.leave(Some("x".into())); // nothing yet: no-op
        walk(&mut h, "/a");
        h.leave(Some("x".into()));
        walk(&mut h, "/b");
        assert_eq!(h.get(0).unwrap().select.as_deref(), Some("x".as_ref()));
        assert_eq!(h.get(1).unwrap().select, None);
    }

    #[test]
    fn archives_are_remembered_with_their_place() {
        let mut h = History::default();
        h.arrive(Path::new("/x/a.zip/src"), Some(Path::new("/x/a.zip")), None);
        assert_eq!(
            h.get(0).unwrap().archive.as_deref(),
            Some(Path::new("/x/a.zip"))
        );
    }

    #[test]
    fn the_oldest_places_go_first() {
        let mut h = History::default();
        for i in 0..MAX_PLACES + 3 {
            walk(&mut h, &format!("/{i}"));
        }
        assert_eq!(h.places().len(), MAX_PLACES);
        assert_eq!(h.places()[0].path, Path::new("/3"));
        assert_eq!(h.current(), MAX_PLACES - 1);
    }
}
