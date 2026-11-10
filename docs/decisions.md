# Decisions

Notable design decisions: what was decided, why, and the alternatives rejected. Newest first.
"(author)" means the author decided. Everything else is the designer's call, made on the author's instruction to make calls and explain them.

## 2026-11-10: screen wake, mi-wm leftovers

### D46. The focus guard lasts its full 500 ms, even after the app confirms our window (FOC-1, FOC-7)
**What:** after `Focus`, `wm-macos` keeps overriding the app's own choice of window for the whole `HOLD_FOCUS_FOR` (500 ms, up to 2 times), instead of stopping at the first report of our window. Once our window has been reported, only a window parked on a hidden workspace is overridden; another window of the app on screen is believed.
**Why:** found in daily use: kitty had windows on workspaces 2 (shown) and 6 (hidden) of one monitor. Moving the pointer onto the workspace 2 window focused it, kitty reported it, and 25 ms later reported its workspace 6 window instead (its last active one). The guard had already ended, so core believed it and FOC-7 switched the monitor to 6; the next crossing switched it back.
**Cost:** a Cmd+` to the app's window on a hidden workspace within 500 ms of focus-follows-mouse is overridden twice before it's followed.
**Rejected:** ignoring every report of a hidden window shortly after `Focus` (Cmd+Tab/Cmd+` still need FOC-7); not activating the app on focus-follows-mouse (keyboard input would go to the wrong app).

### D45. `./wm.sh install` no longer moves mi-wm installs over (NAME-2 to NAME-4 retired) *(author)*
**What:** the mi-wm migration (`migrate_from_mi_wm`, `stop_mi_wm`, the `mi-wm local signing` clean-up, `tests/migration.sh` and its CI step) and the README's "Coming from mi-wm" section are removed. The pre-HARD-1 `sorbo local signing` clean-up stays.
**Why:** the author's own Mac is the only mi-wm install there ever was, and it's already moved over; in the public repo the code only added surface.
**Cost:** anyone who did run mi-wm would have to remove it by hand. Nobody did.

### D44. Apps with windows lost in a display change are asked for their windows again (MON-7)
**What:** when a window is kept as lost (D30), `wm-macos` asks its app for its window list (`AXWindows`) 1, 5, 15 and 45 s later, on the lost-window timer. A rebuilt window found this way takes the lost window's place as before. Each look is logged with how many of the app's windows are still lost. Matching a returning window also changed: a hidden window is compared with where it was parked (not its tile, which a shown window of the same app on another workspace may share), and size counts as well as position, so windows parked in the same corner are told apart.
**Why:** found in daily use after a screen wake: macOS rebuilt the kitty and PyCharm windows on the external monitors but didn't announce them until each app was clicked, minutes later. By then D30's 60 s had passed, so they arrived as new windows and were tiled on the focused workspace, on the laptop. Only Slack, which took focus within 10 s, kept its place.
**Cost:** up to four extra `AXWindows` reads per app after a display change. Two hidden windows of one app, the same size, on different hidden workspaces of one monitor can still swap; telling them apart would need their titles saved before they go missing. Not yet confirmed on a real wake that macOS lists a rebuilt window before its app is activated; the log line says so the next time. If it doesn't, the next step is keeping lost windows until their app is activated.
**Rejected:** keeping lost windows longer on their own (they still only come back on a click, and a really closed window would leave an empty tile all that time); placing every new window on the monitor under its frame (changes where new windows go everywhere, not just after a wake).

## 2026-11-08: sharp edges

### D43. Bounded hit tests, guarded callbacks, checked types (HARD-5, HARD-6, HARD-7; audit findings 5–7)
**What:**
- **HARD-5:** the pointer hit test has a 30 ms budget, checked before each Accessibility call up the element tree. An app that overruns it is treated as "not a window" and skipped for 2 s, with one log line.
- **HARD-6:**
  - The shared state is leaked (`Box::leak`), so the pointer the OS holds is always valid.
  - A private `shared_from` is the only place that pointer turns back into a reference.
  - Every callback goes through `run_or_defer` or `run_or_retry`, built on the pure `guard::run_guarded`. Its work runs under `try_borrow_mut` with `catch_unwind`.
  - If sorbo is busy, a timer fires again 10 ms later, and other work (shortcuts, quit, display changes, Accessibility and app notices, clicks) is queued and run by `next_event` once the state is free.
  - A panic is logged and sorbo carries on.
  - `MacPlatform`'s `Drop` puts hidden windows back if sorbo leaves the main loop by panicking.
- **HARD-7:**
  - `WindowInfo.frame` is `Option<UsableRect>`, and `WindowFrameChanged` carries a `UsableRect`. `UsableRect::new` checks the frame, and wm-macos makes one only in `confirm::confirmed_frame` (confirmed by the window server). Popups get no frame, since core never places them.
  - `WorkspaceNumber` wraps a `NonZeroU32`.
  - `HeldActivation::new` takes an `Activation { app, previous }`, so the two pids are named at the call site.
