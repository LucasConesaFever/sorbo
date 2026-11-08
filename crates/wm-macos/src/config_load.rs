//! Reading the config file from disk (CFG-1, CFG-5, CFG-7). Pure apart from file reads, so
//! it's tested on any OS; also used by `sorbo --check-config`.

use std::path::{Path, PathBuf};

use wm_core::{Config, ConfigError, default_config, format_error, parse_config};

/// `~/.config/sorbo/sorbo.toml`, always (`XDG_CONFIG_HOME` is ignored on purpose: launchd
/// doesn't set it, and the WM, `--check-config` and `./wm.sh` must agree on one file).
pub fn config_path(home: &Path) -> PathBuf {
    home.join(".config").join("sorbo").join("sorbo.toml")
}

/// Where the config in effect came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The file was read and used.
    File,
    /// There is no file: the built-in defaults.
    Defaults,
    /// The file is there but can't be used (unreadable, invalid TOML, no keybinding): the
    /// previous config stays, or the defaults at startup.
    Unusable { kept_previous: bool },
}

/// What was in the file at the last load, to skip saves that change nothing (CFG-4).
#[derive(Clone, Debug, PartialEq)]
pub enum FileText {
    /// There was no file.
    Missing,
    /// The file was there but couldn't be read as text.
    Unreadable,
    Read(String),
}

/// The result of loading the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    /// The config to use now.
    pub config: Config,
    /// The file's problems; empty when it's clean or missing.
    pub errors: Vec<ConfigError>,
    pub source: Source,
    /// What the file held, to skip saves that change nothing.
    pub text: FileText,
}

/// Loads the file at `path`. `previous` is the config in effect, `None` at startup.
pub fn load(path: &Path, previous: Option<&Config>) -> Loaded {
    let fallback = |errors, text| Loaded {
        config: previous.cloned().unwrap_or_else(default_config),
        errors,
        source: Source::Unusable {
            kept_previous: previous.is_some(),
        },
        text,
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded {
                config: default_config(),
                errors: Vec::new(),
                source: Source::Defaults,
                text: FileText::Missing,
            };
        }
        Err(e) => {
            let error = ConfigError {
                line: None,
                message: format!("can't read the file: {e}"),
            };
            return fallback(vec![error], FileText::Unreadable);
        }
    };
    let outcome = parse_config(&text);
    match outcome.config {
        Some(config) => Loaded {
            config,
            errors: outcome.errors,
            source: Source::File,
            text: FileText::Read(text),
        },
        None => fallback(outcome.errors, FileText::Read(text)),
    }
}

/// True when the file holds exactly what it held at the last load (still missing counts):
/// an editor saved without changing anything, so there's nothing to apply (CFG-4). A file
/// that couldn't be read is always worth loading again.
pub fn unchanged(previous: &FileText, path: &Path) -> bool {
    match (previous, std::fs::read_to_string(path)) {
        (FileText::Read(previous), Ok(now)) => *previous == now,
        (FileText::Missing, Err(e)) => e.kind() == std::io::ErrorKind::NotFound,
        _ => false,
    }
}

/// The log lines for a load (CFG-7): a summary, then one line per error.
pub fn log_lines(path: &Path, loaded: &Loaded) -> Vec<String> {
    let shown = path.display();
    let summary = match loaded.source {
        Source::File => format!(
            "config: loaded {shown}: {} bindings, {} errors",
            loaded.config.bindings.len(),
            loaded.errors.len()
        ),
        Source::Defaults => format!("config: no file at {shown}, using defaults"),
        Source::Unusable {
            kept_previous: true,
        } => format!("config: {shown} can't be used; keeping the previous config"),
        Source::Unusable {
            kept_previous: false,
        } => format!("config: {shown} can't be used; using defaults"),
    };
    let name = file_name(path);
    std::iter::once(summary)
        .chain(
            loaded
                .errors
                .iter()
                .map(|e| format!("config: {}", format_error(&name, e))),
        )
        .collect()
}

