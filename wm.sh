#!/usr/bin/env bash
# Install, update and control sorbo.
#
#   ./wm.sh install     build, sign, install, start at login, (re)start
#   ./wm.sh restart     stop cleanly, start again
#   ./wm.sh stop        stop until the next `start` or login
#   ./wm.sh start       start after a `stop`
#   ./wm.sh logs        follow the log
#   ./wm.sh uninstall   stop, remove the login entry and the binary
set -euo pipefail

cd "$(dirname "$0")"

LABEL="io.github.lucasconesafever.sorbo"
CERT_NAME="sorbo signing"
# The certificate installs before HARD-1 used: its key let codesign sign without asking (D39).
OLD_CERT_NAME="sorbo local signing"
# macOS's own LibreSSL. A sorbo-specific name, so a user's OPENSSL (often Homebrew's
# OpenSSL 3, whose .p12 files `security` rejects) is never picked up; only
# tests/certificate.sh sets it, to a fake.
OPENSSL="${SORBO_TEST_OPENSSL:-/usr/bin/openssl}"
MI_WM_CERT_NAME="mi-wm local signing"
RETIRED=()
BIN="$HOME/.local/bin/sorbo"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
LOG_DIR="$HOME/Library/Logs/sorbo"
CONFIG="$HOME/.config/sorbo/sorbo.toml"
BORDERS_CONFIG="$HOME/.config/borders/bordersrc"
BORDERS_FORMULA="FelixKratz/formulae/borders"
DOMAIN="gui/$(id -u)"

# Where an install from before the rename to sorbo put things (NAME-2, NAME-3).
OLD_LABEL="io.github.lucasconesafever.mi-wm"
OLD_BIN="$HOME/.local/bin/mi-wm"
OLD_PLIST="$HOME/Library/LaunchAgents/$OLD_LABEL.plist"
OLD_CONFIG="$HOME/.config/mi-wm/mi-wm.toml"
OLD_LOG_DIR="$HOME/Library/Logs/mi-wm"
OLD_SUPPORT_DIR="$HOME/Library/Application Support/mi-wm"

usage() {
    sed -n '4,9p' "$0" | sed 's/^# //'
    exit 1
}

is_loaded() {
    launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1
}

# Loaded is not the same as running: a copy that exited 0 (e.g. "already running" while a
# dev build held the lock) stays loaded with no process.
is_running() {
    launchctl print "$DOMAIN/$LABEL" 2>/dev/null | grep -q '^[[:space:]]*pid = '
}

# Boots out mi-wm (the name before sorbo) if it's still loaded: its lock file differs from
# sorbo's, so nothing else would stop the two running together (NAME-2). Returns 0 if it
# stopped it, 1 if it wasn't loaded, 2 if stopping it failed.
stop_mi_wm() {
    launchctl print "$DOMAIN/$OLD_LABEL" >/dev/null 2>&1 || return 1
    if ! launchctl bootout "$DOMAIN/$OLD_LABEL"; then
        echo "Could not stop mi-wm ($OLD_LABEL)" >&2
        return 2
    fi
    echo "Stopped mi-wm"
}

do_stop() {
    # Before the first sorbo install, a running mi-wm would otherwise be left next to a dev
    # build of sorbo (`./wm.sh stop`, then `cargo run -p sorbo`).
    local status=0
    stop_mi_wm || status=$?
    if [[ $status == 2 ]]; then
        exit 1
    fi
    if is_loaded; then
        # bootout sends SIGTERM and waits: the WM puts hidden windows back and exits 0.
        launchctl bootout "$DOMAIN/$LABEL"
        echo "sorbo stopped"
    else
        echo "sorbo is not running"
    fi
}

do_start() {
    if [[ ! -f "$PLIST" ]]; then
        echo "sorbo is not installed (run ./wm.sh install)" >&2
        exit 1
    fi
    if is_running; then
        echo "sorbo is already running"
        return
    fi
    if is_loaded; then
        # Loaded but its process exited: unload so bootstrap starts it again.
        launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
    fi
    # Right after a bootout, launchd can refuse for a moment ("5: Input/output error").
    local i
    for i in $(seq 10); do
        if launchctl bootstrap "$DOMAIN" "$PLIST" 2>/dev/null; then
            echo "sorbo started"
            return
        fi
        sleep 0.5
    done
    launchctl bootstrap "$DOMAIN" "$PLIST"
}