**Review fixes:**
- **Hit test:** before messaging any app, it asks the window server who owns the front-most window under the pointer (a list reused for 250 ms). A skipped app costs nothing, and an app whose first Accessibility call times out is marked slow, which also covers a fully hung app.
- **Timers:** a timer whose work panicked fires again a second later (`guard::retry_after`).
- **Startup placement:** `WindowInfo.position_hint`, the unchecked origin the app reports, picks the monitor for a window discovered before its frame is confirmed, as the zero-size frame did before. It's used for nothing else.
**Not guarded:** `next_event` and `execute` themselves, including the hit test. A panic there ends the loop, and `Drop` puts hidden windows back. D43 covers callbacks.
**Why:** the sharp-edges audit found that:
- a slow app could stall the WM for seconds per pointer move;
- re-entering a callback (for example from a future status-bar menu's tracking loop) would panic and abort, leaving windows parked;
- the frame and pid checks depended on every new piece of code remembering them.
**Rejected:**
- Dropping busy callbacks (`try_borrow_mut` alone): it silently loses work, such as the activation hold or a pending reload.
- Switching timers to the default run-loop mode: it changes when they fire during menu tracking, and nothing needed that once re-entry is safe.
- Converting every pid in wm-macos to `Pid`: `HeldActivation` was where two pids sit side by side; elsewhere a pid is the only integer around, and AX takes `i32`.

### D42. `new-window` acts on the copy it resolved, compared by file identity (sharp-edges finding 4)
**What:**
- The app is resolved once to a location:
  - a bundle id → the copy macOS picks for that id;
  - a name → `<name>.app` found in the application folders;
  - `browser` → the default browser's own location.
- That exact location is what gets launched.
- A running app receives the ⌘N only if its bundle is the same file (device and inode) as that location.
**Why:** the old code reduced the app to a bundle id and asked macOS again. So a name or the default browser could resolve to another copy with the same id. Paths were compared as text, so a symlink, a letter-case difference or `/System/Volumes/Data/...` made the real running app look like an impostor, and every press opened another instance. This also makes `browser` follow the SAFE-6 spec ("uses the default browser's URL").
**Rejected:**
- Trusting any running copy in a standard application folder: `~/Applications` and, for admin users, `/Applications` are writable without a password.
- Listing every registered copy (`URLsForApplicationsWithBundleIdentifier`): Launch Services also registers impostors it has seen.
**Accepted:** an app run from App Translocation (a quarantined download) is a different file from its installed copy, so it gets the installed copy launched once.

### D40. A window sorbo can't classify never takes the blame for ⌃⌥Q (HARD-2)
**What:**
- When focus moves to a popup of an app other than the focused window's, core clears focus. A popup of the focused window's own app leaves focus alone.
- wm-macos classifies a popup-classified window again when the user focuses it, and 200 ms, 1 s and 5 s after it appears.
- `close` presses the close button only if the window's app is frontmost and the app's `AXFocusedWindow` is that window; otherwise it logs and does nothing.
**Why:** an app busy as its window opens makes the classification reads fail, and the window is treated as a popup (on purpose: a missed focus costs less than a closed popup). Focus then stayed on the previous window, possibly in another app, and ⌃⌥Q closed it.
**Accepted:** a ⌃⌥Q pressed within a few ms of sorbo focusing a window, before macOS's activation notice arrives, is refused and logged; pressing again works. Move, swap and toggle-floating aren't checked (each can be undone with the next keypress).
**Rejected:** classifying unknown windows as managed (tiles menus); checking every action with macOS (AX calls on every hotkey); clearing focus on every popup (breaks ⌃⌥Q after opening a menu in the same app).

### D41. Failed hides are retried in wm-macos (HARD-3)
**What:** a window counts as hidden only once `set_position` succeeded. Without a readable frame, the parking corner comes from the frame sorbo last set or last confirmed. A failed hide is retried after 0.5, 1, 2 and 4 s, then every 5 s, until it succeeds, core gives the window another place, or it goes away. Only the first failure and the eventual success are logged.
**Why:** a busy app (Slack, Xcode) could leave its window over the new workspace, marked hidden, with its moves ignored. Core already says what it wants (`Placement::Hidden`); making it true is the platform's job.
**Rejected:** a `CommandFailed { id }` event for core to retry (changes the platform trait for one command; left until another command needs it); a fixed short retry interval (a hung app answers each AX call after a 50 ms timeout).

### D39. Builds are signed with the hardened runtime by a key that asks first (HARD-1; replaces D20's `-T`)
**What:**
- `./wm.sh install` signs with `--options runtime`.
- The key is imported with `security import … -x -T ""`: non-extractable, and **no** app on its access list. Every `codesign` with it makes macOS ask; the user clicks Allow (not Always Allow).
- The certificate is renamed `sorbo signing`. The old `sorbo local signing`, and mi-wm's `mi-wm local signing`, are deleted once a build is signed with the new one, and macOS asks once for Accessibility again. Both old keys let `codesign` sign silently, and each matches an Accessibility grant (sorbo's, or the leftover mi-wm entry).
**Why:** with D20's `-T /usr/bin/codesign`, any program run by the user can sign its own binary as sorbo and inherit the Accessibility grant. Without the hardened runtime, `DYLD_INSERT_LIBRARIES` in the user-writable plist loads code into sorbo.
**Spike** (`crates/wm-macos/examples/spike_signing.sh` with the `spike_ax_trust` stand-in, signed like sorbo, run by a one-shot launchd job, macOS 26, 2026-11-08, with the author clicking):
1. **pass:** a self-signed `--options runtime` binary passes `codesign --verify --strict`, shows `flags=0x10000(runtime)` and runs under launchd.
2. **pass:** after one grant (through macOS's own Accessibility prompt), a rebuilt binary (CDHash `3714de2a…` → `8f9ac17d…`) re-signed with the runtime flag was still trusted, with no prompt. Adding the binary with **+** in System Settings did **not** make it trusted (twice, with the switch on). The prompt route did.
3. **pass:** with `DYLD_INSERT_LIBRARIES` set in the job, a dylib whose constructor writes a file was loaded by the plain-signed binary (the control) and ignored by the hardened one.
4. **pass:** with the key imported `-x -T ""`, two `codesign` runs in a row each showed a prompt and signed after Allow. An earlier import with the `-T` simply dropped also prompted for codesign.
5. **pass, after a change:** with the `-T` simply dropped, `security import` puts **`/usr/bin/security` itself** on the key's access list with `export_clear` and no password, so any program can export the private key silently with `security export`. With `-x -T ""` (checked in a scratch keychain), the access list is empty and `security export` fails outright ("The contents of this item cannot be retrieved").
Both parts are kept. The spec's "import without `-T`" becomes "import with `-x -T ""`".
**Remaining risk:**
- A user who clicks "Always Allow" puts `codesign` on the access list, as today.
- A program can still trigger the prompt and hope for a click.
- The **+** route doesn't grant a binary that never asked; the README now says to use macOS's own prompt (`./wm.sh restart` shows it again).
**Rejected:** a dedicated keychain (its password must live where the same user can read it, or be typed anyway); a paid Developer ID (left for Homebrew); changing the old key's access list in place (no plain `security` command; a new certificate is checkable); dropping `-T` alone (step 5).

## 2026-11-08: untrusted app data

### D38. Apps' reports are checked before use (SAFE-1 to SAFE-10)
**What:**
- Core uses a window frame only if it is finite with a positive size (SAFE-1). A window without a usable frame isn't moved, isn't hidden while its workspace shows, and doesn't own the pointer (FOC-9).
- A floating frame is capped to its monitor and kept inside it (SAFE-2).
- wm-macos sends core an Accessibility frame only when the window server lists a window of the same pid (any layer) within 2 points on every edge that isn't already backing another of that app's tracked windows; otherwise it sends an unusable frame and looks again 300 ms later, up to 3 times, and after every move (SAFE-3). A floating window that is hidden while its frame is unknown comes back at a core-chosen centered frame, and its first confirmed frame is placed like a new window's (WIN-1, WIN-3).
- At most 100 windows are tracked per app (SAFE-4). The ⌘N menu walk reads at most 40 menus of 100 items and stops after 250 ms (SAFE-5). `new-window` acts only on the running copy installed under the bundle id, and when only impostors run it opens the installed app as a new instance, since macOS otherwise skips the launch (SAFE-6, found in the device check). `/` is rejected in `new-window` names (SAFE-7). Window ids are 64-bit (SAFE-8). The fullscreen list is cleaned up when an app quits (SAFE-9).
- The single-instance lock is taken before the log is opened, and every 1000 lines the log is reopened if it isn't the file being written (by device and inode) or rotated past 10 MB, retrying failures (SAFE-10).
**Why:** a Trail of Bits `rust-review` audit (threat model: another app of the same user without Accessibility) found no memory-safety bugs, but found that sorbo believed what apps report. An infinite or fake frame let an app take keyboard focus through FOC-9, bypassing macOS's limits on background apps taking focus; unbounded window lists and menus let an app freeze sorbo, which launchd doesn't restart. SAFE-1 and SAFE-2 alone still allowed a fake frame as big as one monitor; the window server, which apps can't fake, narrows that to a real window. **Remaining risk (accepted):** an app can still back a claim with a real window of its own that no other tracked window uses, for example a transparent window as big as the monitor; what it can no longer do is claim an area where it has no window, or reuse one of its windows for a second claim. The final review found the first version accepted any window of the app (one real window could back a fake claim for another) and only layer-0 windows (floating panels would never be confirmed); both were fixed before merge. A spike (`examples/spike_window_list.rs`) showed owner pid and bounds come through without Screen Recording (names don't), and that AX frames and window-server bounds matched for every app window tried, including one parked off screen.
**Rejected:** using the window server's frame when the two disagree (wrong window when an app has several); checking at every pointer move (too many window-server calls); dropping FOC-9; asking for Screen Recording; a pid index for the window table (the 100-window cap keeps scans short); a rate limit on re-tracking after unhide (each pass is bounded). Left out on purpose: lints and `# Safety` docs, tree depth at ~50k windows, FIFOs swapped in for sorbo's files, the latent panic path in two FFI callbacks, and the ignored error in `repark_hidden` (all listed in the audit report).

