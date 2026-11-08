//! HARD-1 spike helper: exits 0 if this process may use Accessibility, 1 if not. With
//! `--prompt`, macOS shows its permission dialog first. Signed by `spike_signing.sh` as a
//! stand-in for sorbo, so the grant check doesn't need a second running WM.

fn main() -> std::process::ExitCode {
    let prompt = std::env::args().any(|a| a == "--prompt");
    #[cfg(target_os = "macos")]
    let trusted = wm_macos::accessibility_trusted(prompt);
    #[cfg(not(target_os = "macos"))]
    let trusted = !prompt && false;
    if trusted {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
