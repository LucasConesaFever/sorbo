//! The window manager binary: wires `wm-core` to the macOS platform.

// The `unsafe` calls into macOS live in wm-macos; the binary only wires things together.
#![forbid(unsafe_code)]

// Used only by the macOS `main`; tested everywhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod args;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod lock;

use std::process::ExitCode;

/// How often to check again while waiting for the Accessibility permission (RUN-9).
#[cfg(target_os = "macos")]
const TRUST_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// Start-up order (spec section 9): arguments, log, lock, Accessibility, platform, run.
/// `--check-config` and `--check-accessibility` answer and exit right after the arguments.
#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    use wm_macos::wm_log;

    let args = match args::parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("sorbo: {msg}");
            return ExitCode::FAILURE;
        }
    };
    if args.check_config {
        return check_config(args.config_path);
    }
    if args.check_accessibility {
        return if wm_macos::accessibility_trusted(false) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    let Some(home) = std::env::var_os("HOME") else {
        wm_log!("HOME is not set");
        return ExitCode::FAILURE;
    };
    // Taken before the log file is opened, so a second copy never touches the running
    // copy's log (SAFE-10); its "already running" line goes to stderr (launchd.log).
    // Held until `main` returns; the OS releases it if the process dies.
    let _lock = match lock::acquire_lock(&lock::lock_path(std::path::Path::new(&home))) {
        Ok(Some(file)) => file,
        Ok(None) => {
            wm_log!("sorbo is already running (see ./wm.sh stop)");
            // Exit 0 so launchd doesn't restart a second copy in a loop (spec section 8).
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            wm_log!("can't take the lock file: {err}");
            return ExitCode::FAILURE;
        }
    };

    // A log we can't open isn't worth a restart loop: keep logging to stderr (launchd sends
    // it to launchd.log) and run anyway.
    if let Some(path) = &args.log_file
        && let Err(err) = wm_macos::init_log_file(path)
    {
        eprintln!(
            "sorbo: can't open log file {}: {err}; logging to stderr",
            path.display()
        );
    }
    wm_log!(
        "sorbo {} ({}) starting, pid {}",
        env!("CARGO_PKG_VERSION"),
        option_env!("SORBO_COMMIT").unwrap_or("dev"),
        std::process::id()
    );

    if !wm_macos::accessibility_trusted(true) {
        let exe = std::env::current_exe().unwrap_or_else(|_| "sorbo".into());
        wm_log!(
            "waiting for Accessibility permission: allow {} in System Settings > \
             Privacy & Security > Accessibility",
            exe.display()
        );
        // This process keeps its first answer, so ask a fresh copy of the binary each time.
        let granted = || {
            std::process::Command::new(&exe)
                .arg("--check-accessibility")
                .status()
                .is_ok_and(|status| status.success())
        };
        while !granted() {
            std::thread::sleep(TRUST_POLL);
        }
        wm_log!("Accessibility permission granted");
    }

    let config_path = wm_macos::config_load::config_path(std::path::Path::new(&home));
    let mut platform = match wm_macos::MacPlatform::new(config_path) {
        Ok(platform) => platform,
        Err(err) => {
            wm_log!("could not start: {err}");
            return ExitCode::FAILURE;
        }
    };
    wm_core::run(&mut platform);
    // Ctrl+C or SIGTERM ended the loop: put back the windows the WM had hidden.
    platform.restore_hidden_windows();
    wm_log!("stopped");
    ExitCode::SUCCESS
}

/// `--check-config [path]` (CFG-8): parse the file exactly as the WM would, print every
/// problem, and exit 1 if there are any. Needs no lock, no Accessibility and no running WM.
#[cfg(target_os = "macos")]
fn check_config(path: Option<std::path::PathBuf>) -> ExitCode {
    use wm_macos::config_load::{check, config_path};

    let explicit = path.is_some();
    let path = match path {
        Some(path) => path,
        None => match std::env::var_os("HOME") {
            Some(home) => config_path(std::path::Path::new(&home)),
            None => {
                eprintln!("sorbo: HOME is not set");
                return ExitCode::FAILURE;
            }
        },
    };
    let (lines, ok) = check(&path, explicit);
    for line in lines {
        println!("{line}");
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    eprintln!("sorbo only runs on macOS");
    ExitCode::FAILURE
}
