#![allow(clippy::missing_safety_doc)]

use crate::{settings, CertInfo, CertVerdict, Command, Credentials, Event, Frame, Prompt, Sink};
use freerdp_sys as sys;
use rdp_core::Profile;
use std::{
    ffi::{c_void, CStr},
    os::raw::c_char,
    ptr,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

/// BGRX, 32 bpp: FREERDP_PIXEL_FORMAT(32, BGRA, 0, 8, 8, 8)
const PIXEL_FORMAT_BGRX32: u32 = 0x2004_0888;
const CHANNEL_RC_OK: u32 = 0;
const MAX_HANDLES: usize = 64;

/// Wakes the FreeRDP thread's wait loop when a command is queued.
struct Wake(sys::HANDLE);
unsafe impl Send for Wake {}
unsafe impl Sync for Wake {}
impl Drop for Wake {
    fn drop(&mut self) {
        unsafe { sys::CloseHandle(self.0) };
    }
}

/// Handle to a running session. Dropping it disconnects and joins the thread.
pub struct Session {
    tx: Sender<Command>,
    wake: Arc<Wake>,
    frame: Arc<Mutex<Frame>>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Spawns the FreeRDP thread and starts connecting. Events arrive through `sink`,
    /// on the FreeRDP thread: the sink must only forward (e.g. into a channel the UI polls).
    pub fn start(profile: Profile, password: Option<String>, sink: Sink) -> Result<Session, String> {
        let handle = unsafe { sys::CreateEventA(ptr::null_mut(), 1, 0, ptr::null()) };
        if handle.is_null() {
            return Err("CreateEvent failed".into());
        }
        let wake = Arc::new(Wake(handle));
        let frame = Arc::new(Mutex::new(Frame::default()));
        let (tx, rx) = mpsc::channel();
        let (w2, f2) = (wake.clone(), frame.clone());
        let thread = thread::Builder::new()
            .name("freerdp".into())
            .spawn(move || run(profile, password, sink, rx, w2, f2))
            .map_err(|e| e.to_string())?;
        Ok(Session {
            tx,
            wake,
            frame,
            thread: Some(thread),
        })
    }

    pub fn send(&self, cmd: Command) {
        if self.tx.send(cmd).is_ok() {
            unsafe { sys::SetEvent(self.wake.0) };
        }
    }

    /// Lock the framebuffer for reading. Keep the guard short: the FreeRDP thread writes into it.
    pub fn frame(&self) -> std::sync::MutexGuard<'_, Frame> {
        self.frame.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.send(Command::Disconnect);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// FreeRDP-thread state

/// rdpContext subclass: FreeRDP allocates `ContextSize` bytes and zero-fills them.
#[repr(C)]
struct Ctx {
    base: sys::rdpContext,
    inner: *mut Inner,
}

/// Latest resize asked for before display control is connected; replayed once it is.
#[derive(Default)]
struct ResizeQueue(Option<(u32, u32)>);

impl ResizeQueue {
    /// `Some(size)` when it can be sent now, otherwise remembered (newest wins) and `None`.
    fn request(&mut self, size: (u32, u32), ready: bool) -> Option<(u32, u32)> {
        self.0 = (!ready).then_some(size);
        ready.then_some(size)
    }

    fn take(&mut self) -> Option<(u32, u32)> {
        self.0.take()
    }
}

/// Touched only from the FreeRDP thread (callbacks and the command loop), so no locking.
struct Inner {
    sink: Sink,
    frame: Arc<Mutex<Frame>>,
    disp: *mut sys::DispClientContext,
    pending_resize: ResizeQueue,
    cliprdr: *mut sys::CliprdrClientContext,
    /// Local clipboard text as UTF-16LE with NUL, offered to the server.
    local_text: Option<Vec<u8>>,
    /// Last text received from the server: not echoed back.
    last_remote: Option<String>,
    scale: u32,
}

unsafe fn inner_of_ctx(ctx: *mut sys::rdpContext) -> &'static mut Inner {
    &mut *(*(ctx as *mut Ctx)).inner
}

unsafe fn inner_of(inst: *mut sys::freerdp) -> &'static mut Inner {
    inner_of_ctx((*inst).context)
}

unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

fn last_error(ctx: *mut sys::rdpContext) -> String {
    unsafe {
        let code = sys::freerdp_get_last_error(ctx);
        let s = cstr(sys::freerdp_get_last_error_string(code));
        let n = cstr(sys::freerdp_get_last_error_name(code));
        format!("{s} ({n}, 0x{code:08X})")
    }
}

fn run(
    profile: Profile,
    password: Option<String>,
    sink: Sink,
    rx: Receiver<Command>,
    wake: Arc<Wake>,
    frame: Arc<Mutex<Frame>>,
) {
    let end = |reason: String, error: bool, sink: &Sink| sink(Event::Disconnected { reason, error });
    // freerdp_client_context_new does this for stock clients; we use plain freerdp_new, so
    // built-in channels (rdpdr, cliprdr, rdpsnd, ...) need the static provider registered once.
    static REGISTER_ADDINS: std::sync::Once = std::sync::Once::new();
    REGISTER_ADDINS.call_once(|| unsafe {
        sys::freerdp_register_addin_provider(Some(sys::freerdp_channels_load_static_addin_entry), 0);
    });
    unsafe {
        let inst = sys::freerdp_new();
        if inst.is_null() {
            return end("freerdp_new failed".into(), true, &sink);
        }
        (*inst).ContextSize = std::mem::size_of::<Ctx>();
        (*inst).PreConnect = Some(pre_connect);
        (*inst).LoadChannels = Some(load_channels);
        (*inst).PostConnect = Some(post_connect);
        (*inst).PostDisconnect = Some(post_disconnect);
        (*inst).AuthenticateEx = Some(authenticate_ex);
        (*inst).VerifyCertificateEx = Some(verify_certificate_ex);
        (*inst).VerifyChangedCertificateEx = Some(verify_changed_certificate_ex);
        (*inst).LogonErrorInfo = Some(logon_error_info);
        if sys::freerdp_context_new(inst) == 0 {
            sys::freerdp_free(inst);
            return end("freerdp_context_new failed".into(), true, &sink);
        }
        let ctx = (*inst).context;
        let scale = profile.display.scale.clamp(100, 500);
        let inner = Box::into_raw(Box::new(Inner {
            sink,
            frame,
            disp: ptr::null_mut(),
            pending_resize: ResizeQueue::default(),
            cliprdr: ptr::null_mut(),
            local_text: None,
            last_remote: None,
            scale,
        }));
        (*(ctx as *mut Ctx)).inner = inner;

        let result = match settings::apply((*ctx).settings, &profile, password.as_deref()) {
            Err(e) => Err((e, true)),
            Ok(()) => {
                if sys::freerdp_connect(inst) == 0 {
                    Err((last_error(ctx), true))
                } else {
                    event_loop(inst, ctx, &rx, &wake)
                }
            }
        };
        let (reason, error) = result.unwrap_or_else(|e| e);
        sys::freerdp_disconnect(inst);
        // Take the sink back out before freeing the context it hangs off.
        let inner = Box::from_raw(inner);
        sys::freerdp_context_free(inst);
        sys::freerdp_free(inst);
        (inner.sink)(Event::Disconnected { reason, error });
    }
}

