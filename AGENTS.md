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
     codecs, audio I/O, call state machine. Provider-agnostic: no Fritz!Box specifics here.
   - `crates/sipster-integrations`: contact and call-list providers (Fritz!Box TR-064, Google,
     KDE, Home Assistant). Talks HTTP/DBus, never SIP.
   - `crates/sipster-ui`: Iced desktop GUI.
   - `crates/sipster-tests`: integration, end-to-end and PBX scenario tests.

3. **No unearned claims.** This is the project's hardest rule, and it exists because it was
   already broken once.
   - Do not add a dependency until code imports it.
   - Do not create a crate, module or config field until it does something.
   - Do not describe a capability in `README.md`, the GitHub description or a release body until
     it demonstrably works. "Planned" must be labelled as planned.
   - Do not publish release artifacts built from anything other than the tagged commit.

4. **Code Quality, Testing & Lint Standards:**
   - `cargo clippy --workspace --all-targets` must be warning-free.
   - Max **100 lines per function**, max **1,000 lines per file**.
   - Tests are written as features land, not retrofitted. Err on the side of too many.
     A test that passes whether or not the feature works is worse than no test.
   - Integration tests live in `crates/sipster-tests/`.

5. **Audio & Media:**
   - Audio I/O: `cpal` (PipeWire / ALSA).
   - Codecs: provided by rvoip's media stack. Opus preferred and offered first; G.711
     (PCMA/PCMU) as universal fallback.
   - Media transport: `rtp` + `sdp`.

6. **Architecture & 32-Bit Support:**
   - Support 64-bit and 32-bit targets (`x86_64`, `i686`, `aarch64`, `armv7`).
   - Use target-conditional dependencies to route around upstream 32-bit blockers without
     degrading 64-bit builds.
   - 32-bit may be dropped in future if it starts costing modern features — ask first.
   - If a change would require dropping an OS (e.g. Windows), split the code path first; only
     ask to drop support if genuinely unavoidable.

## Build & Release

- Builds run inside the **`build-box` distrobox** (Debian), which carries the cross toolchains and
  `alsa`/pkg-config for every target. `cargo build` on the bare host is expected to fail; that is
  not a bug.
- Release artifacts are named **`sipster-{os}-{arch}.{ext}`** — no exceptions, including the
  AppImage (`sipster-linux-x86_64.AppImage`).
- Linux deployment target is an **AppImage**, published to GitHub Releases.
- GitHub Actions runners are not usable for this project, so `release.yml` is
  `workflow_dispatch`-only. It must still be *correct* — it is reference documentation for the
  release process and gets read as such.
- Artifacts must be built from the tagged commit. Tag first, then build, then upload.

## Reference Code
`.references/` holds upstream projects for pattern-mining: `microsip` and `tSIP` for
Windows-softphone UX, `phonerlite`-alikes for feature parity, `rvoip`/`re-sip-library` for
pure-Rust SIP state handling, `baresip`/`pjsip` for protocol correctness when the RFC is
ambiguous, `gnome-calls` for Linux dialer UX. Never commit `.references/`.