ensure_certificate() {
    if security find-certificate -c "$CERT_NAME" >/dev/null 2>&1; then
        return
    fi
    echo "Creating the local signing certificate \"$CERT_NAME\" in your login keychain."
    local tmp status=0
    tmp="$(mktemp -d "${TMPDIR:-/tmp}/sorbo-cert.XXXXXX")" || return 1
    # Always removed, even on Ctrl+C: it holds an unencrypted copy of the key while the
    # certificate is made, and anyone with that copy could sign as sorbo without asking.
    trap 'rm -rf "$tmp"; exit 130' INT TERM
    make_certificate "$tmp" || status=$?
    trap - INT TERM
    rm -rf "$tmp"
    return "$status"
}

# make_certificate <dir>: creates the key and certificate in <dir> and imports them. Each step
# is checked by hand: `set -e` doesn't apply inside a function called from `||`.
make_certificate() {
    local tmp="$1"
    cat >"$tmp/cert.cnf" <<EOF || return 1
[req]
distinguished_name = dn
prompt = no
x509_extensions = ext
[dn]
CN = $CERT_NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
    # macOS's own LibreSSL: Homebrew's OpenSSL 3 writes PKCS#12 files `security` can't import.
    "$OPENSSL" req -x509 -newkey rsa:2048 -nodes -days 3650 \
        -keyout "$tmp/key.pem" -out "$tmp/cert.pem" -config "$tmp/cert.cnf" 2>/dev/null || return 1
    "$OPENSSL" pkcs12 -export -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
        -out "$tmp/cert.p12" -passout pass:sorbo -name "$CERT_NAME" || return 1
    # -x: the private key can never be exported. -T "": no app may use it without asking, so
    # another program can't sign its own binary as sorbo and inherit its Accessibility
    # permission (HARD-1, D39).
    security import "$tmp/cert.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
        -P sorbo -x -T "" >/dev/null || return 1
}

# sign_binary <path>: signs with the hardened runtime, so dyld ignores DYLD_* variables
# (HARD-1). macOS asks to let codesign use the key every time.
sign_binary() {
    echo 'macOS will ask to let codesign use the "sorbo signing" key: click Allow (not Always Allow).'
    codesign --force --options runtime --sign "$CERT_NAME" --identifier "$LABEL" "$1"
}

# After the first signing with the new certificate, removes the old ones: sorbo's before
# HARD-1 and mi-wm's. Their keys let codesign sign silently, and each matches an Accessibility
# grant (D39). Sets CERT_CHANGED=1 so the install can say Accessibility will ask again.
retire_old_certificate() {
    local name
    for name in "$OLD_CERT_NAME" "$MI_WM_CERT_NAME"; do
        if security find-certificate -c "$name" >/dev/null 2>&1; then
            if security delete-identity -c "$name" >/dev/null 2>&1; then
                RETIRED+=("$name")
            else
                echo "warning: could not remove the old \"$name\" certificate; remove it in Keychain Access" >&2
            fi
            CERT_CHANGED=1
        fi
    done
}

# sign_and_retire <path>: the old certificate goes only once signing with the new one worked.
sign_and_retire() {
    sign_binary "$1" || return 1
    retire_old_certificate
}

write_plist() {
    mkdir -p "$(dirname "$PLIST")" "$LOG_DIR"
    cat >"$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>$BIN</string>
        <string>--log-file</string>
        <string>$LOG_DIR/wm.log</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>StandardOutPath</key>
    <string>$LOG_DIR/launchd.log</string>
    <key>StandardErrorPath</key>
    <string>$LOG_DIR/launchd.log</string>
</dict>
</plist>
EOF
    plutil -lint -s "$PLIST"
}

# The user's config file is theirs: create it from the repo's default once, never overwrite.
install_config() {
    if [[ -e "$CONFIG" ]]; then
        echo "Kept your config file $CONFIG"
    else
        mkdir -p "$(dirname "$CONFIG")"
        cp config/sorbo.toml "$CONFIG"
        echo "Created your config file $CONFIG from config/sorbo.toml"
    fi
}