/// Returns `Ok((reason, false))` for a clean end, `Err((reason, true))` on failure.
unsafe fn event_loop(
    inst: *mut sys::freerdp,
    ctx: *mut sys::rdpContext,
    rx: &Receiver<Command>,
    wake: &Wake,
) -> Result<(String, bool), (String, bool)> {
    let mut handles: [sys::HANDLE; MAX_HANDLES] = [ptr::null_mut(); MAX_HANDLES];
    handles[0] = wake.0;
    loop {
        let n = sys::freerdp_get_event_handles(ctx, handles.as_mut_ptr().add(1), (MAX_HANDLES - 1) as u32);
        if n == 0 {
            return Err(("could not get event handles".into(), true));
        }
        let status = sys::WaitForMultipleObjects(n + 1, handles.as_ptr(), 0, 100);
        if status == sys::WAIT_FAILED {
            return Err(("wait failed".into(), true));
        }
        if status == sys::WAIT_OBJECT_0 {
            sys::ResetEvent(wake.0);
            while let Ok(cmd) = rx.try_recv() {
                if !handle_command(inst, ctx, cmd) {
                    sys::freerdp_abort_connect_context(ctx);
                }
            }
        }
        if sys::freerdp_check_event_handles(ctx) == 0 {
            let e = last_error(ctx);
            let clean = sys::freerdp_shall_disconnect_context(ctx) != 0;
            return if clean { Ok((e, false)) } else { Err((e, true)) };
        }
        if sys::freerdp_shall_disconnect_context(ctx) != 0 {
            return Ok(("disconnected".into(), false));
        }
    }
}

/// Returns false when the session should end.
unsafe fn handle_command(inst: *mut sys::freerdp, ctx: *mut sys::rdpContext, cmd: Command) -> bool {
    let input = (*ctx).input;
    let inner = inner_of(inst);
    match cmd {
        Command::Key { scancode, down } => {
            sys::freerdp_input_send_keyboard_event_ex(input, down as sys::BOOL, 0, scancode);
        }
        Command::Unicode { ch, down } => {
            // KBD_FLAGS_RELEASE = 0x8000
            sys::freerdp_input_send_unicode_keyboard_event(input, if down { 0 } else { 0x8000 }, ch);
        }
        Command::Mouse { flags, x, y } => {
            sys::freerdp_input_send_mouse_event(input, flags, x, y);
        }
        Command::Resize { width, height } => {
            if let Some((w, h)) = inner.pending_resize.request((width, height), !inner.disp.is_null()) {
                send_layout(inner, w, h);
            }
        }
        Command::SetClipboardText(t) => {
            if inner.last_remote.as_deref() != Some(t.as_str()) {
                inner.local_text = Some(utf16le_nul(&t));
                send_format_list(inner, true);
            }
        }
        Command::Disconnect => return false,
    }
    true
}

// ---------------------------------------------------------------------------------------------
// Connection callbacks

unsafe extern "C" fn pre_connect(inst: *mut sys::freerdp) -> sys::BOOL {
    let ctx = (*inst).context;
    let ps = (*ctx).pubSub;
    let on_conn: unsafe extern "C" fn(*mut c_void, *const sys::wEventArgs) = on_channel_connected;
    let on_disc: unsafe extern "C" fn(*mut c_void, *const sys::wEventArgs) = on_channel_disconnected;
    sys::PubSub_Subscribe(ps, c"ChannelConnected".as_ptr(), on_conn);
    sys::PubSub_Subscribe(ps, c"ChannelDisconnected".as_ptr(), on_disc);
    1
}

/// libfreerdp recreates the channel table after PreConnect (utils_reload_channels) and only
/// then calls this hook: loading addins in PreConnect would be thrown away.
unsafe extern "C" fn load_channels(inst: *mut sys::freerdp) -> sys::BOOL {
    let ctx = (*inst).context;
    sys::freerdp_client_load_addins((*ctx).channels, (*ctx).settings)
}

unsafe extern "C" fn post_connect(inst: *mut sys::freerdp) -> sys::BOOL {
    if sys::gdi_init(inst, PIXEL_FORMAT_BGRX32) == 0 {
        return 0;
    }
    let ctx = (*inst).context;
    let update = (*ctx).update;
    (*update).BeginPaint = Some(begin_paint);
    (*update).EndPaint = Some(end_paint);
    (*update).DesktopResize = Some(desktop_resize);

    let gdi = (*ctx).gdi;
    let (w, h) = ((*gdi).width as u32, (*gdi).height as u32);
    resize_frame(inner_of(inst), gdi);
    (inner_of(inst).sink)(Event::Connected { width: w, height: h });
    1
}

unsafe extern "C" fn post_disconnect(inst: *mut sys::freerdp) {
    sys::gdi_free(inst);
}

