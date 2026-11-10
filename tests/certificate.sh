#!/usr/bin/env bash
# Tests for the signing steps in wm.sh (HARD-1, decision D39). Each case runs in a temporary
# HOME with fake `security`, `codesign`, `openssl` and `launchctl`, so the real keychain and
# launchd are never touched. Run: bash tests/certificate.sh
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
failures=0

ok() { echo "ok $1"; }
fail() {
    echo "FAIL $1: $2"
    failures=$((failures + 1))
}

# A fresh HOME with the fakes. Every fake appends "<name> <args>" to $HOME/calls.log.
# - security logs its arguments shell-quoted (so an empty one shows as '');
#   find-certificate -c NAME succeeds when NAME is a line of $HOME/certs;
# - codesign fails when $HOME/codesign-fails exists;
# - openssl fails when $HOME/openssl-fails exists, sends SIGINT to the shell running wm.sh
#   (a Ctrl+C) when $HOME/openssl-interrupts exists, otherwise creates its -out/-keyout files.
new_home() {
    local home
    home="$(mktemp -d)"
    mkdir -p "$home/bin" "$home/tmp"
    cat >"$home/bin/security" <<'EOF'
#!/usr/bin/env bash
{ printf 'security'; printf ' %q' "$@"; echo; } >>"$HOME/calls.log"
if [[ "$1" == find-certificate ]]; then
    grep -qxF "$3" "$HOME/certs" 2>/dev/null
fi
EOF
    cat >"$home/bin/codesign" <<'EOF'
#!/usr/bin/env bash
echo "codesign $*" >>"$HOME/calls.log"
[[ ! -e "$HOME/codesign-fails" ]]
EOF
    cat >"$home/bin/openssl" <<'EOF'
#!/usr/bin/env bash
echo "openssl $*" >>"$HOME/calls.log"
[[ -e "$HOME/openssl-fails" ]] && exit 1
if [[ -e "$HOME/openssl-interrupts" ]]; then
    kill -INT "$PPID"
    sleep 1
fi
while [[ $# -gt 0 ]]; do
    case "$1" in -out | -keyout) : >"$2"; shift ;; esac
    shift
done
EOF
    cat >"$home/bin/launchctl" <<'EOF'
#!/usr/bin/env bash
echo "launchctl $*" >>"$HOME/calls.log"
EOF
    chmod +x "$home/bin/"*
    echo "$home"
}

# run <home> <commands...>: sources wm.sh with the fakes and runs the commands in a subshell;
# output goes to $home/out and CERT_CHANGED to $home/changed. Returns the commands' status.
run() {
    local home="$1"
    shift
    (
        export HOME="$home" TMPDIR="$home/tmp" SORBO_TEST_OPENSSL="$home/bin/openssl"
        export PATH="$home/bin:$PATH"
        # shellcheck source=../wm.sh
        source "$ROOT/wm.sh"
        status=0
        eval "$*" || status=$?
        echo "${CERT_CHANGED:-}" >"$HOME/changed"
        exit "$status"
    ) >"$home/out" 2>&1
}

case_import_has_no_trusted_app() {
    local h name="import trusts no app and keeps the key in the keychain"
    h="$(new_home)"
    run "$h" ensure_certificate || { fail "$name" "exit $? ($(cat "$h/out"))"; return; }
    local line
    line="$(grep "^security import" "$h/calls.log")"
    [[ -n "$line" ]] || { fail "$name" "no import"; return; }
    [[ "$line" == *" -x "* ]] || { fail "$name" "no -x: $line"; return; }
    [[ "$line" == *" -T ''"* ]] || { fail "$name" "-T not empty: $line"; return; }
    [[ "$line" != *"/usr/bin/codesign"* ]] || { fail "$name" "codesign trusted: $line"; return; }
    ok "$name"
}

case_sign_uses_runtime() {
    local h name="signing uses the hardened runtime"
    h="$(new_home)"
    run "$h" sign_binary x || { fail "$name" "exit $? ($(cat "$h/out"))"; return; }
    grep -qF -- "codesign --force --options runtime --sign sorbo signing --identifier io.github.lucasconesafever.sorbo x" "$h/calls.log" ||
        { fail "$name" "$(grep codesign "$h/calls.log")"; return; }
    grep -qF 'macOS will ask to let codesign use the "sorbo signing" key: click Allow (not Always Allow).' "$h/out" ||
        { fail "$name" "no prompt line: $(cat "$h/out")"; return; }
    ok "$name"
}

case_old_deleted_after_sign() {
    local h name="old certificate deleted after signing"
    h="$(new_home)"
    echo "sorbo local signing" >"$h/certs"
    run "$h" sign_and_retire x || { fail "$name" "exit $? ($(cat "$h/out"))"; return; }
    local sign_line delete_line
    sign_line="$(grep -n "^codesign" "$h/calls.log" | cut -d: -f1)"
    delete_line="$(grep -nF 'security delete-identity -c sorbo\ local\ signing' "$h/calls.log" | cut -d: -f1)"
    [[ -n "$delete_line" ]] || { fail "$name" "not deleted: $(cat "$h/calls.log")"; return; }
    ((sign_line < delete_line)) || { fail "$name" "deleted before signing"; return; }
    [[ "$(cat "$h/changed")" == "1" ]] || { fail "$name" "CERT_CHANGED not set"; return; }
    ok "$name"
}

case_no_old_certificate_no_change() {
    local h name="no old certificate, nothing deleted"
    h="$(new_home)"
    run "$h" sign_and_retire x || { fail "$name" "exit $? ($(cat "$h/out"))"; return; }
    ! grep -q "delete-identity" "$h/calls.log" || { fail "$name" "deleted something"; return; }
    [[ -z "$(cat "$h/changed")" ]] || { fail "$name" "CERT_CHANGED set"; return; }
    ok "$name"
}

case_denied_sign_keeps_old() {
    local h name="denied signing keeps the old certificate"
    h="$(new_home)"
    echo "sorbo local signing" >"$h/certs"
    : >"$h/codesign-fails"
    if run "$h" sign_and_retire x; then
        fail "$name" "succeeded"
        return
    fi
    ! grep -q "delete-identity" "$h/calls.log" || { fail "$name" "old deleted"; return; }
    ! grep -q "^launchctl" "$h/calls.log" || { fail "$name" "launchctl called"; return; }
    ok "$name"
}

case_install_signs_before_stopping() {
    local name="install signs before stopping the WM"
    local body
    body="$(cd "$ROOT" && bash -c 'source ./wm.sh; declare -f do_install')"
    local sign stop
    sign="$(grep -n "sign_and_retire" <<<"$body" | head -1 | cut -d: -f1)"
    stop="$(grep -n "do_stop" <<<"$body" | head -1 | cut -d: -f1)"
    [[ -n "$sign" && -n "$stop" ]] || { fail "$name" "missing steps"; return; }
    ((sign < stop)) || { fail "$name" "signs after stopping"; return; }
    ok "$name"
}

case_failed_openssl_cleans_tmp() {
    local h name="failed openssl leaves no temp folder"
    h="$(new_home)"
    : >"$h/openssl-fails"
    if run "$h" ensure_certificate; then
        fail "$name" "succeeded"
        return
    fi
    local left
    left="$(find "$h/tmp" -maxdepth 1 -name 'sorbo-cert.*')"
    [[ -z "$left" ]] || { fail "$name" "left $left"; return; }
    ! grep -q "^security import" "$h/calls.log" || { fail "$name" "imported anyway"; return; }
    ok "$name"
}

case_interrupt_cleans_tmp() {
    local h name="Ctrl+C while making the certificate leaves no temp folder"
    h="$(new_home)"
    : >"$h/openssl-interrupts"
    if run "$h" ensure_certificate; then
        fail "$name" "succeeded"
        return
    fi
    local left
    left="$(find "$h/tmp" -maxdepth 1 -name 'sorbo-cert.*')"
    [[ -z "$left" ]] || { fail "$name" "left $left"; return; }
    ok "$name"
}

case_users_openssl_is_ignored() {
    local h name="a user's OPENSSL variable is ignored"
    h="$(new_home)"
    printf '#!/usr/bin/env bash\nexit 1\n' >"$h/bin/brew-openssl"
    chmod +x "$h/bin/brew-openssl"
    OPENSSL="$h/bin/brew-openssl" run "$h" ensure_certificate ||
        { fail "$name" "exit $? ($(cat "$h/out"))"; return; }
    ok "$name"
}

case_import_has_no_trusted_app
case_sign_uses_runtime
case_old_deleted_after_sign
case_no_old_certificate_no_change
case_denied_sign_keeps_old
case_install_signs_before_stopping
case_failed_openssl_cleans_tmp
case_interrupt_cleans_tmp
case_users_openssl_is_ignored

if ((failures > 0)); then
    echo "$failures failed"
    exit 1
fi
echo "all passed"
