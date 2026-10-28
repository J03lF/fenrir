//! Log rotation and retention for the host's own logs and the module logs
//! (W9A-13, C18).
//!
//! - A log is rotated when it reaches `max_file_mb` or when the UTC day
//!   changes; the old content goes to `<dir>/archive/<stem>-<UTC
//!   timestamp>.log`.
//! - Archives older than `retention_days` (by modification time) are
//!   deleted, and at most `max_archives` are kept per log. A line therefore
//!   lives at most `retention_days` plus one day.
//! - Every log and archive file is created `0600`; existing files are
//!   narrowed to `0600` when the host opens or rotates them.
//!
//! Module processes write through an inherited `O_APPEND` descriptor that
//! the host cannot reopen, so their logs are rotated by copy and truncate:
//! the next write of the child lands at the start of the emptied file.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use time::{
    format_description::BorrowedFormatItem, macros::format_description, Date, OffsetDateTime,
};

use crate::config::TelemetryLogsSection;

pub(crate) const ARCHIVE_DIR: &str = "archive";
const ARCHIVE_TIMESTAMP: &[BorrowedFormatItem<'static>] =
    format_description!("[year][month][day]_[hour][minute][second]");
const SECONDS_PER_DAY: u64 = 86_400;
const BYTES_PER_MB: u64 = 1024 * 1024;

/// When logs rotate and how long archives stay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogRetention {
    pub max_bytes: u64,
    pub retention: Duration,
    pub max_archives: usize,
}

impl Default for LogRetention {
    fn default() -> Self {
        Self::from(&TelemetryLogsSection::default())
    }
}

impl From<&TelemetryLogsSection> for LogRetention {
    fn from(cfg: &TelemetryLogsSection) -> Self {
        Self {
            max_bytes: cfg.max_file_mb.saturating_mul(BYTES_PER_MB),
            retention: Duration::from_secs(
                u64::from(cfg.retention_days).saturating_mul(SECONDS_PER_DAY),
            ),
            max_archives: cfg.max_archives,
        }
    }
}

/// Opens `path` for appending, creating it `0600` (and narrowing an
/// existing file to `0600`).
pub(crate) fn open_private_append(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    restrict_mode(&mut options);
    let file = options.open(path)?;
    set_private(path);
    Ok(file)
}

/// Creates (or empties) `path` as a `0600` file.
pub(crate) fn create_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    restrict_mode(&mut options);
    let file = options.open(path)?;
    set_private(path);
    Ok(file)
}

#[cfg(unix)]
fn restrict_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn restrict_mode(_options: &mut OpenOptions) {}

/// Best effort: the file may belong to someone else or be gone already.
fn set_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

pub(crate) fn archive_dir_for(current: &Path) -> PathBuf {
    current
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(ARCHIVE_DIR)
}

/// `app` for `logs/app.log`.
pub(crate) fn log_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("log")
        .to_string()
}

/// True when `name` is an archive of the log called `stem`:
/// `<stem>-YYYYMMDD_HHMMSS.log` or `<stem>-YYYYMMDD_HHMMSS_<n>.log`.
/// (`auth-…` archives never count for `auth-service`, and vice versa.)
pub(crate) fn is_archive_of(name: &str, stem: &str) -> bool {
    let Some(rest) = name
        .strip_prefix(stem)
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.strip_suffix(".log"))
    else {
        return false;
    };
    let Some((stamp, tail)) = rest.split_at_checked(15) else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let stamp_ok =
        stamp.as_bytes().get(8) == Some(&b'_') && digits(&stamp[..8]) && digits(&stamp[9..]);
    let counter_ok = tail.is_empty() || tail.strip_prefix('_').is_some_and(digits);
    stamp_ok && counter_ok
}

