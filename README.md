<div align="center">

<img src="assets/logo.png" alt="Sipster" width="128">

# Sipster

**A lightweight, modern Rust softphone for the Linux desktop.**

Register against your Fritz!Box (or any standards-compliant SIP registrar),
place and take calls from your PC.

[![License: Unlicense](https://img.shields.io/badge/license-Unlicense-blue.svg)](UNLICENSE)
[![Rust 1.89+](https://img.shields.io/badge/rust-1.89%2B-orange.svg)](https://www.rust-lang.org)

<details>
<summary>📸 <b>Screenshots</b></summary>
<br>

| Dialer (Registered) | In-Call | Settings |
| :---: | :---: | :---: |
| <img src="assets/screenshots/main.png" alt="Sipster dialpad, registered" width="260"> | <img src="assets/screenshots/in_call.png" alt="Sipster during a call" width="260"> | <img src="assets/screenshots/settings.png" alt="Sipster settings window" width="300"> |

</details>

</div>

---

## What works today

Sipster is a young project. This list is deliberately limited to things that
demonstrably work — see [Not yet](#not-yet) for the rest.

- **Registration** against a SIP registrar over UDP, with re-registration.
  Tested against an AVM Fritz!Box.
- **Outgoing calls** to an extension, a phone number or a full SIP URI.
- **Incoming calls** with answer and decline, a desktop notification and a
  ringtone that rings until the call is picked up or gives up.
- **Two-way audio** through your normal microphone and speaker
  (PipeWire/ALSA), including **early media** — you hear ringback, IVR prompts
  and announcements that arrive before the call is answered.
- **System tray** icon (StatusNotifierItem), with answer/hang-up entries that
  appear only when they apply. This is what KDE Plasma 6 on Wayland actually
  listens for.
- **Single instance**, enforced by a kernel file lock. A second launch hands
  its request to the copy already running instead of fighting it for SIP
  port 5060.
- **Remote control and URI handling.** `sipster --call 611`, a `tel:` link
  clicked in a browser, and a shell script all take the same path.
- **Settings window**, opened from the wordmark or the ⚙ button, and
  automatically on first run. Account,
  audio devices, theme and sound preferences are all editable **while the app
  runs** — theme, device and sound changes take effect on the spot, and an
  account change re-registers without a restart. Everything is written to
  `sipster.toml` as you change it.
- **AppImage** packaging for x86-64 Linux, plus plain binaries for Linux
  aarch64/i686/armv7 and Windows x86-64/x86.

### Not yet

Named here so nobody has to discover them the hard way:

- **No DTMF during a call.** The dialpad plays a local feedback tone and edits
  the number field. It does not send RFC 4733 or in-band digits to the peer, so
  it cannot drive a phone menu mid-call.
- **No hold or transfer.**
- **No GNOME/KDE contact-store integration.** Contacts come from the FRITZ!Box
  phonebook, Google, CardDAV, and a local folder of `.vcf` files (the
  vdirsyncer/khard convention at `~/.local/share/contacts`). Evolution Data
  Server and Akonadi both need D-Bus clients against services that are not on
  every machine, so neither is implemented rather than shipped unverified.
- **UDP only.** The config format reserves a `transport` field, but TCP and TLS
  are not implemented.
- **No Windows-on-ARM build.** `aarch64-pc-windows-msvc` does not currently
  cross-compile here — `ring` fails under cargo-xwin. See the note in
  `scripts/build.sh`.

## Install

Grab `sipster-linux-x86_64.AppImage` from
[Releases](https://github.com/Bluscream/sipster/releases), make it executable
and run it:

```bash
chmod +x sipster-linux-x86_64.AppImage && ./sipster-linux-x86_64.AppImage
```

Other architectures and Windows builds are attached to the same release as
plain binaries, named `sipster-{os}-{arch}`.

### Runtime requirements

Sipster links exactly one non-system library, ALSA, and dlopens the graphics
stack your desktop already provides:

```console
$ ldd sipster-linux-x86_64
    libasound.so.2      # audio, via cpal — PipeWire's ALSA layer serves this
    libc.so.6  libm.so.6  libgcc_s.so.1
```

Opus is statically linked, so there is nothing else to install. `pw-play` (or
`paplay`) and `notify-send` are used for ringtones and notifications when
present, and are skipped when they are not.

## Configure

There is nothing to set up by hand. On first run Sipster opens its settings
window; fill in the account and press **Apply & reconnect**.

The config file is the only source of configuration — Sipster reads no
environment variables of its own. It lives at
`$XDG_CONFIG_HOME/sipster/sipster.toml`, or wherever `--config-file` points,
and the settings window rewrites it as you make changes. It is written `0600`,
because it holds the account password in the clear (SIP digest auth needs it).

The account fields mirror the Fritz!Box "telephony device" dialog, so you can
copy values across without translating them. In particular, **Username** is the
*Benutzername* on the device's *Anmeldedaten* tab — not the internal number
(620) and not your router's admin login.

```bash
sipster --config-file ~/work-phone.toml   # a second account, side by side
```

Pair that with `--socket` and `--no-single-instance` to run two independently
configured instances at once.

<details>
<summary><b>What the file looks like</b></summary>

Everything below is written and read by the settings window; you only need this
if you would rather edit it directly or deploy it from a script.

```toml
[[accounts]]
label     = "Fritz!Box"
registrar = "fritz.box"    # host, host:port, or a full sip:/sips: URI
port      = 5060
username  = "bluscream"
auth_user = ""             # defaults to username
password  = "…"
transport = "udp"          # only udp is implemented
expires   = 600            # re-registration interval, seconds
local_port = 5060          # falls back to an ephemeral port if taken

[ui]
theme = "dark"          # dark, light, dracula, nord, solarized-dark,
                        # gruvbox-dark, catppuccin-mocha, tokyo-night
ringtone = true
notifications = true
dtmf_feedback = true    # local beep only; not sent to the peer
call_chimes = true
show_banner = true

[audio]
input = "…"             # omit for the system default
output = "…"

[ipc]
socket = "…"            # omit for $XDG_RUNTIME_DIR/sipster.sock
```

Only the first account is used today.

</details>

## Use

Sipster runs as a single instance, so its flags double as a remote control: run
it again with a flag and the request is handed to the copy already running.

```bash
sipster                      # start, or focus the running window
sipster --call '**610'       # dial from a script or a hotkey
sipster tel:+4930123456      # what a tel: link from your browser does
sipster --answer             # answer the ringing call
sipster --hangup             # hang up, or decline
sipster --quit
sipster --help

sipster --config-file ~/other.toml   # use a different config
sipster --log-file /tmp/sipster.log  # log to a file instead of stderr
```

Sipster reads no environment variables of its own. It does honour the platform
conventions that tell any Linux program where to put things — `XDG_CONFIG_HOME`,
`XDG_RUNTIME_DIR`, `HOME` — and `RUST_LOG`, which is a debugging switch rather
than configuration.

Register Sipster as your desktop's handler for `tel:`, `sip:` and `callto:`
links by installing `packaging/sipster.desktop` into
`~/.local/share/applications/`.

Click the **Sipster** wordmark, or the ⚙ button, to open Settings.

For detail while debugging, set `RUST_LOG`, e.g. `RUST_LOG=sipster_core=trace`.
`--log-file <path>` sends logs to a file instead of stderr.

## Architecture

A Cargo workspace with the telephony engine kept strictly separate from the
GUI, so another frontend can reuse all of it:

| Crate | Role |
| ----- | ---- |
| [`sipster-core`](crates/sipster-core) | Headless engine: SIP signalling, SDP, RTP media, codecs and audio I/O, the call state machine, config, single-instance IPC. Provider-agnostic — no Fritz!Box specifics. |
| [`sipster-ui`](crates/sipster-ui) | The desktop skin: [Iced](https://github.com/iced-rs/iced) GUI, system tray, local feedback sounds. Contains no telephony logic. |
| [`sipster-tests`](crates/sipster-tests) | Integration and end-to-end tests. |

The SIP/SDP/RTP stack and codecs come from
[rvoip](https://github.com/eisenzopf/rvoip), pinned to a git revision. Opus is
offered first for quality, with G.711 (PCMA/PCMU) as the fallback every PBX —
the Fritz!Box included — accepts.

## Build

Builds run inside the `build-box` distrobox, which carries the cross toolchains,
`alsa`/pkg-config and the `cmake` that libopus needs. `scripts/build.sh`
re-enters it for you, so run it from the host:

```bash
./scripts/build.sh check           # clippy (warnings denied) + tests
./scripts/build.sh x86_64-linux    # one target
./scripts/build.sh appimage        # dist/sipster-linux-x86_64.AppImage
./scripts/build.sh all             # every target, plus the AppImage
```

Artifacts land in `dist/` named `sipster-{os}-{arch}.{ext}`.

To try registration without the GUI:

```bash
./scripts/register-test.sh          # offers your config, or prompts for one
./scripts/register-test.sh '**9'    # register, then dial
```

## Contributing

[`AGENTS.md`](AGENTS.md) documents the project's rules — the workspace layout,
the pure-Rust boundary and, most importantly, the no-unearned-claims rule that
this README is written under. Read it before opening a PR.

## License

Dedicated to the public domain under [UNLICENSE](UNLICENSE).
