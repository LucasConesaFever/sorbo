#!/usr/bin/env bash
# Tests for migrate_from_mi_wm in wm.sh (NAME-2, NAME-3). Each case runs in a temporary HOME
# with a fake launchctl first in PATH, so the real launchd and the real config are never
# touched. Run: bash tests/migration.sh
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OLD_LABEL="io.github.lucasconesafever.mi-wm"
failures=0

ok() { echo "ok $1"; }
fail() {
    echo "FAIL $1: $2"
    failures=$((failures + 1))
}

# A fresh HOME with the fake launchctl. `print <domain>/<label>` succeeds only when the label
# is a line of $HOME/loaded.
new_home() {
    local home
    home="$(mktemp -d)"
    mkdir -p "$home/bin"
    cat >"$home/bin/launchctl" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$HOME/launchctl.log"
case "$1" in
print) grep -qxF "${2##*/}" "$HOME/loaded" 2>/dev/null ;;
*) exit 0 ;;
esac
EOF
    chmod +x "$home/bin/launchctl"
    echo "$home"
}

# What an mi-wm install leaves in a home folder.
old_install() {
    local home="$1"
    mkdir -p "$home/.config/mi-wm" "$home/Library/Logs/mi-wm" \
        "$home/Library/Application Support/mi-wm" "$home/.local/bin" "$home/Library/LaunchAgents"
    echo "X" >"$home/.config/mi-wm/mi-wm.toml"
    echo "log" >"$home/Library/Logs/mi-wm/wm.log"
    : >"$home/Library/Application Support/mi-wm/wm.lock"
    : >"$home/.local/bin/mi-wm"
    : >"$home/Library/LaunchAgents/$OLD_LABEL.plist"
}

# Runs the migration in a subshell with `home` as HOME; its output goes to $home/out, and
# MIGRATED to $home/migrated. Returns the migration's exit status.
migrate() {
    local home="$1"
    (
        export HOME="$home"
        export PATH="$home/bin:$PATH"
        # shellcheck source=../wm.sh
        source "$ROOT/wm.sh"
        status=0
        migrate_from_mi_wm || status=$?
        echo "${MIGRATED:-}" >"$HOME/migrated"
        exit "$status"
    ) >"$home/out" 2>&1
}

case_old_install_moved() {
    local h name="old install moved"
    h="$(new_home)"
    old_install "$h"
    migrate "$h" || { fail "$name" "exit $? ($(cat "$h/out"))"; return; }
    [[ "$(cat "$h/.config/sorbo/sorbo.toml" 2>/dev/null)" == "X" ]] || { fail "$name" "config not moved"; return; }
    [[ ! -e "$h/.config/mi-wm" ]] || { fail "$name" "old config folder left"; return; }
    [[ -f "$h/Library/Logs/sorbo/wm.log" ]] || { fail "$name" "logs not moved"; return; }
    [[ ! -e "$h/Library/Logs/mi-wm" ]] || { fail "$name" "old logs folder left"; return; }
    [[ ! -e "$h/Library/Application Support/mi-wm" ]] || { fail "$name" "lock folder left"; return; }
    [[ ! -e "$h/.local/bin/mi-wm" ]] || { fail "$name" "old binary left"; return; }
    [[ ! -e "$h/Library/LaunchAgents/$OLD_LABEL.plist" ]] || { fail "$name" "old plist left"; return; }
    [[ "$(cat "$h/migrated")" == "1" ]] || { fail "$name" "MIGRATED not set"; return; }
    ok "$name"
}

case_loaded_agent_booted_out() {
    local h name="loaded agent is booted out"
    h="$(new_home)"
    old_install "$h"
    echo "$OLD_LABEL" >"$h/loaded"
    migrate "$h" || { fail "$name" "exit $? ($(cat "$h/out"))"; return; }
    grep -qx "bootout gui/$(id -u)/$OLD_LABEL" "$h/launchctl.log" 2>/dev/null ||
        { fail "$name" "no bootout in $(cat "$h/launchctl.log" 2>/dev/null)"; return; }
    ok "$name"
}

