#!/usr/bin/env bash
# Interactive registration smoke test against a real PBX.
#
# Prompts for credentials (password is read silently), then runs the headless
# `register` example. The password is passed via the environment only — it never
# appears in your shell history, in `ps` output, or in any file.
#
# Builds require the `build-box` distrobox (it has alsa + cmake); this script
# re-invokes itself inside the container automatically.
#
#   ./scripts/register-test.sh            # register only
#   ./scripts/register-test.sh '**9'      # register, then dial a target
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BOX="${SIPSTER_BOX:-build-box}"
TARGET="${1:-}"

in_container() {
    [[ -f /run/.containerenv || -f /.dockerenv || -n "${CONTAINER_ID:-}" ]]
}

if ! in_container; then
    if ! command -v distrobox >/dev/null 2>&1; then
        echo "error: distrobox not found, and the host lacks the ALSA dev libraries" >&2
        echo "       needed to link (libasound.so). Run this inside the ${BOX} container." >&2
        exit 1
    fi
    echo "==> host detected; re-running inside the '${BOX}' container"
    # Credentials are collected inside the container so they are never
    # exported into the host environment.
    exec distrobox enter "${BOX}" -- bash -lc \
        "cd '${ROOT_DIR}' && ./scripts/register-test.sh ${TARGET@Q}"
fi

read -r -p "Registrar host [fritz.box]: " REGISTRAR
REGISTRAR="${REGISTRAR:-fritz.box}"

read -r -p "SIP username / internal number: " USERNAME
if [[ -z "${USERNAME}" ]]; then
    echo "error: username is required" >&2
    exit 1
fi

read -r -p "Auth user [same as username]: " AUTH_USER

# -s: no echo. The password never reaches the terminal, history, or argv.
read -r -s -p "Password: " PASSWORD
echo

export SIPSTER_REGISTRAR="${REGISTRAR}"
export SIPSTER_USERNAME="${USERNAME}"
export SIPSTER_AUTH_USER="${AUTH_USER}"
export SIPSTER_PASSWORD="${PASSWORD}"
unset PASSWORD

echo
echo "==> registering ${USERNAME}@${REGISTRAR} (password hidden)"
echo

cd "${ROOT_DIR}"
if [[ -n "${TARGET}" ]]; then
    exec cargo run -q -p sipster-core --example register -- "${TARGET}"
else
    exec cargo run -q -p sipster-core --example register
fi
