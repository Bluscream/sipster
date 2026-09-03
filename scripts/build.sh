#!/usr/bin/env bash
# ==============================================================================
# Sipster Build & Automation Script
# ==============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"
export PKG_CONFIG_PATH="/var/home/linuxbrew/.linuxbrew/lib/pkgconfig:${PKG_CONFIG_PATH:-}"

# Throttle Cargo to ~80% of CPU cores so the system stays responsive
TOTAL_CORES=$(nproc 2>/dev/null || echo 4)
export CARGO_BUILD_JOBS=$(( TOTAL_CORES * 80 / 100 ))
[ "${CARGO_BUILD_JOBS}" -lt 1 ] && export CARGO_BUILD_JOBS=1

C_BOLD="\033[1m"; C_GREEN="\033[0;32m"; C_RED="\033[0;31m"; C_BLUE="\033[0;34m"; C_CYAN="\033[0;36m"; C_RESET="\033[0m"

flows_text=$(cat <<'EOF'
RECIPES:
  • Dev Check:  ./scripts/build.sh --lint --compile --test --start 5
  • Local Pack: ./scripts/build.sh --lint --test --compile --appimage --deploy
  • Release:    ./scripts/build.sh --lint --test --compile --appimage --deploy --commit "v0.1.0" --push --release v0.1.0
EOF
)

log_i() { echo -e "${C_BLUE}${C_BOLD}[INFO]${C_RESET} $1"; }
log_s() { echo -e "${C_GREEN}${C_BOLD}[OK]${C_RESET} $1"; }
log_e() { echo -e "${C_RED}${C_BOLD}[ERR]${C_RESET} $1"; }

show_help() {
    cat <<EOF
${C_BOLD}Sipster Build & Automation${C_RESET}
Usage: ./scripts/build.sh [OPTIONS...] (options execute in sequence)

Options:
  --lint           Run clippy and check file length (<= 1000 lines)
  --compile        Build workspace in release mode
  --test           Run all tests across workspace
  --appimage       Bundle standalone AppImage
  --deploy         Install to ~/.local/bin and applications
  --start [SEC]    Run UI (or smoke test for SEC seconds with diagnostics)
  --commit [MSG]   Stage and commit changes (default: "Update")
  --push           Push branch to origin
  --release <VER>  Create GitHub release via gh CLI and attach AppImage
  --help, -h       Show this help

${C_CYAN}${flows_text}${C_RESET}
EOF
}

check_lengths() {
    log_i "Checking file lengths (<= 1000 lines)..."
    while IFS= read -r f; do
        [[ "$f" =~ ^(\./)?(target|\.references|\.git) ]] && continue
        if [[ -f "$f" && "$f" =~ \.(rs|toml|md|sh)$ ]] && [ "$(wc -l < "$f")" -gt 1000 ]; then
            log_e "File exceeds 1000 lines: $f"; exit 1
        fi
    done < <(find . -type f)
    log_s "File lengths ok."
}

do_lint()     { log_i "Linting..."; check_lengths; cargo clippy --workspace --all-targets -- -D warnings; log_s "Clippy clean."; }
do_compile()  { log_i "Compiling release..."; cargo build --workspace --release; log_s "Compiled."; }
do_test()     { log_i "Testing..."; cargo test --workspace; log_s "Tests passed."; }

do_appimage() {
    log_i "Packaging AppImage..."
    do_compile
    local AD="${ROOT}/target/AppDir"
    rm -rf "${AD}" && mkdir -p "${AD}/usr/bin" "${AD}/usr/share/applications" "${AD}/usr/share/icons/hicolor/256x256/apps"
    cp "${ROOT}/target/release/sipster-ui" "${AD}/usr/bin/sipster"

    cat <<'EOF' > "${AD}/AppRun"
#!/bin/sh
exec "$(dirname "$(readlink -f "$0")")/usr/bin/sipster" "$@"
EOF
    chmod +x "${AD}/AppRun"

    cat <<'EOF' > "${AD}/sipster.desktop"
[Desktop Entry]
Name=Sipster
Comment=Modern Pure-Rust Softphone & SIP Client
Exec=sipster
Icon=sipster
Type=Application
Categories=Network;Telephony;
Terminal=false
EOF
    cp "${AD}/sipster.desktop" "${AD}/usr/share/applications/"
    echo "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==" | base64 -d > "${AD}/sipster.png"
    cp "${AD}/sipster.png" "${AD}/usr/share/icons/hicolor/256x256/apps/"

    ARCH=x86_64 appimagetool "${AD}" "${ROOT}/target/sipster-x86_64.AppImage"
    log_s "AppImage created at target/sipster-x86_64.AppImage"
}