unsafe fn resize_frame(inner: &Inner, gdi: *mut sys::rdpGdi) {
    let (w, h, stride) = ((*gdi).width as u32, (*gdi).height as u32, (*gdi).stride);
    let mut f = inner.frame.lock().unwrap_or_else(|e| e.into_inner());
    f.width = w;
    f.height = h;
    f.stride = stride;
    f.data.clear();
    f.data.resize(stride as usize * h as usize, 0);
}

unsafe extern "C" fn begin_paint(ctx: *mut sys::rdpContext) -> sys::BOOL {
    let hwnd = (*(*(*(*ctx).gdi).primary).hdc).hwnd;
    (*(*hwnd).invalid).null = 1;
    (*hwnd).ninvalid = 0;
    1
}

unsafe extern "C" fn end_paint(ctx: *mut sys::rdpContext) -> sys::BOOL {
    let gdi = (*ctx).gdi;
    if gdi.is_null() || (*gdi).suppressOutput != 0 {
        return 1;
    }
    let inv = (*(*(*(*gdi).primary).hdc).hwnd).invalid;
    if (*inv).null != 0 {
        return 1;
    }
    let inner = inner_of_ctx(ctx);
    let (fw, fh) = ((*gdi).width, (*gdi).height);
    let x = (*inv).x.clamp(0, fw);
    let y = (*inv).y.clamp(0, fh);
    let w = ((*inv).w).min(fw - x).max(0);
    let h = ((*inv).h).min(fh - y).max(0);
    if w == 0 || h == 0 {
        return 1;
    }
    {
        let stride = (*gdi).stride as usize;
        let src = std::slice::from_raw_parts((*gdi).primary_buffer, stride * fh as usize);
        let mut f = inner.frame.lock().unwrap_or_else(|e| e.into_inner());
        if f.data.len() == src.len() {
            for row in y..y + h {
                let a = row as usize * stride + x as usize * 4;
                let b = a + w as usize * 4;
                f.data[a..b].copy_from_slice(&src[a..b]);
            }
        }
    }
    (inner.sink)(Event::Frame {
        x: x as u32,
        y: y as u32,
        w: w as u32,
        h: h as u32,
    });
    1
}

unsafe extern "C" fn desktop_resize(ctx: *mut sys::rdpContext) -> sys::BOOL {
    let s = (*ctx).settings;
    let (w, h) = (
        sys::freerdp_settings_get_uint32(s, sys::FreeRDP_DesktopWidth),
        sys::freerdp_settings_get_uint32(s, sys::FreeRDP_DesktopHeight),
    );
    let gdi = (*ctx).gdi;
    if sys::gdi_resize(gdi, w, h) == 0 {
        return 0;
    }
    let inner = inner_of_ctx(ctx);
    resize_frame(inner, gdi);
    (inner.sink)(Event::Resized { width: w, height: h });
    1
}

// ---------------------------------------------------------------------------------------------
// Prompts: block this thread until the UI answers.

unsafe extern "C" fn authenticate_ex(
    inst: *mut sys::freerdp,
    username: *mut *mut c_char,
    password: *mut *mut c_char,
    domain: *mut *mut c_char,
    _reason: sys::rdp_auth_reason,
) -> sys::BOOL {
    let inner = inner_of(inst);
    let (reply, rx) = mpsc::sync_channel(1);
    (inner.sink)(Event::Prompt(Prompt::Credentials {
        username: cstr(*username),
        domain: cstr(*domain),
        reply,
    }));
    let Ok(Some(Credentials {
        username: u,
        password: p,
        domain: d,
    })) = rx.recv()
    else {
        return 0;
    };
    let (u, d) = rdp_core::split_user(&u, &d);
    // ponytail: previous strings are not freed (ownership convention unclear); a few bytes per prompt.
    let dup = |s: &str| libc::strdup(std::ffi::CString::new(s).unwrap_or_default().as_ptr()) as *mut c_char;
    *username = dup(&u);
    *password = dup(&p);
    *domain = dup(&d);
    1
}

