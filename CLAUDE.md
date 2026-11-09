# rustrdp

GTK4/libadwaita RDP client with a connection manager, Rust FFI over vendored FreeRDP 3.33.0. Layout and status: `README.md`, design: `docs/DESIGN.md`.

## Commands

- `cargo test` - rdp-core + freerdp unit tests (no GTK needed; `default-members` exclude the app). First run builds FreeRDP (~25 s).
- `cargo build -p rustrdp` - the app; needs `libgtk-4-dev` and `libadwaita-1-dev` (target floor: GTK 4.14 / libadwaita 1.5, Ubuntu 24.04).
- `SHADOW_CLI=<path>/freerdp-shadow-cli scripts/it-shadow.sh` - integration test against FreeRDP's shadow server under Xvfb. Build steps for the shadow CLI are in `README.md`.
- `cargo fmt --all` before committing (`rustfmt.toml`: max_width 120).
- `vendor/freerdp` is a plain copy of 3.33.0, not a submodule. Override with `RUSTRDP_FREERDP_SRC`.

## FreeRDP FFI gotchas

- Load channel addins in the `LoadChannels` hook, not `PreConnect`; libfreerdp discards earlier loads. Register the built-in provider once: `freerdp_register_addin_provider(freerdp_channels_load_static_addin_entry, 0)`.
- GFX/RemoteFX needs `FastPathOutput`, `FrameMarkerCommandEnabled` and connection type AUTODETECT, or the server sends no frames (symptom: connected, zero frames).
- Plain-TLS connections ask for credentials before the certificate. Answering "no credentials" surfaces as `ERRCONNECT_CONNECT_CANCELLED`.
- The vendored tree sits inside this git repo, so cmake needs `USE_VERSION_FROM_GIT_TAG=OFF`.
- The drive channel has no read-only flag in 3.33. Do not add one to the model or UI.
- One FreeRDP thread per session. Drop the `Session` off the UI thread (it joins the worker) to avoid deadlock.
- The shadow server only sends frames while its screen changes; the test script animates the root window.

## Conventions

- `rdp-core` stays free of GTK and `unsafe`; all FFI lives in `freerdp-sys` and `freerdp`.
- Every change that is rebuilt bumps the patch version in the workspace `Cargo.toml` (0.1.1 -> 0.1.2 -> ...); the AppImage filename carries it.
- A corrupt `profiles.json` is an error shown to the user, never overwritten.
- Passwords go to the keyring only, never into `profiles.json` or exported `.rdp` files.
- Commits: no AI attribution lines; author is the repo owner.

## Not verified yet

Clipboard round trip, audio, drives/printers, dynamic resize, gateway, NLA, real H.264, Wayland, Windows servers. RAIL per-window rendering and .deb/AppImage packaging are not implemented.