do_deploy() {
    log_i "Deploying locally..."
    do_compile
    local B="${HOME}/.local/bin" A="${HOME}/.local/share/applications"
    mkdir -p "$B" "$A"
    cp "${ROOT}/target/release/sipster-ui" "${B}/sipster" && chmod +x "${B}/sipster"
    cat <<EOF > "${A}/sipster.desktop"
[Desktop Entry]
Name=Sipster
Comment=Modern Pure-Rust Softphone
Exec=${B}/sipster
Icon=call-start
Type=Application
Categories=Network;Telephony;
Terminal=false
EOF
    log_s "Deployed to ${B}/sipster"
}

do_start() {
    local dur="${1:-0}"
    local LOG="${ROOT}/target/logs/sipster_run.log"
    mkdir -p "$(dirname "$LOG")"

    if [ "$dur" -eq 0 ]; then
        log_i "Starting UI interactively..."; cargo run -p sipster-ui; return 0
    fi

    log_i "Smoke testing UI for ${dur}s..."
    cargo build -p sipster-ui
    "${ROOT}/target/debug/sipster-ui" > "$LOG" 2>&1 &
    local PID=$! elapsed=0 crashed=0

    while [ "$elapsed" -lt "$dur" ]; do
        if ! kill -0 "$PID" 2>/dev/null; then crashed=1; break; fi
        sleep 1; elapsed=$((elapsed + 1))
    done

    echo -e "\n=== PROCESS DIAGNOSTICS ==="
    if [ "$crashed" -eq 1 ]; then
        wait "$PID" || code=$?
        log_e "Crashed with exit code ${code:-unknown}"; tail -n 20 "$LOG"; exit 1
    fi

    ps -p "$PID" -o pid,vsz,rss,%cpu,%mem,comm 2>/dev/null || true
    kill -TERM "$PID" 2>/dev/null || true; sleep 0.5; kill -KILL "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    log_s "Process exited cleanly after ${dur}s."
    echo "--- Last 15 log lines ($LOG) ---"; tail -n 15 "$LOG"
}

do_commit() {
    local msg="${1:-"Update"}"
    log_i "Committing ('$msg')..."
    git add -A
    git diff-index --quiet HEAD -- && log_s "Nothing to commit." || (git commit -m "$msg" && log_s "Committed.")
}

do_push() {
    local b; b=$(git rev-parse --abbrev-ref HEAD)
    log_i "Pushing $b..."; git push origin "$b"; log_s "Pushed."
}

do_release() {
    local ver="${1:-}"; [ -z "$ver" ] && { log_e "--release requires version"; echo "$flows_text"; exit 1; }
    local img="${ROOT}/target/sipster-x86_64.AppImage"
    [ -f "$img" ] || do_appimage
    command -v gh >/dev/null || { log_e "'gh' CLI required."; exit 1; }
    gh release create "$ver" "$img" --title "Sipster $ver" --generate-notes
    log_s "Release $ver published."
}

[ $# -eq 0 ] && { show_help; exit 0; }

while [ $# -gt 0 ]; do
    case "$1" in
        --lint)     do_lint; shift ;;
        --compile)  do_compile; shift ;;
        --test)     do_test; shift ;;
        --appimage) do_appimage; shift ;;
        --deploy)   do_deploy; shift ;;
        --start)
            shift; dur=0
            [[ $# -gt 0 && "$1" =~ ^[0-9]+$ ]] && { dur="$1"; shift; }
            do_start "$dur" ;;
        --commit)
            shift; msg="Update"
            [[ $# -gt 0 && ! "$1" =~ ^-- ]] && { msg="$1"; shift; }
            do_commit "$msg" ;;
        --push)     do_push; shift ;;
        --release)
            shift; [[ $# -eq 0 || "$1" =~ ^-- ]] && { log_e "--release requires version"; exit 1; }
            do_release "$1"; shift ;;
        --help|-h)  show_help; exit 0 ;;
        *)          log_e "Unknown flag: $1"; show_help; exit 1 ;;
    esac
done
