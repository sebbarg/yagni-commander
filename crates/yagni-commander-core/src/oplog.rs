//! The operation log: one line per file the app creates, copies, moves,
//! renames, trashes or deletes, in one file per day
//! (`operations-YYYY-MM-DD.log`, local date) in the log directory. Enabled by
//! `log = true` in the config; files older than `log_keep_days` are deleted
//! at startup ([`prune`]).
//!
//! Writing is best effort: an operation never fails because its log line
//! could not be written.

use std::fmt::Display;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use jiff::Zoned;
use jiff::civil::Date;

const PREFIX: &str = "operations-";
const SUFFIX: &str = ".log";

/// Appends timestamped lines to today's log file. Shared between the UI
/// thread and background jobs.
#[derive(Debug)]
pub struct OperationLog {
    dir: PathBuf,
    /// Today's file, reopened when the date changes.
    file: Mutex<Option<(Date, File)>>,
}

impl OperationLog {
    /// Creates the log directory if needed and opens today's file, so a
    /// problem shows up at startup rather than silently later.
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let log = Self {
            dir: dir.into(),
            file: Mutex::new(None),
        };
        let today = Zoned::now().date();
        let file = log.open_day(today)?;
        *log.lock() = Some((today, file));
        Ok(log)
    }

    /// Writes `message` with the current local time.
    pub fn write(&self, message: impl Display) {
        self.write_at(&Zoned::now(), message);
    }

    fn write_at(&self, now: &Zoned, message: impl Display) {
        let line = format!("{} {message}\n", now.strftime("%Y-%m-%d %H:%M:%S"));
        let date = now.date();
        let mut current = self.lock();
        if current.as_ref().is_none_or(|(day, _)| *day != date) {
            *current = self.open_day(date).ok().map(|file| (date, file));
        }
        if let Some((_, file)) = current.as_mut() {
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<(Date, File)>> {
        self.file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Opens (appending) or creates the file for `date`: private to the user,
    /// and never through a symlink.
    fn open_day(&self, date: Date) -> io::Result<File> {
        fs::create_dir_all(&self.dir)?;
        let mut options = OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let no_follow = nix::fcntl::OFlag::O_NOFOLLOW.bits();
            options.mode(0o600).custom_flags(no_follow);
        }
        options.open(self.dir.join(file_name(date)))
    }
}

fn file_name(date: Date) -> String {
    format!("{PREFIX}{}{SUFFIX}", date.strftime("%Y-%m-%d"))
}

/// The date in a log file's name, if it is one of ours.
fn file_date(name: &str) -> Option<Date> {
    name.strip_prefix(PREFIX)?
        .strip_suffix(SUFFIX)?
        .parse()
        .ok()
}

/// At startup: deletes old log files in `dir` (always, so they go away
/// even after logging is switched off), then opens the log if `enabled`.
/// Problems come back as a message for the status line; the app runs on
/// without a log.
pub fn start(
    dir: Option<&Path>,
    enabled: bool,
    keep_days: u32,
) -> (Option<OperationLog>, Option<String>) {
    let Some(dir) = dir else {
        let problem = enabled.then(|| "Log disabled: no state directory found".to_owned());
        return (None, problem);
    };
    let pruned = prune(dir, keep_days)
        .err()
        .map(|e| format!("Old logs not deleted: {e}: {}", dir.display()));
    if !enabled {
        return (None, pruned);
    }
    match OperationLog::open(dir) {
        Ok(log) => (Some(log), pruned),
        Err(e) => (None, Some(format!("Log disabled: {e}: {}", dir.display()))),
    }
}

/// Deletes log files in `dir` dated more than `keep_days` days before today.
/// Only files named like ours are touched. Returns how many were deleted.
pub fn prune(dir: &Path, keep_days: u32) -> io::Result<usize> {
    prune_before(dir, keep_days, Zoned::now().date())
}

fn prune_before(dir: &Path, keep_days: u32, today: Date) -> io::Result<usize> {
    let oldest = today
        .checked_sub(jiff::Span::new().days(i64::from(keep_days)))
        .unwrap_or(Date::MIN);
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut deleted = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(date) = name.to_str().and_then(file_date) else {
            continue;
        };
        let is_file = entry.file_type().is_ok_and(|t| t.is_file());
        if date < oldest && is_file {
            fs::remove_file(entry.path())?;
            deleted += 1;
        }
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn at(day: Date, time: &str) -> Zoned {
        format!("{day}T{time}[UTC]").parse().unwrap()
    }

    #[test]
    fn lines_go_to_the_file_of_their_day() {
        let tmp = tempfile::tempdir().unwrap();
        let log = OperationLog::open(tmp.path().join("logs")).unwrap();
        log.write_at(&at(date(2026, 9, 29), "23:59:58"), "copy start");
        log.write_at(&at(date(2026, 9, 29), "23:59:59"), "copy copied a -> b");
        log.write_at(&at(date(2026, 9, 30), "00:00:01"), "copy finished");
        let read = |d| fs::read_to_string(tmp.path().join("logs").join(file_name(d))).unwrap();
        assert_eq!(
            read(date(2026, 9, 29)),
            "2026-09-29 23:59:58 copy start\n2026-09-29 23:59:59 copy copied a -> b\n"
        );
        assert_eq!(
            read(date(2026, 9, 30)),
            "2026-09-30 00:00:01 copy finished\n"
        );
    }

    #[test]
    fn write_uses_the_current_time_and_appends() {
        let tmp = tempfile::tempdir().unwrap();
        let log = OperationLog::open(tmp.path()).unwrap();
        log.write("one");
        let again = OperationLog::open(tmp.path()).unwrap();
        again.write("two");
        let today = Zoned::now().date();
        let text = fs::read_to_string(tmp.path().join(file_name(today))).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" one") && lines[1].ends_with(" two"));
        assert_eq!(&lines[0][..10], today.strftime("%Y-%m-%d").to_string());
    }

    #[cfg(unix)]
    #[test]
    fn log_files_are_private_and_never_opened_through_a_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let tmp = tempfile::tempdir().unwrap();
        let log = OperationLog::open(tmp.path()).unwrap();
        let today = Zoned::now().date();
        let path = tmp.path().join(file_name(today));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let tomorrow = today.tomorrow().unwrap();
        let victim = tmp.path().join("victim");
        fs::write(&victim, b"keep").unwrap();
        symlink(&victim, tmp.path().join(file_name(tomorrow))).unwrap();
        log.write_at(&at(tomorrow, "10:00:00"), "not written");
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    }

    #[test]
    fn opening_fails_where_no_directory_can_be_made() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("file"), b"").unwrap();
        assert!(OperationLog::open(tmp.path().join("file/logs")).is_err());
    }

    #[test]
    fn prune_deletes_only_our_old_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let today = date(2026, 9, 30);
        for d in [date(2026, 9, 22), date(2026, 9, 23), date(2026, 9, 30)] {
            fs::write(dir.join(file_name(d)), b"").unwrap();
        }
        fs::write(dir.join("operations-old.log"), b"").unwrap();
        fs::write(dir.join("notes-2020-01-01.log"), b"").unwrap();
        fs::create_dir(dir.join(file_name(date(2020, 1, 1)))).unwrap();

        assert_eq!(prune_before(dir, 7, today).unwrap(), 1);
        let mut left: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "notes-2020-01-01.log",
                "operations-2020-01-01.log",
                "operations-2026-09-23.log",
                "operations-2026-09-30.log",
                "operations-old.log",
            ]
        );
        assert_eq!(
            prune_before(dir, 0, today).unwrap(),
            1,
            "0 keeps only today"
        );
        assert_eq!(prune(&dir.join("missing"), 7).unwrap(), 0);
        assert_eq!(prune(dir, 7).unwrap(), 0, "today's date: nothing that old");
    }

    #[test]
    fn start_prunes_always_and_opens_only_when_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        fs::create_dir(&dir).unwrap();
        let old = dir.join(file_name(date(2020, 1, 1)));
        fs::write(&old, b"").unwrap();

        let (log, problem) = start(Some(&dir), false, 7);
        assert!(log.is_none() && problem.is_none());
        assert!(!old.exists(), "pruned even with logging off");

        let (log, problem) = start(Some(&dir), true, 7);
        assert!(log.is_some() && problem.is_none());

        let (log, problem) = start(None, true, 7);
        assert!(log.is_none() && problem.unwrap().contains("no state directory"));
        assert_eq!(start(None, false, 7).1, None);

        let file = tmp.path().join("file");
        fs::write(&file, b"").unwrap();
        let (_, problem) = start(Some(&file), false, 7);
        assert!(problem.unwrap().starts_with("Old logs not deleted"));
        // Both fail: the log itself is the more important message.
        let (log, problem) = start(Some(&file), true, 7);
        assert!(log.is_none());
        assert!(problem.unwrap().starts_with("Log disabled"));
    }

    #[test]
    fn file_dates_are_parsed_strictly() {
        assert_eq!(
            file_date("operations-2026-09-30.log"),
            Some(date(2026, 9, 30))
        );
        for name in [
            "operations-2026-13-01.log",
            "operations-.log",
            "ops-2026-09-30.log",
        ] {
            assert_eq!(file_date(name), None, "{name}");
        }
    }
}