unsafe fn certificate_prompt(inst: *mut sys::freerdp, info: CertInfo) -> u32 {
    let inner = inner_of(inst);
    let (reply, rx) = mpsc::sync_channel(1);
    (inner.sink)(Event::Prompt(Prompt::Certificate { info, reply }));
    rx.recv().unwrap_or(CertVerdict::Reject) as u32
}

unsafe extern "C" fn verify_certificate_ex(
    inst: *mut sys::freerdp,
    host: *const c_char,
    port: u16,
    common_name: *const c_char,
    subject: *const c_char,
    issuer: *const c_char,
    fingerprint: *const c_char,
    _flags: u32,
) -> u32 {
    certificate_prompt(
        inst,
        CertInfo {
            host: cstr(host),
            port,
            common_name: cstr(common_name),
            subject: cstr(subject),
            issuer: cstr(issuer),
            fingerprint: cstr(fingerprint),
            changed: false,
        },
    )
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn verify_changed_certificate_ex(
    inst: *mut sys::freerdp,
    host: *const c_char,
    port: u16,
    common_name: *const c_char,
    subject: *const c_char,
    issuer: *const c_char,
    new_fingerprint: *const c_char,
    _old_subject: *const c_char,
    _old_issuer: *const c_char,
    _old_fingerprint: *const c_char,
    _flags: u32,
) -> u32 {
    certificate_prompt(
        inst,
        CertInfo {
            host: cstr(host),
            port,
            common_name: cstr(common_name),
            subject: cstr(subject),
            issuer: cstr(issuer),
            fingerprint: cstr(new_fingerprint),
            changed: true,
        },
    )
}

unsafe extern "C" fn logon_error_info(inst: *mut sys::freerdp, data: u32, kind: u32) -> i32 {
    let msg = format!(
        "{} / {}",
        cstr(sys::freerdp_get_logon_error_info_type(kind)),
        cstr(sys::freerdp_get_logon_error_info_data(data))
    );
    (inner_of(inst).sink)(Event::Notice(msg));
    1
}

// ---------------------------------------------------------------------------------------------
// Dynamic channels: GFX, display control, clipboard

unsafe fn channel_name(e: *const sys::wEventArgs) -> (String, *mut c_void) {
    let e = e as *const sys::ChannelConnectedEventArgs;
    (cstr((*e).name), (*e).pInterface)
}

/// Ask the server for a new desktop size. `inner.disp` must be connected.
unsafe fn send_layout(inner: &mut Inner, width: u32, height: u32) {
    // Display control requires even sizes in 200..=8192.
    let mut m = sys::DISPLAY_CONTROL_MONITOR_LAYOUT {
        Flags: sys::DISPLAY_CONTROL_MONITOR_PRIMARY,
        Width: (width.clamp(200, 8192)) & !1,
        Height: (height.clamp(200, 8192)) & !1,
        DesktopScaleFactor: inner.scale,
        DeviceScaleFactor: 100,
        ..Default::default()
    };
    if let Some(f) = (*inner.disp).SendMonitorLayout {
        f(inner.disp, 1, &mut m);
    }
}

unsafe extern "C" fn on_channel_connected(ctx: *mut c_void, e: *const sys::wEventArgs) {
    let ctx = ctx as *mut sys::rdpContext;
    let (name, iface) = channel_name(e);
    let inner = inner_of_ctx(ctx);
    if name.as_bytes() == &sys::RDPGFX_DVC_CHANNEL_NAME[..sys::RDPGFX_DVC_CHANNEL_NAME.len() - 1] {
        sys::gdi_graphics_pipeline_init((*ctx).gdi, iface as *mut sys::RdpgfxClientContext);
    } else if name.as_bytes() == &sys::DISP_DVC_CHANNEL_NAME[..sys::DISP_DVC_CHANNEL_NAME.len() - 1] {
        inner.disp = iface as *mut sys::DispClientContext;
        if let Some((w, h)) = inner.pending_resize.take() {
            send_layout(inner, w, h);
        }
    } else if name.as_bytes() == &sys::CLIPRDR_SVC_CHANNEL_NAME[..sys::CLIPRDR_SVC_CHANNEL_NAME.len() - 1] {
        let c = iface as *mut sys::CliprdrClientContext;
        (*c).custom = ctx as *mut c_void;
        (*c).MonitorReady = Some(cb_monitor_ready);
        (*c).ServerCapabilities = Some(cb_server_capabilities);
        (*c).ServerFormatList = Some(cb_server_format_list);
        (*c).ServerFormatDataRequest = Some(cb_server_format_data_request);
        (*c).ServerFormatDataResponse = Some(cb_server_format_data_response);
        inner.cliprdr = c;
    }
}

unsafe extern "C" fn on_channel_disconnected(ctx: *mut c_void, e: *const sys::wEventArgs) {
    let ctx = ctx as *mut sys::rdpContext;
    let (name, iface) = channel_name(e);
    let inner = inner_of_ctx(ctx);
    if name.as_bytes() == &sys::RDPGFX_DVC_CHANNEL_NAME[..sys::RDPGFX_DVC_CHANNEL_NAME.len() - 1] {
        sys::gdi_graphics_pipeline_uninit((*ctx).gdi, iface as *mut sys::RdpgfxClientContext);
    } else if name.as_bytes() == &sys::DISP_DVC_CHANNEL_NAME[..sys::DISP_DVC_CHANNEL_NAME.len() - 1] {
        inner.disp = ptr::null_mut();
    } else if name.as_bytes() == &sys::CLIPRDR_SVC_CHANNEL_NAME[..sys::CLIPRDR_SVC_CHANNEL_NAME.len() - 1] {
        inner.cliprdr = ptr::null_mut();
    }
}

// ---- clipboard: text only (CF_UNICODETEXT). Files and images are out of scope.

fn utf16le_nul(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(|u| u.to_le_bytes())
        .collect()
}

fn from_utf16le(b: &[u8]) -> String {
    let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = u.iter().position(|&c| c == 0).unwrap_or(u.len());
    String::from_utf16_lossy(&u[..end])
}

unsafe fn cb_inner(c: *mut sys::CliprdrClientContext) -> &'static mut Inner {
    inner_of_ctx((*c).custom as *mut sys::rdpContext)
}

