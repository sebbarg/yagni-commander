//! The shared clipboard (Ctrl-K Ctrl-C, Ctrl-K Ctrl-V): text items kept in
//! a folder on an ssh host, so every machine logging in there as the same
//! user sees them. Each step runs on an SFTP [`Client`] given the folder;
//! the app finds it with [`folder`] (in the login user's home) and runs the
//! steps on `remote::with_session`.
//!
//! One file per item, `<millis>-<machine>-<random>.txt`, so two machines
//! pushing at once never write the same file: SFTP has no locks, and one
//! shared index would need a read-modify-write. Items are ordered by the
//! host's file times (then their names), so a machine with a wrong clock
//! can't reorder them or make them expire.

use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::remote::{join, time_of};
use crate::sftp::{Client, Name, Upload, WRITE_SIZE};

/// The folder in the login user's home.
pub const FOLDER: &[u8] = b".local/share/yagni-commander/clipboard";
/// Items kept: older ones are deleted by the next push or list.
pub const KEEP: usize = 50;
/// The largest item pushed.
pub const ITEM_CAP: u64 = 1 << 20;
/// Bytes read of each item for its row in the list.
const PREVIEW_BYTES: u32 = 1024;
/// Characters of a preview.
const PREVIEW_CHARS: usize = 200;
/// A partial write older than this is a push that died: deleted.
const STALE_TEMP: Duration = Duration::from_secs(10 * 60);
const SUFFIX: &str = ".txt";
const TEMP_SUFFIX: &str = ".tmp";

/// One item on the host, newest first in [`list`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The file's name in the folder, for [`fetch`] and [`remove`].
    pub name: Vec<u8>,
    /// The machine that pushed it.
    pub machine: String,
    /// The host's time for the file.
    pub modified: SystemTime,
    /// Its first non-blank line, whitespace collapsed, cut short.
    pub preview: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pushed {
    New,
    /// The text was the newest item already: nothing written.
    Same,
}

/// The shared clipboard's folder on the host the client talks to.
pub fn folder(client: &mut Client) -> io::Result<Vec<u8>> {
    let home = client.realpath(b".")?;
    Ok(join(&home, FOLDER))
}

/// Adds `text` as the newest item from `machine`, unless it is the newest
/// already, making the folder first (0700) if needed; then deletes the
/// items past [`KEEP`].
pub fn push(client: &mut Client, dir: &[u8], text: &str, machine: &str) -> io::Result<Pushed> {
    if text.len() as u64 > ITEM_CAP {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "The text is over 1 MiB, too large for the shared clipboard.",
        ));
    }
    make_folder(client, dir)?;
    let entries = read_folder(client, dir)?;
    let kept = prune(client, dir, entries, SystemTime::now());
    if let Some(newest) = kept.first()
        && newest.attrs.size == Some(text.len() as u64)
        && fetch_bytes(client, dir, &newest.name)? == text.as_bytes()
    {
        return Ok(Pushed::Same);
    }
    let name = item_name(SystemTime::now(), machine, &random_hex()?);
    write_item(client, dir, &name, text.as_bytes())?;
    // The new item is the newest: room for KEEP - 1 others.
    for old in kept.iter().skip(KEEP - 1) {
        remove(client, dir, &old.name)?;
    }
    Ok(Pushed::New)
}

/// The items, newest first, after deleting those past [`KEEP`] and partial
/// writes of pushes that died. A folder not made yet holds none.
pub fn list(client: &mut Client, dir: &[u8]) -> io::Result<Vec<Item>> {
    let entries = read_folder(client, dir)?;
    let kept = prune(client, dir, entries, SystemTime::now());
    let paths: Vec<Vec<u8>> = kept.iter().map(|n| join(dir, &n.name)).collect();
    let heads = client.read_heads(&paths, PREVIEW_BYTES)?;
    Ok(kept
        .into_iter()
        .zip(heads)
        // One deleted meanwhile by another machine is left out.
        .filter_map(|(entry, head)| {
            let head = head.ok()?;
            Some(Item {
                machine: machine_of(&entry.name)?.to_owned(),
                modified: time_of(entry.attrs.mtime.unwrap_or(0)),
                preview: preview(&head),
                name: entry.name,
            })
        })
        .collect())
}

