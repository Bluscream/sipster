#!/usr/bin/env bash
# Multi-target builder for Sipster.
#
# All artifacts are named  sipster-{os}-{arch}.{ext}  and land in dist/.
# Builds are expected to run inside the `build-box` distrobox, which carries
# the cross toolchains, alsa/pkg-config and cmake (for libopus). On a bare host
# this script re-invokes itself inside that container automatically.
#
#   ./tools/build.sh                # every target + AppImage
#   ./tools/build.sh x86_64-linux   # one target
#   ./tools/build.sh check          # clippy + test gate
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

# Accept both "appimage" and the legacy "--appimage" spelling.
TARGET="${1:-all}"
TARGET="${TARGET#--}"

# `run` launches the GUI and `clean` unmounts host FUSE mounts; neither wants
# the build container, and unmounting from inside it would target the wrong
# mount namespace. Everything else needs the cross toolchains.
host_only_target() {
    case "$1" in
        run | clean | help | -h) return 0 ;;
        *) return 1 ;;
    esac
}

if ! host_only_target "${TARGET}" && ! in_container \
    && command -v distrobox >/dev/null 2>&1; then
    exec distrobox enter "${BOX}" -- "${SCRIPT_DIR}/build.sh" "$@"
fi

export PKG_CONFIG_ALLOW_CROSS=1

# ── being a good neighbour to games ──────────────────────────────────────────
# A release build saturates every core, which is fine on an idle desktop and
# very much not fine while a game is running: the frame times go with it. When
# one is detected the build takes half the cores and runs at a lower priority,
# trading a slower compile for a playable machine.
#
# Detection reads each process's *executable* — its comm and the target of
# /proc/PID/exe — never its command line. `pgrep -f` was the obvious way to do
# this and is wrong: the pattern lists game names, so the very shell running
# `pgrep -f "VRChat|..."` matches itself, and so does any unrelated command
# that happens to mention one. That threw a permanent false positive and
# halved every build on an idle machine.
#
# Deliberately broad about what counts: anything under Proton/Wine, anything
# launched out of a Steam library, and gamescope.
GAME_MARKERS='VRChat|gamescope|steamapps/common|wine64-preloader|wineserver|\.exe$'

game_running() {
    [[ -n "${SIPSTER_IGNORE_GAMES:-}" ]] && return 1

    local pid exe comm
    for pid in /proc/[0-9]*; do
        pid="${pid#/proc/}"
        # Our own process tree is never a game, and reading it would
        # reintroduce the self-match this replaced.
        [[ "${pid}" == "$$" || "${pid}" == "${PPID}" ]] && continue

        comm="$(cat "/proc/${pid}/comm" 2>/dev/null)" || continue
        exe="$(readlink "/proc/${pid}/exe" 2>/dev/null)"
        if [[ "${comm}" =~ ${GAME_MARKERS} || "${exe}" =~ ${GAME_MARKERS} ]]; then
            GAME_PROCESS="${comm}"
            return 0
        fi
    done
    return 1
}

# Cargo's -j and a nice level, empty when the machine is idle.
CARGO_JOBS=()
NICE=()
GAME_PROCESS=""
if game_running; then
    half=$(( $(nproc) / 2 ))
    (( half < 1 )) && half=1
    CARGO_JOBS=(-j "${half}")
    NICE=(nice -n 10)
fi