## 2026-11-08: getting ready to go public

### D37. MIT licence, CI with no third-party actions, crates never published
**What:** the repo is MIT-licensed *(author)*. All crates share their metadata through `[workspace.package]`, including `publish = false` and `rust-version = "1.89"` (`File::try_lock`, used for the single-instance lock, is stable from 1.89). `wm-core` and the binary `#![forbid(unsafe_code)]`; every `unsafe` stays in `wm-macos`. GitHub Actions run fmt, clippy, the tests and `tests/migration.sh` on macOS, and `cargo test -p wm-core` on Linux. Workflows have no permissions by default and `contents: read` per job, and use only `actions/checkout`, pinned to a commit. Dependabot proposes weekly grouped updates for cargo and actions, a week after release. The author keeps the work email in history *(author, the audit's option a)*.
**Why:** from an open-sourcing audit before publishing (Trail of Bits `open-sourcing` skill): without a licence the code isn't open source; nothing checked the "wm-core builds on Linux" rule; a crate pushed to crates.io by mistake can't be taken back. MIT is the simplest permissive licence and every dependency allows it.
**Rejected:** `MIT OR Apache-2.0` (the Rust norm; the author chose MIT alone); third-party actions such as `dtolnay/rust-toolchain` or a cache action (each is a new dependency under AGENTS.md, and rustup is already on the runners); rewriting history to remove the email (the author keeps it).

## 2026-11-08: rename to sorbo

### D36. The project is sorbo; `./wm.sh install` moves an mi-wm install over once (NAME-1 to NAME-4)
**What:** everything the user sees is named sorbo: the binary `~/.local/bin/sorbo`, `~/.config/sorbo/sorbo.toml`, `~/Library/Logs/sorbo/`, the lock folder, the launchd label and signing identifier `io.github.lucasconesafever.sorbo`, the `sorbo local signing` certificate, messages, Objective-C class names and the hotkey signature. The binary crate is `crates/sorbo`; `wm-core`, `wm-macos` and `wm.sh` keep their names. `migrate_from_mi_wm` in `wm.sh` boots out mi-wm, removes its login entry and binary, moves the config (never over an existing one) and the logs, and removes the old lock folder; any failed step stops the install before a default config could be created. The Rust code knows only the new paths.
**Why:** *(author)* the project needed a real name before going public; *sorbo* ("sip") points at C1, SIP stays on. The migration is the only bridge, so it must make two WMs running together impossible (their lock files differ) and must never lose the config.
**Rejected:** renaming the library crates (every `use wm_core` line changes for no visible gain); keeping the old identifier or certificate to avoid the one Accessibility prompt (carries "mi-wm" forever); a Rust fallback to the old config path (two places to look, forever); deleting the old certificate or Accessibility entry from the script (the author's keychain and privacy settings are theirs to change).

## 2026-11-08: focus border

### D35. The focus border is JankyBorders, set up by `./wm.sh install` (BORDER-1 to BORDER-3)
**What:** the install offers to `brew install FelixKratz/formulae/borders` when it's missing (only on a typed yes, never without a terminal), creates `~/.config/borders/bordersrc` from the repo's `config/bordersrc` (the author's look) only if it doesn't exist, and runs `brew services start borders` if `borders` isn't running. Every step only warns on failure. Uninstall leaves JankyBorders alone and prints how to remove it.
**Why:** the author wants new users to get the border by installing from the repo. JankyBorders already draws it well and follows focus changes made by mi-wm; drawing borders inside mi-wm would mean overlay windows tracking every window move and resize, a large feature repeating a solved problem.
**Rejected:** drawing borders in mi-wm; making JankyBorders a hard requirement; installing it without asking (a third-party Homebrew tap).

## 2026-11-08: resize mode

### D34. Core owns the resize mode; its keys exist only while it's on (MODE-1 to MODE-6)
**What:** `Wm.mode` (`Normal | Resize`) lives in core with three actions: `ResizeMode` toggles, `ExitMode` leaves, `ResizeToward(direction)` moves the nearest divider on that axis around the focused tiled window one step toward the direction. Any other action leaves the mode, then runs. Core emits `ModeChanged` when the mode changes; wm-macos then registers the mode's ten fixed keys (arrows, `h j k l`, Escape, Enter, no modifiers) as Carbon hotkeys, or unregisters them, and draws a "resize" label before the workspace numbers in the menu bar row. A config reload replaces only the config's hotkeys.
**Why:** the author wanted a Hyprland/AeroSpace-style resize mode. Core deciding the mode keeps "other bindings leave it" and the arrow logic pure and tested; registering plain keys only during the mode means apps never lose them otherwise. "The arrow moves the line" is one rule for both sides of a divider, with no grow/shrink to think about. The label goes before the boxes: menu bar items keep their right edge and grow leftward, so the boxes stay where they are on screen (placing it after them, as first built, made every box jump left by the label's width when the mode started).
**Rejected:** configurable keys inside the mode, or other modes (not needed for the habit); a timeout that ends a forgotten mode (the label shows it, and Escape is one key); resizing floating windows from the keyboard (the mouse does it); the label after the workspace numbers (shifts every box on screen, see above).

## 2026-11-08: config review fixes

### D33. Gaps are applied per side and never take more than half a cell (LAY-1)
**What:** the tree now splits the area inside the outer gap, and each window gives up half the inner gap only on the sides that face another window; both insets of an axis together are capped at half the cell. Before, the area was grown by half the inner gap and every window shrunk by it on all sides.
**Why:** with large gaps (up to 100) and several windows, a cell narrower than the inner gap gave a window a negative width or height, which was sent to the app; capping only that shrink would have let edge windows stick out past the outer gap when `inner > 2 × outer`, because of the grown area. Applying gaps per side is what the gaps mean, so ordinary layouts get the same frames; only resized splits move by a fraction of a point (the ratio now applies to the area inside the outer gap).
**Rejected:** clamping each frame to a minimum size afterwards (windows would overlap their neighbours' gaps and could leave the screen).

## 2026-11-08: reaching floating windows

### D32. The pointer over a floating window's area targets that window (FOC-9)
**What:** when macOS reports the pointer over a tiled window, core checks the floating windows on visible workspaces first: if one's frame contains the pointer, that floating window is the target (the latest added wins when several overlap). Popups, unmanaged windows and a reported floating window are left as macOS reports them.
**Why:** focusing a tiled window activates its app, and macOS then stacks that app's window above every other app's floating window. A spike showed `AXRaise` on the floating window afterwards only lifts it to just below the active app (public APIs can't keep another app's window above the active one). Once covered, the hit test always names the tile, so the floating window couldn't be reached by the mouse. Treating the floating window's area as its own gives Hyprland's pointer behaviour; the visual difference is that the floating window is hidden while a tile under it has focus, and comes back as soon as the pointer enters its area. It raises nothing in the background, so D17 stands.
**Rejected:** keeping a floating window focused until a click (breaks FOC-1 and doesn't help once a tile is focused); re-raising floating windows after every focus change (can't go above the active app, D17).

## 2026-11-08: window rules

### D31. Window rules only float or tile, and core never sees app names (RULE-1 to RULE-4)
**What:** `[[rules]]` blocks with `app` (name or bundle id, any case) and `floating` (true/false). `wm_core::rule_effect` turns the rules and an app's bundle id and name into a `RuleEffect`; `wm-macos` reads those from `NSRunningApplication` when it starts tracking a window and sends the effect in `WindowInfo.rule`; core uses it instead of the window's kind to decide whether the window starts floating. Later matching rules win. A rule with any mistake is skipped with its line.
**Why:** the author wants Calculator and System Settings always floating, and declined rules that send apps to workspaces. Sending the verdict with the window keeps core about windows and workspaces (no app names, no "rules changed" event) while the matching stays pure and tested in core. Rules apply when a window is first seen, so saving the file doesn't rearrange open windows.
**Rejected:** workspace rules (declined by the author); matching by window title (titles are often empty when a window appears, and change); giving core the rules and app names (a core event for every config change, for no gain).

## 2026-11-08: surviving a display flap at unlock

### D30. Monitor removals wait 2 s; windows lost in a display change are re-adopted (MON-6, MON-7)
**What:** in `wm-macos`, a display snapshot that lacks a monitor core knows about is held for 2 s from when the settle timer first sees it (additions still apply after the usual 300 ms); if the monitor is back by then, the snapshot sent is the full one. Separately, a window whose AX element is destroyed within 5 s of a display change isn't reported destroyed: it's kept on a "lost" list with the last placement core asked for (a frame, or hidden) and its last known frame. A window of the same app tracked within 60 s takes over the lost window's `WindowId` (the lost window whose frame is nearest), and the last placement is applied to it again; core never hears about it. Lost windows not claimed after 60 s, or whose app quits, are reported destroyed then.
**Why:** found in daily use: on unlock, macOS reported the external monitor gone for 1.3 s, and in the same moment invalidated every window that was on it (Brave, VS Code, PyCharm); they reappeared 6–25 s later with new AX elements. Core moved the monitor's workspaces to the laptop, saw their windows "closed", dropped workspace 2, and then tiled the "new" windows on the laptop's focused workspace. The windows came back at exactly their previous frames, so the nearest frame of the same app identifies them well. Keeping this in `wm-macos` means core's model never sees the flap.
**Cost:** a window really closed within 5 s of a display change leaves an empty tile for up to 60 s. Two lost windows of one app at the same frame may swap identities (they're on the same workspace then, so it rarely matters). A real unplug takes 2 s to re-lay out.
**Rejected:** ignoring display changes while the screen is locked (macOS sends the flap right as it unlocks, and windows were invalidated anyway); matching by window title (the old element can't be read after it's invalidated, and titles change).

## 2026-11-08: staying on an emptied workspace

### D29. An app activation caused by the previous app quitting doesn't switch workspaces (FOC-8)
**What:** when macOS activates an app whose focused window is on a hidden workspace, `wm-macos` holds the activation for up to 150 ms (`focus_guard::HeldActivation`). If the previously active app terminates in that time, or already has, core hears `FocusLeft` and stays where it is with nothing focused. Otherwise the activation is reported as usual after the wait and FOC-7 switches workspaces. Activations whose window is already on screen are never held.
**Why:** found in daily use: closing PyCharm's only window on workspace 4 made PyCharm quit, macOS activated VS Code, whose window was on hidden workspace 2, and FOC-7 switched the monitor to 2, so workspace 4 vanished. The author wants to stay on the empty workspace, as in Hyprland. macOS activates the next app because the active one quit, but the "terminated" notice can arrive after the activation (measured: Calculator's came 10 ms later), hence the short wait.
**Cost:** Cmd+Tab or a Dock click to a window on a hidden workspace switches about 150 ms later. After a quit, macOS still has the other app active, so typing goes to its hidden window until the author clicks or switches (accepted by the author).
**Rejected:** "no key press or click in the last second" (the click or ⌘Q that closed the app is itself recent input); "the previous app has no windows left" (its windows are reported gone after the activation when it quits with windows open); a keyboard event tap to recognise ⌘Tab (sees every keystroke).

## 2026-11-08: configuration file

### D26. The config is parsed in core, with `toml`'s own parser; the file is the whole truth
**What:** `wm_core::config::parse_config(text)` turns the file's text into `Config { bindings, gaps }` plus `ConfigError { line, message }`s. It uses `toml::de::DeTable::parse` (the `toml` crate with only its `std` and `parse` features), which keeps the position of every key and value, so each error names its line. A bad line is skipped and reported; invalid TOML (including the exact same key twice) or a file with no valid binding is unusable, and the previous config stays (the defaults at startup). The same combo spelled two ways is reported and the first one wins. Gaps outside 0–100 use their default. The built-in defaults are the repo's `config/mi-wm.toml` compiled in with `include_str!`, so "no file" and "the shipped file" are the same thing. `KeyCombo` now carries explicit `Modifiers { cmd, alt, ctrl, shift }`, `Key` covers letters, digits, named keys and punctuation, and `Action::Launch` takes an `App` (`Browser`, `Bundle(id)`, `Named(name)`).
**Why:** parsing is pure logic and belongs in core with unit tests on any OS. `DeTable` gives spans everywhere without serde: deserializing into serde types loses spans as soon as a section's shape isn't fixed. A file that's the whole truth (as in Hyprland and AeroSpace) means removing a line unbinds it, with no hidden merge. Keeping the previous config on an unusable file is what keeps a typo from leaving the author with no keybindings.
**Rejected:** serde derive (approved by the author, but not needed, and it would drop spans); merging the file over the defaults (a removed line would silently come back); a lenient pre-parser to catch the exact duplicate key per line (another dependency or a hand-written TOML scanner for a mistake the banner already pins to a line).

### D27. Reload: FSEvents on the config directory, debounced, skipped when the bytes are the same
**What:** `wm-macos` watches `~/.config/mi-wm/` (created if missing) with an FSEvents stream with file-level events, delivered on the main dispatch queue. Every event re-arms a 200 ms run-loop timer; when it fires, the file is read and, if its bytes differ from the last load, parsed and applied: hotkeys are unregistered and registered again only when the bindings changed, `Event::GapsChanged` goes to core only when the gaps changed, and the banner and dot are updated. At startup the config is loaded and its gaps queued before the first `MonitorsChanged`, so the first layout already uses them.
**Why:** editors save either by writing in place or by writing a new file and renaming it over the old one; a watch on the directory with file events catches both (verified on device both ways). The timer turns an editor's several writes into one reload, and the byte comparison makes an autosave that changes nothing free.
**Rejected:** a GCD vnode source on the directory (sees renames but not in-place writes); a vnode source on the file (loses the file on the first atomic save); polling.

### D28. Errors: the log always, a non-activating banner, a dot, and `--check-config`
**What:** every load writes a `config:` summary line and one line per error to the log. While the config has errors (the file's, plus bindings macOS refused to register), a borderless non-activating `NSPanel` at status-window level shows them in red under the menu bar of the main display, shown with `orderFrontRegardless` and never made key. Clicking it hides it until the error set changes; a red dot after the menu bar row stays while errors exist and brings the banner back. A `new-window` app that isn't installed shows for 5 seconds without the dot. `mi-wm --check-config [path]` parses a file exactly as the WM would and exits 1 on errors, without the lock, Accessibility or a running WM. What the banner says is a pure `BannerState` in `wm-macos`.
**Why:** the author wanted Hyprland's error bar, and a log line for configuring with an LLM that can't see the screen; the command lets that LLM check a file before saving it into place. Non-activating keeps popups open (FOC-5, BAR-5). Dismissal is there because a full-width bar would cover the top of tiled windows.
**Rejected:** an alert or notification (activates mi-wm or needs permission); a banner that can't be dismissed (Hyprland's; covers windows here).

## 2026-11-08: workspaces in the menu bar

### D25. Core derives the menu bar row; one status item draws it with a custom view
**What:** `Wm::workspace_row()` lists every workspace with its state (focused, visible on another monitor, hidden). `wm_core::run` executes `Command::ShowWorkspaces(row)` after an event's own commands whenever `Wm::take_row_change()` reports a change. `wm-macos` shows it in one `NSStatusItem` whose button holds a `define_class!` `NSView` subclass (`MiWmRowView`) that draws in `drawRect:` and turns clicks into `Action::GoToWorkspace(n)`. Box layout and hit testing are a pure module (`wm-macos/src/bar.rs`) shared by drawing and clicking.
**Why:** the row is a state derivation, so it belongs in core with unit tests. Reporting it from `run` rather than appending it to `handle`'s output keeps the ~80 tests that compare `handle`'s output exactly untouched; `wm-macos` sees the same order. Drawing in the view resolves colours against the menu bar's own appearance (light/dark follows the wallpaper), and needs no new crate.
**Rejected:** one status item per workspace (items sit apart and can be reordered by ⌘-drag); an `NSImage` drawing handler (needs `block2` as a new dependency); pre-rendered images (resolve colours against the app's appearance, not the menu bar's); `wm-macos` querying `Wm` getters (logic outside core).

## 2026-11-08: running mi-wm every day

### D20. Builds are signed with a local self-signed certificate
**What:** `./wm.sh install` creates a self-signed certificate `mi-wm local signing` (Code Signing usage) in the login keychain once, with `/usr/bin/openssl` (LibreSSL) and `security import -T /usr/bin/codesign`, and signs every build with it and the identifier `io.github.lucasconesafever.mi-wm`.
**Why:** macOS keys the Accessibility grant on the code signature. A linker (ad-hoc) signature is identified by the binary's hash, so every rebuild loses the grant. Spike (2026-11-08, run outside kitty as a one-shot launchd job): the designated requirement becomes `identifier ... and certificate leaf = H"..."`; after granting once, a rebuilt, re-signed binary copied to the same path was still trusted with no prompt. `codesign` signs with the certificate untrusted (`CSSMERR_TP_NOT_TRUSTED`), and with `-T /usr/bin/codesign` no keychain prompt appeared on the first or later signings.
**Rejected:** ad-hoc signing (grant lost on every rebuild); an Apple Developer ID (paid; left for Homebrew distribution later); an `.app` bundle (not needed for the grant to stick).

### D21. launchd restarts the WM only after a failure; restart = bootout + bootstrap
**What:** the LaunchAgent has `KeepAlive { SuccessfulExit: false }` and `ThrottleInterval 10`. `./wm.sh` stops with `launchctl bootout` and starts with `bootstrap` (retried briefly); `restart` and `install` do both.
**Why:** a crash (non-zero exit, signal) comes back within 10 s; a deliberate stop (SIGTERM → hidden windows restored → exit 0) stays stopped. `bootout` is documented to send SIGTERM and wait; reloading also picks up a changed plist.
**Rejected:** `launchctl kickstart -k` (signal undocumented, reported as SIGKILL, which would skip restoring windows; and it reuses the plist launchd already holds); `KeepAlive true` (a stop would be undone immediately).

### D22. The WM writes its own log file
**What:** `--log-file <path>` makes the WM open the file and `dup2` it over stderr, so `wm_log!` lines and panic messages land there. It rotates at 10 MB (checked at open and every 1,000 lines), keeping one `.1` backup. Timestamps are local time from `localtime_r`. `dup2`/`localtime_r` are hand-written externs (as in D15).
**Why:** launchd's `StandardErrorPath` can't be rotated from inside the process and grows forever; owning the file keeps the log bounded and readable with no new dependency.
**Rejected:** launchd's output paths alone; `newsyslog` configuration (needs root); a logging crate (new dependency for one file).

### D23. One WM at a time via a lock file, taken before the Accessibility check
**What:** `File::try_lock` on `~/Library/Application Support/mi-wm/wm.lock`; if it's held, log "already running" and exit 0.
**Why:** two WMs fight over every window. The OS releases the lock when the process dies, so a crash leaves nothing stale. Taking it first means a second copy never shows a permission prompt. Exit 0 so launchd doesn't loop on it.
**Rejected:** a PID file (stale after a crash, needs liveness checks).

### D24. The Accessibility wait asks a fresh process
**What:** while waiting for the permission, the WM runs `<itself> --check-accessibility` every 2 s and continues when that exits 0.
**Why:** measured during acceptance: `AXIsProcessTrusted` keeps returning the answer from the process's first check, so a polling WM never saw the grant; a new process sees it at once.
**Rejected:** exiting and letting launchd restart it (prompts again on every start, and a restart every 10 s).

## 2026-11-08: v1 `wm-macos` design

"Spike" means measured on the author's Mac with the programs in `crates/wm-macos/examples/`.

### D8. `wm-macos` runs on one thread, driven by the run loop
**What:** every event source is a run-loop source on the main thread. Callbacks push into an `Rc<RefCell<VecDeque<Event>>>`. `next_event` runs the run loop until the queue has something, and `execute` calls macOS directly.
**Why:** AX and Carbon callbacks come through a run loop anyway. AX handles aren't `Send`, so a single thread avoids `unsafe impl Send`, `Arc`/`Mutex` and channels. Hung apps are handled by a 50 ms AX timeout, not by threads.
**Rejected:** a run-loop thread plus a core thread connected by a channel.

### D9. Hit testing uses Accessibility, not the window server list
**What:** `AXUIElementCopyElementAtPosition`, then walk up to the window, then look it up in our table.
**Why:** spike: the window server's front-to-back list put invisible overlays first (the cursor's window, JankyBorders). AX skipped them, at the same cost (~2 ms).
**Rejected:** `CGWindowListCopyWindowInfo` as the hit-test source.

### D10. Classification leans toward "popup"; menus are always `Other`
**What:** only `AXStandardWindow` (resizable → Normal, fixed → Floating) and dialog/floating subroles are managed. `AXUnknown`, failed reads and anything else are `Popup`. A pointer over any menu or menu item is `Other`, whatever window AX says it's in.
**Why:** spike: extension popups, print panels and app menus are `AXUnknown`. A web `<select>` dropdown reports the page's window, so a role check on the element is needed. A wrongly-ignored window costs less than a closed popup (the project's reason to exist).

### D11. Hidden windows go to the bottom corner with the least overlap, across all displays
**What:** candidates are both bottom corners of every display, excluding any whose parked window would sit horizontally over another display; pick the one where the parked window overlaps screens least, preferring the window's own display.
**Why:** spike: macOS allows 1 px off the sides but keeps ~32 px at the bottom and refuses the top. Choosing geometrically works for the author's home and office layouts without asking for a monitor arrangement.
**Rejected:** always the window's own display (AeroSpace), which needs every display to have a free corner.
**Amended during implementation:** the horizontal-span rule was added after macOS pulled a wide window parked at the built-in display's bottom-right fully onto the external display next to it.

### D12. Focus = activate the app, then set main and raise the window
**What:** `AXFrontmost` on the app, then `AXMain` + `AXRaise` on the window. `activateWithOptions` is the fallback.
**Why:** spike: 20/20 across 6 apps. Raising first lost to Brave restoring its own last window (18/20).

### D13. `wm-macos` assigns its own `WindowId`s
**What:** a counter, plus a table matched with `CFEqual`.
**Why:** the only route from an AX element to a `CGWindowID` is the private `_AXUIElementGetWindow`.

### D14. Hotkeys via Carbon `RegisterEventHotKey`
**What:** register only the keymap's combos; find `+`/`-` on the current layout with `UCKeyTranslate`.
**Why:** it can't intercept normal typing and needs no permission.
**Rejected:** an active keyboard event tap (sees every keystroke, can delay typing).

### D15. Restore hidden windows on Ctrl+C with a GCD signal source and hand-written externs
**What:** `dispatch_source` for `SIGINT`/`SIGTERM` on the main queue, declared with `extern "C"`.
**Why:** it runs as a normal main-thread callback (a raw `signal()` handler can't safely stop the run loop), and libSystem is always linked, so no new dependency.
**Rejected:** the `ctrlc`, `libc` and `dispatch2` crates.

### D16. `MacPlatform`'s shared state lives for the whole process
**What:** `MacPlatform::new` leaks one `Rc` reference (`std::mem::forget`), so the state the OS callbacks point at is never freed.
**Why:** timers, AX observers, Carbon hotkeys, the event tap and the GCD signal sources all hold a raw pointer to it and are never unregistered. Without the leak, dropping `MacPlatform` (possible from safe code) would let a later callback use freed memory.
**Rejected:** a `Drop` that unregisters everything: much more code, and the WM creates `MacPlatform` once and exits right after dropping it.

### D17. Callbacks wake the WM loop by posting an AppKit event
**What:** `Shared::wake` posts one application-defined `NSEvent` to ourselves; `next_event`'s AppKit wait returns on it.
**Why:** measured: AppKit keeps waiting for an *event* even after a run-loop source fires and `CFRunLoopStop` is called, so pointer moves were handled up to 1 s late. With the posted event: 6–32 ms.
**Rejected:** stopping the run loop (doesn't end AppKit's wait); a shorter pump timeout (still laggy, and wakes the CPU constantly).

### D18. Window moves are reported once the window has been still for 300 ms
**What:** move/resize notifications restart a per-window 300 ms timer; when it fires, the frame is compared with the last frame we set (2-point tolerance) and reported only if it differs.
**Why:** found in acceptance: a 250 ms "anything right after our SetFrame is an echo" rule swallowed the final position of a drag whenever the release landed in that window, so the window wasn't snapped back. Settling reports the resting position every time and avoids fighting the user mid-drag.
**Rejected:** frame-only comparison on every notification (a drag would trigger dozens of snap-backs a second while the mouse still holds the window).

### D19. Monitors are identified by port (author)
**What:** an external display's `MonitorId` is `port-<PortID>` from the IOKit registry; the built-in display, or any display without a `PortID`, falls back to the display UUID.
**Why:** at the office the author plugs different monitors into the same cables (HDMI and USB-C). Workspaces should belong to the cable, not to whichever monitor is on it. Spike: each port reports its own stable `PortID`.
**Rejected:** identity by monitor UUID (the first design; doesn't survive a monitor swap); by position in the display arrangement (AeroSpace; an unfamiliar monitor may be placed elsewhere).
**Risk:** `PortID` is an undocumented registry key read with public IOKit calls. If it disappears, the fallback is identity by monitor.

## 2026-11-07: v1 core design

### D1. `wm-core` is a pure state machine (author)
**What:** `Wm::handle(Event) -> Vec<Command>`. Core never calls the OS. The `Platform` trait has two methods, `next_event` and `execute`.
**Why:** every requirement becomes a unit test that runs on Linux, and bugs can be replayed from an event log.
**Rejected:** core calling methods on a platform trait directly (e.g. `platform.move_window(..)`). That works too, but the tests would need a stateful fake to assert against, and logic leaks into the fake.

### D2. Pointer hit testing happens in `wm-macos`
**What:** `PointerMoved` carries `target: Window(id) | Other`, computed by `wm-macos` from the real z-order.
**Why:** menus and popovers can overlap other windows. If core hit-tested against its own rectangles, it would "see" the window underneath a menu and focus it, closing the menu. That's the AeroSpace bug this project exists to fix.
**Rejected:** core-side hit testing using tracked rectangles.

### D3. Focus is only emitted when it changes
**What:** core never emits `Focus(x)` when `x` is already focused. Pointer motion that doesn't cross into a different window, or crosses into `Other` or a popup, produces no commands.
**Why:** re-focusing an already-focused window is what closed popups in AeroSpace.

### D4. Popups stay open only while the pointer stays in their app's focused window (author)
**What:** crossing into a different window changes focus, and the popup may close. The old scenarios 7 and 8 (popup survives crossing to the terminal) are dropped.
**Why:** the real problem was popups closing on small movements *within* the same app. Crossing to another window and losing the popup is expected.

### D5. Dwindle tree as a recursive enum with `Box`
**What:** `enum Node { Window(WindowId), Split(Box<Split>) }`. Each split stores its axis and ratio.
**Why:** it's the fundamentals-level way to write a tree in Rust, and plenty fast for a dozen windows per workspace.
**Rejected:** an arena (`Vec<Node>` plus indices), which is faster but adds bookkeeping the project doesn't need.

### D6. Split axis is fixed at creation (`preserve_split = true`)
**What:** a split's axis changes only through ToggleSplit.
**Why:** this matches the author's `hyprland.conf`.

### D7. A workspace's current monitor is derived, not stored
**What:** a workspace lives on its `home` monitor if that's connected, otherwise on the main monitor.
**Why:** MON-3 and MON-4 fall out automatically, with no field to keep in sync.

### D8. MON-5 re-homes the workspace
**What:** a workspace that's visible on another monitor when its home reconnects stays put, and its `home` becomes that monitor.
**Why:** "stays where it is" then holds permanently, instead of the workspace jumping away later.
**Rejected:** keeping the old home and moving the workspace back when it stops being visible, which would be surprising.

### D9. One invariant covers WS-5, WS-6 and WS-7
**What:** after every event, delete workspaces that are empty and not visible.
**Why:** one rule that's easy to test, instead of removal logic scattered across every action.

### D10. `MonitorsChanged` is a full snapshot
**What:** `wm-macos` sends the complete monitor list after display changes settle, and core diffs it.
**Why:** macOS display callbacks are noisy and arrive in partial steps. A snapshot is simpler to get right than separate connect and disconnect events.

### D11. Modifier is Ctrl+Option (author)
*Since D26 (2026-11-08) this is the default file's choice: a binding can use any modifiers.*
**Why:** Cmd collides with app shortcuts (Cmd+Q, Cmd+1–9, Cmd+Space). Plain Option collides with the Spanish layout's @ # | (Option+2/3/1).
**Rejected:** Cmd; Option; a Caps Lock hyper key via Karabiner.

### D12. Move-to-workspace follows the window (author)
**Why:** this matches the author's Hyprland `movetoworkspace`.

### D13. OS-driven focus on a hidden workspace switches to it (author, Q1)
**What:** if Cmd+`, Cmd+Tab or a Dock click focuses a window on a hidden workspace, the WM shows that workspace. The pointer warps to the window if it isn't already over it.
**Why:** this is Hyprland-like. The warp stops the next small mouse movement from stealing focus back.
**Consequence:** activating an app whose most recent window is on a hidden workspace switches to that workspace. Scenario 11 is reworded accordingly.

### D14. v1 launch apps are Terminal.app, Chrome (falling back to Safari) and Finder, as new windows (author, Q2, Q4)
*Superseded in part by D26 (2026-11-08): any app can be bound with `new-window`, and the default file's browser key opens the system's default browser instead of "Chrome, else Safari".*

### D15. v1 extra bindings: swap, resize, toggle split, close
**Why:** the author relies on these in Hyprland and asked for resizing and window-moving options. The scratchpad and pseudo-tiling are left out.

### D16. Minimized, hidden and fullscreen windows forget their workspace (author)
**What:** `wm-macos` reports them as destroyed, and their return as a new window, which lands in the focused workspace.
**Why:** WM users rarely minimize. Remembering the workspace would add a third window state, two events, and special cases in the emptiness and placement rules.
**Rejected:** remembering the workspace and restoring the window there.

### D17. Floating windows aren't kept on top
**Why:** keeping floats above tiled windows means raising other apps' windows behind the scenes. On macOS that can activate those apps, which closes popups, the very thing the project exists to prevent.
**Rejected:** Hyprland-style always-on-top floats, via a separate `Raise` command.

### D18. `FocusLeft` event
**What:** when macOS focus moves to something unmanaged, core clears its focused window. The next crossing re-focuses.
**Why:** otherwise `focused_window` goes stale, and focus-follows-mouse would silently do nothing over that window. This is popup-safe, because a nudge inside the same window isn't a crossing.
