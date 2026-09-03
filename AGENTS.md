# Sipster Agent Guidelines & Project Architecture

## Mission
Build a modern, 100% pure-Rust softphone (SIP client) optimized for desktop Linux (Bazzite, Wayland/PipeWire) and FRITZ!Box IP telephony.

## Core Rules & Constraints
1. **100% Pure Rust Only:**
   - Zero C/C++ build steps (`no cc`, `no cmake`, `no libbaresip/pjsip` native C library links).
   - Use pure-Rust crates for networking, SIP/SDP parsing, RTP streaming, audio decoding, and GUI.
2. **Workspace Architecture:**
   - `crates/sipster-core`: Headless protocol engine, state machine, audio pipeline, and Fritz!Box account management. Exposes high-level async APIs and event streams.
   - `crates/sipster-ui`: Modern GUI client (Iced) communicating with `sipster-core`.
3. **Audio & Media:**
   - Audio I/O: `cpal` (PipeWire / ALSA).
   - Codecs: `audio-codec` (prioritize PCMA/PCMU G.711 and G.722 HD voice for Fritz!Box).
   - Media transport: `rtp` and `sdp`.
4. **Reference Code:**
   - Look in `.references/` for patterns (e.g., `microsip` for user-friendly flow, `rvoip` for pure-Rust SIP state handling, `gnome-calls` for Linux dialer UX). Do NOT commit `.references/`.
5. **Licensing:**
   - Public domain ([UNLICENSE](file:///run/media/system/Data/Projects/sipster/UNLICENSE)).