# Announces the throttle once, after the output helpers are defined.
announce_throttle() {
    if (( ${#CARGO_JOBS[@]} )); then
        step "${GAME_PROCESS} is running — building on ${CARGO_JOBS[1]}/$(nproc) cores at nice 10"
        echo "  (set SIPSTER_IGNORE_GAMES=1 to use the whole machine anyway)"
    fi
}

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
  aarch64-windows   Windows on ARM (needs ninja + clang in the box)
  appimage          Linux x86_64 AppImage (sipster-linux-x86_64.AppImage)
  linux             all Linux binaries
  windows           all working Windows binaries
  all               every working target above, including the AppImage
  check             clippy (warnings denied) + test
  run [args...]     run the built AppImage without FUSE-mounting it
  clean             remove dist/ and release orphaned AppImage leftovers

Environment:
  SIPSTER_BOX       distrobox to build in (default: build-box)

Test the AppImage with '$0 run', never by executing dist/*.AppImage
directly: a killed AppImage orphans its /tmp/.mount_* FUSE mount, and these
accumulate. 'run' extracts instead of mounting, and 'clean' clears whatever
a killed run did leave behind.

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
            "${NICE[@]}" cargo build --release -p sipster-ui "${CARGO_JOBS[@]}" --target "${target}"
            ;;
        i686-unknown-linux-gnu)
            PKG_CONFIG_PATH=/usr/lib/i386-linux-gnu/pkgconfig \
            PKG_CONFIG_LIBDIR=/usr/lib/i386-linux-gnu/pkgconfig \
            CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_LINKER=i686-linux-gnu-gcc \
            "${NICE[@]}" cargo build --release -p sipster-ui "${CARGO_JOBS[@]}" --target "${target}"
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
            "${NICE[@]}" cargo build --release -p sipster-ui "${CARGO_JOBS[@]}" --target "${target}"
            ;;
        aarch64-pc-windows-msvc)
            # cargo-xwin hands the C compiler MSVC-style include flags
            # (`/imsvc <dir>`) but leaves CC as plain `clang`, whose GNU driver
            # reads `/imsvc` as a filename. It overwrites CC and CFLAGS itself,
            # so this cannot be corrected with environment variables — instead
            # a shim earlier on PATH rewrites the flag to the GNU spelling.
            # `ring` is the crate that trips over it.
            local shim
            shim="$(mktemp -d)"
            cat > "${shim}/clang" <<'SHIM'
#!/usr/bin/env bash
# `/imsvc <dir>` is clang-cl's spelling of "system include directory".
# `-imsvc` is cl-mode only, so the GNU driver wants `-isystem`.
args=()
for a in "$@"; do
    case "$a" in
        /imsvc) args+=("-isystem") ;;
        *)      args+=("$a") ;;
    esac
done
exec /usr/bin/clang "${args[@]}"
SHIM
            chmod +x "${shim}/clang"
            PATH="${shim}:${PATH}" \
                "${NICE[@]}" cargo xwin build --release -p sipster-ui "${CARGO_JOBS[@]}" --target "${target}"
            local status=$?
            rm -rf "${shim}"
            (( status == 0 )) || return "${status}"
            ;;
        *)
            "${NICE[@]}" cargo build --release -p sipster-ui "${CARGO_JOBS[@]}" --target "${target}"
            ;;
    esac

    mkdir -p "${DIST_DIR}"
    # Copy to a temporary name and rename over the target. `cp` truncates the
    # destination in place, which fails with "Text file busy" when the previous
    # build is still running — and because that is only a warning from cp, a
    # stale binary would otherwise be tested as if it were the new one. Rename
    # swaps the directory entry instead, which the kernel always allows.
    cp "${ROOT_DIR}/target/${target}/release/${bin_name}" "${DIST_DIR}/.${output_name}.new"
    mv -f "${DIST_DIR}/.${output_name}.new" "${DIST_DIR}/${output_name}"
    ok "${DIST_DIR}/${output_name} ($(du -h "${DIST_DIR}/${output_name}" | cut -f1))"
}

# Packages the x86_64 Linux build as an AppImage.
build_appimage() {
    local target="x86_64-unknown-linux-gnu"
    local out="${DIST_DIR}/sipster-linux-x86_64.AppImage"

    command -v appimagetool >/dev/null 2>&1 || die \
        $'appimagetool not found in PATH\n  get it from https://github.com/AppImage/appimagetool/releases'

    step "Building AppImage"
    "${NICE[@]}" cargo build --release -p sipster-ui "${CARGO_JOBS[@]}" --target "${target}"

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

build_windows() {
    build_target "x86_64-pc-windows-gnu"    "sipster-ui.exe" "sipster-windows-x86_64.exe"
    build_target "i686-pc-windows-gnu"      "sipster-ui.exe" "sipster-windows-x86.exe"
    build_target "aarch64-pc-windows-msvc"  "sipster-ui.exe" "sipster-windows-aarch64.exe"
}

# ── AppImage leftover hygiene ────────────────────────────────────────────────
#
# An AppImage that does not exit cleanly leaves something behind in /tmp, and
# which thing depends on how it was started:
#
#   - Mounted (the default): it self-mounts at /tmp/.mount_<name>XXXXXX and
#     unmounts on clean exit only. Killed, the mountpoint is orphaned and
#     reports "Transport endpoint is not connected" until someone runs
#     fusermount3 by hand.
#   - APPIMAGE_EXTRACT_AND_RUN (what `run` uses): no mount, but the payload is
#     unpacked to /tmp/appimage_extracted_<hash> and only deleted on clean
#     exit. Killed, that stays — ~40 MB per run, and /tmp is tmpfs here, so it
#     is RAM.
#
# Neither is fatal, both accumulate over a testing session, so clean up both.

# Releases orphaned Sipster AppImage mounts and extracted payloads.
#
# Safe by construction. A mountpoint is only unmounted when reading it fails,
# which means its FUSE daemon is already gone and nothing can be using it;
# a running instance's mount reads fine and is left alone. Note that `stat` on
# an orphaned mountpoint still succeeds — only readdir fails — so the check has
# to actually list the directory. An extracted payload is only deleted when no
# live process references its path.
prune_appimage_leftovers() {
    local path mounts=0 payloads=0 busy=0
    shopt -s nullglob

    for path in /tmp/.mount_sipste*; do
        if ls -A "${path}" >/dev/null 2>&1; then
            busy=$((busy + 1))
            continue
        fi
        fusermount3 -u "${path}" 2>/dev/null || fusermount -u "${path}" 2>/dev/null || true
        rmdir "${path}" 2>/dev/null || true
        mounts=$((mounts + 1))
    done

    for path in /tmp/appimage_extracted_*; do
        # Only payloads this project produced: `run` extracts Sipster itself,
        # and build_appimage extracts appimagetool (which is also an AppImage,
        # self-extracted because the build container has no FUSE). Anything
        # else in /tmp belongs to another application — leave it be.
        [[ -f "${path}/usr/bin/sipster" || -f "${path}/usr/bin/appimagetool" ]] || continue
        if pgrep -f "${path}" >/dev/null 2>&1; then
            busy=$((busy + 1))
            continue
        fi
        rm -rf "${path}"
        payloads=$((payloads + 1))
    done

    shopt -u nullglob

    (( mounts > 0 )) && ok "released ${mounts} orphaned AppImage mount(s)"
    (( payloads > 0 )) && ok "removed ${payloads} orphaned extracted payload(s)"
    (( busy > 0 )) && echo "  note: ${busy} leftover(s) still in use; left alone"
    return 0
}

# Runs the built AppImage without ever mounting it.
#
# APPIMAGE_EXTRACT_AND_RUN makes the runtime unpack to a temp dir instead of
# using FUSE, so however the process dies there is nothing to orphan. Use this
# rather than launching dist/*.AppImage directly when testing.
run_appimage() {
    local image="${DIST_DIR}/sipster-linux-x86_64.AppImage"
    [[ -f "${image}" ]] || die "${image} not found — run '$0 appimage' first"

    prune_appimage_leftovers
    step "Running ${image##*/} (extract-and-run, no FUSE mount)"
    APPIMAGE_EXTRACT_AND_RUN=1 "${image}" "$@"
}

# The gate a change must pass before it is committed. Warnings are denied here
# even though a plain `cargo clippy` only warns, so "warning-free" is enforced
# rather than merely intended.
# A file this long has usually stopped being one thing. The limit is a
# prompt to split it, not a law of nature, so it warns well before it bites.
FILE_LINES_WARN=750
FILE_LINES_FAIL=1000

# Warns about long source files and fails on oversized ones.
#
# Counts every line, comments and tests included: they are all things a reader
# has to scroll past to find what they came for.
check_file_lengths() {
    step "source file lengths (warn ${FILE_LINES_WARN}, fail ${FILE_LINES_FAIL})"
    local over_limit=0 file lines
    while read -r lines file; do
        if (( lines >= FILE_LINES_FAIL )); then
            printf '  \033[31mtoo long\033[0m %s (%s lines, limit %s) — split it\n' \
                "${file}" "${lines}" "${FILE_LINES_FAIL}" >&2
            over_limit=1
        elif (( lines > FILE_LINES_WARN )); then
            printf '  \033[33mgetting long\033[0m %s (%s lines)\n' "${file}" "${lines}" >&2
        fi
    done < <(cd "${ROOT_DIR}" && find crates -name '*.rs' -not -path '*/target/*' \
                 -exec wc -l {} + | grep -v ' total$' | sort -rn)

    if (( over_limit )); then
        printf '\033[31mfail\033[0m at least one file is over %s lines\n' "${FILE_LINES_FAIL}" >&2
        return 1
    fi
    ok "no file over ${FILE_LINES_FAIL} lines"
}

run_check() {
    check_file_lengths
    step "cargo clippy (warnings denied)"
    "${NICE[@]}" cargo clippy --workspace --all-targets "${CARGO_JOBS[@]}" -- -D warnings
    step "cargo test"
    "${NICE[@]}" cargo test --workspace "${CARGO_JOBS[@]}"
    ok "workspace is clean"
}

announce_throttle

# TARGET was parsed near the top, before the distrobox re-exec.
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
    run)              shift || true; run_appimage "$@" ;;
    clean)
                      step "Removing ${DIST_DIR}"
                      rm -rf "${DIST_DIR}"
                      prune_appimage_leftovers
                      ok "clean"
                      ;;
    help|-h|--help)   usage ;;
    *)                usage; exit 1 ;;
esac
