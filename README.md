# Sipster

A lightweight, modern, 100% pure-Rust softphone / SIP client designed for Linux (Bazzite/Fedora/Wayland) and standard SIP telephony.

![Sipster Preview](https://raw.githubusercontent.com/Bluscream/sipster/assets/assets/screenshot.png)

## Architecture

This project is organized as a Cargo workspace with clean separation between the protocol/media engine and the graphical frontend:

* [`crates/sipster-core`](crates/sipster-core): Pure-Rust SIP signaling (`rsip`), SDP negotiation (`sdp`), RTP packetization (`rtp`), audio codecs (`audio-codec` / G.711, G.722), and audio I/O (`cpal`). No C dependencies.
* [`crates/sipster-ui`](crates/sipster-ui): Modern desktop user interface built with pure-Rust GUI toolkit ([Iced](https://github.com/iced-rs/iced)), handling dialpad, call control, contacts, and account management.

## License
This project is dedicated to the public domain under [UNLICENSE](UNLICENSE).
