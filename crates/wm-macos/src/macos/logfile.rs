//! Writing log lines: local timestamps, and pointing stderr at the log file (spec section 7).

use std::ffi::{c_char, c_int, c_long};
use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::mem::ManuallyDrop;
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::logging::{self, LocalTime, LogAction, ROTATE_BYTES, ROTATE_CHECK_LINES};

/// macOS `struct tm` (`<time.h>`).
#[repr(C)]
struct Tm {
    tm_sec: c_int,
    tm_min: c_int,
    tm_hour: c_int,
    tm_mday: c_int,
    tm_mon: c_int,
    tm_year: c_int,
    tm_wday: c_int,
    tm_yday: c_int,
    tm_isdst: c_int,
    tm_gmtoff: c_long,
    tm_zone: *const c_char,
}

// libSystem is always linked, so these need no new dependency (decision D15).
unsafe extern "C" {
    fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
    fn dup2(old: c_int, new: c_int) -> c_int;
}

/// The log file stderr points at, if `init_log_file` was called.
struct LogState {
    path: PathBuf,
    /// Lines written since the size was last checked.
    lines: u32,
}

static LOG: Mutex<Option<LogState>> = Mutex::new(None);

/// The current local time.
fn local_now() -> LocalTime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let mut tm = Tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };
    // SAFETY: both pointers are valid for the call; `localtime_r` only writes into `tm`.
    unsafe { localtime_r(&secs, &mut tm) };
    LocalTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
        millis: now.subsec_millis(),
    }
}

/// Opens (rotating first if too big) the log at `path` and points stderr at it.
fn point_stderr_at(path: &Path) -> io::Result<()> {
    let file = logging::open_log(path)?;
    // SAFETY: both are open file descriptors; fd 2 gets its own duplicate, so dropping
    // `file` afterwards is fine.
    if unsafe { dup2(file.as_raw_fd(), 2) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Keeps stderr on the log at `path` (SAFE-10): reopens it if the file there isn't the one
/// being written (deleted, replaced, or an earlier reopen failed), rotates it when too big.
/// Failures are left for the next check; there's nowhere else to report them.
fn check_log(path: &Path) {
    // SAFETY: fd 2 is open for the whole process. `ManuallyDrop` keeps this `File` from
    // closing it; it's only used to read fd 2's metadata.
    let stderr = ManuallyDrop::new(unsafe { File::from_raw_fd(2) });
    let Ok(writing) = stderr.metadata() else {
        return;
    };
    match logging::log_action(path, &writing, ROTATE_BYTES) {
        LogAction::Keep => {}
        LogAction::Reopen => {
            let _ = point_stderr_at(path);
        }
        LogAction::Rotate => {
            if logging::rotate_if_needed(path, ROTATE_BYTES).is_ok() {
                let _ = point_stderr_at(path);
            }
        }
    }
}

/// Sends stderr (log lines, panic messages) to the log file at `path`, creating its
/// directory and rotating it if needed.
pub fn init_log_file(path: &Path) -> io::Result<()> {
    point_stderr_at(path)?;
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(LogState {
        path: path.to_path_buf(),
        lines: 0,
    });
    Ok(())
}

/// Writes `[<local time>] <message>` to stderr. Used by `wm_log!`.
pub fn log_line(args: fmt::Arguments) {
    let line = format!("[{}] {}\n", logging::format_timestamp(&local_now()), args);
    let _ = io::stderr().write_all(line.as_bytes());

    let mut log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = log.as_mut() {
        state.lines += 1;
        if state.lines >= ROTATE_CHECK_LINES {
            state.lines = 0;
            check_log(&state.path);
        }
    }
}
