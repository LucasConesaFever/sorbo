//! The log file: readable timestamps and size-based rotation (RUN-5, spec section 7).
//!
//! Pure `std` code, so it compiles and tests on any OS. The macOS side (local time, pointing
//! stderr at the file) lives in `macos/logfile.rs`.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// The log is rotated once it grows past this many bytes (10 MB).
pub const ROTATE_BYTES: u64 = 10 * 1024 * 1024;
/// How often, in log lines, the file size is checked.
pub const ROTATE_CHECK_LINES: u32 = 1000;

/// What to do with the log at a periodic check (SAFE-10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogAction {
    /// All is well.
    Keep,
    /// The file at the log path isn't the one being written (deleted, replaced, or a previous
    /// reopen failed): open the path again.
    Reopen,
    /// The file being written is over the limit: rotate it, then open the path again.
    Rotate,
}

/// Decides `LogAction` for the log at `path`, given the metadata of the file actually being
/// written (fd 2) and the size `limit`. "The same file" means the same device and inode.
pub fn log_action(path: &Path, writing: &fs::Metadata, limit: u64) -> LogAction {
    use std::os::unix::fs::MetadataExt;
    let same = fs::metadata(path)
        .is_ok_and(|at_path| at_path.dev() == writing.dev() && at_path.ino() == writing.ino());
    if !same {
        LogAction::Reopen
    } else if writing.len() > limit {
        LogAction::Rotate
    } else {
        LogAction::Keep
    }
}

/// A moment in local time, broken into calendar fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
}

/// `2026-11-08 14:03:27.512`.
pub fn format_timestamp(t: &LocalTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.year, t.month, t.day, t.hour, t.minute, t.second, t.millis
    )
}

/// Where the previous log goes when rotating: `wm.log` → `wm.log.1`.
pub fn backup_path(log: &Path) -> PathBuf {
    let mut name: OsString = log.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

/// If `log` is larger than `limit` bytes, moves it to `backup_path(log)` (replacing any
/// older backup) and returns `true`. A missing log is not an error.
pub fn rotate_if_needed(log: &Path, limit: u64) -> io::Result<bool> {
    let len = match fs::metadata(log) {
        Ok(meta) => meta.len(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    if len <= limit {
        return Ok(false);
    }
    fs::rename(log, backup_path(log))?;
    Ok(true)
}

/// Opens the log for appending: creates its directory if needed, and rotates it first if
/// it's already too big.
pub fn open_log(log: &Path) -> io::Result<File> {
    if let Some(dir) = log.parent() {
        fs::create_dir_all(dir)?;
    }
    rotate_if_needed(log, ROTATE_BYTES)?;
    OpenOptions::new().create(true).append(true).open(log)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A fresh, empty directory for one test, under the system temp dir.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sorbo-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn safe_10_deleted_log_is_reopened() {
        let dir = temp_dir("log-deleted");
        let path = dir.join("wm.log");
        let file = File::create(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(
            log_action(&path, &file.metadata().unwrap(), 10),
            LogAction::Reopen
        );
    }

    #[test]
    fn safe_10_replaced_log_is_reopened() {
        let dir = temp_dir("log-replaced");
        let path = dir.join("wm.log");
        let file = File::create(&path).unwrap();
        fs::rename(&path, dir.join("old.log")).unwrap();
        File::create(&path).unwrap();
        assert_eq!(
            log_action(&path, &file.metadata().unwrap(), 10),
            LogAction::Reopen
        );
    }

    #[test]
    fn safe_10_big_log_is_rotated() {
        let dir = temp_dir("log-big");
        let path = dir.join("wm.log");
        write_bytes(&path, 11);
        let file = File::open(&path).unwrap();
        assert_eq!(
            log_action(&path, &file.metadata().unwrap(), 10),
            LogAction::Rotate
        );
    }

    #[test]
    fn safe_10_small_log_is_kept() {
        let dir = temp_dir("log-small");
        let path = dir.join("wm.log");
        write_bytes(&path, 5);
        let file = File::open(&path).unwrap();
        assert_eq!(
            log_action(&path, &file.metadata().unwrap(), 10),
            LogAction::Keep
        );
    }

    fn write_bytes(path: &Path, n: usize) {
        fs::write(path, vec![b'x'; n]).unwrap();
    }

    #[test]
    fn run_5_timestamp_is_readable_local_time() {
        let t = LocalTime {
            year: 2026,
            month: 11,
            day: 8,
            hour: 14,
            minute: 3,
            second: 27,
            millis: 512,
        };
        assert_eq!(format_timestamp(&t), "2026-11-08 14:03:27.512");
        let t = LocalTime {
            year: 2026,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
            second: 5,
            millis: 6,
        };
        assert_eq!(format_timestamp(&t), "2026-01-02 03:04:05.006");
    }

    #[test]
    fn run_5_backup_path_appends_1() {
        assert_eq!(
            backup_path(Path::new("/x/wm.log")),
            PathBuf::from("/x/wm.log.1")
        );
    }

    #[test]
    fn run_5_small_log_is_not_rotated() {
        let dir = temp_dir("small");
        let log = dir.join("wm.log");
        write_bytes(&log, 10);
        assert!(!rotate_if_needed(&log, 100).unwrap());
        assert!(log.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_5_large_log_is_rotated() {
        let dir = temp_dir("large");
        let log = dir.join("wm.log");
        write_bytes(&log, 200);
        assert!(rotate_if_needed(&log, 100).unwrap());
        assert!(!log.exists());
        assert_eq!(fs::metadata(dir.join("wm.log.1")).unwrap().len(), 200);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_5_rotation_replaces_previous_backup() {
        let dir = temp_dir("replace");
        let log = dir.join("wm.log");
        fs::write(dir.join("wm.log.1"), "old").unwrap();
        write_bytes(&log, 200);
        assert!(rotate_if_needed(&log, 100).unwrap());
        assert_eq!(fs::metadata(dir.join("wm.log.1")).unwrap().len(), 200);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_5_missing_log_is_not_rotated() {
        let dir = temp_dir("missing");
        assert!(!rotate_if_needed(&dir.join("wm.log"), 100).unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_5_open_creates_missing_directory() {
        let dir = temp_dir("open");
        let log = dir.join("a").join("b").join("wm.log");
        open_log(&log).unwrap();
        assert!(log.exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
