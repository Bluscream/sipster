#!/usr/bin/env bash
# Multi-target builder for Sipster.
#
# All artifacts are named  sipster-{os}-{arch}.{ext}  and land in dist/.
# Builds are expected to run inside the `build-box` distrobox, which carries
# the cross toolchains, alsa/pkg-config and cmake (for libopus). On a bare host
# this script re-invokes itself inside that container automatically.
#
#   ./scripts/build.sh                # every target + AppImage
#   ./scripts/build.sh x86_64-linux   # one target
#   ./scripts/build.sh check          # clippy + test gate
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
DIST_DIR="${ROOT_DIR}/dist"
PKG_DIR="${ROOT_DIR}/packaging"
ASSET_DIR="${ROOT_DIR}/assets"
BOX="${SIPSTER_BOX:-build-box}"

in_container() {
    [[ -f /run/.containerenv || -f /.dockerenv || -n "${CONTAINER_ID:-}" ]]
}

if ! in_container && command -v distrobox >/dev/null 2>&1; then
    exec distrobox enter "${BOX}" -- "${SCRIPT_DIR}/build.sh" "$@"
fi

export PKG_CONFIG_ALLOW_CROSS=1

# ── output helpers ───────────────────────────────────────────────────────────
# Colours only when stdout is a terminal, so CI logs stay clean.
if [[ -t 1 ]]; then
    BOLD=$'\e[1m'; RED=$'\e[31m'; GREEN=$'\e[32m'; RESET=$'\e[0m'
else
    BOLD=''; RED=''; GREEN=''; RESET=''
fi
step() { echo "${BOLD}==>${RESET} $*"; }
ok()   { echo "${GREEN}  ok${RESET} $*"; }
die()  { echo "${RED}error:${RESET} $*" >&2; exit 1; }

usage() {
    cat <<EOF
Usage: $0 [target]

Targets:
  x86_64-linux      aarch64-linux    i686-linux      armv7-linux
  x86_64-windows    x86-windows
  aarch64-windows   currently broken, see the note in build_windows()
  appimage          Linux x86_64 AppImage (sipster-linux-x86_64.AppImage)
  linux             all Linux binaries
  windows           all working Windows binaries
  all               every working target above, including the AppImage
  check             clippy (warnings denied) + test
  clean             remove dist/

Environment:
  SIPSTER_BOX       distrobox to build in (default: build-box)

Artifacts are written to dist/ as sipster-{os}-{arch}.{ext}
EOF
}

# build_target <rust-target> <binary-name> <output-name>
build_target() {
    local target="$1" bin_name="$2" output_name="$3"

    step "Building ${target} -> ${output_name}"
    case "${target}" in
        aarch64-unknown-linux-gnu)
            PKG_CONFIG_PATH=/usr/lib/aarch64-linux-gnu/pkgconfig \
            PKG_CONFIG_LIBDIR=/usr/lib/aarch64-linux-gnu/pkgconfig \
            CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
            cargo build --release -p sipster-ui --target "${target}"
            ;;
        i686-unknown-linux-gnu)
            PKG_CONFIG_PATH=/usr/lib/i386-linux-gnu/pkgconfig \
            PKG_CONFIG_LIBDIR=/usr/lib/i386-linux-gnu/pkgconfig \
            CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_LINKER=i686-linux-gnu-gcc \
            cargo build --release -p sipster-ui --target "${target}"
            ;;
        armv7-unknown-linux-gnueabihf)
            # libopus compiles its ARM NEON intrinsics unconditionally, but the
            # bare armv7 target does not enable NEON, so every `always_inline`
            # intrinsic fails with "target specific option mismatch". armv7
            # hardfloat implies VFPv3 and, on every Cortex-A this would run on,
            # NEON as well — so ask for it explicitly rather than patching the
            # upstream codec.
            CFLAGS_armv7_unknown_linux_gnueabihf="-mfpu=neon" \
            PKG_CONFIG_PATH=/usr/lib/arm-linux-gnueabihf/pkgconfig \
            PKG_CONFIG_LIBDIR=/usr/lib/arm-linux-gnueabihf/pkgconfig \
            CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=arm-linux-gnueabihf-gcc \
            cargo build --release -p sipster-ui --target "${target}"
            ;;
        aarch64-pc-windows-msvc)
            cargo xwin build --release -p sipster-ui --target "${target}"
            ;;
        *)
            cargo build --release -p sipster-ui --target "${target}"
            ;;
    esac

    mkdir -p "${DIST_DIR}"
    cp "${ROOT_DIR}/target/${target}/release/${bin_name}" "${DIST_DIR}/${output_name}"
    ok "${DIST_DIR}/${output_name} ($(du -h "${DIST_DIR}/${output_name}" | cut -f1))"
}

