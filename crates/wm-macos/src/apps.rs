//! Which app a `new-window` binding means (CFG-3).

use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// The app to open for a value the config parser read as a bundle id (it has a dot and no
/// spaces). App names can look like that too (`draw.io`), so when no app has that bundle id,
/// it's tried as an app name. `installed` finds the app macOS has for a bundle id; `named`
/// finds an app by name. Both give where the app is (`T`), which is what gets launched and
/// compared with running apps, so the copy found is the copy used.
pub fn bundle_or_name<T>(
    id: &str,
    installed: impl Fn(&str) -> Option<T>,
    named: impl Fn(&str) -> Option<T>,
) -> Result<T, String> {
    installed(id)
        .or_else(|| named(id))
        .ok_or_else(|| format!("no app with bundle id or name \"{id}\" is installed"))
}

/// A file's identity on disk: its device and inode (SAFE-6). Two paths to the same app
/// (a symlink, a different letter case, `/Applications` vs `/System/Volumes/Data/Applications`)
/// have the same identity even though their text differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileId {
    dev: u64,
    ino: u64,
}

/// The identity of the file at `path`, or `None` if it can't be read.
pub fn file_id(path: &Path) -> Option<FileId> {
    std::fs::metadata(path).ok().map(|m| FileId {
        dev: m.dev(),
        ino: m.ino(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Stand-ins for "where the app is": the lookup by bundle id, and by name.
    fn installed(id: &str) -> Option<String> {
        (id == "com.apple.Terminal").then(|| "/System/Applications/Utilities/Terminal.app".into())
    }

    fn named(name: &str) -> Option<String> {
        (name == "draw.io").then(|| "/Applications/draw.io.app".to_string())
    }

    #[test]
    fn cfg_3_bundle_id_is_used_as_is() {
        assert_eq!(
            bundle_or_name("com.apple.Terminal", installed, named),
            Ok("/System/Applications/Utilities/Terminal.app".to_string())
        );
    }

    #[test]
    fn cfg_3_name_that_looks_like_a_bundle_id_is_found_by_name() {
        assert_eq!(
            bundle_or_name("draw.io", installed, named),
            Ok("/Applications/draw.io.app".to_string())
        );
    }

    #[test]
    fn cfg_3_neither_bundle_id_nor_name_is_an_error() {
        assert_eq!(
            bundle_or_name("com.example.missing", installed, named),
            Err("no app with bundle id or name \"com.example.missing\" is installed".to_string())
        );
    }
}

/// The running app to act on (SAFE-6): the first whose bundle is the very app `new-window`
/// resolved (`installed`), compared by file identity. Any app can claim a bundle id in its
/// Info.plist, so a copy elsewhere is an impostor and is skipped. `running` pairs each
/// running app with its bundle's identity. `None`: the resolved app isn't running.
pub fn pick_running<T>(running: &[(T, Option<FileId>)], installed: Option<FileId>) -> Option<&T> {
    let installed = installed?;
    running
        .iter()
        .find(|(_, id)| *id == Some(installed))
        .map(|(app, _)| app)
}

/// Whether opening the installed app must ask for a new instance (SAFE-6): when only
/// impostors claim its bundle id, macOS treats the id as running and won't start the real
/// app unless asked to (seen on macOS 26, acceptance check 7).
pub fn open_fresh(others_claim_the_id: bool) -> bool {
    others_claim_the_id
}

#[cfg(test)]
mod pick_running_tests {
    use super::*;

    /// A temp folder with a fake app bundle `Real.app`, an impostor `Fake.app`, and a
    /// symlink `Link.app` to the real one. Removed when dropped.
    struct Apps(std::path::PathBuf);

    impl Apps {
        fn new(tag: &str) -> Apps {
            let dir = std::env::temp_dir().join(format!("sorbo-apps-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("Real.app")).unwrap();
            std::fs::create_dir_all(dir.join("Fake.app")).unwrap();
            std::os::unix::fs::symlink(dir.join("Real.app"), dir.join("Link.app")).unwrap();
            Apps(dir)
        }
        fn id(&self, name: &str) -> Option<FileId> {
            file_id(&self.0.join(name))
        }
    }

    impl Drop for Apps {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn safe_6_impostor_listed_first_is_skipped() {
        let apps = Apps::new("impostor");
        let running = [("fake", apps.id("Fake.app")), ("real", apps.id("Real.app"))];
        assert_eq!(pick_running(&running, apps.id("Real.app")), Some(&"real"));
    }

    #[test]
    fn safe_6_same_app_through_another_path_matches() {
        let apps = Apps::new("link");
        let running = [("real", apps.id("Link.app"))];
        assert_eq!(pick_running(&running, apps.id("Real.app")), Some(&"real"));
    }

    #[test]
    fn safe_6_real_app_is_opened_fresh_when_only_impostors_run() {
        assert!(open_fresh(true));
        assert!(!open_fresh(false));
    }

    #[test]
    fn safe_6_only_impostors_means_not_running() {
        let apps = Apps::new("only");
        let running = [("fake", apps.id("Fake.app")), ("gone", None)];
        assert_eq!(pick_running(&running, apps.id("Real.app")), None);
        assert_eq!(pick_running(&running, None), None);
    }
}