warn() {
    echo "warning: $*" >&2
}

# The focus border is JankyBorders.
# Nothing here may fail the sorbo install (BORDER-3): every command that can fail is in an
# `if` or ends in `|| warn`, so `set -e` never stops on it.
install_border() {
    if ! command -v borders >/dev/null 2>&1; then
        if ! command -v brew >/dev/null 2>&1; then
            echo "Border: JankyBorders draws a border around the focused window; it needs Homebrew (https://brew.sh)"
            return 0
        fi
        local answer=""
        # Only ask when someone can answer; never install without a yes (BORDER-1).
        if [[ -t 0 ]]; then
            read -r -p "Install JankyBorders for a border around the focused window? [y/N] " answer || answer=""
        fi
        if [[ "$answer" == [yY] ]]; then
            brew install "$BORDERS_FORMULA" || warn "installing JankyBorders failed; sorbo works without it"
        fi
        if ! command -v borders >/dev/null 2>&1; then
            echo "Border: install JankyBorders later with: brew install $BORDERS_FORMULA, then ./wm.sh install"
            return 0
        fi
    fi
    # The user's border config is theirs: create it once, never overwrite (BORDER-2).
    if [[ -e "$BORDERS_CONFIG" ]]; then
        echo "Border: kept your config $BORDERS_CONFIG"
    elif mkdir -p "$(dirname "$BORDERS_CONFIG")" && cp config/bordersrc "$BORDERS_CONFIG" \
        && chmod +x "$BORDERS_CONFIG"; then
        echo "Border: created $BORDERS_CONFIG from config/bordersrc"
    else
        warn "could not create $BORDERS_CONFIG"
    fi
    if pgrep -x borders >/dev/null 2>&1; then
        echo "Border: JankyBorders is running"
    elif command -v brew >/dev/null 2>&1; then
        brew services start borders >/dev/null || warn "could not start JankyBorders (brew services start borders)"
    else
        warn "JankyBorders isn't running; start it with: borders &"
    fi
}

# Moves an install from before the rename (mi-wm) over to sorbo, once (NAME-2, NAME-3).
# Each step does nothing when there's nothing to move. Sets MIGRATED=1 when it did
# anything. Returns 1 when a step fails, and the install stops there, before
# install_config could create a default config. The checks are explicit: bash ignores
# `set -e` inside a function called from an `if` or `||`.
migrate_from_mi_wm() {
    # First, so mi-wm and sorbo never run together: their lock files differ (NAME-2).
    local status=0
    stop_mi_wm || status=$?
    case $status in
    0) MIGRATED=1 ;;
    2) return 1 ;;
    esac
    if [[ -e "$OLD_PLIST" ]]; then
        rm -f "$OLD_PLIST"
        echo "Removed the mi-wm login entry $OLD_PLIST"
        MIGRATED=1
    fi
    if [[ -e "$OLD_BIN" ]]; then
        rm -f "$OLD_BIN"
        echo "Removed $OLD_BIN"
        MIGRATED=1
    fi
    # -L too: a dotfiles symlink counts, even a broken one, and mv moves the link itself.
    if [[ -e "$OLD_CONFIG" || -L "$OLD_CONFIG" ]]; then
        if [[ -e "$CONFIG" || -L "$CONFIG" ]]; then
            echo "Your old config $OLD_CONFIG was left in place: $CONFIG already exists"
        else
            mkdir -p "$(dirname "$CONFIG")" && mv "$OLD_CONFIG" "$CONFIG" ||
                { echo "Could not move your config $OLD_CONFIG to $CONFIG" >&2; return 1; }
            rmdir "$(dirname "$OLD_CONFIG")" 2>/dev/null || true
            echo "Moved your config $OLD_CONFIG to $CONFIG"
            MIGRATED=1
        fi
    fi
    if [[ -d "$OLD_LOG_DIR" && ! -e "$LOG_DIR" ]]; then
        mkdir -p "$(dirname "$LOG_DIR")" && mv "$OLD_LOG_DIR" "$LOG_DIR" ||
            { echo "Could not move the logs $OLD_LOG_DIR to $LOG_DIR" >&2; return 1; }
        echo "Moved the logs $OLD_LOG_DIR to $LOG_DIR"
        MIGRATED=1
    fi
    if [[ -d "$OLD_SUPPORT_DIR" ]]; then
        rm -rf "$OLD_SUPPORT_DIR"
        echo "Removed $OLD_SUPPORT_DIR"
        MIGRATED=1
    fi
}

