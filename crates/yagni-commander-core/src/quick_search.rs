//! Quick search: typed characters build a prefix, shown in a small box, and
//! the cursor jumps to names starting with it. See [`crate::Commander::search_type`].

#[derive(Debug, Default)]
pub(crate) struct QuickSearch {
    prefix: String,
}

impl QuickSearch {
    /// Whether typing `ch` starts or extends a quick search: any printable
    /// character except space (which toggles selection).
    pub fn accepts(ch: char) -> bool {
        !ch.is_control() && !ch.is_whitespace()
    }

    /// The typed prefix, or `None` while no search is open.
    pub(crate) fn prefix(&self) -> Option<&str> {
        (!self.prefix.is_empty()).then_some(self.prefix.as_str())
    }

    /// The prefix with `ch` appended, to try before accepting it.
    pub(crate) fn candidate(&self, ch: char) -> String {
        let mut prefix = self.prefix.clone();
        prefix.push(ch);
        prefix
    }

    pub(crate) fn set(&mut self, prefix: String) {
        self.prefix = prefix;
    }

    /// Removes the last character; an empty prefix closes the search.
    pub(crate) fn backspace(&mut self) {
        self.prefix.pop();
    }

    pub(crate) fn clear(&mut self) {
        self.prefix.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_printable_characters_but_not_space() {
        for ch in ['a', 'Z', '7', 'æ', 'ø', '.', '-', '_'] {
            assert!(QuickSearch::accepts(ch), "{ch}");
        }
        for ch in [' ', '\t', '\n', '\u{7f}'] {
            assert!(!QuickSearch::accepts(ch), "{ch:?}");
        }
    }

    #[test]
    fn prefix_grows_shrinks_and_clears() {
        let mut search = QuickSearch::default();
        assert_eq!(search.prefix(), None);
        assert_eq!(search.candidate('n'), "n");
        search.set("ne".into());
        assert_eq!(search.candidate('w'), "new");
        assert_eq!(search.prefix(), Some("ne"));
        search.backspace();
        assert_eq!(search.prefix(), Some("n"));
        search.backspace();
        assert_eq!(search.prefix(), None);
        search.backspace();
        search.set("x".into());
        search.clear();
        assert_eq!(search.prefix(), None);
    }
}