/// The whole text of item `name` (invalid UTF-8 replaced).
pub fn fetch(client: &mut Client, dir: &[u8], name: &[u8]) -> io::Result<String> {
    Ok(String::from_utf8_lossy(&fetch_bytes(client, dir, name)?).into_owned())
}

/// Deletes item `name`; one already gone is fine.
pub fn remove(client: &mut Client, dir: &[u8], name: &[u8]) -> io::Result<()> {
    match client.remove(&join(dir, name)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// This computer's name for the item names: [`machine_from`] the host name.
pub fn machine() -> String {
    let name = nix::unistd::gethostname().unwrap_or_default();
    machine_from(&name.to_string_lossy())
}

/// A host name as it goes into an item's name: up to its first `.`, any
/// character but ASCII letters, digits, `-` and `_` as `_`; `unknown` when
/// nothing is left.
pub fn machine_from(host: &str) -> String {
    let short = host.split('.').next().unwrap_or_default();
    let name: String = short
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() {
        "unknown".into()
    } else {
        name
    }
}

/// How long ago `then` was, for a row: `now`, `5 min`, `3 h`, `2 d`.
pub fn age(now: SystemTime, then: SystemTime) -> String {
    let minutes = now.duration_since(then).unwrap_or_default().as_secs() / 60;
    match minutes {
        0 => "now".into(),
        1..60 => format!("{minutes} min"),
        60..1440 => format!("{} h", minutes / 60),
        _ => format!("{} d", minutes / 1440),
    }
}

/// An item's file name.
fn item_name(now: SystemTime, machine: &str, random: &str) -> String {
    let millis = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{millis:013}-{machine}-{random}{SUFFIX}")
}

/// The machine in an item's file name; `None` for any other file.
fn machine_of(name: &[u8]) -> Option<&str> {
    let stem = std::str::from_utf8(name).ok()?.strip_suffix(SUFFIX)?;
    let (millis, rest) = stem.split_once('-')?;
    let (machine, random) = rest.rsplit_once('-')?;
    let digits = |s: &str, hex: bool| {
        !s.is_empty()
            && s.chars().all(|c| {
                if hex {
                    c.is_ascii_hexdigit()
                } else {
                    c.is_ascii_digit()
                }
            })
    };
    (digits(millis, false) && digits(random, true) && !machine.is_empty()).then_some(machine)
}

/// A row's text: the first non-blank line, whitespace collapsed, at most
/// [`PREVIEW_CHARS`] characters (a character cut at the end of the bytes
/// read is dropped).
fn preview(head: &[u8]) -> String {
    let text = String::from_utf8_lossy(head);
    let text = text.trim_end_matches('\u{FFFD}');
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(PREVIEW_CHARS)
        .collect()
}

/// 32 random bits as hex.
fn random_hex() -> io::Result<String> {
    Ok(crate::askpass::random_token()?[..8].to_owned())
}

/// The folder's entries; none if it doesn't exist.
fn read_folder(client: &mut Client, dir: &[u8]) -> io::Result<Vec<Name>> {
    let mut names = Vec::new();
    match client.read_dir(dir, |name| names.push(name)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
        Ok(()) => Ok(names),
    }
}

/// Makes `dir` and any missing parents; `dir` itself 0700.
fn make_folder(client: &mut Client, dir: &[u8]) -> io::Result<()> {
    if client.stat(dir).is_ok_and(|a| a.is_dir()) {
        return Ok(());
    }
    let mut prefix = Vec::new();
    for (ix, part) in dir.split(|&b| b == b'/').enumerate() {
        if ix > 0 {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(part);
        if part.is_empty() || client.stat(&prefix).is_ok_and(|a| a.is_dir()) {
            continue;
        }
        let last = prefix.len() == dir.len();
        let made = client.mkdir(&prefix, if last { 0o700 } else { 0o755 });
        // Another machine may have made it meanwhile.
        if made.is_err() && !client.stat(&prefix).is_ok_and(|a| a.is_dir()) {
            return made;
        }
    }
    Ok(())
}

/// Writes `data` as item `name`: into a temporary name beside it, 0600,
/// renamed into place once complete.
fn write_item(client: &mut Client, dir: &[u8], name: &str, data: &[u8]) -> io::Result<()> {
    let temp = join(dir, format!(".{name}{TEMP_SUFFIX}").as_bytes());
    let mut upload = Upload::new(client.open_write(&temp, 0o600, true)?);
    let written = data
        .chunks(WRITE_SIZE)
        .enumerate()
        .try_for_each(|(ix, chunk)| upload.write(client, (ix * WRITE_SIZE) as u64, chunk));
    let done = match written {
        Ok(()) => upload.finish(client, None, Some(0o600), false),
        Err(e) => {
            upload.abandon(client);
            Err(e)
        }
    };
    let placed = done.and_then(|()| client.rename(&temp, &join(dir, name.as_bytes())));
    if placed.is_err() {
        let _ = client.remove(&temp);
    }
    placed
}

fn fetch_bytes(client: &mut Client, dir: &[u8], name: &[u8]) -> io::Result<Vec<u8>> {
    let handle = client.open_read(&join(dir, name))?;
    let mut data = Vec::new();
    let read = client.download(&handle, |offset, piece| {
        let end = offset as usize + piece.len();
        if end as u64 > ITEM_CAP {
            return Ok(false);
        }
        if data.len() < end {
            data.resize(end, 0);
        }
        data[offset as usize..end].copy_from_slice(piece);
        Ok(true)
    });
    let closed = client.close(handle);
    read?;
    closed?;
    Ok(data)
}

/// Deletes stale partial writes and the items past [`KEEP`]; returns the
/// items kept, newest first. Best effort: a failed delete is left for the
/// next time.
fn prune(client: &mut Client, dir: &[u8], entries: Vec<Name>, now: SystemTime) -> Vec<Name> {
    let (mut items, others): (Vec<Name>, Vec<Name>) = entries
        .into_iter()
        .partition(|n| n.attrs.is_file() && machine_of(&n.name).is_some());
    for other in others {
        let temp = other.name.starts_with(b".") && other.name.ends_with(TEMP_SUFFIX.as_bytes());
        let stale = now
            .duration_since(time_of(other.attrs.mtime.unwrap_or(0)))
            .is_ok_and(|age| age > STALE_TEMP);
        if temp && stale && other.attrs.is_file() {
            let _ = remove(client, dir, &other.name);
        }
    }
    items.sort_by(|a, b| (b.attrs.mtime, &b.name).cmp(&(a.attrs.mtime, &a.name)));
    for old in items.drain(KEEP.min(items.len())..) {
        let _ = remove(client, dir, &old.name);
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sftp::tests::local_client;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn bytes(path: &Path) -> Vec<u8> {
        path.as_os_str().as_bytes().to_vec()
    }

    /// A folder for the items inside a temporary one, not made yet.
    fn setup() -> (tempfile::TempDir, Vec<u8>, Client) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = bytes(&tmp.path().join("a/clip"));
        (tmp, dir, local_client())
    }

    fn files(dir: &[u8]) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(std::str::from_utf8(dir).unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn set_mtime(dir: &[u8], name: &str, secs: u64) {
        let path = Path::new(std::str::from_utf8(dir).unwrap()).join(name);
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn a_push_shows_in_the_list() {
        let (_tmp, dir, mut client) = setup();
        let pushed = push(&mut client, &dir, "\n  hello   big\tworld \nmore", "laptop").unwrap();
        assert_eq!(pushed, Pushed::New);
        let items = list(&mut client, &dir).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].machine, "laptop");
        assert_eq!(items[0].preview, "hello big world");
        let text = fetch(&mut client, &dir, &items[0].name).unwrap();
        assert_eq!(text, "\n  hello   big\tworld \nmore");
    }

    #[test]
    fn the_folder_is_private_and_so_are_the_items() {
        let (_tmp, dir, mut client) = setup();
        push(&mut client, &dir, "secret", "m").unwrap();
        let path = Path::new(std::str::from_utf8(&dir).unwrap());
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(path), 0o700);
        let item = path.join(&files(&dir)[0]);
        assert_eq!(mode(&item), 0o600);
    }

    #[test]
    fn listing_a_missing_folder_is_empty() {
        let (_tmp, dir, mut client) = setup();
        assert!(list(&mut client, &dir).unwrap().is_empty());
    }

    #[test]
    fn the_same_text_is_not_pushed_twice() {
        let (_tmp, dir, mut client) = setup();
        push(&mut client, &dir, "one", "m").unwrap();
        assert_eq!(push(&mut client, &dir, "one", "m").unwrap(), Pushed::Same);
        assert_eq!(files(&dir).len(), 1);
        push(&mut client, &dir, "two", "m").unwrap();
        assert_eq!(push(&mut client, &dir, "one", "m").unwrap(), Pushed::New);
        assert_eq!(files(&dir).len(), 3);
    }

    #[test]
    fn too_large_a_text_is_refused() {
        let (_tmp, dir, mut client) = setup();
        let big = "x".repeat(ITEM_CAP as usize + 1);
        let e = push(&mut client, &dir, &big, "m").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        let fits = "x".repeat(ITEM_CAP as usize);
        push(&mut client, &dir, &fits, "m").unwrap();
        let items = list(&mut client, &dir).unwrap();
        assert_eq!(fetch(&mut client, &dir, &items[0].name).unwrap(), fits);
    }

    #[test]
    fn newest_first_by_the_hosts_time() {
        let (_tmp, dir, mut client) = setup();
        std::fs::create_dir_all(std::str::from_utf8(&dir).unwrap()).unwrap();
        let path = Path::new(std::str::from_utf8(&dir).unwrap());
        // Names say b is newest; the host's times say a.
        std::fs::write(path.join("0000000000009-m-0a.txt"), "b").unwrap();
        std::fs::write(path.join("0000000000001-m-0b.txt"), "a").unwrap();
        set_mtime(&dir, "0000000000009-m-0a.txt", 1_000);
        set_mtime(&dir, "0000000000001-m-0b.txt", 2_000);
        let previews: Vec<String> = list(&mut client, &dir)
            .unwrap()
            .into_iter()
            .map(|i| i.preview)
            .collect();
        assert_eq!(previews, ["a", "b"]);
    }

    #[test]
    fn items_in_one_second_keep_their_order() {
        let (_tmp, dir, mut client) = setup();
        for text in ["first", "second", "third"] {
            push(&mut client, &dir, text, "m").unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        for name in files(&dir) {
            set_mtime(&dir, &name, 5_000);
        }
        let previews: Vec<String> = list(&mut client, &dir)
            .unwrap()
            .into_iter()
            .map(|i| i.preview)
            .collect();
        assert_eq!(previews, ["third", "second", "first"]);
    }

    #[test]
    fn only_the_newest_are_kept() {
        let (_tmp, dir, mut client) = setup();
        std::fs::create_dir_all(std::str::from_utf8(&dir).unwrap()).unwrap();
        let path = Path::new(std::str::from_utf8(&dir).unwrap());
        for ix in 0..KEEP + 3 {
            let name = format!("{ix:013}-m-ab.txt");
            std::fs::write(path.join(&name), format!("{ix}")).unwrap();
            set_mtime(&dir, &name, 1_000 + ix as u64);
        }
        let items = list(&mut client, &dir).unwrap();
        assert_eq!(items.len(), KEEP);
        assert_eq!(items[0].preview, format!("{}", KEEP + 2));
        assert_eq!(files(&dir).len(), KEEP);
        // A push keeps KEEP including itself.
        push(&mut client, &dir, "new", "m").unwrap();
        assert_eq!(files(&dir).len(), KEEP);
        assert!(!files(&dir).contains(&format!("{:013}-m-ab.txt", 3)));
    }

    #[test]
    fn stale_partial_writes_are_removed_and_fresh_ones_kept() {
        let (_tmp, dir, mut client) = setup();
        std::fs::create_dir_all(std::str::from_utf8(&dir).unwrap()).unwrap();
        let path = Path::new(std::str::from_utf8(&dir).unwrap());
        std::fs::write(path.join(".0000000000001-m-ab.txt.tmp"), "old").unwrap();
        set_mtime(&dir, ".0000000000001-m-ab.txt.tmp", 1_000);
        std::fs::write(path.join(".0000000000002-m-ab.txt.tmp"), "now").unwrap();
        std::fs::write(path.join("notes.md"), "not ours").unwrap();
        assert!(list(&mut client, &dir).unwrap().is_empty());
        assert_eq!(files(&dir), [".0000000000002-m-ab.txt.tmp", "notes.md"]);
    }

    #[test]
    fn a_binary_item_reads_lossily() {
        let (_tmp, dir, mut client) = setup();
        std::fs::create_dir_all(std::str::from_utf8(&dir).unwrap()).unwrap();
        let path = Path::new(std::str::from_utf8(&dir).unwrap());
        std::fs::write(path.join("0000000000001-m-ab.txt"), b"a\xffb\n").unwrap();
        let items = list(&mut client, &dir).unwrap();
        assert_eq!(items[0].preview, "a\u{FFFD}b");
        assert_eq!(
            fetch(&mut client, &dir, &items[0].name).unwrap(),
            "a\u{FFFD}b\n"
        );
    }

    #[test]
    fn removing_a_gone_item_is_fine() {
        let (_tmp, dir, mut client) = setup();
        push(&mut client, &dir, "x", "m").unwrap();
        let name = list(&mut client, &dir).unwrap()[0].name.clone();
        remove(&mut client, &dir, &name).unwrap();
        remove(&mut client, &dir, &name).unwrap();
        assert!(list(&mut client, &dir).unwrap().is_empty());
    }

    #[test]
    fn the_folder_is_in_the_login_home() {
        let mut client = local_client();
        let dir = folder(&mut client).unwrap();
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert_eq!(dir, join(&bytes(&cwd), FOLDER));
    }

    #[test]
    fn previews_are_cut_and_skip_blank_lines() {
        assert_eq!(preview(b"  \n\t\nline two\n"), "line two");
        assert_eq!(preview(b""), "");
        let long = "y".repeat(300);
        assert_eq!(preview(long.as_bytes()).chars().count(), PREVIEW_CHARS);
        // A character cut by the end of the bytes read.
        assert_eq!(preview("ab\u{e9}".as_bytes()[..3].as_ref()), "ab");
    }

    #[test]
    fn names_carry_the_machine() {
        let now = UNIX_EPOCH + Duration::from_millis(1_763_738_412_345);
        let name = item_name(now, "my-box", "0a1b2c3d");
        assert_eq!(name, "1763738412345-my-box-0a1b2c3d.txt");
        assert_eq!(machine_of(name.as_bytes()), Some("my-box"));
        assert_eq!(machine_of(b"notes.txt"), None);
        assert_eq!(machine_of(b"12-m-xyz.txt"), None);
        assert_eq!(machine_of(b"12--ab.txt"), None);
        assert_eq!(machine_of(b".12-m-ab.txt.tmp"), None);
        assert_eq!(random_hex().unwrap().len(), 8);
    }

    #[test]
    fn machine_names_are_safe() {
        assert_eq!(machine_from("laptop.local"), "laptop");
        assert_eq!(machine_from("Seb's Mac"), "Seb_s_Mac");
        assert_eq!(machine_from(""), "unknown");
        assert!(!machine().is_empty());
    }

    #[test]
    fn ages_read_short() {
        let then = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let after = |secs| then + Duration::from_secs(secs);
        assert_eq!(age(after(59), then), "now");
        assert_eq!(age(after(5 * 60), then), "5 min");
        assert_eq!(age(after(3 * 3600 + 5), then), "3 h");
        assert_eq!(age(after(2 * 86400), then), "2 d");
        // A file time ahead of this clock.
        assert_eq!(age(then, after(100)), "now");
    }
}