do_install() {
    if ! command -v cargo >/dev/null 2>&1; then
        echo "cargo not found: install Rust from https://rustup.rs" >&2
        exit 1
    fi
    if pgrep -x AeroSpace >/dev/null 2>&1; then
        echo "warning: AeroSpace is running; quit it, or the two window managers will fight" >&2
    fi

    SORBO_COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)" \
        cargo build --release -p sorbo
    ensure_certificate || {
        echo "could not create the signing certificate; nothing was installed" >&2
        exit 1
    }
    sign_and_retire target/release/sorbo || {
        echo "signing failed or was denied; nothing was installed" >&2
        exit 1
    }

    migrate_from_mi_wm || exit 1
    do_stop
    mkdir -p "$(dirname "$BIN")"
    # Copy then rename: overwriting a signed binary in place can get its next launch killed.
    cp target/release/sorbo "$BIN.new"
    mv "$BIN.new" "$BIN"
    write_plist
    install_config
    install_border
    do_start

    cat <<EOF

Installed $BIN
Config: $CONFIG (saved changes apply immediately; check it with: $BIN --check-config)
Log: $LOG_DIR/wm.log (./wm.sh logs)
Commands: ./wm.sh install | restart | stop | start | logs | uninstall

The first time, macOS asks to allow sorbo in System Settings > Privacy & Security >
Accessibility. The WM starts by itself once it's allowed. Updates keep the permission.
EOF
    if [[ -n "${MIGRATED:-}" ]]; then
        cat <<EOF

mi-wm is now sorbo. macOS asks again for Accessibility, this time for sorbo, once.
Remove the old mi-wm entry in System Settings > Privacy & Security > Accessibility:
it still trusts binaries signed as mi-wm.
EOF
    elif [[ -n "${CERT_CHANGED:-}" ]]; then
        cat <<EOF

New signing certificate: allow sorbo in Accessibility again. Turning the old entry off
and on isn't enough: remove it with -, run ./wm.sh restart, and allow sorbo in the prompt.
EOF
    fi
    if ((${#RETIRED[@]} > 0)); then
        echo
        echo "Removed the old signing certificate(s): ${RETIRED[*]}."
    fi
    if [[ -z "${MIGRATED:-}" && " ${RETIRED[*]-} " == *"mi-wm"* ]]; then
        cat <<EOF
Also remove the old mi-wm entry in System Settings > Privacy & Security > Accessibility:
it still trusts binaries signed as mi-wm.
EOF
    fi
}

do_uninstall() {
    if [[ -f "$PLIST" ]] || is_loaded; then
        do_stop
    fi
    local removed=0
    if [[ -f "$PLIST" ]]; then rm "$PLIST"; removed=1; fi
    if [[ -f "$BIN" ]]; then rm "$BIN"; removed=1; fi
    if [[ $removed == 1 ]]; then
        echo "sorbo uninstalled (logs kept in $LOG_DIR, config kept in $CONFIG)"
    else
        echo "sorbo is not installed"
    fi
    cat <<EOF
To remove the rest by hand:
  - the "$CERT_NAME" certificate: Keychain Access > login > My Certificates
  - the Accessibility entry: System Settings > Privacy & Security > Accessibility
  - JankyBorders, if you no longer want the focus border:
    brew services stop borders && brew uninstall borders
EOF
}

# Sourced (tests/migration.sh): define the functions, run nothing.
[[ "${BASH_SOURCE[0]}" == "$0" ]] || return 0

case "${1:-}" in
install) do_install ;;
uninstall) do_uninstall ;;
start) do_start ;;
stop) do_stop ;;
restart)
    do_stop
    do_start
    ;;
logs) tail -F "$LOG_DIR/wm.log" ;;
*) usage ;;
esac
