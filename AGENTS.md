# sorbo

Tiling window manager for macOS, inspired by Hyprland.

Requirements live in `docs/requirements.md` and are the source of truth. Requirement IDs (WS-1, MON-4, FOC-3...) are referenced from tests and from `docs/decisions.md`.

## Fixed architectural decisions

- Public macOS APIs only. Workspaces are emulated, not native Spaces. SIP stays enabled. Never propose private APIs (SkyLight/CGS) or code injection.
- `crates/wm-core`: pure Rust, no macOS dependencies, no `unsafe`. Must build and test on Linux.
- `crates/wm-macos`: the only crate that talks to the OS (Accessibility, CoreGraphics, NSWorkspace). Compiled only on macOS.
- `crates/sorbo`: the binary that wires core and platform together.
- The platform is abstracted behind a trait defined in `wm-core`. Tests use a fake in-memory implementation.
- Default layout is dwindle (binary tree, split along the longer side).
- macOS-only dependencies must be target-specific (`[target.'cfg(target_os = "macos")'.dependencies]`) so `cargo test` still works on Linux.

## How we work

- Design before code: a change in behaviour updates `docs/requirements.md` first.
- TDD in `wm-core`. Each test names the requirement it covers (e.g. `ws_2_assigns_lowest_free_number`).
- Record notable decisions in `docs/decisions.md`: what, why, alternatives rejected.
- Ask before adding a dependency.
- All docs, comments and commit messages in English.

## Commands

```bash
./wm.sh install      # build, sign, install and (re)start the WM
./wm.sh stop|start|restart|logs|uninstall
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

Before marking a task done, run `cargo fmt --all`, `cargo clippy` and `cargo test`, and report the results.

CI (`.github/workflows/ci.yml`) runs the same checks plus `bash tests/certificate.sh` on macOS, and `cargo test -p wm-core` on Linux. The project is MIT-licensed (`LICENSE`); the crates are never published (`publish = false`). Rust 1.89 or later.

## Status

Implemented: the core (workspaces, dwindle tiling, focus, multiple monitors), the macOS platform, running under launchd (`./wm.sh install`, see `README.md`; log at `~/Library/Logs/sorbo/wm.log`), workspaces in the menu bar, the config file (`config/sorbo.toml` is the shipped default), floating window rules, the focus border (JankyBorders, `config/bordersrc`), the resize mode, and the hardening from two security audits: untrusted app data (SAFE-1..10) and sharp edges (HARD-1..7).

- Dev loop: `./wm.sh stop`, then `cargo run -p sorbo` from a terminal that has Accessibility permission (Ctrl+C restores hidden windows), then `./wm.sh start`. A second copy refuses to start while one is running.
- Diagnostic spikes live in `crates/wm-macos/examples/`; run `spike_pointer` to see what macOS reports for a misbehaving popup.

## Out of scope for v1

A separate status bar (the workspace row in Apple's menu bar exists), animations, window rules other than floating (no workspace rules), running shell commands from a binding. Do not implement them unless asked.
