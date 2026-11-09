# rustrdp

GTK4/libadwaita RDP client with a connection manager, Rust FFI over vendored FreeRDP 3.33.0. Layout and status: `README.md`, design: `docs/DESIGN.md`.

## Commands

- `cargo test` - rdp-core + freerdp unit tests (no GTK needed; `default-members` exclude the app). First run builds FreeRDP (~25 s).
- `cargo build -p rustrdp` - the app; needs `libgtk-4-dev` and `libadwaita-1-dev` (target floor: GTK 4.14 / libadwaita 1.5, Ubuntu 24.04).
- `SHADOW_CLI=<path>/freerdp-shadow-cli scripts/it-shadow.sh` - integration test against FreeRDP's shadow server under Xvfb. Build steps for the shadow CLI are in `README.md`.
- `packaging/build-appimage.sh` - release build to `target/appimage/rustrdp-<version>-x86_64.AppImage`; needs `linuxdeploy` (appimage plugin) on PATH. It bumps the patch version in the workspace `Cargo.toml` on every run (`NO_BUMP=1` to rebuild the same version). Bundles host GTK4/libadwaita/OpenSSL/FFmpeg: test builds only until the LGPL FFmpeg build is done.
- GUI smoke test without a desktop: Xvfb + shadow server (`/sec:tls -auth`) + `xdotool`; give the app its own `XDG_CONFIG_HOME` and run it via `dbus-run-session`. The app is single-instance over D-Bus, so without `dbus-run-session` a new launch hands off to (and exits in favour of) any running `rustrdp`. Never `pkill`/`killall` by name: the user runs a live `rustrdp` and xfwm4 on this machine; kill only PIDs you started (`setsid` + `kill -- -PGID`). The app rewrites `profiles.json` pretty-printed (`"view": "Scale"`), so patch it with that spacing.
- NLA test: `freerdp-shadow-cli /sam-file:<file>` with a `user:::<NT hash>:::` line (hash via `openssl dgst -md4 -provider legacy`).
- `RUSTRDP_DEBUG_KEYS=1 rustrdp` prints every key (keycode, keyval, scancode), focus change and whether FreeRDP accepted the send to stderr: first thing to ask for on a "keyboard does nothing" report.
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
- Our FreeRDP is built with `WITH_X11=OFF` and `WITH_WAYLAND=OFF`, so every `freerdp_keyboard_*` lookup returns 0 (nothing typeable). `scancode_from_keycode` therefore uses WinPR's `WINPR_KEYCODE_TYPE_XKB` table (indexed by X11 keycode; the `EVDEV` table is indexed by evdev code, 8 lower). Unit-tested in `crates/freerdp/src/lib.rs`.
- A plain `target/release/rustrdp` has no rpath to the built FreeRDP and links the system `libfreerdp3` instead (`ldd` shows `/lib/x86_64-linux-gnu`). Only the AppImage (which bundles the built libs via `LD_LIBRARY_PATH`) and `cargo test` exercise our build, so verify FreeRDP-level behaviour through those, not through the bare release binary.
- `target/release/build` keeps one `freerdp-sys-*` out dir per past configuration; never pick one with `find | head -1`. `build-appimage.sh` asks cargo for the right one.
- NLA sends the username verbatim: `DOMAIN\user` in the username field fails with `ERRCONNECT_LOGON_FAILURE`. `rdp_core::split_user` splits it (used in `settings.rs` and the credentials prompt).
- Display-control resizes sent before the `disp` channel connects are dropped by FreeRDP; `ResizeQueue` in `session.rs` holds the latest and replays it on connect.
- Release FreeRDP is built with `WITH_VERBOSE_WINPR_ASSERT=ON` (runtime asserts, prints a warning at start).

## Conventions

- `rdp-core` stays free of GTK and `unsafe`; all FFI lives in `freerdp-sys` and `freerdp`.
- Every rebuilt change bumps the patch version in the workspace `Cargo.toml`; `build-appimage.sh` does it automatically, so build through the script and commit the `Cargo.toml`/`Cargo.lock` change.
- GTK: `set_default_size` includes the client-side shadow while `window.width()` does not; use the surface-size difference (`View::frame`), or the window shrinks on every resize.
- `ViewMode` (Actual / FitWindow / Scale, per profile) only applies to Fixed/Fullscreen; in "Match window" (Dynamic) the View menu is greyed out.
- `.rdp` imports are untrusted: they open the editor as a new profile and are saved only on Save. Clipboard redirection is off by default for new profiles.
- A corrupt `profiles.json` is an error shown to the user, never overwritten.
- Passwords go to the keyring only, never into `profiles.json` or exported `.rdp` files.
- Commits: no AI attribution lines; author is the repo owner.

## Not verified yet

Confirmed on Windows 11 (0.1.4, by the user): login with stored credentials over NLA, dynamic resize. Not confirmed: clipboard round trip, audio, drives/printers, gateway, real H.264, Wayland. RAIL per-window rendering and .deb packaging are not implemented.
