use std::{env, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = env::var("RUSTRDP_FREERDP_SRC")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest.join("../../vendor/freerdp"));
    println!("cargo:rerun-if-env-changed=RUSTRDP_FREERDP_SRC");
    println!("cargo:rerun-if-changed=wrapper.h");

    let mut cfg = cmake::Config::new(&src);
    cfg.profile("Release");
    for (k, v) in [
        ("BUILD_SHARED_LIBS", "ON"),
        // vendored tree sits inside our git repo; version is hard-coded in the tree
        ("USE_VERSION_FROM_GIT_TAG", "OFF"),
        ("BUILD_TESTING", "OFF"),
        ("WITH_CLIENT", "OFF"),
        ("WITH_CLIENT_SDL", "OFF"),
        ("WITH_CLIENT_COMMON", "ON"),
        ("WITH_SERVER", "OFF"),
        ("WITH_SHADOW", "OFF"),
        ("WITH_PROXY", "OFF"),
        ("WITH_SAMPLE", "OFF"),
        ("WITH_MANPAGES", "OFF"),
        ("WITH_X11", "OFF"),
        ("WITH_WAYLAND", "OFF"),
        ("WITH_FUSE", "OFF"),
        ("WITH_AAD", "OFF"),
        ("WITH_PKCS11", "OFF"),
        ("WITH_OPENH264", "OFF"),
        ("WITH_FFMPEG", "ON"),
        ("WITH_VIDEO_FFMPEG", "ON"),
        ("WITH_PULSE", "ON"),
        ("WITH_ALSA", "ON"),
        // ponytail: CUPS printer redirection is enabled automatically when libcups-dev is present
        ("CMAKE_INSTALL_LIBDIR", "lib"),
    ] {
        cfg.define(k, v);
    }
    let dst = cfg.build();
    let lib = dst.join("lib");
    println!("cargo:rustc-link-search=native={}", lib.display());
    for l in ["freerdp-client3", "freerdp3", "winpr3"] {
        println!("cargo:rustc-link-lib=dylib={l}");
    }
    // dependents read DEP_FREERDP3_LIBDIR to set an rpath
    println!("cargo:libdir={}", lib.display());

    let inc = dst.join("include");
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", inc.join("freerdp3").display()))
        .clang_arg(format!("-I{}", inc.join("winpr3").display()))
        .allowlist_function("freerdp_.*|gdi_.*|PubSub_.*|CreateEventA|SetEvent|ResetEvent|CloseHandle|WaitForMultipleObjects|WaitForSingleObject|ClipboardCreate|ClipboardDestroy|ClipboardRegisterFormat|ClipboardGetFormatId|ClipboardSetData|ClipboardGetData|ClipboardRegisterSynthesizer|ClipboardLock|ClipboardUnlock|ClipboardEmpty|ClipboardCountFormats|ClipboardGetFormatIds|ClipboardGetFormatName|ClipboardGetFormatNameA|ClipboardLockData|winpr_.*|free")
        .allowlist_type("rdp.*|freerdp|RdpgfxClientContext|CliprdrClientContext|RailClientContext|DispClientContext|CLIPRDR_.*|DISPLAY_CONTROL_MONITOR_LAYOUT|ChannelConnectedEventArgs|ChannelDisconnectedEventArgs|wPubSub|wClipboard|FreeRDP_Settings_Keys_.*|SmartcardCertInfo|AccessTokenType|RDP_CLIENT_ENTRY_POINTS.*|rdp_client_entry_points_v1|wMessage|HANDLE|PIXEL_FORMAT.*|rdpClientContext|rdpGdi|gdiBitmap|HGDI_.*|GDI_.*")
        .allowlist_var("FreeRDP_.*|PIXEL_FORMAT_.*|CF_.*|CB_.*|RDPGFX_DVC_CHANNEL_NAME|CLIPRDR_SVC_CHANNEL_NAME|DISP_DVC_CHANNEL_NAME|RAIL_SVC_CHANNEL_NAME|KBD_.*|PTR_FLAGS_.*|VERIFY_CERT_FLAG_.*|WAIT_.*|INFINITE|GFX_.*|RDP_SCANCODE_.*|FREERDP_.*|RDP_CLIENT_INTERFACE_VERSION|DISPLAY_CONTROL_MONITOR_PRIMARY")
        .default_enum_style(bindgen::EnumVariation::Consts)
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate_comments(false)
        .generate()
        .expect("bindgen failed");
    bindings
        .write_to_file(PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs"))
        .unwrap();
}
