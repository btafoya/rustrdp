# rustrdp

GTK4 / libadwaita RDP client with a connection manager, written in Rust on top of FreeRDP 3.33.0
(vendored in `vendor/freerdp`, built by `crates/freerdp-sys`). Linux only. MIT OR Apache-2.0.

Design: [docs/DESIGN.md](docs/DESIGN.md). Differences from it are listed at the bottom.

## Layout

| Crate | Role |
|---|---|
| `freerdp-sys` | `build.rs` builds the vendored FreeRDP with CMake and generates bindings with bindgen |
| `freerdp` | Safe wrapper: one FreeRDP thread per session, `Command` in / `Event` out, framebuffer, input, clipboard, prompts |
| `rdp-core` | Profiles, groups, search, JSON store, `.rdp` import/export, reachability probe, keyring. No GTK, no unsafe |
| `rustrdp` (`crates/app`) | GTK4/libadwaita app: manager, profile editor, session window, certificate/credential dialogs |

## Build and test

Needs: Rust, cmake, ninja, clang (bindgen), and dev packages for FreeRDP's dependencies
(openssl, ffmpeg, pulse, alsa, X11 libs). The GUI additionally needs `libgtk-4-dev` and
`libadwaita-1-dev` (Ubuntu 24.04: GTK 4.14, libadwaita 1.5).

```bash
cargo test                      # rdp-core + freerdp unit tests (no GTK needed)
cargo build -p rustrdp          # the app (needs GTK dev packages)
```

`cargo test` and `cargo build` build FreeRDP the first time (about 25 s with ninja).
The workspace's `default-members` exclude the app so the core crates build without GTK.

Test AppImage (needs `linuxdeploy` with the appimage plugin on PATH; output in `target/appimage/`):
`packaging/build-appimage.sh`. It bundles FreeRDP, GTK4 and libadwaita from the build host, so it runs on the
same or newer glibc only, and FreeRDP links the distro's GPL-built FFmpeg: not for distribution yet.

Integration test against FreeRDP's own shadow server (Xvfb + `freerdp-shadow-cli`):

```bash
# build the shadow server once, from vendor/freerdp:
#   cmake -S vendor/freerdp -B /tmp/shadow -G Ninja -DWITH_SHADOW=ON -DWITH_SERVER=ON -DWITH_CLIENT=OFF \
#     -DWITH_CLIENT_COMMON=OFF -DWITH_PROXY=OFF -DWITH_SAMPLE=OFF -DWITH_MANPAGES=OFF -DBUILD_TESTING=OFF \
#     -DWITH_FFMPEG=OFF -DWITH_AAD=OFF -DWITH_PKCS11=OFF -DUSE_VERSION_FROM_GIT_TAG=OFF -DWITH_X11=ON
#   cmake --build /tmp/shadow --target freerdp-shadow-cli
SHADOW_CLI=/tmp/shadow/server/shadow/cli/freerdp-shadow-cli scripts/it-shadow.sh
```

## Status

Verified (shadow server, Ubuntu 24.04, X11):
- connect over TLS, framebuffer (RDPGFX negotiated, software GDI), keyboard/mouse commands, clean disconnect
- the GTK app: manager list, search, favorites, groups, status dots; Connect opens a session window,
  FreeRDP's credential and certificate callbacks show as dialogs, the remote desktop renders live,
  Disconnect returns to the manager

Verified against Windows 11 (0.1.4): login with stored credentials over NLA, dynamic resize through display control.

Implemented but not verified against a real server:
- clipboard (text only, both directions), audio playback (FreeRDP rdpsnd pulse/alsa), drive and printer
  redirection, gateway settings, `.rdp` import/export in the UI, H.264 (FreeRDP's FFmpeg; the shadow
  server cannot encode H.264)

Known gaps:
- **RAIL:** the engine passes a RemoteApp program to the server, but there is no per-window GTK
  rendering yet. A RemoteApp profile shows the whole remote surface in one window.
- **Shared folders are read/write:** FreeRDP 3.33's drive channel has no read-only flag, so the UI does not offer one.
- No microphone, smartcard, Wake-on-LAN, IME/Unicode input (keys are sent as scancodes), Windows or macOS.
- No .deb / AppImage packaging yet. Printer redirection needs CUPS found when FreeRDP is configured.
- The vendored FreeRDP is a plain copy of the 3.33.0 tree, not a git submodule.
- Not tested on Wayland or against Windows versions other than 11.

## Differences from docs/DESIGN.md

- Keyring: the `keyring` crate (Secret Service over libdbus) instead of `oo7`.
- Frames are drawn with cairo (`Rgb24` surface) in a `gtk::DrawingArea`, not a `gdk::MemoryTexture` widget.
- The FreeRDP context uses plain `freerdp_new` rather than `freerdp_client_context_new`, so the built-in
  channel provider is registered once in `Session` and addins are loaded from the `LoadChannels` hook
  (loading them in `PreConnect` is discarded by libfreerdp).
- Input and the framebuffer live on the FreeRDP thread; the UI copies dirty rectangles on `Event::Frame`.
