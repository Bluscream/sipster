#!/usr/bin/env bash
# ==============================================================================
# Sipster Build & Automation Script
#
# Flags supported:
#   --lint            Run cargo clippy and enforce file length checks (<= 1000 lines)
#   --compile         Compile the workspace in release mode
#   --test            Run workspace tests including sipster-tests crate
#   --appimage        Package sipster-ui into a standalone Linux AppImage
#   --deploy          Deploy compiled binary and desktop entry to ~/.local
#   --start           Launch sipster-ui
#   --commit [MSG]    Stage changes and commit with an optional message
#   --push            Push committed changes to upstream git remote
#   --release <VER>   Create a GitHub release (via gh release create) with AppImage asset
#   --flow <MODE>     Run bundled workflows:
#                       check  -> lint + compile + test + start
#                       local  -> lint + test + compile + appimage + deploy (no push/release)
#                       remote -> lint + test + compile + appimage + deploy + commit + push + release <VER>
#   --help, -h        Display this help text
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${WORKSPACE_ROOT}"

# Ensure Linuxbrew / local pkg-config paths are active on Bazzite
export PKG_CONFIG_PATH="/var/home/linuxbrew/.linuxbrew/lib/pkgconfig:${PKG_CONFIG_PATH:-}"

# Colors for terminal output
BOLD="\033[1m"
GREEN="\033[0;32m"
YELLOW="\033[0;33m"
RED="\033[0;31m"
BLUE="\033[0;34m"
RESET="\033[0m"

log_info() {
    echo -e "${BLUE}${BOLD}[INFO]${RESET} $1"
}

log_success() {
    echo -e "${GREEN}${BOLD}[SUCCESS]${RESET} $1"
}

log_warn() {
    echo -e "${YELLOW}${BOLD}[WARN]${RESET} $1"
}

log_error() {
    echo -e "${RED}${BOLD}[ERROR]${RESET} $1"
}

show_help() {
    cat <<EOF
${BOLD}Sipster Build & Automation Tool${RESET}

Usage: ./scripts/build.sh [OPTIONS]

Individual Actions:
  --lint             Run cargo clippy across workspace and check file length (<= 1000 lines)
  --compile          Build workspace in release mode (--release)
  --test             Run all tests (sipster-core, sipster-ui, sipster-tests)
  --appimage         Create standalone AppImage bundle
  --deploy           Install binary and desktop shortcut to ~/.local
  --start            Run sipster-ui directly
  --commit [MSG]     Commit changes to git (auto-stages tracked/new non-ignored files)
  --push             Push commits to git origin
  --release <VER>    Publish GitHub release for <VER> (attaching AppImage via gh CLI)

Bundled Workflows:
  --flow check       Runs: lint -> compile -> test -> start
  --flow local       Runs: lint -> test -> compile -> appimage -> deploy (local only, no push/release)
  --flow remote [V]  Runs: lint -> test -> compile -> appimage -> deploy -> commit -> push -> release [VER]

General:
  --help, -h         Show this usage menu
EOF
}

check_file_lengths() {
    log_info "Verifying file lengths (<= 1000 lines)..."
    local exceeded=0
    while IFS= read -r file; do
        [[ "$file" =~ ^(\./)?(target|\.references|\.git) ]] && continue
        if [[ -f "$file" && "$file" =~ \.(rs|toml|md|sh)$ ]]; then
            lines=$(wc -l < "$file")
            if [ "$lines" -gt 1000 ]; then
                log_error "File exceeds 1000 lines ($lines lines): $file"
                exceeded=1
            fi
        fi
    done < <(find . -type f)

    if [ "$exceeded" -ne 0 ]; then
        log_error "File length check failed! Files must not exceed 1000 lines."
        exit 1
    fi
    log_success "File length checks passed."
}

do_lint() {
    log_info "Running Cargo Clippy on workspace..."
    check_file_lengths
    cargo clippy --workspace --all-targets -- -D warnings
    log_success "Clippy lints passed without warnings."
}

do_compile() {
    log_info "Compiling Sipster in release mode..."
    cargo build --workspace --release
    log_success "Compilation finished successfully."
}

do_test() {
    log_info "Running test suite (including sipster-tests)..."
    cargo test --workspace
    log_success "All tests passed successfully."
}

do_appimage() {
    log_info "Packaging Sipster into AppImage..."
    do_compile

    local APPDIR="${WORKSPACE_ROOT}/target/AppDir"
    rm -rf "${APPDIR}"
    mkdir -p "${APPDIR}/usr/bin"
    mkdir -p "${APPDIR}/usr/share/icons/hicolor/256x256/apps"
    mkdir -p "${APPDIR}/usr/share/applications"

    cp "${WORKSPACE_ROOT}/target/release/sipster-ui" "${APPDIR}/usr/bin/sipster"

    cat <<'EOF' > "${APPDIR}/AppRun"
#!/bin/sh
HERE="$(dirname "$(readlink -f "${0}")")"
exec "${HERE}/usr/bin/sipster" "$@"
EOF
    chmod +x "${APPDIR}/AppRun"

    cat <<'EOF' > "${APPDIR}/sipster.desktop"
[Desktop Entry]
Name=Sipster
Comment=Modern Pure-Rust Softphone & SIP Client
Exec=sipster
Icon=sipster
Type=Application
Categories=Network;Telephony;InstantMessaging;
Terminal=false
EOF
    cp "${APPDIR}/sipster.desktop" "${APPDIR}/usr/share/applications/"

    if [ ! -f "${APPDIR}/sipster.png" ]; then
        echo "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==" | base64 -d > "${APPDIR}/sipster.png"
        cp "${APPDIR}/sipster.png" "${APPDIR}/usr/share/icons/hicolor/256x256/apps/sipster.png"
    fi

    local OUTPUT_APPIMAGE="${WORKSPACE_ROOT}/target/Sipster-x86_64.AppImage"
    ARCH=x86_64 appimagetool "${APPDIR}" "${OUTPUT_APPIMAGE}"
    log_success "AppImage created at: ${OUTPUT_APPIMAGE}"
}

