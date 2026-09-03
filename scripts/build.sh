#!/usr/bin/env bash
set -euo pipefail

# Standardized multi-target builder for Sipster using build-box distrobox container
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

export PKG_CONFIG_ALLOW_CROSS=1

TARGET="${1:-all}"

build_target() {
    local target="$1"
    local bin_name="$2"
    local output_name="$3"

    echo "===> Building ${target} -> ${output_name}..."
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

    mkdir -p "${ROOT_DIR}/dist"
    cp "${ROOT_DIR}/target/${target}/release/${bin_name}" "${ROOT_DIR}/dist/${output_name}"
    echo "Saved ${ROOT_DIR}/dist/${output_name}"
}

case "${TARGET}" in
    x86_64-linux)
        build_target "x86_64-unknown-linux-gnu" "sipster-ui" "sipster-linux-x86_64"
        ;;
    aarch64-linux)
        build_target "aarch64-unknown-linux-gnu" "sipster-ui" "sipster-linux-aarch64"
        ;;
    i686-linux)
        build_target "i686-unknown-linux-gnu" "sipster-ui" "sipster-linux-x86"
        ;;
    armv7-linux)
        build_target "armv7-unknown-linux-gnueabihf" "sipster-ui" "sipster-linux-armv7"
        ;;
    x86_64-windows)
        build_target "x86_64-pc-windows-gnu" "sipster-ui.exe" "sipster-windows-x86_64.exe"
        ;;
    x86-windows)
        build_target "i686-pc-windows-gnu" "sipster-ui.exe" "sipster-windows-x86.exe"
        ;;
    aarch64-windows)
        build_target "aarch64-pc-windows-msvc" "sipster-ui.exe" "sipster-windows-aarch64.exe"
        ;;
    all)
        build_target "x86_64-unknown-linux-gnu" "sipster-ui" "sipster-linux-x86_64"
        build_target "aarch64-unknown-linux-gnu" "sipster-ui" "sipster-linux-aarch64"
        build_target "i686-unknown-linux-gnu" "sipster-ui" "sipster-linux-x86"
        build_target "armv7-unknown-linux-gnueabihf" "sipster-ui" "sipster-linux-armv7"
        build_target "x86_64-pc-windows-gnu" "sipster-ui.exe" "sipster-windows-x86_64.exe"
        build_target "i686-pc-windows-gnu" "sipster-ui.exe" "sipster-windows-x86.exe"
        build_target "aarch64-pc-windows-msvc" "sipster-ui.exe" "sipster-windows-aarch64.exe"
        ;;
    *)
        echo "Usage: $0 [x86_64-linux|aarch64-linux|i686-linux|armv7-linux|x86_64-windows|x86-windows|aarch64-windows|all]"
        exit 1
        ;;
esac
