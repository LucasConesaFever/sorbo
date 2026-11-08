#!/usr/bin/env bash
# HARD-1 spike.
# Throwaway: checks, on this Mac, what the signing fix relies on. Uses its own certificate
# ("sorbo spike signing"), its own identifier and launchd label, and its own folder, so the
# installed sorbo is never touched. Run from the repo root after `cargo build --release -p sorbo`.
#
#   bash crates/wm-macos/examples/spike_signing.sh setup     # certificate (imported without -T)
#   bash crates/wm-macos/examples/spike_signing.sh check1    # hardened binary runs under launchd
#   bash crates/wm-macos/examples/spike_signing.sh check2a   # before the grant: not trusted
#   (macOS asks to allow sorbo-spike in Accessibility: allow it, then run check2b)
#   bash crates/wm-macos/examples/spike_signing.sh check2b   # rebuilt + re-signed: still trusted
#   bash crates/wm-macos/examples/spike_signing.sh check3    # DYLD_INSERT_LIBRARIES ignored
#   bash crates/wm-macos/examples/spike_signing.sh check4    # codesign asks (twice)
#   bash crates/wm-macos/examples/spike_signing.sh check5    # export doesn't hand out the key
#   bash crates/wm-macos/examples/spike_signing.sh cleanup
set -uo pipefail

CERT="sorbo spike signing"
LABEL="io.github.lucasconesafever.sorbo-spike"
DIR="$HOME/.local/share/sorbo-spike"
BIN="$DIR/sorbo"
DOMAIN="gui/$(id -u)"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"

pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; }

sign() { # sign <runtime|plain>; may show a keychain prompt (the key has no -T)
    local opts=()
    [[ "$1" == runtime ]] && opts=(--options runtime)
    codesign --force ${opts[@]+"${opts[@]}"} --sign "$CERT" --identifier "$LABEL" "$BIN"
}

fresh_copy() {
    mkdir -p "$DIR"
    cp "$ROOT/target/release/examples/spike_ax_trust" "$BIN.new" && mv "$BIN.new" "$BIN"
}

# run_job [dylib]: runs `$BIN --check-accessibility` once under launchd (so the binary, not
# kitty, is the responsible process); prints its exit code.
run_job() {
    local env=""
    if [[ -n "${1:-}" ]]; then
        env="<key>EnvironmentVariables</key><dict><key>DYLD_INSERT_LIBRARIES</key><string>$1</string></dict>"
    fi
    cat >"$DIR/job.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>$LABEL</string>
<key>ProgramArguments</key><array><string>$BIN</string><string>${ARG:---check}</string></array>
<key>RunAtLoad</key><true/>
<key>StandardErrorPath</key><string>$DIR/job.err</string>
$env
</dict></plist>
EOF
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null
    launchctl bootstrap "$DOMAIN" "$DIR/job.plist" || { echo "bootstrap-failed"; return; }
    local i code=""
    for i in $(seq 20); do
        sleep 0.25
        code="$(launchctl print "$DOMAIN/$LABEL" 2>/dev/null | awk -F'= ' '/last exit code/ {print $2; exit}')"
        [[ -n "$code" && "$code" != "(never exited)" ]] && break
    done
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null
    echo "${code:-unknown}"
}

case "${1:-}" in
setup)
    if security find-certificate -c "$CERT" >/dev/null 2>&1; then
        echo "certificate exists"
        exit 0
    fi
    tmp="$(mktemp -d)"
    cat >"$tmp/cert.cnf" <<EOF
[req]
distinguished_name = dn
prompt = no
x509_extensions = ext
[dn]
CN = $CERT
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
    /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 30 \
        -keyout "$tmp/key.pem" -out "$tmp/cert.pem" -config "$tmp/cert.cnf" 2>/dev/null &&
        /usr/bin/openssl pkcs12 -export -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
            -out "$tmp/cert.p12" -passout pass:spike -name "$CERT" &&
        security import "$tmp/cert.p12" -k "$KEYCHAIN" -P spike -x -T "" >/dev/null
    status=$?
    rm -rf "$tmp"
    [[ $status == 0 ]] && echo "certificate created (-x -T "": non-extractable, no app trusted)" || echo "setup failed"
    ;;
