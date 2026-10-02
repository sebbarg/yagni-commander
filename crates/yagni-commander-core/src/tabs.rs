//! The tabs of one side: a never-empty list of panels and the one in front.

use std::path::Path;

use crate::panel::Panel;

#[derive(Debug)]
pub struct Tabs {
    panels: Vec<Panel>,
    active: usize,
}

impl Tabs {
    pub(crate) fn new(panel: Panel) -> Self {
        Self::from_panels(vec![panel], 0)
    }

    /// `panels` must not be empty; `active` is clamped to the last tab.
    pub(crate) fn from_panels(panels: Vec<Panel>, active: usize) -> Self {
        assert!(!panels.is_empty(), "a side always has a tab");
        let active = active.min(panels.len() - 1);
        Self { panels, active }
    }

    pub fn count(&self) -> usize {
        self.panels.len()
    }

    /// Index of the tab in front.
    pub fn index(&self) -> usize {
        self.active
    }

    pub fn active(&self) -> &Panel {
        &self.panels[self.active]
    }

    pub(crate) fn active_mut(&mut self) -> &mut Panel {
        &mut self.panels[self.active]
    }

    pub fn get(&self, ix: usize) -> Option<&Panel> {
        self.panels.get(ix)
    }

    pub(crate) fn get_mut(&mut self, ix: usize) -> Option<&mut Panel> {
        self.panels.get_mut(ix)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Panel> {
        self.panels.iter()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Panel> {
        self.panels.iter_mut()
    }

    /// Ctrl-T: inserts `panel` after the tab in front and brings it to front.
    pub(crate) fn open(&mut self, panel: Panel) {
        self.active += 1;
        self.panels.insert(self.active, panel);
    }

    /// Ctrl-W: closes the tab in front; its right neighbor comes to front,
    /// else its left one. False (nothing happens) on the only tab.
    pub(crate) fn close(&mut self) -> bool {
        if self.panels.len() == 1 {
            return false;
        }
        self.panels.remove(self.active);
        self.active = self.active.min(self.panels.len() - 1);
        true
    }

    /// Ctrl-Tab, wrapping around. False with one tab.
    pub(crate) fn next(&mut self) -> bool {
        let n = self.panels.len();
        self.active = (self.active + 1) % n;
        n > 1
    }

    /// Ctrl-Shift-Tab, wrapping around. False with one tab.
    pub(crate) fn prev(&mut self) -> bool {
        let n = self.panels.len();
        self.active = (self.active + n - 1) % n;
        n > 1
    }

    /// A click on tab `ix`. False if it is out of range or already in front.
    pub(crate) fn select(&mut self, ix: usize) -> bool {
        if ix >= self.panels.len() || ix == self.active {
            return false;
        }
        self.active = ix;
        true
    }
}

/// A tab's label: the folder's last component, `/` for the root.
pub fn label(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn panel(name: &str) -> Panel {
        Panel::empty(PathBuf::from("/").join(name), false)
    }

    fn names(tabs: &Tabs) -> Vec<String> {
        tabs.iter().map(|p| label(p.path())).collect()
    }

    #[test]
    fn open_inserts_after_the_active_tab_and_activates_it() {
        let mut tabs = Tabs::new(panel("a"));
        tabs.open(panel("b"));
        tabs.select(0);
        tabs.open(panel("c"));
        assert_eq!(names(&tabs), ["a", "c", "b"]);
        assert_eq!(tabs.index(), 1);
        assert_eq!(tabs.count(), 3);
    }

    #[test]
    fn close_never_removes_the_last_tab() {
        let mut tabs = Tabs::new(panel("a"));
        assert!(!tabs.close());
        assert_eq!(names(&tabs), ["a"]);
    }

    #[test]
    fn close_activates_the_right_neighbor_else_the_left_one() {
        let mut tabs = Tabs::from_panels(vec![panel("a"), panel("b"), panel("c")], 1);
        assert!(tabs.close());
        assert_eq!(
            (names(&tabs), tabs.index()),
            (vec!["a".into(), "c".into()], 1)
        );
        assert!(tabs.close()); // the last tab: its left neighbor
        assert_eq!((names(&tabs), tabs.index()), (vec!["a".to_string()], 0));
    }

    #[test]
    fn next_and_prev_wrap_and_do_nothing_with_one_tab() {
        let mut one = Tabs::new(panel("a"));
        assert!(!one.next());
        assert!(!one.prev());
        let mut tabs = Tabs::from_panels(vec![panel("a"), panel("b"), panel("c")], 2);
        assert!(tabs.next());
        assert_eq!(tabs.index(), 0);
        assert!(tabs.prev());
        assert_eq!(tabs.index(), 2);
    }

    #[test]
    fn select_ignores_the_active_tab_and_out_of_range() {
        let mut tabs = Tabs::from_panels(vec![panel("a"), panel("b")], 0);
        assert!(!tabs.select(0));
        assert!(!tabs.select(2));
        assert!(tabs.select(1));
        assert_eq!(tabs.active().path(), Path::new("/b"));
    }

    #[test]
    fn from_panels_clamps_the_active_index() {
        let tabs = Tabs::from_panels(vec![panel("a"), panel("b")], 7);
        assert_eq!(tabs.index(), 1);
    }

    #[test]
    fn labels_are_the_last_component_or_the_root() {
        assert_eq!(label(Path::new("/")), "/");
        assert_eq!(label(Path::new("/home/seb")), "seb");
        assert_eq!(label(Path::new("/home/seb/")), "seb");
        use std::os::unix::ffi::OsStrExt;
        let odd = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/a\xffb"));
        assert_eq!(label(odd), "a\u{fffd}b");
    }
}
