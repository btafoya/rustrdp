//! Profile → rdpSettings. Generic `freerdp_settings_set_*` with FreeRDP_* key IDs.

use freerdp_sys as sys;
use rdp_core::{split_user, DisplayMode, Profile};
use std::ffi::CString;

struct Cfg {
    s: *mut sys::rdpSettings,
    failed: Vec<&'static str>,
}

impl Cfg {
    fn bool(&mut self, k: sys::FreeRDP_Settings_Keys_Bool, v: bool, what: &'static str) {
        if unsafe { sys::freerdp_settings_set_bool(self.s, k, v as sys::BOOL) } == 0 {
            self.failed.push(what);
        }
    }
    fn u32(&mut self, k: sys::FreeRDP_Settings_Keys_UInt32, v: u32, what: &'static str) {
        if unsafe { sys::freerdp_settings_set_uint32(self.s, k, v) } == 0 {
            self.failed.push(what);
        }
    }
    fn str(&mut self, k: sys::FreeRDP_Settings_Keys_String, v: &str, what: &'static str) {
        let Ok(c) = CString::new(v) else {
            self.failed.push(what);
            return;
        };
        if unsafe { sys::freerdp_settings_set_string(self.s, k, c.as_ptr()) } == 0 {
            self.failed.push(what);
        }
    }
    fn device(&mut self, args: &[&str], what: &'static str) {
        let cs: Vec<CString> = args.iter().filter_map(|a| CString::new(*a).ok()).collect();
        if cs.len() != args.len() {
            self.failed.push(what);
            return;
        }
        let ptrs: Vec<*const std::os::raw::c_char> = cs.iter().map(|c| c.as_ptr()).collect();
        if unsafe { sys::freerdp_client_add_device_channel(self.s, ptrs.len(), ptrs.as_ptr()) } == 0 {
            self.failed.push(what);
        }
    }
}

/// # Safety
/// `settings` must be a valid `rdpSettings*` owned by a live FreeRDP context.
pub(crate) unsafe fn apply(settings: *mut sys::rdpSettings, p: &Profile, password: Option<&str>) -> Result<(), String> {
    let mut c = Cfg {
        s: settings,
        failed: Vec::new(),
    };
    c.str(sys::FreeRDP_ServerHostname, &p.host, "host");
    c.u32(sys::FreeRDP_ServerPort, p.port as u32, "port");
    let (user, domain) = split_user(&p.username, &p.domain);
    if !user.is_empty() {
        c.str(sys::FreeRDP_Username, &user, "username");
    }
    if !domain.is_empty() {
        c.str(sys::FreeRDP_Domain, &domain, "domain");
    }
    if let Some(pw) = password {
        c.str(sys::FreeRDP_Password, pw, "password");
    }

    // Display. Software GDI gives us one framebuffer for legacy updates and RDPGFX alike.
    c.bool(sys::FreeRDP_SoftwareGdi, true, "software gdi");
    c.u32(sys::FreeRDP_ColorDepth, 32, "color depth");
    c.u32(sys::FreeRDP_DesktopWidth, p.display.width, "width");
    c.u32(sys::FreeRDP_DesktopHeight, p.display.height, "height");
    c.u32(
        sys::FreeRDP_DesktopScaleFactor,
        p.display.scale.clamp(100, 500),
        "scale",
    );
    let dynamic = p.display.mode == DisplayMode::Dynamic;
    c.bool(sys::FreeRDP_SupportDisplayControl, dynamic, "display control");
    c.bool(sys::FreeRDP_DynamicResolutionUpdate, dynamic, "dynamic resolution");
    c.bool(
        sys::FreeRDP_Fullscreen,
        p.display.mode == DisplayMode::Fullscreen,
        "fullscreen",
    );

    // Graphics pipeline with H.264 (decoded by FreeRDP's FFmpeg into the GDI buffer).
    c.bool(sys::FreeRDP_SupportGraphicsPipeline, true, "gfx");
    c.bool(sys::FreeRDP_GfxH264, true, "h264");
    c.bool(sys::FreeRDP_GfxAVC444, true, "avc444");
    c.bool(sys::FreeRDP_GfxAVC444v2, true, "avc444v2");
    c.bool(sys::FreeRDP_RemoteFxCodec, true, "remotefx");
    // Same companions stock FreeRDP sets whenever RemoteFX/GFX is on. Without them the server
    // sends no usable updates (found with the shadow server: connected, zero frames).
    c.bool(sys::FreeRDP_FastPathOutput, true, "fastpath output");
    c.bool(sys::FreeRDP_FrameMarkerCommandEnabled, true, "frame markers");
    if unsafe {
        sys::freerdp_set_connection_type(settings, 7 /* CONNECTION_TYPE_AUTODETECT */)
    } == 0
    {
        c.failed.push("connection type");
    }

    // Redirection. Nothing is shared unless listed.
    c.bool(sys::FreeRDP_RedirectClipboard, p.redirect.clipboard, "clipboard");
    c.bool(sys::FreeRDP_AudioPlayback, p.redirect.audio, "audio");
    c.bool(sys::FreeRDP_AudioCapture, false, "audio capture");
    if p.redirect.printers {
        c.device(&["printer"], "printer redirection");
    }
    for f in &p.redirect.folders {
        c.device(&["drive", &f.name, &f.path], "drive redirection");
    }

    if let Some(g) = &p.gateway {
        c.bool(sys::FreeRDP_GatewayEnabled, true, "gateway");
        c.str(sys::FreeRDP_GatewayHostname, &g.host, "gateway host");
        c.u32(sys::FreeRDP_GatewayUsageMethod, 1, "gateway usage");
        if !g.username.is_empty() {
            c.str(sys::FreeRDP_GatewayUsername, &g.username, "gateway user");
        }
        if !g.domain.is_empty() {
            c.str(sys::FreeRDP_GatewayDomain, &g.domain, "gateway domain");
        }
    }

    if let Some(app) = &p.remote_app {
        c.bool(sys::FreeRDP_RemoteApplicationMode, true, "remote app");
        c.str(sys::FreeRDP_RemoteApplicationProgram, app, "remote app program");
    }

    // Raw overrides last, so they win.
    for (k, v) in &p.overrides {
        let (Ok(kc), Ok(vc)) = (CString::new(k.as_str()), CString::new(v.as_str())) else {
            c.failed.push("override");
            continue;
        };
        if sys::freerdp_settings_set_value_for_name(settings, kc.as_ptr(), vc.as_ptr()) == 0 {
            return Err(format!("override {k}={v} rejected by FreeRDP"));
        }
    }

    if c.failed.is_empty() {
        Ok(())
    } else {
        Err(format!("FreeRDP rejected settings: {}", c.failed.join(", ")))
    }
}
