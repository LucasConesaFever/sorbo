//! Only one WM at a time (RUN-6): a lock file held for the whole run.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

/// `<home>/Library/Application Support/sorbo/wm.lock`.
pub fn lock_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/sorbo/wm.lock")
}

/// Takes the lock at `path`, creating its directory if needed. `Ok(None)` means another
/// process holds it. Keep the returned `File` alive for as long as the lock should last; the
/// OS releases it when the file is closed or the process dies.
pub fn acquire_lock(path: &Path) -> io::Result<Option<File>> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sorbo-lock-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn run_6_lock_path_is_under_application_support() {
        assert_eq!(
            lock_path(Path::new("/Users/a")),
            PathBuf::from("/Users/a/Library/Application Support/sorbo/wm.lock")
        );
    }

    #[test]
    fn run_6_second_lock_is_refused() {
        let dir = temp_dir("second");
        let path = dir.join("wm.lock");
        let first = acquire_lock(&path).unwrap();
        assert!(first.is_some());
        assert!(acquire_lock(&path).unwrap().is_none());
        drop(first);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_6_lock_is_free_after_release() {
        let dir = temp_dir("release");
        let path = dir.join("wm.lock");
        drop(acquire_lock(&path).unwrap());
        assert!(acquire_lock(&path).unwrap().is_some());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_6_lock_creates_missing_directory() {
        let dir = temp_dir("mkdir");
        let path = dir.join("a").join("b").join("wm.lock");
        assert!(acquire_lock(&path).unwrap().is_some());
        fs::remove_dir_all(&dir).unwrap();
    }
}
