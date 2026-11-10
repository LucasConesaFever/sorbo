# Contributing to sorbo

Thanks for your interest. sorbo is a small, personal project; issues and pull requests are welcome, and bigger changes are easier to agree on in an issue first.

## How the project is organised

- `docs/requirements.md` is the source of truth. Requirements have IDs (WS-1, MON-4, FOC-3...) that tests and decisions refer to.
- Features start from the requirements, then the code. Notable decisions are recorded in `docs/decisions.md`.
- `crates/wm-core` is pure Rust with no macOS dependencies and must build and test on Linux. `crates/wm-macos` is the only crate that talks to macOS. `crates/sorbo` is the binary that wires them together.
- Public macOS APIs only: no private APIs (SkyLight/CGS), no code injection, SIP stays on.

## Development loop

Rust 1.89 or later. With sorbo installed (`./wm.sh install`):

```bash
./wm.sh stop
cargo run -p sorbo     # from a terminal app that has Accessibility permission
                       # Ctrl+C stops it and puts hidden windows back
./wm.sh start
```

Only one copy runs at a time; a second one refuses to start.

## Before sending a change

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
bash tests/certificate.sh  # when wm.sh changes
```

CI runs the same checks, plus `cargo test -p wm-core` on Linux.

- Tests in `wm-core` come first (TDD) and are named after the requirement they cover, e.g. `ws_2_assigns_lowest_free_number`.
- Ask in an issue before adding a dependency.
- Docs, comments and commit messages are in English.

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