/// Announce what we can provide: unicode text if we have it, otherwise an empty list.
unsafe fn send_format_list(inner: &mut Inner, have_text: bool) {
    let c = inner.cliprdr;
    if c.is_null() {
        return;
    }
    let mut fmt = sys::CLIPRDR_FORMAT {
        formatId: sys::CF_UNICODETEXT,
        formatName: ptr::null_mut(),
    };
    let list = sys::CLIPRDR_FORMAT_LIST {
        numFormats: have_text as u32,
        formats: if have_text { &mut fmt } else { ptr::null_mut() },
        ..Default::default()
    };
    if let Some(f) = (*c).ClientFormatList {
        f(c, &list);
    }
}

unsafe extern "C" fn cb_server_capabilities(
    _c: *mut sys::CliprdrClientContext,
    _caps: *const sys::CLIPRDR_CAPABILITIES,
) -> u32 {
    CHANNEL_RC_OK
}

unsafe extern "C" fn cb_monitor_ready(c: *mut sys::CliprdrClientContext, _m: *const sys::CLIPRDR_MONITOR_READY) -> u32 {
    let mut general = sys::CLIPRDR_GENERAL_CAPABILITY_SET {
        capabilitySetType: sys::CB_CAPSTYPE_GENERAL as u16,
        capabilitySetLength: 12,
        version: sys::CB_CAPS_VERSION_2,
        generalFlags: sys::CB_USE_LONG_FORMAT_NAMES,
    };
    let caps = sys::CLIPRDR_CAPABILITIES {
        cCapabilitiesSets: 1,
        capabilitySets: &mut general as *mut _ as *mut sys::CLIPRDR_CAPABILITY_SET,
        ..Default::default()
    };
    if let Some(f) = (*c).ClientCapabilities {
        f(c, &caps);
    }
    let inner = cb_inner(c);
    let have = inner.local_text.is_some();
    send_format_list(inner, have);
    CHANNEL_RC_OK
}

