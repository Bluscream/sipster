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

# systemd's environment.d is only applied when the user session starts, so the
# variables are usually absent from an already-running shell. Load them here.
load_environment_d() {
    local dir="${XDG_CONFIG_HOME:-${HOME}/.config}/environment.d"
    [[ -d "${dir}" ]] || return 0
    local file
    for file in "${dir}"/*.conf; do
        [[ -e "${file}" ]] || continue
        # Warn if a file holding a password is readable by other users.
        if [[ "$(stat -c '%a' "${file}")" != "600" ]] && grep -qi 'password' "${file}"; then
            echo "warning: ${file} contains a password but is mode $(stat -c '%a' "${file}")." >&2
            echo "         consider: chmod 600 ${file}" >&2
        fi
        # KEY=VALUE lines only; never execute the file.
        while IFS='=' read -r key value; do
            [[ "${key}" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || continue
            [[ -n "${!key:-}" ]] && continue          # already-set env wins
            # systemd strips one layer of surrounding quotes; match that, or
            # the quotes end up inside the value and corrupt the SIP URI.
            if [[ "${value}" == \"*\" && ${#value} -ge 2 ]]; then
                value="${value:1:${#value}-2}"
            elif [[ "${value}" == \'*\' && ${#value} -ge 2 ]]; then
                value="${value:1:${#value}-2}"
            fi
            export "${key}=${value}"
        done < <(grep -E '^[[:space:]]*[A-Za-z_][A-Za-z0-9_]*=' "${file}" | sed 's/^[[:space:]]*//')
    done
}

load_environment_d

# Accept either prefix; SIPSTER_ wins when both are present.
REGISTRAR="${SIPSTER_REGISTRAR:-${SIP_REGISTRAR:-}}"
USERNAME="${SIPSTER_USERNAME:-${SIP_USERNAME:-}}"
AUTH_USER="${SIPSTER_AUTH_USER:-${SIP_AUTH_USER:-}}"
PASSWORD="${SIPSTER_PASSWORD:-${SIP_PASSWORD:-}}"

# Prompt only for what the environment did not supply.
if [[ -z "${REGISTRAR}" ]]; then
    read -r -p "Registrar host [fritz.box]: " REGISTRAR
    REGISTRAR="${REGISTRAR:-fritz.box}"
fi

# On a Fritz!Box this is the "Benutzername" on the telephony device's
# "Anmeldedaten" tab — a name like "bluscream", NOT the internal number (620)
# and NOT the router's admin login.
if [[ -z "${USERNAME}" ]]; then
    read -r -p "SIP username (Fritz!Box 'Benutzername'): " USERNAME
fi
if [[ -z "${USERNAME}" ]]; then
    echo "error: username is required" >&2
    exit 1
fi

# -s: no echo. The password never reaches the terminal, history, or argv.
if [[ -z "${PASSWORD}" ]]; then
    read -r -s -p "Password: " PASSWORD
    echo
fi

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
