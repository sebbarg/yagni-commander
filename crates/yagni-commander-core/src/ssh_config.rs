//! The hosts in `~/.ssh/config`, for the Connect dialog (Ctrl-F).
//!
//! Only what the dialog shows is read: each `Host` alias that names one
//! host (no `*`, `?` or `!`), with its `HostName`, `User` and `Port`.
//! `Include` is followed like ssh does (relative to `~/.ssh`, `~` expanded,
//! `*` and `?` in any component, at most [`MAX_DEPTH`] deep). Lines under a
//! `Match` belong to no alias. ssh itself decides how to connect: this list
//! only offers the names.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// How deep `Include`s nest before the rest are ignored (ssh's own limit).
const MAX_DEPTH: usize = 16;

/// One host from the config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshHost {
    /// The name after `Host`: what is passed to ssh.
    pub alias: String,
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<String>,
}

impl SshHost {
    /// `user@hostname:port`, the parts the config gives, for the list's
    /// second column; empty when it gives none.
    pub fn detail(&self) -> String {
        let mut text = String::new();
        if let Some(user) = &self.user {
            text.push_str(user);
            text.push('@');
        }
        text.push_str(self.host_name.as_deref().unwrap_or(&self.alias));
        if let Some(port) = &self.port {
            text.push(':');
            text.push_str(port);
        }
        if text == self.alias {
            String::new()
        } else {
            text
        }
    }

    /// Whether `filter` (lowercase) is in the alias or the detail.
    pub fn matches(&self, filter: &str) -> bool {
        filter.is_empty()
            || self.alias.to_lowercase().contains(filter)
            || self.detail().to_lowercase().contains(filter)
    }
}

/// `home/.ssh/config` for the Connect dialog's Edit button: made empty if
/// missing, readable by the user only (ssh refuses a config others can
/// write), in a `.ssh` made 0700 if that is missing too.
pub fn ensure_config(home: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let dir = home.join(".ssh");
    match fs::DirBuilder::new().mode(0o700).create(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    let path = dir.join("config");
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(path),
    }
}

/// The hosts of `home/.ssh/config`, in the order they first appear. A
/// missing or unreadable file gives none.
pub fn hosts(home: &Path) -> Vec<SshHost> {
    let ssh_dir = home.join(".ssh");
    let mut reader = Reader {
        home,
        ssh_dir: &ssh_dir,
        hosts: Vec::new(),
        current: Vec::new(),
        seen: HashSet::new(),
    };
    reader.read_file(&ssh_dir.join("config"), 0);
    reader.hosts
}

struct Reader<'a> {
    home: &'a Path,
    ssh_dir: &'a Path,
    hosts: Vec<SshHost>,
    /// Indexes into `hosts` of the aliases the lines being read apply to.
    current: Vec<usize>,
    /// Files being read, so an `Include` loop ends.
    seen: HashSet<PathBuf>,
}

impl Reader<'_> {
    fn read_file(&mut self, path: &Path, depth: usize) {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if depth > MAX_DEPTH || !self.seen.insert(key.clone()) {
            return;
        }
        if let Ok(text) = fs::read_to_string(path) {
            for line in text.lines() {
                self.read_line(line, depth);
            }
        }
        self.seen.remove(&key);
    }

    fn read_line(&mut self, line: &str, depth: usize) {
        let Some((keyword, args)) = split_line(line) else {
            return;
        };
        match keyword.to_ascii_lowercase().as_str() {
            "host" => {
                self.current = args
                    .iter()
                    .filter(|a| !a.contains(['*', '?', '!']))
                    .map(|alias| self.host_index(alias))
                    .collect();
            }
            "match" => self.current.clear(),
            "include" => {
                for pattern in &args {
                    for file in self.expand(pattern) {
                        self.read_file(&file, depth + 1);
                    }
                }
            }
            "hostname" => self.set(&args, |h| &mut h.host_name),
            "user" => self.set(&args, |h| &mut h.user),
            "port" => self.set(&args, |h| &mut h.port),
            _ => {}
        }
    }

    fn host_index(&mut self, alias: &str) -> usize {
        match self.hosts.iter().position(|h| h.alias == alias) {
            Some(ix) => ix,
            None => {
                self.hosts.push(SshHost {
                    alias: alias.to_owned(),
                    ..SshHost::default()
                });
                self.hosts.len() - 1
            }
        }
    }

    /// Sets a field on every current alias; the first value wins, as in ssh.
    fn set(&mut self, args: &[String], field: fn(&mut SshHost) -> &mut Option<String>) {
        let Some(value) = args.first() else { return };
        for &ix in &self.current {
            field(&mut self.hosts[ix]).get_or_insert_with(|| value.clone());
        }
    }

    /// The files an `Include` pattern names, sorted.
    fn expand(&self, pattern: &str) -> Vec<PathBuf> {
        let path = match pattern.strip_prefix("~/") {
            Some(rest) => self.home.join(rest),
            None if Path::new(pattern).is_absolute() => PathBuf::from(pattern),
            None => self.ssh_dir.join(pattern),
        };
        let mut found = vec![PathBuf::from("/")];
        for part in path.components().skip(1) {
            let part = part.as_os_str().to_string_lossy();
            let mut next = Vec::new();
            for dir in &found {
                if !part.contains(['*', '?']) {
                    next.push(dir.join(&*part));
                    continue;
                }
                let Ok(entries) = fs::read_dir(dir) else {
                    continue;
                };
                let mut names: Vec<_> = entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|name| wildcard(&part, name))
                    .collect();
                names.sort();
                next.extend(names.into_iter().map(|name| dir.join(name)));
            }
            found = next;
        }
        found.retain(|p| p.is_file());
        found
    }
}

