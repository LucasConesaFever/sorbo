# Requirements — v1

The project is named **sorbo** (formerly mi-wm; renamed 2026-11-08, NAME-1).

A tiling window manager for macOS, inspired by Hyprland, built for personal daily use.

## Motivation

AeroSpace is the current tool, and two problems drive this project:

1. Connecting a monitor creates an extra workspace (e.g. "11") that has no keybinding and is not reachable.
2. With focus-follows-mouse enabled, floating windows and popups close when the mouse moves over another window.

Neither happens with Hyprland on Linux. The goal is to get Hyprland-like behavior on macOS.

## Glossary

| Term | Meaning |
| --- | --- |
| **Window** | A single window belonging to an application. |
| **Workspace** | A numbered, WM-managed group of windows (emulated; not a native macOS Space). |
| **Visible workspace** | The workspace currently shown on a given monitor. Each monitor has exactly one. |
| **Focused workspace** | The visible workspace on the monitor that currently has focus. |
| **Assigned monitor** | The monitor a workspace belongs to ("home" monitor). |
| **Floating window** | A window not managed by the tiling layout. |
| **Popup** | Transient UI owned by an app: menus, panels, popovers, extension popups, dialogs, sheets. |

## Constraints

- **C1.** Public macOS APIs only. Workspaces are emulated. SIP must stay enabled.
- **C2.** `wm-core` is pure Rust with no macOS dependencies and is testable on Linux.
- **C3.** `wm-macos` is the only crate that talks to the OS. It is accessed through a platform trait, with a fake implementation for tests.

## Scope

**In scope for v1:** default behavior with hardcoded defaults (layout and keybindings).