unsafe extern "C" fn cb_server_format_list(
    c: *mut sys::CliprdrClientContext,
    list: *const sys::CLIPRDR_FORMAT_LIST,
) -> u32 {
    let mut ok = sys::CLIPRDR_FORMAT_LIST_RESPONSE::default();
    ok.common.msgFlags = sys::CB_RESPONSE_OK as u16;
    if let Some(f) = (*c).ClientFormatListResponse {
        f(c, &ok);
    }
    let formats = if (*list).numFormats == 0 || (*list).formats.is_null() {
        &[][..]
    } else {
        std::slice::from_raw_parts((*list).formats, (*list).numFormats as usize)
    };
    if formats.iter().any(|f| f.formatId == sys::CF_UNICODETEXT) {
        let req = sys::CLIPRDR_FORMAT_DATA_REQUEST {
            requestedFormatId: sys::CF_UNICODETEXT,
            ..Default::default()
        };
        (*c).lastRequestedFormatId = sys::CF_UNICODETEXT;
        if let Some(f) = (*c).ClientFormatDataRequest {
            return f(c, &req);
        }
    }
    CHANNEL_RC_OK
}

unsafe extern "C" fn cb_server_format_data_request(
    c: *mut sys::CliprdrClientContext,
    req: *const sys::CLIPRDR_FORMAT_DATA_REQUEST,
) -> u32 {
    let inner = cb_inner(c);
    let mut resp = sys::CLIPRDR_FORMAT_DATA_RESPONSE::default();
    match (&inner.local_text, (*req).requestedFormatId == sys::CF_UNICODETEXT) {
        (Some(t), true) => {
            resp.common.msgFlags = sys::CB_RESPONSE_OK as u16;
            resp.common.dataLen = t.len() as u32;
            resp.requestedFormatData = t.as_ptr();
        }
        _ => resp.common.msgFlags = sys::CB_RESPONSE_FAIL as u16,
    }
    match (*c).ClientFormatDataResponse {
        Some(f) => f(c, &resp),
        None => CHANNEL_RC_OK,
    }
}

unsafe extern "C" fn cb_server_format_data_response(
    c: *mut sys::CliprdrClientContext,
    resp: *const sys::CLIPRDR_FORMAT_DATA_RESPONSE,
) -> u32 {
    let ok = (*resp).common.msgFlags & sys::CB_RESPONSE_OK as u16 != 0;
    if ok && !(*resp).requestedFormatData.is_null() && (*c).lastRequestedFormatId == sys::CF_UNICODETEXT {
        let bytes = std::slice::from_raw_parts((*resp).requestedFormatData, (*resp).common.dataLen as usize);
        let text = from_utf16le(bytes);
        let inner = cb_inner(c);
        inner.last_remote = Some(text.clone());
        (inner.sink)(Event::ClipboardText(text));
    }
    CHANNEL_RC_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_roundtrip_with_nul_and_non_bmp() {
        let s = "héllo 🦀";
        let b = utf16le_nul(s);
        assert_eq!(&b[b.len() - 2..], &[0, 0]);
        assert_eq!(from_utf16le(&b), s);
        assert_eq!(from_utf16le(&[]), "");
    }

    #[test]
    fn resize_before_display_control_is_kept_latest_only_and_replayed_once() {
        let mut q = ResizeQueue::default();
        assert_eq!(q.request((800, 600), false), None);
        assert_eq!(q.request((1270, 744), false), None);
        assert_eq!(q.take(), Some((1270, 744)));
        assert_eq!(q.take(), None);
        // once the channel is up, requests go straight through and leave nothing queued
        assert_eq!(q.request((1000, 700), true), Some((1000, 700)));
        assert_eq!(q.take(), None);
    }

    #[test]
    fn bgrx32_matches_freerdp_macro() {
        // (32<<24)|(BGRA=4<<16)|(0<<12)|(8<<8)|(8<<4)|8
        assert_eq!(PIXEL_FORMAT_BGRX32, (32 << 24) | (4 << 16) | (8 << 8) | (8 << 4) | 8);
    }
}
