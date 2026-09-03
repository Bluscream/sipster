# Sipster Agent Guidelines & Project Architecture

## Mission
Build a modern, 100% pure-Rust softphone (SIP client) optimized for desktop Linux (Bazzite, Wayland/PipeWire) and standard SIP VoIP providers/PBXs.

## Core Rules & Constraints
1. **100% Pure Rust Only:**
   - Zero C/C++ build steps (`no cc`, `no cmake`, `no libbaresip/pjsip` native C library links).
   - Use pure-Rust crates for networking, SIP/SDP parsing, RTP streaming, audio decoding, and GUI.
2. **Workspace Architecture:**
   - `crates/sipster-core`: Headless protocol engine, state machine, audio pipeline, and SIP account management. Exposes high-level async APIs and event streams.
   - `crates/sipster-ui`: Modern GUI client (Iced) communicating with `sipster-core`.
   - `crates/sipster-tests`: Dedicated integration, end-to-end, and PBX scenario testing suite.
3. **Code Quality, Testing & Lint Standards:**
   - Run `cargo clippy --workspace --all-targets` and ensure zero warnings.
   - **Line limits:** Maximum **100 lines per function** (enforced by clippy `too_many_lines` / `clippy.toml`) and maximum **1,000 lines per file**.
   - Keep integration tests centralized in `crates/sipster-tests/`.
4. **Audio & Media:**
   - Audio I/O: `cpal` (PipeWire / ALSA).
   - Codecs: `audio-codec` (prioritize PCMA/PCMU G.711, G.722, and Opus).
   - Media transport: `rtp` and `sdp`.
5. **Architecture & 32-Bit Support:**
   - Support both 64-bit and 32-bit targets (e.g. `x86_64`, `i686`, `aarch64`).
   - Use target-conditional pragmas and feature gating (e.g. `[target.'cfg(...)'.dependencies]`) to bypass upstream 32-bit blockers without losing modern features on 64-bit.
   - We may drop 32-bit support in the future if required, but maintain it as long as possible without sacrificing modern features.
6. **Reference Code:**
   - Look in `.references/` for patterns (e.g., `microsip` for user-friendly flow, `rvoip` for pure-Rust SIP state handling, `gnome-calls` for Linux dialer UX). Do NOT commit `.references/`.