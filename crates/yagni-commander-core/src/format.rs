//! Display formatting for entry columns.

use std::time::SystemTime;

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::entry::{Entry, EntryKind};

/// Human-readable size with binary units, e.g. `1.5 KiB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Modification time in the system time zone, e.g. `2026-09-30 14:05`.
pub fn format_modified(time: SystemTime) -> String {
    format_modified_in(time, TimeZone::system())
}

fn format_modified_in(time: SystemTime, tz: TimeZone) -> String {
    match Timestamp::try_from(time) {
        Ok(ts) => ts.to_zoned(tz).strftime("%Y-%m-%d %H:%M").to_string(),
        Err(_) => String::new(),
    }
}

/// `ls -l` style permissions, e.g. `drwxr-xr-x`. Empty when the mode is unknown.
pub fn format_permissions(entry: &Entry) -> String {
    let Some(mode) = entry.mode else {
        return String::new();
    };
    let kind = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o010000 => 'p',
        0o140000 => 's',
        0o060000 => 'b',
        0o020000 => 'c',
        _ if entry.kind == EntryKind::Dir => 'd',
        _ => '-',
    };
    let bit = |mask: u32, c: char| if mode & mask != 0 { c } else { '-' };
    // Execute position also shows setuid/setgid/sticky: lowercase when executable too.
    let special = |exec: u32, special: u32, set: char, unset: char| match (
        mode & exec != 0,
        mode & special != 0,
    ) {
        (true, true) => set,
        (false, true) => unset,
        (true, false) => 'x',
        (false, false) => '-',
    };
    [
        kind,
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        special(0o100, 0o4000, 's', 'S'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        special(0o010, 0o2000, 's', 'S'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        special(0o001, 0o1000, 't', 'T'),
    ]
    .iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn with_mode(mode: u32) -> Entry {
        Entry {
            mode: Some(mode),
            kind: EntryKind::File,
            ..Entry::parent()
        }
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MiB");
    }

    #[test]
    fn formats_modified_in_given_zone() {
        let t = UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        assert_eq!(format_modified_in(t, TimeZone::UTC), "2026-09-21 14:13");
    }

    #[test]
    fn formats_permissions() {
        assert_eq!(format_permissions(&with_mode(0o100644)), "-rw-r--r--");
        assert_eq!(format_permissions(&with_mode(0o040755)), "drwxr-xr-x");
        assert_eq!(format_permissions(&with_mode(0o120777)), "lrwxrwxrwx");
        assert_eq!(format_permissions(&with_mode(0o104755)), "-rwsr-xr-x");
        assert_eq!(format_permissions(&with_mode(0o041777)), "drwxrwxrwt");
        assert_eq!(format_permissions(&with_mode(0o102644)), "-rw-r-Sr--");
        assert_eq!(format_permissions(&Entry::parent()), "");
    }

    #[test]
    fn formats_size_unit_boundaries() {
        assert_eq!(format_size(1024), "1.0 KiB");
        assert_eq!(format_size(1024 * 1024 - 1), "1024.0 KiB");
        assert_eq!(format_size(1 << 30), "1.0 GiB");
        // PiB is the largest unit, so larger values keep counting in it.
        assert_eq!(format_size(u64::MAX), "16384.0 PiB");
    }

    #[test]
    fn formats_modified_in_system_zone() {
        let formatted = format_modified(std::time::SystemTime::now());
        // `YYYY-MM-DD HH:MM`, whatever the local zone is.
        assert_eq!(formatted.len(), 16, "{formatted}");
        assert_eq!(&formatted[4..5], "-");
        assert_eq!(&formatted[10..11], " ");
        assert_eq!(&formatted[13..14], ":");
    }

    #[test]
    fn formats_modified_in_non_utc_zone() {
        let t = UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        let tz = TimeZone::fixed(jiff::tz::offset(2));
        assert_eq!(format_modified_in(t, tz), "2026-09-21 16:13");
    }

    #[test]
    fn unrepresentable_time_formats_as_empty() {
        // Far beyond the year 9999 that timestamps support.
        let t = UNIX_EPOCH + Duration::from_secs(1 << 40);
        assert_eq!(format_modified_in(t, TimeZone::UTC), "");
    }

    #[test]
    fn formats_special_file_types() {
        assert_eq!(format_permissions(&with_mode(0o010644)), "prw-r--r--");
        assert_eq!(format_permissions(&with_mode(0o140755)), "srwxr-xr-x");
        assert_eq!(format_permissions(&with_mode(0o060660)), "brw-rw----");
        assert_eq!(format_permissions(&with_mode(0o020620)), "crw--w----");
    }

    #[test]
    fn formats_special_bits_without_execute() {
        assert_eq!(format_permissions(&with_mode(0o104644)), "-rwSr--r--");
        assert_eq!(format_permissions(&with_mode(0o102755)), "-rwxr-sr-x");
        assert_eq!(format_permissions(&with_mode(0o041776)), "drwxrwxrwT");
    }

    #[test]
    fn mode_without_type_bits_falls_back_to_entry_kind() {
        let dir = Entry {
            kind: EntryKind::Dir,
            ..with_mode(0o755)
        };
        assert_eq!(format_permissions(&dir), "drwxr-xr-x");
        assert_eq!(format_permissions(&with_mode(0o644)), "-rw-r--r--");
    }
}
