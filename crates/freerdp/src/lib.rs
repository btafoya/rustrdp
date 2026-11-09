//! Safe wrapper over libfreerdp. One FreeRDP thread per session; the UI talks to it
//! through `Command`s in and `Event`s out. All `unsafe` lives in this crate.

mod session;
mod settings;

pub use session::Session;

use std::sync::mpsc::SyncSender;

/// RDP pointer flags for `Command::Mouse` (protocol constants, MS-RDPBCGR 2.2.8.1.1.3.1.1.3).
pub mod ptr {
    pub const MOVE: u16 = 0x0800;
    pub const DOWN: u16 = 0x8000;
    pub const BUTTON1: u16 = 0x1000; // left
    pub const BUTTON2: u16 = 0x2000; // right
    pub const BUTTON3: u16 = 0x4000; // middle
    pub const WHEEL: u16 = 0x0200;
    pub const HWHEEL: u16 = 0x0400;
    pub const WHEEL_NEGATIVE: u16 = 0x0100;
}

/// Desktop pixels, BGRX, `stride` bytes per row.
#[derive(Default, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct CertInfo {
    pub host: String,
    pub port: u16,
    pub common_name: String,
    pub subject: String,
    pub issuer: String,
    pub fingerprint: String,
    /// The server presented a different certificate than the one stored.
    pub changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CertVerdict {
    Reject = 0,
    AcceptAndStore = 1,
    AcceptOnce = 2,
}

#[derive(Clone, Debug)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    pub domain: String,
}

/// Blocking requests: the FreeRDP thread waits for `reply`. Dropping `reply` answers "no".
pub enum Prompt {
    Credentials {
        username: String,
        domain: String,
        reply: SyncSender<Option<Credentials>>,
    },
    Certificate {
        info: CertInfo,
        reply: SyncSender<CertVerdict>,
    },
}

pub enum Event {
    Connected {
        width: u32,
        height: u32,
    },
    /// Remote desktop size changed; fetch pixels with `Session::frame()`.
    Resized {
        width: u32,
        height: u32,
    },
    /// Dirty rectangle in `Session::frame()`.
    Frame {
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    },
    ClipboardText(String),
    /// Informational server message (logon error info).
    Notice(String),
    Prompt(Prompt),
    Disconnected {
        reason: String,
        error: bool,
    },
}

pub enum Command {
    /// RDP scancode; bit 8 set for extended keys.
    Key {
        scancode: u32,
        down: bool,
    },
    Unicode {
        ch: u16,
        down: bool,
    },
    Mouse {
        flags: u16,
        x: u16,
        y: u16,
    },
    Resize {
        width: u32,
        height: u32,
    },
    SetClipboardText(String),
    Disconnect,
}

pub type Sink = Box<dyn Fn(Event) + Send + Sync>;

/// GDK keycode (X11 keycode; evdev + 8 on Wayland too) → RDP scancode, bit 8 set for extended keys.
/// Returns 0 for keys RDP has no scancode for.
pub fn scancode_from_keycode(keycode: u32) -> u32 {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| unsafe {
        freerdp_sys::freerdp_keyboard_init(0);
    });
    unsafe { freerdp_sys::freerdp_keyboard_get_rdp_scancode_from_x11_keycode(keycode) }
}
