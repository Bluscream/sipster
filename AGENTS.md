# Sipster Agent Guidelines & Project Architecture

## Mission
Sipster is a modern, performant and memory-safe re-imagination of **PhonerLite**: a desktop
softphone that registers against a home PBX (primarily an AVM **Fritz!Box**, but any standards
compliant SIP registrar) and lets you place and take calls from your PC.

Beyond raw telephony it aims for the convenience features that make a softphone worth leaving
open all day:
- **Contact sync** from Fritz!Box phonebook, Google Contacts, KDE (Akonadi/vCard) and
  Home Assistant. Where one source can already sync into another, prefer implementing the single
  downstream path rather than all four.
- **Call list** with Fritz!Box call-history sync, so the PC and the router agree on what happened.

Primary platform is desktop Linux (Bazzite/Fedora, Wayland/PipeWire); Windows is supported and
must not be broken casually.

## Core Rules & Constraints

1. **Rust-first, not Rust-purist:**
   - Prefer pure-Rust crates for SIP/SDP/RTP parsing, codecs, HTTP, XML and GUI.
   - Do **not** link heavyweight native telephony *stacks* (`libpjsip`, `libbaresip`). These
     are the thing we are replacing.
   - A C **codec** is acceptable. **Opus is our preferred/default codec** (best quality) and is
     offered first in SDP; the peer negotiates it when supported. G.711 (PCMU/PCMA, pure Rust)
     is the universal fallback every PBX — including the Fritz!Box — accepts. Opus is backed by
     libopus (built via `cmake`, available in the `build-box`).
   - Linking the platform's own system libraries is **acceptable and expected** — e.g. `libasound`
     via `cpal` on Linux, Wayland/X11 client libs via the GUI stack. Document them as runtime
     requirements instead of pretending they do not exist.
   - Never claim "100% pure Rust" in user-facing docs unless `ldd` on the shipped binary backs it.

2. **Workspace Architecture:**
   - `crates/sipster-core`: headless protocol engine — SIP signaling, SDP negotiation, RTP media,
     codecs, audio I/O, call state machine, config, single-instance IPC and CLI parsing.
     Provider-agnostic: no Fritz!Box specifics here.
   - `crates/sipster-ui`: Iced desktop GUI, system tray, local feedback sounds. Presentation
     only — if another frontend would want it, it belongs in core instead.
   - `crates/sipster-tests`: integration, end-to-end and PBX scenario tests.
   - `crates/sipster-integrations` (**planned, does not exist yet**): contact and call-list
     providers (Fritz!Box TR-064, Google, KDE, Home Assistant). Talks HTTP/DBus, never SIP.
     Create it when the first provider actually lands, not before — see rule 3.

3. **No unearned claims.** This is the project's hardest rule, and it exists because it was
   already broken once.
   - Do not add a dependency until code imports it.
   - Do not create a crate, module or config field until it does something.
   - Do not describe a capability in `README.md`, the GitHub description or a release body until
     it demonstrably works. "Planned" must be labelled as planned.
   - Do not publish release artifacts built from anything other than the tagged commit.

4. **Code Quality, Testing & Lint Standards:**
   - `./scripts/build.sh check` is the gate: clippy with `-D warnings`, then the test suite.
     Run it before every commit. Warning-free is enforced, not aspirational.
   - MSRV is declared in `[workspace.package] rust-version`; clippy's `incompatible_msrv`
     lint enforces it. Raise it deliberately, with a comment saying which feature forced it.
   - Max **100 lines per function**, max **1,000 lines per file**.
   - The repo is *not* rustfmt-formatted and `cargo fmt` is not part of the gate. Match the
     surrounding style; do not reformat files you are not otherwise changing.
   - Tests are written as features land, not retrofitted. Err on the side of too many.
     A test that passes whether or not the feature works is worse than no test.
   - Integration tests live in `crates/sipster-tests/`.

5. **Audio & Media:**
   - Call audio I/O: `rvoip-audio-device` (cpal underneath → PipeWire/ALSA), wired up in
     `sipster-core::audio`. Bound on early media, not only on answer.
   - Codecs, RTP and SDP: provided by rvoip's media stack, not by direct dependencies of
     ours. Opus preferred and offered first; G.711 (PCMA/PCMU) as universal fallback.
   - *Local feedback* sound (dialpad tones, call chimes, ringtone) is a separate concern
     that lives in `sipster-ui::sound` and never touches a call's media stream. It is
     synthesized to an in-memory WAV and piped to `pw-play`/`paplay`, so we ship no audio
     assets. Note this is **not** DTMF: nothing is sent to the peer.

6. **Architecture & 32-Bit Support:**
   - Support 64-bit and 32-bit targets (`x86_64`, `i686`, `aarch64`, `armv7`).
   - Use target-conditional dependencies to route around upstream 32-bit blockers without
     degrading 64-bit builds.
   - 32-bit may be dropped in future if it starts costing modern features — ask first.
   - If a change would require dropping an OS (e.g. Windows), split the code path first; only
     ask to drop support if genuinely unavoidable.

## Assets

- `assets/` holds every image the project ships or displays: `logo.png` (the app icon
  the AppImage installs), `banner.png`, and `screenshots/`. It is a normal tracked directory — the old
  orphan `assets` branch is gone; do not recreate that pattern.
- `crates/sipster-ui/assets/icons/` holds the sized PNGs that are `include_bytes!`'d into
  the binary for the tray and window icon. Those are code inputs, not documentation assets.
- **App-id invariant:** the Wayland `application_id` in `sipster-ui/src/main.rs` (`APP_ID`),
  the `packaging/sipster.desktop` filename, and its `StartupWMClass` must all read
  `sipster`. If they diverge the window falls back to a generic placeholder icon.

## Build & Release

- Builds run inside the **`build-box` distrobox** (Debian), which carries the cross toolchains and
  `alsa`/pkg-config for every target. `cargo build` on the bare host is expected to fail; that is
  not a bug.
- Release artifacts are named **`sipster-{os}-{arch}.{ext}`** — no exceptions, including the
  AppImage (`sipster-linux-x86_64.AppImage`).
- Not every target in rule 6 currently builds. `aarch64-pc-windows-msvc` is blocked on
  `ring` under cargo-xwin and is excluded from `build.sh all`; the reason is documented at
  `build_windows()`. Ship only what was actually built — never pad a release with an
  artifact name that has no artifact behind it.
- Linux deployment target is an **AppImage**, published to GitHub Releases.
- GitHub Actions runners are not usable for this project, so `release.yml` is
  `workflow_dispatch`-only. It must still be *correct* — it is reference documentation for the
  release process and gets read as such.
- Artifacts must be built from the tagged commit. Tag first, then build, then upload.

### Release checklist

1. `./scripts/build.sh check` — must be clean.
2. Bump `[workspace.package] version` in the root `Cargo.toml` (members inherit it) and
   refresh `Cargo.lock` with a build.
3. Commit, then `git tag vX.Y.Z`.
4. `./scripts/build.sh all` from the tagged commit.
5. Smoke-test `dist/sipster-linux-x86_64.AppImage` — it must start, register and show a
   tray icon, not merely link.
6. `gh release create vX.Y.Z dist/*` with notes that claim only what rule 3 permits.

## Reference Code
`.references/` holds upstream projects for pattern-mining: `microsip` and `tSIP` for
Windows-softphone UX, `phonerlite`-alikes for feature parity, `rvoip`/`re-sip-library` for
pure-Rust SIP state handling, `baresip`/`pjsip` for protocol correctness when the RFC is
ambiguous, `gnome-calls` for Linux dialer UX. Never commit `.references/`.