/// A free archive path for `stem` at `now`.
fn archive_path(dir: &Path, stem: &str, now: OffsetDateTime) -> PathBuf {
    let stamp = now
        .format(ARCHIVE_TIMESTAMP)
        .unwrap_or_else(|_| "00000000_000000".to_string());
    let first = dir.join(format!("{stem}-{stamp}.log"));
    if !first.exists() {
        return first;
    }
    (1..)
        .map(|n| dir.join(format!("{stem}-{stamp}_{n}.log")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

/// Moves `current` into the archive (rename) and prunes. For files nobody
/// else holds open: the host's own logs at startup, a module log before
/// the module starts again. Empty or missing files are left alone.
pub(crate) fn archive_by_rename(
    current: &Path,
    policy: &LogRetention,
    now: OffsetDateTime,
) -> io::Result<Option<PathBuf>> {
    let Ok(meta) = fs::metadata(current) else {
        return Ok(None);
    };
    if meta.len() == 0 {
        return Ok(None);
    }
    let dir = archive_dir_for(current);
    fs::create_dir_all(&dir)?;
    let target = archive_path(&dir, &log_stem(current), now);
    fs::rename(current, &target)?;
    set_private(&target);
    prune_archives(&dir, &log_stem(current), policy, now.into())?;
    Ok(Some(target))
}

/// Copies `current` into the archive, empties it and prunes. For files a
/// running process keeps writing through an `O_APPEND` descriptor.
pub(crate) fn archive_by_copy_truncate(
    current: &Path,
    policy: &LogRetention,
    now: OffsetDateTime,
) -> io::Result<Option<PathBuf>> {
    let Ok(meta) = fs::metadata(current) else {
        return Ok(None);
    };
    if meta.len() == 0 {
        return Ok(None);
    }
    let dir = archive_dir_for(current);
    fs::create_dir_all(&dir)?;
    let target = archive_path(&dir, &log_stem(current), now);
    {
        let mut source = File::open(current)?;
        let mut sink = create_private(&target)?;
        io::copy(&mut source, &mut sink)?;
        sink.flush()?;
    }
    OpenOptions::new().write(true).open(current)?.set_len(0)?;
    set_private(current);
    prune_archives(&dir, &log_stem(current), policy, now.into())?;
    Ok(Some(target))
}

/// Deletes the archives of `stem` that are past `retention` and the oldest
/// ones beyond `max_archives`.
pub(crate) fn prune_archives(
    dir: &Path,
    stem: &str,
    policy: &LogRetention,
    now: SystemTime,
) -> io::Result<usize> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(0);
    };
    let mut archives: Vec<(PathBuf, SystemTime)> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| is_archive_of(name, stem))
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((entry.path(), modified))
        })
        .collect();
    // Newest first; the names sort like their timestamps.
    archives.sort_by(|left, right| right.0.cmp(&left.0));
    let mut removed = 0;
    for (index, (path, modified)) in archives.iter().enumerate() {
        let expired = now
            .duration_since(*modified)
            .is_ok_and(|age| age > policy.retention);
        if expired || index >= policy.max_archives {
            fs::remove_file(path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// `auth` for `auth-20261028_101010.log` (and `_<n>` variants).
pub(crate) fn archive_stem(name: &str) -> Option<&str> {
    let (stem, _) = name.rsplit_once('-')?;
    is_archive_of(name, stem).then_some(stem)
}

/// Prunes the archives of every log in `dir` (a log's `archive/` folder).
pub(crate) fn prune_all_archives(
    dir: &Path,
    policy: &LogRetention,
    now: SystemTime,
) -> io::Result<usize> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(0);
    };
    let mut stems: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(archive_stem)
                .map(str::to_string)
        })
        .collect();
    stems.sort();
    stems.dedup();
    let mut removed = 0;
    for stem in stems {
        removed += prune_archives(dir, &stem, policy, now)?;
    }
    Ok(removed)
}

