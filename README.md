nertsio
=======

*for the love of Nerts*

**nertsio** is an online multiplayer implementation of the card game Nerts.

# Community
Chat with us on [Matrix](https://matrix.to/#/#nertsio:synapse.vpzom.click) or [Discord](https://discord.gg/bmGh9Ym6ce).

# Build Instructions
## Windows, Linux
Required Dependencies (at least for Linux, I don't know about Windows. Other dependencies will be automatically downloaded by Cargo):
- Rust & Cargo
- Git
- pkg-config
- alsa-lib (+ dev headers)
- OpenSSL (+ dev headers)

Run `cargo build --release -p nertsio_ui`. The resulting executable will be placed in `target/release`.

## Android
Android builds require [a fork of cargo-quad-apk](https://github.com/vpzomtrrfrt/cargo-quad-apk/tree/all).

Run `cargo quad-apk build -p nertsio_ui`. The resulting package will be placed in `target/android-artifacts/release/apk`.

## WASM
WASM builds require wasm-bindgen-cli to be installed, and its version needs to match the resolved version of wasm-bindgen.

Run `./mkWasmUi.sh`. The resulting files will be placed in `target/wasm32-unknown-unknown/dist`.