check1)
    fresh_copy
    sign runtime || { fail 1 "codesign failed (prompt denied?)"; exit 1; }
    codesign --verify --strict "$BIN" && echo "verify --strict ok" || fail 1 "verify --strict"
    codesign -dv "$BIN" 2>&1 | grep -E "flags|Identifier"
    code="$(run_job)"
    echo "job exit code: $code (0 trusted, 1 not trusted; anything else means it didn't run)"
    [[ "$code" == 0 || "$code" == 1 ]] && pass 1 || fail 1 "exit $code; stderr: $(cat "$DIR/job.err" 2>/dev/null)"
    ;;
check2a)
    fresh_copy
    sign runtime || { fail 2 "codesign failed"; exit 1; }
    ARG=--prompt run_job >/dev/null
    code="$(run_job)"
    echo "exit $code: macOS should have asked to allow $BIN; allow it, then run check2b"
    ;;
check2b)
    touch "$ROOT/crates/wm-macos/examples/spike_ax_trust.rs"
    (cd "$ROOT" && cargo build --release -p wm-macos --example spike_ax_trust 2>&1 | tail -1)
    fresh_copy
    sign runtime || { fail 2 "codesign failed"; exit 1; }
    code="$(run_job)"
    [[ "$code" == 0 ]] && pass "2 (rebuilt, re-signed hardened binary still trusted)" || fail 2 "exit $code"
    ;;
check3)
    marker="$DIR/injected"
    cat >"$DIR/inject.c" <<EOF
#include <stdio.h>
__attribute__((constructor)) static void hit(void) {
    FILE *f = fopen("$marker", "w"); if (f) { fputs("injected\n", f); fclose(f); }
}
EOF
    clang -dynamiclib -o "$DIR/inject.dylib" "$DIR/inject.c" && codesign --force -s - "$DIR/inject.dylib"
    rm -f "$marker"
    sign plain >/dev/null 2>&1 || { fail 3 "codesign (plain) failed"; exit 1; }
    run_job "$DIR/inject.dylib" >/dev/null
    [[ -f "$marker" ]] && echo "control: plain signature loads the dylib" || echo "control: dylib NOT loaded even without runtime (check is inconclusive)"
    rm -f "$marker"
    sign runtime >/dev/null 2>&1 || { fail 3 "codesign (runtime) failed"; exit 1; }
    run_job "$DIR/inject.dylib" >/dev/null
    [[ -f "$marker" ]] && fail 3 "hardened binary loaded the dylib" || pass "3 (hardened binary ignored DYLD_INSERT_LIBRARIES)"
    ;;
check4)
    fresh_copy
    echo "codesign #1: macOS should ask; click Allow (not Always Allow)"
    sign runtime && echo "signed #1" || echo "codesign #1 failed/denied"
    echo "codesign #2: macOS should ask again"
    sign runtime && echo "signed #2" || echo "codesign #2 failed/denied"
    echo "PASS 4 only if a prompt appeared both times"
    ;;
check5)
    rm -f "$DIR/x.p12"
    echo "security export: a prompt or a failure is a pass; a silent export is a fail"
    security export -k "$KEYCHAIN" -t identities -f pkcs12 -P x -o "$DIR/x.p12" >/dev/null 2>&1
    echo "export exit: $?"
    if [[ -s "$DIR/x.p12" ]]; then
        echo "a file was written: only a FAIL if no prompt appeared"
    else
        pass 5
    fi
    rm -f "$DIR/x.p12"
    ;;
cleanup)
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null
    security delete-identity -c "$CERT" "$KEYCHAIN" 2>/dev/null || security delete-certificate -c "$CERT" "$KEYCHAIN"
    rm -rf "$DIR"
    echo "removed; remove the sorbo-spike entry from Accessibility by hand"
    ;;
*)
    sed -n 2,14p "$0"
    exit 2
    ;;
esac