do_deploy() {
    log_info "Deploying Sipster locally to ~/.local..."
    do_compile

    local BIN_DIR="${HOME}/.local/bin"
    local APP_DIR="${HOME}/.local/share/applications"
    mkdir -p "${BIN_DIR}" "${APP_DIR}"

    cp "${WORKSPACE_ROOT}/target/release/sipster-ui" "${BIN_DIR}/sipster"
    chmod +x "${BIN_DIR}/sipster"

    cat <<EOF > "${APP_DIR}/sipster.desktop"
[Desktop Entry]
Name=Sipster
Comment=Modern Pure-Rust Softphone & SIP Client
Exec=${BIN_DIR}/sipster
Icon=call-start
Type=Application
Categories=Network;Telephony;
Terminal=false
EOF

    log_success "Sipster deployed to ${BIN_DIR}/sipster and ${APP_DIR}/sipster.desktop"
}

do_start() {
    log_info "Starting Sipster UI..."
    cargo run -p sipster-ui
}

do_commit() {
    local msg="${1:-"Update sipster"}"
    log_info "Committing changes with message: '$msg'..."
    git add -A
    if git diff-index --quiet HEAD --; then
        log_warn "No changes to commit."
    else
        git commit -m "$msg"
        log_success "Committed successfully."
    fi
}

do_push() {
    log_info "Pushing to remote origin..."
    local branch
    branch=$(git rev-parse --abbrev-ref HEAD)
    git push origin "$branch"
    log_success "Pushed branch $branch to origin."
}

do_release() {
    local version="$1"
    if [ -z "$version" ]; then
        log_error "Missing version argument for --release (e.g. --release v0.1.0)"
        exit 1
    fi

    log_info "Creating GitHub release ${version}..."
    local APPIMAGE="${WORKSPACE_ROOT}/target/Sipster-x86_64.AppImage"
    if [ ! -f "$APPIMAGE" ]; then
        log_info "AppImage not found, building now..."
        do_appimage
    fi

    if ! command -v gh &>/dev/null; then
        log_error "GitHub CLI ('gh') is not installed. Please install 'gh' to publish releases."
        exit 1
    fi

    gh release create "${version}" "${APPIMAGE}" \
        --title "Sipster ${version}" \
        --generate-notes
    log_success "GitHub release ${version} published successfully!"
}

# Parse command line flags
if [ $# -eq 0 ]; then
    show_help
    exit 0
fi

while [ $# -gt 0 ]; do
    case "$1" in
        --lint)
            do_lint
            shift
            ;;
        --compile)
            do_compile
            shift
            ;;
        --test)
            do_test
            shift
            ;;
        --appimage)
            do_appimage
            shift
            ;;
        --deploy)
            do_deploy
            shift
            ;;
        --start)
            do_start
            shift
            ;;
        --commit)
            shift
            commit_msg="Update"
            if [ $# -gt 0 ] && [[ ! "$1" =~ ^-- ]]; then
                commit_msg="$1"
                shift
            fi
            do_commit "$commit_msg"
            ;;
        --push)
            do_push
            shift
            ;;
        --release)
            shift
            if [ $# -eq 0 ] || [[ "$1" =~ ^-- ]]; then
                log_error "--release requires a version string (e.g. --release v0.1.0)"
                exit 1
            fi
            version="$1"
            shift
            do_release "$version"
            ;;
        --flow)
            shift
            if [ $# -eq 0 ] || [[ "$1" =~ ^-- ]]; then
                log_error "--flow requires a mode: 'check', 'local', or 'remote'"
                exit 1
            fi
            flow_mode="$1"
            shift

            case "$flow_mode" in
                check)
                    log_info "Executing flow: check (lint -> compile -> test -> start)..."
                    do_lint
                    do_compile
                    do_test
                    do_start
                    ;;
                local)
                    log_info "Executing flow: local (lint -> test -> compile -> appimage -> deploy)..."
                    do_lint
                    do_test
                    do_compile
                    do_appimage
                    do_deploy
                    log_success "Local flow completed successfully!"
                    ;;
                remote)
                    release_ver=""
                    if [ $# -gt 0 ] && [[ ! "$1" =~ ^-- ]]; then
                        release_ver="$1"
                        shift
                    fi
                    if [ -z "$release_ver" ]; then
                        log_error "--flow remote requires a release version (e.g. --flow remote v0.1.0)"
                        exit 1
                    fi
                    log_info "Executing flow: remote (lint -> test -> compile -> appimage -> deploy -> commit -> push -> release ${release_ver})..."
                    do_lint
                    do_test
                    do_compile
                    do_appimage
                    do_deploy
                    do_commit "Release ${release_ver}"
                    do_push
                    do_release "$release_ver"
                    log_success "Remote flow completed successfully!"
                    ;;
                *)
                    log_error "Unknown flow mode: $flow_mode. Available: check, local, remote"
                    exit 1
                    ;;
            esac
            ;;
        --help|-h)
            show_help
            exit 0
            ;;
        *)
            log_error "Unknown flag: $1"
            show_help
            exit 1
            ;;
    esac
done
