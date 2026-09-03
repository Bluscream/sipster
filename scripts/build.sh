#!/usr/bin/env bash
# ==============================================================================
# Sipster Build & Automation Script
#
# Flags supported:
#   --lint        Run cargo clippy and enforce file length checks (<= 1000 lines)
#   --compile     Compile the workspace in release mode
#   --test        Run workspace tests including sipster-tests crate
#   --appimage    Package sipster-ui into a standalone Linux AppImage
#   --deploy      Deploy compiled binary and desktop entry to ~/.local
#   --start       Launch sipster-ui
#   --commit      Stage changes and commit with an optional message
#   --push        Push committed changes to upstream git remote
#   --release     Run full pipeline: lint -> test -> compile -> appimage
#   --help, -h    Display this help text
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

Options:
  --lint         Run cargo clippy across workspace and check file length (<= 1000 lines)
  --compile      Build workspace in release mode (--release)
  --test         Run all tests (sipster-core, sipster-ui, sipster-tests)
  --appimage     Create standalone AppImage bundle
  --deploy       Install binary and desktop shortcut to ~/.local/bin and ~/.local/share/applications
  --start        Run sipster-ui directly
  --commit [MSG] Commit changes to git (auto-stages tracked/new non-ignored files)
  --push         Push commits to git origin
  --release      Execute full release flow: lint -> test -> compile -> appimage
  --help, -h     Show this usage menu
EOF
}

check_file_lengths() {
    log_info "Verifying file lengths (<= 1000 lines)..."
    local exceeded=0
    while IFS= read -r file; do
        # Skip target, references, git
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

    # AppRun
    cat <<'EOF' > "${APPDIR}/AppRun"
#!/bin/sh
HERE="$(dirname "$(readlink -f "${0}")")"
exec "${HERE}/usr/bin/sipster" "$@"
EOF
    chmod +x "${APPDIR}/AppRun"

    # Desktop entry
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

    # Minimal PNG icon placeholder if not existing
    if [ ! -f "${APPDIR}/sipster.png" ]; then
        # Create 1x1 transparent/colored icon or touch
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
    log_info "Executing full release workflow..."
    do_lint
    do_test
    do_compile
    do_appimage
    log_success "Release build completed successfully!"
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
            do_release
            shift
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
