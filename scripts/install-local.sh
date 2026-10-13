#!/usr/bin/env bash
# User-local installation; no sudo. Only the three named artifacts are replaced.
set -euo pipefail

usage() {
    printf '%s\n' \
        'Usage: scripts/install-local.sh [--yes]' \
        '' \
        'Builds and installs or updates Speaches AI, its desktop launcher and bar block.' \
        'Finish recording first: gracefully quitting discards temporary audio.' \
        'Restarts i3 only after successful startup. User settings are never replaced.' \
        'Keeps one latest rollback backup under the application state directory.' \
        '' \
        '  --yes     Skip the confirmation prompt' \
        '  --help    Show this help'
}
assume_yes=false
case "${1:-}" in
    '') ;;
    --yes) assume_yes=true ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
esac
if (( $# > 1 )); then usage >&2; exit 2; fi
if (( EUID == 0 )); then
    echo 'Run this as your desktop user, not with sudo.' >&2
    exit 1
fi
for tool in cargo install cp mktemp mv i3-msg timeout; do
    command -v "$tool" >/dev/null 2>&1 || {
        printf 'Required command not found: %s\n' "$tool" >&2; exit 1;
    }
done
if [[ -z "${DISPLAY:-}" || -z "${XDG_RUNTIME_DIR:-}" ]]; then
    echo 'Run this inside your Regolith X11 desktop session.' >&2
    exit 1
fi
timeout 5 i3-msg -t get_version >/dev/null
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
block_source="$root/integration/i3xrocks/80_speaches"
desktop_source="$root/integration/desktop/speaches-ai.desktop"
bin_dir="$HOME/.local/bin"
bar_dir="$HOME/.config/regolith3/i3xrocks/conf.d"
stock_dir=/usr/share/i3xrocks/conf.d
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/speaches-ai"
applications_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
# XDG base directories must be absolute; relative values would break Exec too.
for directory in "$HOME" "$state_dir" "$applications_dir"; do
    [[ "$directory" == /* ]] || { echo 'HOME and XDG directories must be absolute.' >&2; exit 1; }
done
[[ -r "$block_source" && -r "$desktop_source" ]] || { echo 'Missing repository integration template.' >&2; exit 1; }
if [[ ! -d "$bar_dir" && ! -d "$stock_dir" ]]; then
    printf 'Stock i3xrocks configuration not found: %s\n' "$stock_dir" >&2; exit 1
fi
installed="$bin_dir/speaches-ai"
block="$bar_dir/80_speaches"
desktop="$applications_dir/speaches-ai.desktop"
mode=Install
[[ ! -e "$installed" && ! -L "$installed" ]] || mode=Update
for artifact in "$installed" "$block" "$desktop"; do
    [[ ! -d "$artifact" ]] || { printf 'Refusing to replace a directory: %s\n' "$artifact" >&2; exit 1; }
done
printf '%s\n' "$mode executable: $installed" "Install bar block: $block" \
    "Install desktop launcher: $desktop" \
    'This will gracefully quit Speaches AI (discarding temporary audio) and restart i3.'
if [[ "$assume_yes" != true ]]; then
    if ! read -r -p 'Continue? [y/N] ' answer; then exit 1; fi
    case "$answer" in y|Y|yes|YES) ;; *) echo 'Cancelled; nothing changed.'; exit 0 ;; esac
fi

# Build and stage everything before asking the existing process to quit.
cd -- "$root"
cargo build --locked --bin speaches-ai --target-dir "$root/target"
binary="$root/target/debug/speaches-ai"
[[ -x "$binary" ]] || { echo 'Native debug executable was not produced.' >&2; exit 1; }
mkdir -p -- "$bin_dir" "$state_dir" "$applications_dir" "$(dirname -- "$bar_dir")"
chmod 700 -- "$state_dir"
backup="$state_dir/rollback-latest"
staged_binary='' staged_block='' staged_desktop='' staged_stock='' pending_backup=''
transaction=false
was_running=false
committed=false
log="$state_dir/application.log"
(umask 077; touch -- "$log")
chmod 600 -- "$log"

refresh_desktop() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$applications_dir" || echo 'Warning: desktop cache refresh failed.' >&2
    fi
}
status_of() { "$1" status; }
wait_for() {
    local executable=$1 desired=$2 current attempt
    for ((attempt = 0; attempt < 50; attempt++)); do
        if current=$(status_of "$executable"); then
            if [[ "$desired" == stopped && "$current" == OFF ]] ||
               [[ "$desired" == started && -n "$current" && "$current" != OFF ]]; then return 0; fi
        fi
        sleep 0.1
    done
    return 1
}
launch() { nohup "$1" show >>"$log" 2>&1 </dev/null & }
restore() {
    local destination=$1 name=$2 staged
    if [[ -e "$backup/$name" || -L "$backup/$name" ]]; then
        staged=$(mktemp "$(dirname -- "$destination")/.restore.XXXXXX") || return
        rm -f -- "$staged"
        cp -a -- "$backup/$name" "$staged" && mv -fT -- "$staged" "$destination"
    else
        rm -f -- "$destination"
    fi
}
cleanup() {
    local result=$?
    trap - EXIT
    if [[ "$transaction" == true && "$committed" != true ]]; then
        echo 'Installation failed; restoring the previous owned files…' >&2
        # Best effort, including the case where show started but readiness failed.
        if [[ -x "$installed" ]]; then
            "$installed" quit >/dev/null 2>&1 || true
            wait_for "$installed" stopped || echo 'Warning: app did not confirm shutdown.' >&2
        fi
        restore "$installed" binary || echo 'Warning: binary restore failed.' >&2
        restore "$block" 80_speaches || echo 'Warning: bar restore failed.' >&2
        restore "$desktop" speaches-ai.desktop || echo 'Warning: desktop restore failed.' >&2
        refresh_desktop
        if [[ "$was_running" == true ]]; then
            launch "$control"
            wait_for "$control" started || printf 'Warning: previous app did not restart; see %s\n' "$log" >&2
        fi
    fi
    for temporary in "$staged_binary" "$staged_block" "$staged_desktop"; do
        [[ -z "$temporary" ]] || rm -f -- "$temporary"
    done
    for temporary in "$staged_stock" "$pending_backup"; do
        [[ -z "$temporary" ]] || rm -rf -- "$temporary"
    done
    exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
staged_binary=$(mktemp "$bin_dir/.speaches-ai.XXXXXX")
install -m 755 -- "$binary" "$staged_binary"
staged_block=$(mktemp "$(dirname -- "$bar_dir")/.speaches-block.XXXXXX")
install -m 644 -- "$block_source" "$staged_block"
staged_desktop=$(mktemp "$applications_dir/.speaches-ai.XXXXXX.desktop")
# Exec has two escaping layers: quoted argument syntax, then desktop string syntax.
# Percent is a field-code escape, even inside quotes. Never evaluate the result.
exec_path=${installed//\\/\\\\}
exec_path=${exec_path//\"/\\\"}
exec_path=${exec_path//\$/\\\$}
exec_path=${exec_path//\`/\\\`}
exec_path=${exec_path//\\/\\\\}
exec_path=${exec_path//%/%%}
exec_path=${exec_path//$'\n'/\\n}
exec_path=${exec_path//$'\r'/\\r}
exec_path=${exec_path//$'\t'/\\t}
while IFS= read -r line || [[ -n "$line" ]]; do
    if [[ "$line" == 'Exec=@EXEC@' ]]; then
        printf 'Exec="%s" show\n' "$exec_path"
    else
        printf '%s\n' "$line"
    fi
done <"$desktop_source" >"$staged_desktop"
chmod 644 -- "$staged_desktop"
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$staged_desktop"
fi
if [[ ! -d "$bar_dir" ]]; then
    staged_stock=$(mktemp -d "$(dirname -- "$bar_dir")/.stock.XXXXXX")
    cp -a -- "$stock_dir/." "$staged_stock/"
fi
# Bounded policy: keep only these three prior owned files, never entire config trees.
pending_backup=$(mktemp -d "$state_dir/.rollback.XXXXXX")
for name in binary 80_speaches speaches-ai.desktop; do
    case "$name" in binary) artifact=$installed ;; 80_speaches) artifact=$block ;; *) artifact=$desktop ;; esac
    if [[ -e "$artifact" || -L "$artifact" ]]; then cp -a -- "$artifact" "$pending_backup/$name"; fi
done
rm -rf -- "$backup"
mv -- "$pending_backup" "$backup"
pending_backup=''
printf 'Latest rollback backup: %s\n' "$backup"
control=$binary
[[ ! -x "$installed" ]] || control=$installed
status=$(status_of "$control")
transaction=true
if [[ "$status" != OFF ]]; then
    was_running=true
    echo 'Stopping the running instance…'
    "$control" quit >/dev/null
    if ! wait_for "$control" stopped; then
        # No replacement happened; do not start a second process or restore files.
        transaction=false
        echo 'The app did not stop. Quit it manually, then rerun this script.' >&2
        exit 1
    fi
fi
mv -fT -- "$staged_binary" "$installed"
staged_binary=''
if [[ -n "$staged_stock" ]]; then
    mv -T -- "$staged_stock" "$bar_dir"
    staged_stock=''
    echo 'Copied stock bar blocks into the new user configuration directory.'
fi
mv -fT -- "$staged_block" "$block"
staged_block=''
mv -fT -- "$staged_desktop" "$desktop"
staged_desktop=''
launch "$installed"
if ! wait_for "$installed" started; then
    printf 'Startup could not be confirmed. Check %s\n' "$log" >&2
    exit 1
fi
committed=true
refresh_desktop
echo 'Restarting i3 to refresh the bar…'
timeout 10 i3-msg restart
printf '\n%s complete. Open Speaches AI from your application menu or microphone bar icon.\nLog: %s\n' "$mode" "$log"
