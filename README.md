# sorbo

A tiling window manager for macOS, inspired by Hyprland. Public macOS APIs only, no SIP changes: *sorbo* is Spanish for "sip", and SIP stays on.

## Install and run

Requirements: macOS 26 and Rust 1.89 or later (install it from <https://rustup.rs>).

```bash
git clone https://github.com/LucasConesaFever/sorbo.git
cd sorbo
./wm.sh install
```

`install` builds the WM, signs it with a local certificate (`sorbo signing`) it creates in your login keychain, installs it at `~/.local/bin/sorbo`, and sets it up to start at login.

The first time:

- Every install, macOS asks to let `codesign` use the "sorbo signing" key: click **Allow** (not Always Allow). The key can't be used without that yes, so no other program can sign itself as sorbo and take over its Accessibility permission. The build is also signed with the hardened runtime, so nothing can load code into it.
- macOS asks you to allow `sorbo` in **System Settings → Privacy & Security → Accessibility**. The WM starts by itself as soon as you allow it. If you closed that prompt, run `./wm.sh restart` to see it again. Don't add sorbo by hand with **+**: in testing (decision D39), macOS didn't trust an entry added that way, only the one its own prompt creates.

Quit any other window manager (AeroSpace, yabai, Rectangle...) first.

### Updating from an install before 2026-11-08

Older installs signed sorbo with a `sorbo local signing` certificate whose key `codesign` could use without asking. The next `./wm.sh install` creates `sorbo signing`, signs with it, and then removes the old certificate. Because the signature changes, sorbo has to be allowed in Accessibility again. Turning the old sorbo entry off and on isn't enough: remove it with −, run `./wm.sh restart`, and allow sorbo in the prompt macOS shows.

## Menu bar

While the WM runs, Apple's menu bar shows a row of workspace numbers next to the clock: the focused workspace in a box of your accent colour, a workspace showing on another monitor in a grey box, and other workspaces with windows as dim numbers. Click a number to go to it, the same as ⌃⌥ + that number (this also reaches workspaces 11 and up). The row disappears when the WM stops.

## Focus border

The coloured border around the focused window is [JankyBorders](https://github.com/FelixKratz/JankyBorders), which sorbo sets up rather than draws. `./wm.sh install` offers to install it with Homebrew if it's missing (only on a yes), creates `~/.config/borders/bordersrc` from the repo's [`config/bordersrc`](config/bordersrc) if you don't have one (never overwritten afterwards), and starts it with `brew services` if it isn't running. Edit that file to change the colours or width, then `brew services restart borders`. sorbo works the same without it.

## Resize mode

⌃⌥R turns on the resize mode, and "resize" shows before the workspace numbers. Then, without modifiers, the arrow keys (or `h` `j` `k` `l`) move the divider next to the focused window in the arrow's direction, 5% per press. Escape or Return leaves the mode, and so does any other sorbo shortcut, which also runs (⌃⌥2 leaves and goes to workspace 2). Outside the mode, apps get those keys as usual. The mode resizes tiled windows only; drag a floating window's edge to resize it.

## Configuration

Keybindings, the apps they open and the gaps between windows are set in `~/.config/sorbo/sorbo.toml`. `./wm.sh install` creates it from the repo's [`config/sorbo.toml`](config/sorbo.toml) the first time and never overwrites it afterwards. Without the file, sorbo uses that same default.

Save the file and the changes apply within a second, no restart needed. The file is the whole truth: a binding you delete is gone.

```toml
[gaps]
outer = 5   # points between windows and the screen edge (0-100)
inner = 4   # points between windows (0-100)

[bindings]
ctrl-alt-1 = "workspace 1"
ctrl-alt-shift-1 = "move-to-workspace 1"
cmd-alt-q = "close"
ctrl-alt-k = "new-window kitty"
```

- **Combos:** modifiers `cmd`, `alt` (Option), `ctrl`, `shift`, in any order, joined with `-`, then one key. At least one of `cmd`, `alt`, `ctrl`.
- **Keys:** `a`–`z`, `0`–`9`, `enter`, `tab`, `space`, `escape`, `backspace`, `delete`, `left`, `right`, `up`, `down`, `home`, `end`, `pageup`, `pagedown`, `f1`–`f12`, and punctuation by name: `plus`, `minus`, `equal`, `comma`, `period`, `slash`, `backslash`, `semicolon`, `quote`, `backtick`, `leftbracket`, `rightbracket`. Punctuation is found on your current keyboard layout (with Shift if that's the only way to type it, so `plus` works on a US keyboard too).
- **Actions:** `workspace <n|next|prev>`, `move-to-workspace <n>`, `focus <left|right|up|down>`, `swap <left|right|up|down>`, `resize <grow|shrink>`, `resize-mode`, `toggle-split`, `toggle-floating`, `close`, `new-window <app>`. The app is a bundle id (`com.apple.Terminal`), an app name as in `/Applications` (`kitty`, `Google Chrome`), or `browser` for your default browser. App names can't contain `/`. If several running apps claim the same bundle id, only the one installed under that id gets the new window.

**Window rules:** make an app's windows always start floating (or tiled):

```toml
[[rules]]
app = "Calculator"        # app name as in /Applications, or a bundle id, any case
floating = true

[[rules]]
app = "System Settings"
floating = true
```

A saved rule applies to the app's next window; `./wm.sh restart` applies it to windows already open. ⌃⌥V still toggles a window either way.

**Mistakes:** a line with a mistake is skipped and the rest applies. A file that isn't valid TOML, or that has no valid binding at all, is ignored and the previous config stays. Either way:

- a red bar at the top of the screen names the line (click it to hide it; a red dot stays after the workspace numbers, click the dot to see the bar again);
- the log gets a line starting with `config:` (`grep config: ~/Library/Logs/sorbo/wm.log`);
- `~/.local/bin/sorbo --check-config [path]` checks a file without the WM: it prints each problem and `ok: N bindings`, and exits 1 if there are problems. It can't know about combos macOS refuses (taken by another app); those only show in the bar and the log.

## Commands

| Command | What it does |
|---|---|
| `./wm.sh install` | build, sign, install, start at login, then (re)start |
| `./wm.sh restart` | stop cleanly (hidden windows come back) and start again |
| `./wm.sh stop` | stop until the next `start` or the next login |
| `./wm.sh start` | start after a `stop` |
| `./wm.sh logs` | follow the log |
| `./wm.sh uninstall` | stop it and remove the login entry and the binary |

If the WM crashes, it restarts by itself within about 10 seconds. Windows that were on a hidden workspace come back on the visible one.

## Update

```bash
git pull
./wm.sh install
```

Updates keep the Accessibility permission (except once, when the signing certificate changes; see above). Each update asks once to let `codesign` use the key: click Allow.

## Logs

`~/Library/Logs/sorbo/wm.log`, rotated at 10 MB (the previous one is kept as `wm.log.1`). Follow it with `./wm.sh logs`, or open it in Console.app.

## Uninstall

```bash
./wm.sh uninstall
```

Logs and your config file are kept. Remove the `sorbo signing` certificate (Keychain Access → login → My Certificates) and the Accessibility entry by hand if you don't plan to reinstall.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). To report a security problem, see [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