/// One pass over a directory of logs written by other processes (the
/// module logs): rotate by copy and truncate what is due, then prune.
/// `days` remembers the UTC day each file was started; it lives across
/// passes.
pub(crate) fn maintain_log_dir(
    dir: &Path,
    policy: &LogRetention,
    days: &mut std::collections::HashMap<PathBuf, Date>,
    now: OffsetDateTime,
) -> io::Result<Vec<PathBuf>> {
    let today = now.date();
    let mut rotated = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(rotated);
    };
    let mut seen = std::collections::HashSet::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("log") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        seen.insert(path.clone());
        if meta.len() == 0 {
            days.insert(path, today);
            continue;
        }
        let started = *days
            .entry(path.clone())
            .or_insert_with(|| file_day(&meta, today));
        if rotation_due(meta.len(), started, today, policy).is_some() {
            if let Some(target) = archive_by_copy_truncate(&path, policy, now)? {
                rotated.push(target);
            }
            days.insert(path, today);
        }
    }
    days.retain(|path, _| seen.contains(path));
    prune_all_archives(&dir.join(ARCHIVE_DIR), policy, now.into())?;
    Ok(rotated)
}

/// Why a log is due for rotation, if it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RotationDue {
    Size,
    Day,
}

/// `len` bytes written since `started` (UTC day of the current file).
pub(crate) fn rotation_due(
    len: u64,
    started: Date,
    today: Date,
    policy: &LogRetention,
) -> Option<RotationDue> {
    if len == 0 {
        return None;
    }
    if len >= policy.max_bytes {
        return Some(RotationDue::Size);
    }
    (today != started).then_some(RotationDue::Day)
}

/// The UTC day a file was started: its creation time where the platform
/// has one, otherwise `fallback`.
pub(crate) fn file_day(meta: &fs::Metadata, fallback: Date) -> Date {
    meta.created()
        .ok()
        .map_or(fallback, |created| OffsetDateTime::from(created).date())
}

/// A `Write` target for the host's own log: appends to `path` (0600) and
/// rotates it by rename when it reaches the size cap or the day changes.
pub struct RotatingFile {
    path: PathBuf,
    policy: LogRetention,
    file: File,
    written: u64,
    day: Date,
}

impl RotatingFile {
    pub fn open(path: PathBuf, policy: LogRetention) -> io::Result<Self> {
        let file = open_private_append(&path)?;
        let meta = file.metadata()?;
        let today = OffsetDateTime::now_utc().date();
        Ok(Self {
            day: file_day(&meta, today),
            written: meta.len(),
            path,
            policy,
            file,
        })
    }

