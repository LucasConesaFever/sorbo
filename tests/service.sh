#!/usr/bin/env bash
# Tests for starting and stopping the launchd service in wm.sh (RUN-1, decision D49). Each
# case runs in a temporary HOME with a fake `launchctl`, so the real launchd is never
# touched. Run: bash tests/service.sh
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
failures=0

ok() { echo "ok $1"; }
fail() {
    echo "FAIL $1: $2"
    failures=$((failures + 1))
}

# A fresh HOME with a fake launchctl that appends "launchctl <args>" to $HOME/calls.log.
# The service is loaded while $HOME/loaded exists and has a process while $HOME/running does.
# Like the real one, `bootout` can return while the old process is still listed: it stays
# listed for the number of `print` calls written in $HOME/linger.
new_home() {
    local home
    home="$(mktemp -d)"
    mkdir -p "$home/bin" "$home/Library/LaunchAgents"
    : >"$home/Library/LaunchAgents/io.github.lucasconesafever.sorbo.plist"
    cat >"$home/bin/launchctl" <<'FAKE'
#!/usr/bin/env bash
echo "launchctl $*" >>"$HOME/calls.log"
case "$1" in
print)
    [[ -e "$HOME/loaded" ]] || exit 113
    if [[ -e "$HOME/stopping" ]]; then
        left="$(cat "$HOME/stopping")"
        if ((left <= 0)); then
            rm -f "$HOME/loaded" "$HOME/running" "$HOME/stopping"
            exit 113
        fi
        echo $((left - 1)) >"$HOME/stopping"
    fi
    [[ -e "$HOME/running" ]] && echo "	pid = 4242"
    exit 0
    ;;
bootout)
    if [[ -e "$HOME/linger" ]]; then
        cp "$HOME/linger" "$HOME/stopping"
    else
        rm -f "$HOME/loaded" "$HOME/running"
    fi
    ;;
bootstrap) touch "$HOME/loaded" "$HOME/running" ;;
esac
FAKE
    chmod +x "$home/bin/launchctl"
    echo "$home"
}

# run <home> <commands...>: sources wm.sh with the fake and runs the commands in a subshell;
# output goes to $home/out. Returns the commands' status.
run() {
    local home="$1"
    shift
    (
        export HOME="$home" PATH="$home/bin:$PATH"
        # shellcheck source=../wm.sh
        source "$ROOT/wm.sh"
        eval "$*"
    ) >"$home/out" 2>&1
}

case_run_1_restart_waits_for_the_old_copy_to_exit() {
    local h name="restart starts sorbo again when the old copy takes a moment to exit"
    h="$(new_home)"
    touch "$h/loaded" "$h/running"
    echo 3 >"$h/linger"
    run "$h" 'do_stop; do_start' || { fail "$name" "exit $? ($(cat "$h/out"))"; return; }
    if ! grep -q "^launchctl bootstrap" "$h/calls.log"; then
        fail "$name" "never bootstrapped ($(cat "$h/out"))"
    elif grep -q "already running" "$h/out"; then
        fail "$name" "said it was already running"
    else
        ok "$name"
    fi
}

case_run_1_restart_of_a_quick_exit() {
    local h name="restart starts sorbo again when the old copy exits at once"
    h="$(new_home)"
    touch "$h/loaded" "$h/running"
    run "$h" 'do_stop; do_start' || { fail "$name" "exit $? ($(cat "$h/out"))"; return; }
    if grep -q "sorbo started" "$h/out"; then ok "$name"; else fail "$name" "$(cat "$h/out")"; fi
}

case_run_1_restart_waits_for_the_old_copy_to_exit
case_run_1_restart_of_a_quick_exit

if ((failures > 0)); then
    echo "$failures failed"
    exit 1
fi
echo "all passed"