/// The file's name for error messages (`sorbo.toml`).
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// What `sorbo --check-config` prints, and whether the file is fine (CFG-8). `explicit` is
/// true when the path was given on the command line: a missing file is then an error (a
/// typo in the path), while the default path may simply not exist yet.
pub fn check(path: &Path, explicit: bool) -> (Vec<String>, bool) {
    let loaded = load(path, None);
    if loaded.source == Source::Defaults {
        return if explicit {
            (vec![format!("no file at {}", path.display())], false)
        } else {
            let line = format!("no file at {}; defaults would be used", path.display());
            (vec![line], true)
        };
    }
    let name = file_name(path);
    let mut lines: Vec<String> = loaded
        .errors
        .iter()
        .map(|e| format_error(&name, e))
        .collect();
    if loaded.errors.is_empty() {
        lines.push(format!("ok: {} bindings", loaded.config.bindings.len()));
        return (lines, true);
    }
    if let Source::Unusable { .. } = loaded.source {
        lines.push("the file can't be used: the WM would keep its previous config".into());
    }
    lines.push(format!("{} errors", loaded.errors.len()));
    (lines, false)
}

/// The directories to watch for saves: the file's own, plus the real file's when the path
/// is a symlink (dotfiles kept in a repo), since editing the target only changes that one.
pub fn watch_dirs(path: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = path.parent().map(Path::to_path_buf).into_iter().collect();
    if let Some(real) = std::fs::canonicalize(path)
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        let own = dirs.first().and_then(|d| std::fs::canonicalize(d).ok());
        if own.as_ref() != Some(&real) {
            dirs.push(real);
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use wm_core::{Action, default_config};

    /// A fresh empty directory for one test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sorbo-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const ONE_BINDING: &str = "[bindings]\ncmd-alt-q = \"close\"\n";

    #[test]
    fn cfg_1_config_path_is_under_dot_config() {
        assert_eq!(
            config_path(Path::new("/Users/me")),
            PathBuf::from("/Users/me/.config/sorbo/sorbo.toml")
        );
    }

    #[test]
    fn cfg_1_missing_file_means_defaults() {
        let path = temp_dir("missing").join("sorbo.toml");
        let loaded = load(&path, None);
        assert_eq!(loaded.config, default_config());
        assert_eq!(loaded.source, Source::Defaults);
        assert_eq!(loaded.errors, vec![]);
        assert_eq!(loaded.text, FileText::Missing);
    }

    #[test]
    fn cfg_1_file_is_loaded() {
        let path = temp_dir("loaded").join("sorbo.toml");
        std::fs::write(&path, ONE_BINDING).unwrap();
        let loaded = load(&path, None);
        assert_eq!(loaded.source, Source::File);
        assert_eq!(loaded.config.bindings.len(), 1);
        assert_eq!(loaded.config.bindings[0].1, Action::Close);
        assert_eq!(loaded.text, FileText::Read(ONE_BINDING.to_string()));
    }

    #[test]
    fn cfg_1_deleted_file_means_defaults() {
        let path = temp_dir("deleted").join("sorbo.toml");
        std::fs::write(&path, ONE_BINDING).unwrap();
        let first = load(&path, None);
        std::fs::remove_file(&path).unwrap();
        let loaded = load(&path, Some(&first.config));
        assert_eq!(loaded.config, default_config());
        assert_eq!(loaded.source, Source::Defaults);
    }

    #[test]
    fn cfg_5_unusable_reload_keeps_previous() {
        let path = temp_dir("unusable").join("sorbo.toml");
        std::fs::write(&path, ONE_BINDING).unwrap();
        let previous = load(&path, None).config;
        std::fs::write(&path, "[bindings]\ncmd-alt-q = \"close\n").unwrap();
        let loaded = load(&path, Some(&previous));
        assert_eq!(loaded.config, previous);
        assert_eq!(
            loaded.source,
            Source::Unusable {
                kept_previous: true
            }
        );
        assert!(!loaded.errors.is_empty());
    }

    #[test]
    fn cfg_5_unusable_at_startup_uses_defaults() {
        let path = temp_dir("unusable-start").join("sorbo.toml");
        std::fs::write(&path, "[bindings]\n").unwrap();
        let loaded = load(&path, None);
        assert_eq!(loaded.config, default_config());
        assert_eq!(
            loaded.source,
            Source::Unusable {
                kept_previous: false
            }
        );
        assert!(!loaded.errors.is_empty());
    }

    #[test]
    fn cfg_4_unchanged_content_is_not_reapplied() {
        let path = temp_dir("unchanged").join("sorbo.toml");
        std::fs::write(&path, ONE_BINDING).unwrap();
        let read = FileText::Read(ONE_BINDING.to_string());
        assert!(unchanged(&read, &path));
        std::fs::write(&path, "[bindings]\nctrl-alt-q = \"close\"\n").unwrap();
        assert!(!unchanged(&read, &path));
        std::fs::remove_file(&path).unwrap();
        assert!(unchanged(&FileText::Missing, &path));
        assert!(!unchanged(&read, &path));
    }

    #[test]
    fn cfg_4_deleting_an_unreadable_file_is_a_change() {
        let path = temp_dir("unreadable").join("sorbo.toml");
        // Not UTF-8, so it can't be read as text.
        std::fs::write(&path, [0xFF, 0xFE, 0x00]).unwrap();
        let loaded = load(&path, None);
        assert_eq!(loaded.text, FileText::Unreadable);
        std::fs::remove_file(&path).unwrap();
        assert!(!unchanged(&loaded.text, &path));
    }

    #[test]
    fn cfg_8_check_reports_ok_errors_and_missing_files() {
        let dir = temp_dir("check");
        let path = dir.join("sorbo.toml");
        std::fs::write(&path, ONE_BINDING).unwrap();
        assert_eq!(
            check(&path, true),
            (vec!["ok: 1 bindings".to_string()], true)
        );

        std::fs::write(
            &path,
            "[bindings]\ncmd-alt-q = \"close\"\nctrl-alt-x = \"focus lft\"\n",
        )
        .unwrap();
        let (lines, ok) = check(&path, true);
        assert!(!ok);
        assert!(lines[0].starts_with("sorbo.toml line 3:"), "{lines:?}");
        assert_eq!(lines.last().unwrap(), "1 errors");

        // A path given on purpose that isn't there is a mistake; the default path may
        // legitimately be missing.
        let missing = dir.join("sorbo.tml");
        assert!(!check(&missing, true).1);
        assert!(check(&missing, false).1);
    }

    #[cfg(unix)]
    #[test]
    fn cfg_4_symlinked_file_watches_both_directories() {
        let dir = temp_dir("symlink");
        let dotfiles = dir.join("dotfiles");
        let config = dir.join("config");
        std::fs::create_dir_all(&dotfiles).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(dotfiles.join("sorbo.toml"), ONE_BINDING).unwrap();
        let link = config.join("sorbo.toml");
        std::os::unix::fs::symlink(dotfiles.join("sorbo.toml"), &link).unwrap();
        let dirs = watch_dirs(&link);
        assert_eq!(dirs.len(), 2, "{dirs:?}");
        assert_eq!(dirs[0], config);
        assert_eq!(dirs[1], dotfiles.canonicalize().unwrap());
        assert_eq!(watch_dirs(&dotfiles.join("sorbo.toml")).len(), 1);
    }

    #[test]
    fn cfg_7_log_lines_report_counts_and_errors() {
        let path = temp_dir("log").join("sorbo.toml");
        std::fs::write(
            &path,
            "[bindings]\ncmd-alt-q = \"close\"\nctrl-alt-left = \"focus lft\"\n",
        )
        .unwrap();
        let lines = log_lines(&path, &load(&path, None));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(
            lines[0],
            format!("config: loaded {}: 1 bindings, 1 errors", path.display())
        );
        assert!(
            lines[1].starts_with("config: sorbo.toml line 3: unknown direction"),
            "{}",
            lines[1]
        );

        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            log_lines(&path, &load(&path, None)),
            vec![format!(
                "config: no file at {}, using defaults",
                path.display()
            )]
        );

        std::fs::write(&path, "[bindings]\n").unwrap();
        let lines = log_lines(&path, &load(&path, Some(&default_config())));
        assert_eq!(
            lines[0],
            format!(
                "config: {} can't be used; keeping the previous config",
                path.display()
            )
        );
        assert!(lines[1].contains("no keybindings"), "{}", lines[1]);
    }
}