case_existing_sorbo_config_kept() {
    local h name="existing sorbo config kept"
    h="$(new_home)"
    old_install "$h"
    echo "OLD" >"$h/.config/mi-wm/mi-wm.toml"
    mkdir -p "$h/.config/sorbo"
    echo "NEW" >"$h/.config/sorbo/sorbo.toml"
    migrate "$h" || { fail "$name" "exit $? ($(cat "$h/out"))"; return; }
    [[ "$(cat "$h/.config/sorbo/sorbo.toml")" == "NEW" ]] || { fail "$name" "new config overwritten"; return; }
    [[ "$(cat "$h/.config/mi-wm/mi-wm.toml")" == "OLD" ]] || { fail "$name" "old config lost"; return; }
    grep -q "left in place" "$h/out" || { fail "$name" "no 'left in place' line"; return; }
    ok "$name"
}

case_symlink_moved_as_link() {
    local h name="symlink moved as a link"
    h="$(new_home)"
    old_install "$h"
    mkdir -p "$h/dotfiles"
    echo "DOT" >"$h/dotfiles/mi-wm.toml"
    rm "$h/.config/mi-wm/mi-wm.toml"
    ln -s ../../dotfiles/mi-wm.toml "$h/.config/mi-wm/mi-wm.toml"
    migrate "$h" || { fail "$name" "exit $? ($(cat "$h/out"))"; return; }
    [[ -L "$h/.config/sorbo/sorbo.toml" ]] || { fail "$name" "not a symlink"; return; }
    [[ "$(cat "$h/.config/sorbo/sorbo.toml")" == "DOT" ]] || { fail "$name" "link doesn't resolve"; return; }
    ok "$name"
}

case_failed_move_stops() {
    local h name="failed move stops"
    h="$(new_home)"
    old_install "$h"
    : >"$h/.config/sorbo" # a file where the folder should be
    if migrate "$h"; then
        fail "$name" "migration succeeded"
        return
    fi
    [[ "$(cat "$h/.config/mi-wm/mi-wm.toml")" == "X" ]] || { fail "$name" "old config lost"; return; }
    ok "$name"
}

case_second_run_silent() {
    local h name="second run is silent"
    h="$(new_home)"
    old_install "$h"
    migrate "$h" || { fail "$name" "first run: exit $? ($(cat "$h/out"))"; return; }
    migrate "$h" || { fail "$name" "second run: exit $? ($(cat "$h/out"))"; return; }
    [[ ! -s "$h/out" ]] || { fail "$name" "printed: $(cat "$h/out")"; return; }
    [[ -z "$(cat "$h/migrated")" ]] || { fail "$name" "MIGRATED set"; return; }
    ok "$name"
}

case_stop_stops_mi_wm() {
    local h name="stop also stops a loaded mi-wm"
    h="$(new_home)"
    echo "$OLD_LABEL" >"$h/loaded"
    (
        export HOME="$h"
        export PATH="$h/bin:$PATH"
        # shellcheck source=../wm.sh
        source "$ROOT/wm.sh"
        do_stop
    ) >"$h/out" 2>&1 || { fail "$name" "exit $? ($(cat "$h/out"))"; return; }
    grep -qx "bootout gui/$(id -u)/$OLD_LABEL" "$h/launchctl.log" 2>/dev/null ||
        { fail "$name" "mi-wm not booted out: $(cat "$h/launchctl.log" 2>/dev/null)"; return; }
    ok "$name"
}

case_old_install_moved
case_loaded_agent_booted_out
case_existing_sorbo_config_kept
case_symlink_moved_as_link
case_failed_move_stops
case_second_run_silent
case_stop_stops_mi_wm

if ((failures > 0)); then
    echo "$failures failed"
    exit 1
fi
