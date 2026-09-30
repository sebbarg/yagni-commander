//! Type-to-jump: letters and digits typed in quick succession form a prefix
//! the cursor jumps to. There is no visible search box.

use std::time::{Duration, Instant};

/// A pause longer than this starts a new prefix.
const RESET_AFTER: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
pub struct QuickSearch {
    prefix: String,
    last_key: Option<Instant>,
}

impl QuickSearch {
    /// Whether `ch` starts or extends a quick search.
    pub fn accepts(ch: char) -> bool {
        ch.is_alphanumeric()
    }

    /// The prefix to try when `ch` is typed at `now`.
    pub fn candidate(&self, ch: char, now: Instant) -> String {
        let fresh = self
            .last_key
            .is_none_or(|last| now.saturating_duration_since(last) > RESET_AFTER);
        let mut prefix = if fresh {
            String::new()
        } else {
            self.prefix.clone()
        };
        prefix.push(ch);
        prefix
    }

    /// Records that `prefix` matched, so the next key extends it.
    pub fn accept(&mut self, prefix: String, now: Instant) {
        self.prefix = prefix;
        self.last_key = Some(now);
    }

    /// Any other key or cursor movement ends the search.
    pub fn reset(&mut self) {
        self.prefix.clear();
        self.last_key = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_letters_and_digits_only() {
        for ch in ['a', 'Z', '7', 'æ', 'ø'] {
            assert!(QuickSearch::accepts(ch), "{ch}");
        }
        for ch in [' ', '.', '-', '_', '/'] {
            assert!(!QuickSearch::accepts(ch), "{ch}");
        }
    }

    #[test]
    fn keys_in_quick_succession_extend_the_prefix() {
        let t0 = Instant::now();
        let mut search = QuickSearch::default();
        assert_eq!(search.candidate('r', t0), "r");
        search.accept("r".into(), t0);
        let t1 = t0 + Duration::from_millis(500);
        assert_eq!(search.candidate('e', t1), "re");
        search.accept("re".into(), t1);
        assert_eq!(search.candidate('a', t1 + RESET_AFTER), "rea");
    }

    #[test]
    fn a_pause_or_reset_starts_over() {
        let t0 = Instant::now();
        let mut search = QuickSearch::default();
        search.accept("re".into(), t0);
        let later = t0 + RESET_AFTER + Duration::from_millis(1);
        assert_eq!(search.candidate('x', later), "x");
        search.reset();
        assert_eq!(search.candidate('x', t0), "x");
    }

    #[test]
    fn rejected_candidate_leaves_prefix_unchanged() {
        let t0 = Instant::now();
        let mut search = QuickSearch::default();
        search.accept("re".into(), t0);
        let _ = search.candidate('q', t0);
        assert_eq!(search.candidate('a', t0), "rea");
    }
}
