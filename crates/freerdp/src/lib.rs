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
    /// The certificate trusted earlier; set when `changed`.
    pub old: Option<StoredCert>,
    /// Why the certificate failed verification (name mismatch, gateway, ...).
    pub notes: Vec<&'static str>,
}

#[derive(Clone, Debug)]
pub struct StoredCert {
    pub subject: String,
    pub issuer: String,
    pub fingerprint: String,
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
///
/// Uses WinPR's X11-keycode table (`WINPR_KEYCODE_TYPE_XKB`; the `EVDEV` table is indexed by evdev codes, 8 lower)
/// directly. `freerdp_keyboard_get_rdp_scancode_from_x11_keycode` returns 0 for every key when FreeRDP is built
/// without X11/Wayland support, as it is here.
pub fn scancode_from_keycode(keycode: u32) -> u32 {
    unsafe {
        let vk = freerdp_sys::GetVirtualKeyCodeFromKeycode(keycode, freerdp_sys::WINPR_KEYCODE_TYPE_XKB);
        freerdp_sys::GetVirtualScanCodeFromVirtualKeyCode(vk, freerdp_sys::WINPR_KBD_TYPE_IBM_ENHANCED)
    }
}

/// Scancodes currently held down, so they can all be released when focus is lost: a key-up that goes
/// to another window would otherwise leave the remote with a stuck Ctrl/Alt.
#[derive(Default)]
pub struct HeldKeys(std::collections::BTreeSet<u32>);

impl HeldKeys {
    pub fn press(&mut self, scancode: u32) {
        self.0.insert(scancode);
    }

    pub fn release(&mut self, scancode: u32) {
        self.0.remove(&scancode);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Everything still held, in scancode order; leaves the set empty.
    pub fn take_all(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.0).into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: FreeRDP built without X11/Wayland support never fills its own keycode table, so every key
    /// mapped to 0 and nothing could be typed. X11 keycode = evdev + 8.
    #[test]
    fn keycodes_map_to_rdp_scancodes_without_freerdp_x11_support() {
        for (name, keycode, scancode) in [
            ("a", 38, 0x1E),
            ("Return", 36, 0x1C),
            ("BackSpace", 22, 0x0E),
            ("space", 65, 0x39),
            ("F1", 67, 0x3B),
            ("Control_L", 37, 0x1D),
            ("Control_R", 105, 0x11D),
            ("Super_L", 133, 0x15B),
            ("Left", 113, 0x14B),
            ("Delete", 119, 0x153),
        ] {
            assert_eq!(scancode_from_keycode(keycode), scancode, "{name} (keycode {keycode})");
        }
        assert_eq!(scancode_from_keycode(0), 0, "no key");
    }

    #[test]
    fn held_keys_are_released_once_on_focus_loss() {
        let mut h = HeldKeys::default();
        h.press(0x1D); // Ctrl
        h.press(0x38); // Alt
        h.press(0x38); // auto-repeat
        h.press(0x1E); // A
        h.release(0x1E);
        assert_eq!(h.len(), 2);
        assert_eq!(h.take_all(), vec![0x1D, 0x38]);
        assert!(h.take_all().is_empty());
    }
}
