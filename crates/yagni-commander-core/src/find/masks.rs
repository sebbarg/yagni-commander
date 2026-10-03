//! Alt-F7's name masks: `*.rs;*.toml | target/ .git/`.

/// Include and exclude masks; lowercased unless case-sensitive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Masks {
    include: Vec<Vec<char>>,
    exclude: Vec<Exclude>,
    case_sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Exclude {
    pattern: Vec<char>,
    /// Ends in `/`: folders only, and they are not entered.
    folders_only: bool,
}

impl Masks {
    /// Masks are separated by `;` or spaces; after a `|` they exclude. An
    /// include mask without `*` or `?` means `*text*`; excludes are taken
    /// as written. Case-insensitive.
    pub fn parse(text: &str) -> Self {
        Self::new(text, false)
    }

    /// Like [`Masks::parse`]; `case_sensitive` (the dialog's box) matches
    /// names in the case typed.
    pub fn new(text: &str, case_sensitive: bool) -> Self {
        let (include, exclude) = text.split_once('|').unwrap_or((text, ""));
        let tokens = |text| tokens(text, case_sensitive);
        let include = tokens(include)
            .map(|t| {
                if t.contains(['*', '?']) {
                    t
                } else {
                    format!("*{t}*")
                }
            })
            .map(|t| t.chars().collect())
            .collect();
        let exclude = tokens(exclude)
            .filter_map(|t| {
                let folders_only = t.ends_with('/');
                let t = t.trim_end_matches('/');
                (!t.is_empty()).then(|| Exclude {
                    pattern: t.chars().collect(),
                    folders_only,
                })
            })
            .collect();
        Self {
            include,
            exclude,
            case_sensitive,
        }
    }

    /// Whether an entry named `name` is a result. Folder-only excludes
    /// apply to folders.
    pub fn matches(&self, name: &str, is_dir: bool) -> bool {
        let name = self.folded(name);
        let included = self.include.is_empty() || self.include.iter().any(|m| wildcard(m, &name));
        included
            && !self
                .exclude
                .iter()
                .any(|e| (is_dir || !e.folders_only) && wildcard(&e.pattern, &name))
    }

    /// Whether the walk goes into the folder `name`.
    pub fn enters(&self, name: &str) -> bool {
        let name = self.folded(name);
        !self
            .exclude
            .iter()
            .any(|e| e.folders_only && wildcard(&e.pattern, &name))
    }

    fn folded(&self, name: &str) -> Vec<char> {
        if self.case_sensitive {
            name.chars().collect()
        } else {
            name.to_lowercase().chars().collect()
        }
    }
}

fn tokens(text: &str, case_sensitive: bool) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| c == ';' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(move |t| {
            if case_sensitive {
                t.to_owned()
            } else {
                t.to_lowercase()
            }
        })
}

/// `*` (any run) and `?` (one character), backtracking to the last `*`.
fn wildcard(pattern: &[char], name: &[char]) -> bool {
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
            }
            Some(&c) if c == '?' || c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                Some((sp, sn)) => {
                    p = sp + 1;
                    n = sn + 1;
                    star = Some((sp, sn + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_masks_match_everything() {
        let m = Masks::parse("");
        assert!(m.matches("anything", false) && m.matches("dir", true));
        assert!(m.enters("dir"));
    }

    #[test]
    fn wildcards_and_case() {
        let m = Masks::parse("*.RS");
        assert!(m.matches("main.rs", false));
        assert!(!m.matches("main.rs.bak", false));
        let m = Masks::parse("a?c");
        assert!(m.matches("abc", false) && m.matches("a c", false) && !m.matches("ac", false));
        assert!(Masks::parse("*").matches("", false));
        assert!(Masks::parse("a*b*c").matches("aXbYbc", false));
        assert!(!Masks::parse("a*b*c").matches("aXbYb", false));
    }

    #[test]
    fn a_mask_without_wildcards_means_contains() {
        let m = Masks::parse("read");
        assert!(m.matches("README.md", false) && m.matches("unread", false));
        assert!(!m.matches("red", false));
    }

    #[test]
    fn several_masks_by_semicolon_or_space() {
        let m = Masks::parse("*.rs; *.toml  *.md");
        for name in ["a.rs", "Cargo.toml", "x.md"] {
            assert!(m.matches(name, false), "{name}");
        }
        assert!(!m.matches("a.txt", false));
    }

    #[test]
    fn excludes_after_a_bar() {
        let m = Masks::parse("*.rs | *_test.rs target/");
        assert!(m.matches("main.rs", false));
        assert!(!m.matches("main_test.rs", false));
        assert!(!m.enters("target"), "a folder exclude is not entered");
        assert!(m.enters("src"));
        // A plain exclude keeps a folder out of the results but is entered.
        let m = Masks::parse("| *.bak");
        assert!(!m.matches("old.bak", true) && m.enters("old.bak"));
        assert!(m.matches("new", true));
        // A folder-only exclude doesn't touch files of that name.
        let m = Masks::parse("| build/");
        assert!(m.matches("build", false) && !m.matches("build", true));
    }

    #[test]
    fn case_sensitive_masks_match_the_case_as_typed() {
        let m = Masks::new("README", true);
        assert!(m.matches("README", false));
        assert!(!m.matches("readme.md", false));
        let m = Masks::new("*.RS | Target/", true);
        assert!(m.matches("a.RS", false) && !m.matches("a.rs", false));
        assert!(!m.enters("Target") && m.enters("target"));
        assert_eq!(Masks::new("x", false), Masks::parse("x"));
    }

    #[test]
    fn exclude_tokens_are_exact() {
        let m = Masks::parse("| git/");
        assert!(m.enters(".github"), "no implied *git*");
        assert!(!m.enters("GIT"), "case-insensitive");
        assert!(
            Masks::parse("| /").enters("x"),
            "an empty exclude is dropped"
        );
    }
}