**Out of scope for v1:** a separate status bar, animations. *(Workspaces in Apple's menu bar, BAR-1 to BAR-6, and the configuration file, CFG-1 to CFG-10, were added 2026-11-08.)*

## Functional requirements

### Workspaces

- **WS-1.** Workspaces are numbered. Keybindings give direct access to workspaces 1–10.
- **WS-2.** When a new workspace is needed, it gets the lowest free number.
- **WS-3.** If workspaces 1–10 all exist, the next one gets 11 (and so on), even though it has no direct keybinding in v1.
- **WS-4.** "Next / previous workspace on the current monitor" keybindings make every workspace reachable, including those above 10.
- **WS-5.** An empty workspace is removed when it stops being visible on its assigned monitor.
- **WS-6.** A visible workspace is never removed, even if it is empty.
- **WS-7.** Moving focus to another monitor (by mouse or keyboard) does not count as leaving a workspace and never removes one.
- **WS-8.** Moving a window to a workspace that doesn't exist creates that workspace on the monitor the window was on.

### Monitors

- **MON-1.** A newly connected monitor is detected automatically and assigned a workspace following WS-2 and WS-3.
- **MON-2.** Monitors are identified by the **port** they're plugged into, not by the monitor itself, its position or connection order *(author, changed 2026-11-08)*. A different monitor on the same port is the same monitor to the WM (hot desks: different monitors on the same cables). The same monitor moved to another port counts as a different monitor. The built-in display is always the same monitor. If macOS doesn't report a port, the monitor's own identity is used instead.
- **MON-3.** When a monitor is disconnected, its workspaces move to the main monitor and keep their windows.
- **MON-4.** Workspaces remember their assigned monitor. When that monitor reconnects, its workspaces return to it.
- **MON-5.** Exception to MON-4: a workspace that is visible on another monitor at the moment of reconnection stays where it is.
- **MON-6.** A monitor that disappears and comes back within 2 seconds (macOS does this when the screen wakes or unlocks) changes nothing: its workspaces stay where they are *(author, added 2026-11-08)*.
- **MON-7.** Windows that macOS rebuilds during a display change (they vanish and the same app shows them again) keep their workspace and place instead of arriving as new windows *(author, added 2026-11-08)*.

### Focus

- **FOC-1.** Focus follows the mouse: focus changes when the pointer crosses into another window's border.
- **FOC-2.** Moving the mouse within the focused window never changes focus.
- **FOC-3.** When focus changes via keyboard, the pointer warps to the center of the newly focused window.
- **FOC-4.** A pointer warp (FOC-3) must not trigger a focus change (no feedback loop between FOC-1 and FOC-3).
- **FOC-5.** Moving the pointer within the focused window, or over any popup, must not close popups. The WM emits no command for such movement: no re-focus, no raise. Crossing into a *different* window changes focus as normal, and the popup is allowed to close.
- **FOC-6.** Some popups are closed by macOS itself when their app deactivates. That's accepted when focus moves to another window (FOC-5). FOC-5 is verified manually against Brave and Chrome extension popups (scenarios 7 and 8).
- **FOC-7.** When macOS moves focus by itself (Cmd+`, Cmd+Tab, Dock click) to a window on a non-visible workspace, the WM switches to that workspace (Hyprland-like). If the pointer isn't over the newly focused window, it warps to its center.
- **FOC-8.** When the active app quits and macOS activates another app in its place, the WM doesn't follow it to a hidden workspace: it stays on the current workspace, empty if it is, with no window focused (Hyprland-like) *(author, added 2026-11-08)*. Cmd+Tab, Cmd+` and Dock clicks still follow FOC-7.
- **FOC-9.** The area a floating window covers on a visible workspace belongs to that window, as if floating windows were always above tiled ones (Hyprland-like). The pointer crossing into that area focuses the floating window and brings it to the front, even when a tiled window had covered it. Popups and the pointer's own window, if it's floating, still decide as before *(author reported floating windows unreachable behind tiles, 2026-11-08)*.
- **FOC-10.** Moving the pointer into a monitor whose visible workspace is empty makes that monitor the focused one, with no window focused, so a new window opens there and the menu bar shows that workspace as focused (Hyprland-like). A monitor with windows still waits for the pointer to cross into one (FOC-1). Moving within a monitor, or out to its menu bar and back, isn't entering it *(author, added 2026-11-10)*.

### Windows and applications

- **WIN-1.** Every window is managed independently, including several windows of the same application. Windows of the same app may live in different workspaces.
- **WIN-2.** The "launch app" keybinding opens a **new window** of the application instead of focusing the existing one: a new window of the running process, like Cmd+N, not a new process via `open -n`. If the app isn't running, it's started.
- **WIN-5.** A minimized, hidden (Cmd+H) or native-fullscreen window leaves the layout and forgets its workspace. When it comes back, it's treated as a new window (WIN-3).
- **WIN-3.** A newly created window is placed in the focused workspace, regardless of where the application opened it.
- **WIN-4.** Activating an app (Dock click, Cmd+Tab) must not make windows from non-visible workspaces appear on screen.

### Layout

- **LAY-1.** Default layout is Hyprland's dwindle: each new window splits the focused window in two along its longer side, forming a binary tree.
- **LAY-2.** Closing a window gives its space back to its sibling in the tree.
- **LAY-3.** A window shown in a tile gets the tile's size, even when it comes from another display *(added 2026-11-12, D48)*.
- **LAY-4.** A tiled window that answers its tile with the same other frame three times in a row (an app with a minimum size larger than the tile) floats where it is instead of being resized again. A window dragged around doesn't count *(author, added 2026-11-12, D50)*.

### Keybindings (v1, hardcoded)

- **KEY-1.** Go to workspace 1–10.
- **KEY-2.** Move focused window to workspace 1–10.
- **KEY-3.** Next / previous workspace on current monitor (WS-4).
- **KEY-4.** Move focus left / right / up / down.
- **KEY-5.** Toggle floating for the focused window.
- **KEY-6.** Open a new window of: Terminal.app, the default browser, Finder. *(Was "Chrome, Safari if Chrome isn't installed" until 2026-11-08; any app can now be bound, CFG-3.)*
- **KEY-7.** The default keybindings, i.e. those of the shipped `config/sorbo.toml` (CFG-1). They follow the author's `hyprland.conf`, with Ctrl+Option (⌃⌥) in place of Super:

  | Keys | Action |
  |---|---|
  | ⌃⌥ 1…9, 0 | Go to workspace 1–10 |
  | ⌃⌥⇧ 1…9, 0 | Move focused window to workspace 1–10, and follow it |
  | ⌃⌥ Tab / ⌃⌥⇧ Tab | Next / previous workspace on current monitor |
  | ⌃⌥ arrows | Move focus (crosses monitors) |
  | ⌃⌥⇧ arrows | Swap window (moves to the adjacent monitor at the edge) |
  | ⌃⌥ + / ⌃⌥ − | Grow / shrink focused window's split |
  | ⌃⌥ R | Resize mode: arrows or H J K L move the dividers, Escape or Return leaves (MODE-1) |
  | ⌃⌥ J | Toggle split direction |
  | ⌃⌥ V | Toggle floating |
  | ⌃⌥ Q | Close focused window |
  | ⌃⌥ Return | New Terminal window |
  | ⌃⌥ B | New window of the default browser |
  | ⌃⌥ E | New Finder window |

- **KEY-8.** Swap the focused window with its neighbor in a direction. At a monitor edge, move it to the adjacent monitor.
- **KEY-9.** Grow / shrink the focused window's split.
- **KEY-10.** Toggle the split direction of the focused window's pair.
- **KEY-11.** Close the focused window.

### Menu bar *(added 2026-11-08)*

- **BAR-1.** The menu bar shows a row with every existing workspace, in number order.
- **BAR-2.** The focused workspace, a workspace visible on another monitor, and a hidden workspace look different (accent-colour box, grey box, dim number) *(author)*.
- **BAR-3.** The row updates as soon as workspaces, focus or monitors change.
- **BAR-4.** Clicking a number does what ⌃⌥ + that number does, including for workspaces above 10 *(author)*.
- **BAR-5.** Updating the row never changes which app is frontmost or focused.
- **BAR-6.** The row adapts to light and dark menu bars.

### Configuration *(added 2026-11-08)*

- **CFG-1.** Keybindings, launch apps and gaps come from `~/.config/sorbo/sorbo.toml`. Without the file, the defaults (today's behaviour) apply.
- **CFG-2.** A binding can use any combination of Cmd, Option, Ctrl and Shift (at least one of Cmd/Option/Ctrl) and any key in the documented list.
- **CFG-3.** A binding can open a new window of any installed app, or of the default browser.
- **CFG-4.** Saving the file applies it within about a second, without restarting the WM *(author)*.
- **CFG-5.** A line with a mistake is skipped and reported; every other line still applies. A file that isn't valid TOML, or has no valid binding, is reported and the last working config stays in effect.
- **CFG-6.** Errors show in a banner at the top of the main screen until fixed or dismissed; a dismissed error leaves a dot in the menu bar row *(author: Hyprland-style banner)*.
- **CFG-7.** Every config load and every error is written to the log with a `config:` prefix *(author)*.
- **CFG-8.** `sorbo --check-config [path]` reports the file's errors and exits non-zero if there are any.
- **CFG-9.** Reloading or showing errors never changes which app is frontmost or closes popups.
- **CFG-10.** The repo ships a default file, installed on first install and never overwritten; personal files stay out of the repo *(author)*.

### Running *(added 2026-11-08)*

- **RUN-1.** One script installs, updates, starts, stops, restarts, follows the log of, and uninstalls the WM. Installing and updating are the same command. It works for anyone with Rust on macOS, not only the author.
- **RUN-2.** The WM starts at login.
- **RUN-3.** The WM restarts automatically after a crash, at most once every 10 seconds. A deliberate stop is not a crash.
- **RUN-4.** The Accessibility permission is granted once and survives every later update.
- **RUN-5.** The WM logs to a file with readable local timestamps. Crash messages land in the same file. The log never grows beyond about 20 MB.
- **RUN-6.** Only one WM runs at a time. A second copy says so and exits without disturbing the first.
- **RUN-7.** A stop (deliberate or before an update) brings hidden windows back on screen first.
- **RUN-8.** After a restart, windows that were visible stay on their monitor's workspace. Windows from hidden workspaces land on the main monitor's visible workspace *(author: manual re-sort accepted)*.
- **RUN-9.** Without the Accessibility permission the WM asks once and waits for it, without restarting or asking again.

## Acceptance scenarios

1. One workspace open, connect a second monitor → the new monitor shows workspace 2.
2. Workspaces 1–10 exist, connect a monitor → it shows workspace 11, reachable with next/previous workspace.
3. External monitor showing workspaces 2 and 3 is disconnected → both move to the main monitor with their windows.
4. Same monitor reconnected → workspaces 2 and 3 return to it, unless one is visible on the main monitor at that moment, in which case that one stays.
5. Switch away from an empty workspace on its assigned monitor → it is removed.
6. Empty workspace visible on a monitor, focus moves to another monitor → it is not removed.
7. Brave with an extension popup open, pointer moves around within the Brave window and over the popup → the popup stays open.
8. Same as 7 in Chrome.
9. Pointer moves within the focused window → focus does not change.
10. Focus changed by keyboard → pointer warps to the new window's center and no extra focus change happens.
11. Two windows of the same browser on workspaces 1 and 2 → each stays in its workspace. A window never appears on a workspace it doesn't belong to. If activating the app focuses the window on the hidden workspace, the WM switches to that workspace (FOC-7).
12. "New terminal" keybinding while a terminal is already open → a second terminal window opens in the focused workspace.

### Window rules *(added 2026-11-08)*

- **RULE-1.** A rule in `sorbo.toml` matches an app by name or bundle id, ignoring case.
- **RULE-2.** A rule makes the app's windows start floating (`floating = true`) or tiled (`false`), both new windows and those open when the WM starts *(author: Calculator and System Settings float)*.
- **RULE-3.** A rule with a mistake is skipped and reported with its line; the rest of the file applies.
- **RULE-4.** Rules never apply to popups, and saving the file doesn't change windows that are already open.

### Resize mode *(added 2026-11-08)*

- **MODE-1.** A binding (`resize-mode`, default ⌃⌥R) turns the resize mode on, and off again.
- **MODE-2.** In the mode, the arrow keys and `h j k l` move the nearest divider on that axis around the focused tiled window one step in the key's direction.
- **MODE-3.** Escape, Enter, or any other sorbo binding leaves the mode; another binding also runs.
- **MODE-4.** The menu bar shows when the mode is on.
- **MODE-5.** Outside the mode the arrow keys, `h j k l`, Escape and Enter are never taken from apps.
- **MODE-6.** Entering, using and leaving the mode never changes which app is frontmost or closes popups.

### Focus border *(added 2026-11-08)*

- **BORDER-1.** `./wm.sh install` offers to install JankyBorders when it's missing and Homebrew is there; it never installs it without a yes.
- **BORDER-2.** The install creates `~/.config/borders/bordersrc` from the repo's default only if it doesn't exist, and starts `borders` if it isn't running.
- **BORDER-3.** Nothing about the border can make the sorbo install fail.

### Name *(added 2026-11-08)*

- **NAME-1.** The project is named sorbo. Its binary, menu bar, messages and files use the name: `~/.local/bin/sorbo`, `~/.config/sorbo/sorbo.toml`, `~/Library/Logs/sorbo/`.
- **NAME-2.** *Retired 2026-11-10 (D45).* Installing sorbo over mi-wm stopped mi-wm and removed its login entry and binary.
- **NAME-3.** *Retired 2026-11-10 (D45).* The install moved mi-wm's config and logs over to sorbo's.
- **NAME-4.** *Retired 2026-11-10 (D45).* The install said what was left of mi-wm to remove by hand.

### Untrusted app data *(added 2026-11-08)*

- **SAFE-1.** A window frame reported by an app is used only if its four numbers are finite and its width and height are positive. A window without a usable frame isn't moved and doesn't own the pointer until a usable frame arrives.
- **SAFE-2.** A floating window's frame is never larger than its monitor.
- **SAFE-3.** sorbo uses a window frame reported by an app only when the window server shows a window of that app within 2 points of it. An unconfirmed frame is checked again 300 ms later, up to three times, and after every move or resize. *(Kept only if the spike in the spec confirms it works without new permissions.)*
- **SAFE-4.** sorbo tracks at most 100 windows per app, and logs when an app goes over.
- **SAFE-5.** Looking for an app's ⌘N menu item reads at most 40 menus of 100 items each and gives up after 250 ms.
- **SAFE-6.** `new-window` acts only on a running app that is the installed app for its bundle id; other apps claiming the id are ignored.
- **SAFE-7.** A `new-window` app name containing `/` is a config error.
- **SAFE-8.** Window ids never wrap.
- **SAFE-9.** sorbo forgets an app's fullscreen windows when the app quits, and lists each fullscreen window once.
- **SAFE-10.** The single-instance lock is taken before the log file is opened. Every 1000 lines, sorbo reopens `wm.log` if it no longer writes to it, and rotates it past 10 MB. A failed reopen is retried at the next check.

### Sharp edges *(added 2026-11-08)*

- **HARD-1.** `./wm.sh install` signs sorbo with the hardened runtime, and its signing key can't be used without the user's yes: macOS asks once per install. An existing `sorbo local signing` certificate is replaced once by `sorbo signing`, an existing `mi-wm local signing` certificate is removed, and the install says that macOS will ask again for Accessibility. *(Each part is kept only if the spike in the spec confirms it.)*
- **HARD-2.** Focus moving to another app's popup leaves sorbo with no focused window. A popup-classified window is classified again when it gets focus, and 200 ms, 1 s and 5 s after it appears. `close` acts only on the frontmost app's focused window.
- **HARD-3.** A window counts as hidden only once it has been moved off screen. A hide that fails is retried, with growing gaps up to every 5 s, until it succeeds or the window gets another command.
- **HARD-4.** Integers written as `0x…`, `0o…` or `0b…` in `sorbo.toml` are read with their base.
- **HARD-5.** Finding what's under the pointer takes at most about 30 ms of Accessibility calls per pointer move (plus at most one call's 50 ms timeout). An app that makes it overrun, or doesn't answer at all, is skipped (treated as "not a window") for 2 s, without messaging it.
- **HARD-6.** A callback that arrives while sorbo is busy never aborts it and is never lost: timers try again shortly, and all other work (shortcuts, quit, display changes, app and Accessibility notices, clicks) runs once sorbo is free. A panic inside a callback is logged and sorbo keeps running (a timer whose work panicked fires again a second later); if sorbo stops because of a panic, hidden windows are still brought back.
- **HARD-7.** Window frames reach core only as usable frames, workspace numbers can't be 0, and a held activation names which pid is which. These are enforced by types, not by remembering to check. A window found at startup before its frame is confirmed still starts on the monitor the app reports it on.

## Resolved questions

- **Q1.** Cmd+` to a window in a non-visible workspace → switch to that workspace (FOC-7).
- **Q2.** WIN-2 → new window of the running process.
- **Q3.** Keybindings → KEY-7 table.
- **Q4.** Launch apps → Terminal.app, Chrome/Safari, Finder (KEY-6).