/// A config line's keyword and arguments (`Key value`, `Key=value`,
/// quoted arguments); `None` for blank lines and comments.
fn split_line(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let keyword = line[..end].to_owned();
    let rest = line[end..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest);
    let mut args = Vec::new();
    let mut chars = rest.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = chars.peek() else { break };
        if first == '#' {
            break;
        }
        let mut arg = String::new();
        if first == '"' {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                arg.push(c);
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                arg.push(c);
            }
        }
        args.push(arg);
    }
    Some((keyword, args))
}

/// `*` (any run) and `?` (one character) against a whole name.
fn wildcard(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    // The usual backtracking match over the last `*`.
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < n.len() {
        if i < p.len() && (p[i] == '?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    p[i..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(config: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(home.path().join(".ssh")).unwrap();
        fs::write(home.path().join(".ssh/config"), config).unwrap();
        home
    }

    fn aliases(hosts: &[SshHost]) -> Vec<&str> {
        hosts.iter().map(|h| h.alias.as_str()).collect()
    }

    #[test]
    fn reads_hosts_with_their_details() {
        let home = home_with(
            "# servers\n\
             Host web db\n\
             \x20 HostName 10.0.0.5\n\
             \x20 User=deploy\n\
             Host *.example.com !bad\n\
             \x20 User nobody\n\
             host Box\n\
             \x20 hostname \"box.lan\"   # comment\n\
             \x20 Port 2222\n\
             Host web\n\
             \x20 User other\n",
        );
        let hosts = hosts(home.path());
        assert_eq!(aliases(&hosts), ["web", "db", "Box"]);
        assert_eq!(hosts[0].detail(), "deploy@10.0.0.5", "the first value wins");
        assert_eq!(hosts[1].user.as_deref(), Some("deploy"));
        assert_eq!(hosts[2].detail(), "box.lan:2222");
    }

    #[test]
    fn match_blocks_belong_to_no_host() {
        let home = home_with("Host a\nMatch exec true\n  HostName x\nHost b\n  HostName y\n");
        let hosts = hosts(home.path());
        assert_eq!(hosts[0].host_name, None);
        assert_eq!(hosts[1].host_name.as_deref(), Some("y"));
    }

    #[test]
    fn includes_are_followed_relative_to_ssh_with_globs() {
        let home = home_with("Include config.d/*.conf ~/other\nHost last\n");
        let ssh = home.path().join(".ssh");
        fs::create_dir(ssh.join("config.d")).unwrap();
        fs::write(ssh.join("config.d/b.conf"), "Host b\n").unwrap();
        fs::write(ssh.join("config.d/a.conf"), "Host a\n").unwrap();
        fs::write(ssh.join("config.d/skip.txt"), "Host skip\n").unwrap();
        fs::write(home.path().join("other"), "Host other\n").unwrap();
        assert_eq!(aliases(&hosts(home.path())), ["a", "b", "other", "last"]);
    }

    #[test]
    fn an_include_loop_ends() {
        let home = home_with("Host a\nInclude config\n");
        assert_eq!(aliases(&hosts(home.path())), ["a"]);
    }

    #[test]
    fn ensure_config_makes_a_private_file_and_keeps_an_existing_one() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let path = ensure_config(home.path()).unwrap();
        assert_eq!(path, home.path().join(".ssh/config"));
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&home.path().join(".ssh")), 0o700);
        fs::write(&path, "Host a\n").unwrap();
        ensure_config(home.path()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "Host a\n");
    }

    #[test]
    fn no_config_no_hosts() {
        let home = tempfile::tempdir().unwrap();
        assert!(hosts(home.path()).is_empty());
    }

    #[test]
    fn the_detail_leaves_out_what_the_alias_says() {
        let host = SshHost {
            alias: "box".into(),
            ..SshHost::default()
        };
        assert_eq!(host.detail(), "");
        assert!(host.matches("bo") && host.matches(""));
        let full = SshHost {
            alias: "w".into(),
            host_name: Some("web.example.com".into()),
            user: Some("me".into()),
            port: None,
        };
        assert!(full.matches("example"));
        assert!(!full.matches("db"));
    }

    #[test]
    fn wildcards_match_whole_names() {
        assert!(wildcard("*.conf", "a.conf"));
        assert!(wildcard("a?c", "abc"));
        assert!(wildcard("*", ""));
        assert!(!wildcard("*.conf", "a.conf.bak"));
        assert!(wildcard("*a*b", "xxaxxb"));
        assert!(!wildcard("a", "ab"));
    }
}