    fn rotate_if_due(&mut self, incoming: usize) -> io::Result<()> {
        let now = OffsetDateTime::now_utc();
        let projected = self.written.saturating_add(incoming as u64);
        let due = match rotation_due(self.written, self.day, now.date(), &self.policy) {
            Some(due) => Some(due),
            // Rotate before a line would cross the cap.
            None if self.written > 0 && projected > self.policy.max_bytes => {
                Some(RotationDue::Size)
            }
            None => None,
        };
        if due.is_none() {
            return Ok(());
        }
        self.file.flush()?;
        archive_by_rename(&self.path, &self.policy, now)?;
        self.file = open_private_append(&self.path)?;
        self.written = 0;
        self.day = now.date();
        Ok(())
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // A failed rotation must not lose the line: keep the current file.
        let _ = self.rotate_if_due(buf.len());
        let written = self.file.write(buf)?;
        self.written = self.written.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use time::macros::datetime;

    fn tmp(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("w9-host-logs-{label}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn policy(max_bytes: u64, days: u64, max_archives: usize) -> LogRetention {
        LogRetention {
            max_bytes,
            retention: Duration::from_secs(days * SECONDS_PER_DAY),
            max_archives,
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn archives(dir: &Path, stem: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir.join(ARCHIVE_DIR))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|e| e.file_name().to_str().map(str::to_string))
                    .filter(|name| is_archive_of(name, stem))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn defaults_are_seven_days_and_fifty_megabytes() {
        let policy = LogRetention::default();
        assert_eq!(policy.max_bytes, 50 * 1024 * 1024);
        assert_eq!(policy.retention, Duration::from_secs(7 * 86_400));
        assert!(policy.max_archives >= 7);
    }

    #[test]
    fn archive_names_belong_to_exactly_one_log() {
        assert!(is_archive_of("auth-20261028_101010.log", "auth"));
        assert!(is_archive_of("auth-20261028_101010_3.log", "auth"));
        assert!(!is_archive_of("auth-service-20261028_101010.log", "auth"));
        assert!(is_archive_of(
            "auth-service-20261028_101010.log",
            "auth-service"
        ));
        assert!(!is_archive_of("auth-20261028_1010.log", "auth"));
        assert!(!is_archive_of("auth-20261028_101010_.log", "auth"));
        assert!(!is_archive_of("auth-20261028x101010.log", "auth"));
        assert!(!is_archive_of("auth.log", "auth"));
        assert!(!is_archive_of("app-20261028_101010.txt", "app"));
    }

    #[test]
    fn rotation_is_due_on_size_or_day_change() {
        let p = policy(100, 7, 5);
        let day = datetime!(2026-10-28 10:00 UTC).date();
        let next = datetime!(2026-10-29 00:00 UTC).date();
        assert_eq!(rotation_due(0, day, next, &p), None);
        assert_eq!(rotation_due(99, day, day, &p), None);
        assert_eq!(rotation_due(100, day, day, &p), Some(RotationDue::Size));
        assert_eq!(rotation_due(1, day, next, &p), Some(RotationDue::Day));
    }

    #[cfg(unix)]
    #[test]
    fn copy_truncate_keeps_the_writer_appending_and_files_private() {
        let dir = tmp("copytrunc");
        let log = dir.join("athenos.log");
        // The child's descriptor: O_APPEND, opened before the rotation.
        let mut child = open_private_append(&log).unwrap();
        child.write_all(b"before rotation\n").unwrap();
        assert_eq!(mode(&log), 0o600);

        let now = datetime!(2026-10-28 12:00:00 UTC);
        let archived = archive_by_copy_truncate(&log, &policy(1024, 7, 5), now)
            .unwrap()
            .unwrap();
        assert_eq!(fs::read_to_string(&archived).unwrap(), "before rotation\n");
        assert_eq!(mode(&archived), 0o600);
        assert_eq!(fs::metadata(&log).unwrap().len(), 0);

        child.write_all(b"after\n").unwrap();
        let mut text = String::new();
        File::open(&log).unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "after\n", "no hole, no old content");
        // A second rotation in the same second gets its own name.
        let again = archive_by_copy_truncate(&log, &policy(1024, 7, 5), now)
            .unwrap()
            .unwrap();
        assert_ne!(again, archived);
        assert_eq!(archives(&dir, "athenos").len(), 2);
        // Empty logs are not archived.
        assert!(archive_by_copy_truncate(&log, &policy(1024, 7, 5), now)
            .unwrap()
            .is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn existing_files_are_narrowed_to_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("perm");
        let log = dir.join("app.log");
        fs::write(&log, "x").unwrap();
        fs::set_permissions(&log, fs::Permissions::from_mode(0o644)).unwrap();
        drop(open_private_append(&log).unwrap());
        assert_eq!(mode(&log), 0o600);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_drops_expired_and_surplus_archives_of_that_log_only() {
        let dir = tmp("prune");
        let archive = dir.join(ARCHIVE_DIR);
        fs::create_dir_all(&archive).unwrap();
        for name in [
            "auth-20261020_000000.log",
            "auth-20261026_000000.log",
            "auth-20261027_000000.log",
            "auth-20261028_000000.log",
            "auth-service-20261001_000000.log",
            "notes.txt",
        ] {
            fs::write(archive.join(name), "x").unwrap();
        }
        let now = SystemTime::now();
        // Everything was just written: only the count limit applies.
        let removed = prune_archives(&archive, "auth", &policy(1, 7, 2), now).unwrap();
        assert_eq!(removed, 2);
        assert_eq!(
            archives(&dir, "auth"),
            vec!["auth-20261027_000000.log", "auth-20261028_000000.log"]
        );
        // Eight days later both are past a 7-day retention.
        let later = now + Duration::from_secs(8 * SECONDS_PER_DAY);
        prune_archives(&archive, "auth", &policy(1, 7, 10), later).unwrap();
        assert!(archives(&dir, "auth").is_empty());
        // Other logs and foreign files are untouched.
        assert!(archive.join("auth-service-20261001_000000.log").exists());
        assert!(archive.join("notes.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn archive_stems_are_recovered_from_names() {
        assert_eq!(archive_stem("auth-20261028_101010.log"), Some("auth"));
        assert_eq!(
            archive_stem("auth-service-20261028_101010_2.log"),
            Some("auth-service")
        );
        assert_eq!(archive_stem("auth-service.log"), None);
        assert_eq!(archive_stem("notes.txt"), None);
    }

    #[test]
    fn module_log_maintenance_rotates_on_size_and_day_and_prunes() {
        let dir = tmp("modules");
        let big = dir.join("athenos.log");
        let small = dir.join("auth-service.log");
        let empty = dir.join("athenos-web.log");
        fs::write(&big, vec![b'x'; 200]).unwrap();
        fs::write(&small, b"short\n").unwrap();
        fs::write(&empty, b"").unwrap();
        fs::write(dir.join("notes.txt"), vec![b'y'; 500]).unwrap();
        let p = policy(100, 7, 5);
        let mut days = std::collections::HashMap::new();
        let today = OffsetDateTime::now_utc();

        let rotated = maintain_log_dir(&dir, &p, &mut days, today).unwrap();
        assert_eq!(rotated.len(), 1, "only the file over the cap: {rotated:?}");
        assert_eq!(fs::metadata(&big).unwrap().len(), 0);
        assert_eq!(fs::metadata(&small).unwrap().len(), 6);
        assert_eq!(archives(&dir, "athenos").len(), 1);
        assert!(archives(&dir, "athenos-web").is_empty());

        // Next day: the small file rotates; the emptied one stays put.
        let tomorrow = today + time::Duration::days(1);
        let rotated = maintain_log_dir(&dir, &p, &mut days, tomorrow).unwrap();
        assert_eq!(rotated.len(), 1, "{rotated:?}");
        assert_eq!(fs::metadata(&small).unwrap().len(), 0);
        assert_eq!(archives(&dir, "auth-service").len(), 1);
        assert_eq!(archives(&dir, "athenos").len(), 1);

        // Nine days later everything archived is past retention.
        let later = today + time::Duration::days(9);
        maintain_log_dir(&dir, &p, &mut days, later).unwrap();
        assert!(archives(&dir, "athenos").is_empty());
        assert!(archives(&dir, "auth-service").is_empty());
        assert!(dir.join("notes.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_host_writer_rotates_at_the_size_cap() {
        let dir = tmp("writer");
        let log = dir.join("app.log");
        let mut writer = RotatingFile::open(log.clone(), policy(64, 7, 3)).unwrap();
        for n in 0..20 {
            writeln!(writer, "line {n:02} ........................").unwrap();
        }
        writer.flush().unwrap();
        let current = fs::metadata(&log).unwrap().len();
        assert!(current <= 64, "current file stays under the cap: {current}");
        let kept = archives(&dir, "app");
        assert!(!kept.is_empty());
        assert!(kept.len() <= 3, "{kept:?}");
        for name in &kept {
            let len = fs::metadata(dir.join(ARCHIVE_DIR).join(name))
                .unwrap()
                .len();
            assert!(len <= 64, "{name}: {len}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_host_writer_rotates_when_the_day_changes() {
        let dir = tmp("writer-day");
        let log = dir.join("app.log");
        let mut writer = RotatingFile::open(log.clone(), policy(1 << 20, 7, 3)).unwrap();
        writer.write_all(b"yesterday\n").unwrap();
        writer.day = writer.day.previous_day().unwrap();
        writer.write_all(b"today\n").unwrap();
        writer.flush().unwrap();
        assert_eq!(fs::read_to_string(&log).unwrap(), "today\n");
        let kept = archives(&dir, "app");
        assert_eq!(kept.len(), 1);
        assert_eq!(
            fs::read_to_string(dir.join(ARCHIVE_DIR).join(&kept[0])).unwrap(),
            "yesterday\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