# Packages the x86_64 Linux build as an AppImage.
build_appimage() {
    local target="x86_64-unknown-linux-gnu"
    local out="${DIST_DIR}/sipster-linux-x86_64.AppImage"

    command -v appimagetool >/dev/null 2>&1 || die \
        $'appimagetool not found in PATH\n  get it from https://github.com/AppImage/appimagetool/releases'

    step "Building AppImage"
    cargo build --release -p sipster-ui --target "${target}"

    local staging appdir
    staging="$(mktemp -d)"
    # Always clean up the staging tree, including on failure.
    trap 'rm -rf "${staging}"' RETURN
    appdir="${staging}/Sipster.AppDir"
    mkdir -p "${appdir}/usr/bin"

    install -m755 "${ROOT_DIR}/target/${target}/release/sipster-ui" "${appdir}/usr/bin/sipster"
    install -m755 "${PKG_DIR}/AppRun"                               "${appdir}/AppRun"
    install -m644 "${PKG_DIR}/sipster.desktop"                      "${appdir}/sipster.desktop"
    # The icon lives in assets/ with the rest of the project's imagery.
    install -m644 "${ASSET_DIR}/logo.png"                           "${appdir}/sipster.png"

    mkdir -p "${DIST_DIR}"
    # ARCH: required when appimagetool cannot infer the target arch.
    # APPIMAGE_EXTRACT_AND_RUN: appimagetool is itself an AppImage and cannot
    #   self-mount inside the container (no FUSE), so make it self-extract.
    ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 appimagetool "${appdir}" "${out}"
    ok "${out} ($(du -h "${out}" | cut -f1))"
}

build_linux() {
    build_target "x86_64-unknown-linux-gnu"        "sipster-ui" "sipster-linux-x86_64"
    build_target "aarch64-unknown-linux-gnu"       "sipster-ui" "sipster-linux-aarch64"
    build_target "i686-unknown-linux-gnu"          "sipster-ui" "sipster-linux-x86"
    build_target "armv7-unknown-linux-gnueabihf"   "sipster-ui" "sipster-linux-armv7"
}

# aarch64-pc-windows-msvc is deliberately not built here.
#
# cargo-xwin exports MSVC-style include flags (`/imsvc <dir>`) in
# CFLAGS_aarch64_pc_windows_msvc, but the `ring` build script drives plain
# `clang` rather than `clang-cl`, and clang's GNU driver reads `/imsvc` as a
# filename. cargo-xwin overwrites the variable, so it cannot be corrected from
# the outside. Until that is fixed upstream there is no aarch64 Windows
# artifact — do not add one to a release without building it first.
build_windows() {
    build_target "x86_64-pc-windows-gnu" "sipster-ui.exe" "sipster-windows-x86_64.exe"
    build_target "i686-pc-windows-gnu"   "sipster-ui.exe" "sipster-windows-x86.exe"
}

# The gate a change must pass before it is committed. Warnings are denied here
# even though a plain `cargo clippy` only warns, so "warning-free" is enforced
# rather than merely intended.
run_check() {
    step "cargo clippy (warnings denied)"
    cargo clippy --workspace --all-targets -- -D warnings
    step "cargo test"
    cargo test --workspace
    ok "workspace is clean"
}

# Accept both "appimage" and the legacy "--appimage" spelling.
TARGET="${1:-all}"
TARGET="${TARGET#--}"

case "${TARGET}" in
    x86_64-linux)     build_target "x86_64-unknown-linux-gnu"      "sipster-ui" "sipster-linux-x86_64" ;;
    aarch64-linux)    build_target "aarch64-unknown-linux-gnu"     "sipster-ui" "sipster-linux-aarch64" ;;
    i686-linux|x86-linux)
                      build_target "i686-unknown-linux-gnu"        "sipster-ui" "sipster-linux-x86" ;;
    armv7-linux)      build_target "armv7-unknown-linux-gnueabihf" "sipster-ui" "sipster-linux-armv7" ;;
    x86_64-windows)   build_target "x86_64-pc-windows-gnu"   "sipster-ui.exe" "sipster-windows-x86_64.exe" ;;
    x86-windows)      build_target "i686-pc-windows-gnu"     "sipster-ui.exe" "sipster-windows-x86.exe" ;;
    aarch64-windows)  build_target "aarch64-pc-windows-msvc" "sipster-ui.exe" "sipster-windows-aarch64.exe" ;;
    appimage)         build_appimage ;;
    linux)            build_linux ;;
    windows)          build_windows ;;
    all)              build_linux; build_windows; build_appimage ;;
    check)            run_check ;;
    clean)            step "Removing ${DIST_DIR}"; rm -rf "${DIST_DIR}"; ok "clean" ;;
    help|-h|--help)   usage ;;
    *)                usage; exit 1 ;;
esac
