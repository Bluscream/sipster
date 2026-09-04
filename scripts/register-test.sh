#!/usr/bin/env bash
# Interactive registration smoke test against a real PBX.
#
# Sipster is configured by its config file only — there are no credential
# environment variables any more. This script therefore writes a throwaway
# config, runs the headless `register` example against it, and deletes it
# again. The password never reaches your shell history, `ps` output, or your
# real config.
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
    # Credentials are collected inside the container so they never enter the
    # host environment.
    exec distrobox enter "${BOX}" -- bash -lc \
        "cd '${ROOT_DIR}' && ./scripts/register-test.sh ${TARGET@Q}"
fi

# Offer the existing config as the default, so the common case is one Enter.
DEFAULT_CONFIG="${XDG_CONFIG_HOME:-${HOME}/.config}/sipster/sipster.toml"
if [[ -f "${DEFAULT_CONFIG}" ]]; then
    echo "Found a config at ${DEFAULT_CONFIG}"
    read -r -p "Use it? [Y/n]: " use_existing
    if [[ ! "${use_existing}" =~ ^[Nn] ]]; then
        cd "${ROOT_DIR}"
        exec cargo run -q -p sipster-core --example register -- \
            --config-file "${DEFAULT_CONFIG}" ${TARGET:+"${TARGET}"}
    fi
fi

read -r -p "Registrar host [fritz.box]: " REGISTRAR
REGISTRAR="${REGISTRAR:-fritz.box}"

# On a Fritz!Box this is the "Benutzername" on the telephony device's
# "Anmeldedaten" tab — a name like "bluscream", NOT the internal number (620)
# and NOT the router's admin login.
read -r -p "SIP username (Fritz!Box 'Benutzername'): " USERNAME
if [[ -z "${USERNAME}" ]]; then
    echo "error: username is required" >&2
    exit 1
fi

# -s: no echo. The password never reaches the terminal or the history.
read -r -s -p "Password: " PASSWORD
echo

# Written 0600 before anything is put in it, and removed however we exit.
CONFIG="$(mktemp -t sipster-register-XXXXXX.toml)"
chmod 600 "${CONFIG}"
trap 'rm -f "${CONFIG}"' EXIT

# A here-doc keeps the password off the command line entirely.
cat > "${CONFIG}" <<EOF
[[accounts]]
label = "register-test"
registrar = "${REGISTRAR}"
username = "${USERNAME}"
password = "${PASSWORD}"
EOF
unset PASSWORD

echo
echo "==> registering ${USERNAME}@${REGISTRAR} (throwaway config, password hidden)"
echo

cd "${ROOT_DIR}"
cargo run -q -p sipster-core --example register -- \
    --config-file "${CONFIG}" ${TARGET:+"${TARGET}"}
