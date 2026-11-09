//! One window per session: framebuffer view, input, clipboard bridge, prompts.
//! RAIL: the engine accepts `remote_app` profiles; per-window GTK rendering of RemoteApp
//! windows is not implemented yet, so a RemoteApp session shows the whole remote desktop surface.

use crate::alert;
use adw::prelude::*;
use freerdp::{ptr, CertInfo, CertVerdict, Command, Credentials, Event, Prompt, Session};
use gtk::{cairo, gio, glib};
use rdp_core::{DisplayMode, Profile, ViewMode};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

type Shared = Rc<RefCell<Option<Session>>>;
type Surface = Rc<RefCell<Option<cairo::ImageSurface>>>;

fn send(s: &Shared, cmd: Command) {
    if let Some(sess) = s.borrow().as_ref() {
        sess.send(cmd);
    }
}

/// How the remote desktop of a fixed size is laid out in the window (see `ViewMode`).
struct View {
    mode: Cell<ViewMode>,
    /// "Match window": the server resizes the desktop to the window (needs display control; Dynamic profiles).
    dynamic: Cell<bool>,
    /// Remote desktop size; (0, 0) until connected.
    size: Cell<(u32, u32)>,
    area: gtk::DrawingArea,
    scroll: gtk::ScrolledWindow,
    window: adw::ApplicationWindow,
    header: adw::HeaderBar,
    toasts: adw::ToastOverlay,
}

impl View {
    /// (scale, x offset, y offset) from remote to widget coordinates.
    fn transform(&self) -> (f64, f64, f64) {
        let (sw, sh) = self.size.get();
        let (aw, ah) = (self.area.width() as f64, self.area.height() as f64);
        if self.mode.get() != ViewMode::Scale || sw == 0 || sh == 0 || aw == 0.0 || ah == 0.0 {
            return (1.0, 0.0, 0.0);
        }
        let s = (aw / sw as f64).min(ah / sh as f64);
        (s, (aw - sw as f64 * s) / 2.0, (ah - sh as f64 * s) / 2.0)
    }

    /// Widget point to remote pixel, clamped to the desktop.
    fn to_remote(&self, x: f64, y: f64) -> (u16, u16) {
        let (s, ox, oy) = self.transform();
        let (sw, sh) = self.size.get();
        let cx = ((x - ox) / s).clamp(0.0, sw.saturating_sub(1) as f64);
        let cy = ((y - oy) / s).clamp(0.0, sh.saturating_sub(1) as f64);
        (cx as u16, cy as u16)
    }

    fn screen_size(&self) -> Option<(i32, i32)> {
        let surface = self.window.surface()?;
        let g = WidgetExt::display(&self.window)
            .monitor_at_surface(&surface)?
            .geometry();
        Some((g.width(), g.height()))
    }

    /// Re-derive scrolling and window size from the mode and the remote size.
    fn apply(&self) {
        let (sw, sh) = self.size.get();
        if sw == 0 || sh == 0 {
            return;
        }
        if self.dynamic.get() {
            // The desktop follows the window, so the area just fills it.
            self.scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
            self.area.set_content_width(0);
            self.area.set_content_height(0);
            self.area.queue_draw();
            return;
        }
        let chrome = self.header.height();
        if self.mode.get() == ViewMode::FitWindow {
            if let Some((mw, mh)) = self.screen_size() {
                if sw as i32 > mw || sh as i32 + chrome > mh {
                    self.mode.set(ViewMode::Scale);
                    if let Some(a) = self.window.lookup_action("view").and_downcast::<gio::SimpleAction>() {
                        a.set_state(&"scale".to_variant());
                    }
                    self.toasts
                        .add_toast(adw::Toast::new("Session is larger than the screen: scaling to fit"));
                }
            }
        }
        match self.mode.get() {
            ViewMode::Scale => {
                self.scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
                self.area.set_content_width(0);
                self.area.set_content_height(0);
                self.snap();
            }
            mode => {
                let policy = if mode == ViewMode::Actual {
                    gtk::PolicyType::Automatic
                } else {
                    gtk::PolicyType::Never
                };
                self.scroll.set_policy(policy, policy);
                self.area.set_content_width(sw as i32);
                self.area.set_content_height(sh as i32);
                if mode == ViewMode::FitWindow && !self.window.is_maximized() && !self.window.is_fullscreen() {
                    let (fw, fh) = self.frame();
                    self.window.set_default_size(sw as i32 + fw, sh as i32 + chrome + fh);
                }
            }
        }
        self.area.queue_draw();
    }

    /// Scale mode: match the window's aspect ratio to the desktop so there are no bars.
    fn snap(&self) {
        let (sw, sh) = self.size.get();
        if self.mode.get() != ViewMode::Scale || sw == 0 || self.window.is_maximized() || self.window.is_fullscreen() {
            return;
        }
        let (aw, ah) = (self.area.width(), self.area.height());
        let delta = (aw as f64 * sh as f64 / sw as f64).round() as i32 - ah;
        if aw > 0 && delta.abs() > 2 {
            let (fw, fh) = self.frame();
            self.window
                .set_default_size(self.window.width() + fw, self.window.height() + fh + delta);
        }
    }

    /// Size the toplevel surface has beyond the window widget (client-side shadow); default sizes include it.
    fn frame(&self) -> (i32, i32) {
        self.window.surface().map_or((0, 0), |s| {
            (s.width() - self.window.width(), s.height() - self.window.height())
        })
    }
}

const WINDOW_SIZE: (i32, i32) = (1280, 800);

/// `RUSTRDP_DEBUG_KEYS=1` prints key, focus and send details to stderr (for "keyboard does nothing" reports).
fn debug_keys() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUSTRDP_DEBUG_KEYS").is_some())
}

/// The key's character as a UTF-16 unit (BMP only), for keys without an RDP scancode.
fn unicode_of(keyval: gtk::gdk::Key) -> Option<u16> {
    keyval.to_unicode().and_then(|c| u16::try_from(c as u32).ok())
}

pub fn open(app: &adw::Application, mut profile: Profile, password: Option<String>) {
    // Dynamic resolution: start the desktop at about the drawing area's size (window minus header bar and
    // shadow) instead of the profile's, so the first frame already fits. Event::Connected corrects any difference.
    if profile.display.mode == DisplayMode::Dynamic {
        profile.display.width = (WINDOW_SIZE.0 - 10) as u32;
        profile.display.height = (WINDOW_SIZE.1 - 56) as u32;
    }
    let (tx, rx) = async_channel::unbounded::<Event>();
    let session = match Session::start(
        profile.clone(),
        password,
        Box::new(move |e| {
            let _ = tx.send_blocking(e);
        }),
    ) {
        Ok(s) => s,
        Err(e) => {
            let w = adw::ApplicationWindow::builder().application(app).build();
            w.present();
            return alert(&w, "Could not start the session", &e);
        }
    };
    let session: Shared = Rc::new(RefCell::new(Some(session)));
    let surface: Surface = Rc::new(RefCell::new(None));
    let dynamic = profile.display.mode == DisplayMode::Dynamic;

    let area = gtk::DrawingArea::builder()
        .focusable(true)
        .can_focus(true)
        .hexpand(true)
        .vexpand(true)
        .build();
    let scroll = gtk::ScrolledWindow::builder().child(&area).build();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&scroll));

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(
        &profile.name,
        &format!("{}:{}", profile.host, profile.port),
    )));
    // Header buttons must not take keyboard focus: it belongs to the remote desktop, and a focused button
    // swallows every keystroke until the desktop is clicked again.
    let cad = gtk::Button::builder()
        .label("Ctrl+Alt+Del")
        .focus_on_click(false)
        .build();
    let full = gtk::Button::builder()
        .icon_name("view-fullscreen-symbolic")
        .tooltip_text("Fullscreen")
        .focus_on_click(false)
        .build();
    let bye = gtk::Button::builder()
        .label("Disconnect")
        .css_classes(["destructive-action"])
        .focus_on_click(false)
        .build();
    header.pack_start(&cad);
    header.pack_end(&bye);
    header.pack_end(&full);
    let menu = gio::Menu::new();
    if dynamic {
        menu.append(Some("Match window (resize desktop)"), Some("win.view::match"));
    }
    menu.append(Some("Actual size (scroll)"), Some("win.view::actual"));
    menu.append(Some("Fit window to session"), Some("win.view::fit"));
    menu.append(Some("Scale session to window"), Some("win.view::scale"));
    let view_button = gtk::MenuButton::builder()
        .icon_name("zoom-fit-best-symbolic")
        .tooltip_text("View")
        .menu_model(&menu)
        .focus_on_click(false)
        .build();
    if let Some(popover) = view_button.popover() {
        // GTK hands focus back to the menu button when the popover closes; give it to the desktop instead.
        popover.connect_closed({
            let area = area.clone();
            move |_| {
                area.grab_focus();
            }
        });
    }
    header.pack_end(&view_button);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&toasts));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(profile.name.as_str())
        .default_width(WINDOW_SIZE.0)
        .default_height(WINDOW_SIZE.1)
        .content(&toolbar)
        .build();
    if profile.display.mode == DisplayMode::Fullscreen {
        window.fullscreen();
    }

    // A Dynamic profile starts as "Match window" (the desktop follows the window); the View menu can leave it.
    let initial = if dynamic {
        ViewMode::Actual
    } else {
        profile.display.view
    };
    let view = Rc::new(View {
        mode: Cell::new(initial),
        dynamic: Cell::new(dynamic),
        size: Cell::new((0, 0)),
        area: area.clone(),
        scroll: scroll.clone(),
        window: window.clone(),
        header: header.clone(),
        toasts: toasts.clone(),
    });
    {
        let name = if dynamic {
            "match"
        } else {
            match initial {
                ViewMode::Actual => "actual",
                ViewMode::FitWindow => "fit",
                ViewMode::Scale => "scale",
            }
        };
        let action = gio::SimpleAction::new_stateful("view", Some(glib::VariantTy::STRING), &name.to_variant());
        action.connect_activate({
            let (view, session) = (view.clone(), session.clone());
            move |a, param| {
                let Some(param) = param else { return };
                let choice = param.str().unwrap_or("actual");
                view.dynamic.set(choice == "match");
                view.mode.set(match choice {
                    "fit" => ViewMode::FitWindow,
                    "scale" => ViewMode::Scale,
                    _ => ViewMode::Actual,
                });
                a.set_state(param);
                view.apply();
                if choice == "match" {
                    // Ask the server for a desktop the size of the window right away.
                    let (w, h) = (view.area.width(), view.area.height());
                    if w > 0 && h > 0 {
                        send(
                            &session,
                            Command::Resize {
                                width: w as u32,
                                height: h as u32,
                            },
                        );
                    }
                }
            }
        });
        window.add_action(&action);
        app.set_accels_for_action("win.view::fit", &["<Ctrl><Alt>1"]);
        app.set_accels_for_action("win.view::scale", &["<Ctrl><Alt>2"]);
        area.connect_resize({
            let view = view.clone();
            let generation = Rc::new(Cell::new(0u32));
            move |_, _, _| {
                generation.set(generation.get() + 1);
                let (g, generation, view) = (generation.get(), generation.clone(), view.clone());
                glib::timeout_add_local_once(Duration::from_millis(250), move || {
                    if generation.get() == g {
                        view.snap();
                    }
                });
            }
        });
    }

    // ---- drawing
    area.set_draw_func({
        let (surface, view) = (surface.clone(), view.clone());
        move |_, cr, _, _| {
            if let Some(s) = surface.borrow().as_ref() {
                let (scale, ox, oy) = view.transform();
                if view.mode.get() == ViewMode::Scale {
                    cr.set_source_rgb(0.0, 0.0, 0.0);
                    let _ = cr.paint();
                }
                cr.translate(ox, oy);
                cr.scale(scale, scale);
                if cr.set_source_surface(s, 0.0, 0.0).is_ok() {
                    cr.source().set_filter(cairo::Filter::Good);
                    let _ = cr.paint();
                }
            }
        }
    });

    // ---- input
    let key = gtk::EventControllerKey::new();
    let held = Rc::new(RefCell::new(freerdp::HeldKeys::default()));
    key.connect_key_pressed({
        let (s, held) = (session.clone(), held.clone());
        move |_, keyval, keycode, state| {
            let sc = freerdp::scancode_from_keycode(keycode);
            if debug_keys() {
                eprintln!("[keys] press keycode={keycode} keyval={keyval:?} state={state:?} scancode=0x{sc:x}");
            }
            if sc != 0 {
                held.borrow_mut().press(sc);
                send(
                    &s,
                    Command::Key {
                        scancode: sc,
                        down: true,
                    },
                );
            } else if let Some(ch) = unicode_of(keyval) {
                // No RDP scancode for this key: type the character instead.
                send(&s, Command::Unicode { ch, down: true });
            }
            glib::Propagation::Stop
        }
    });
    key.connect_key_released({
        let (s, held) = (session.clone(), held.clone());
        move |_, keyval, keycode, _| {
            let sc = freerdp::scancode_from_keycode(keycode);
            if sc != 0 {
                held.borrow_mut().release(sc);
                send(
                    &s,
                    Command::Key {
                        scancode: sc,
                        down: false,
                    },
                );
            } else if let Some(ch) = unicode_of(keyval) {
                send(&s, Command::Unicode { ch, down: false });
            }
        }
    });
    area.add_controller(key);

    // Focus leaving (menu, dialog, Alt+Tab) swallows the key-ups: release what is held so the remote has no stuck modifier.
    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter(|_| {
        if debug_keys() {
            eprintln!("[keys] desktop gained keyboard focus");
        }
    });
    focus.connect_leave({
        let (s, held) = (session.clone(), held.clone());
        move |_| {
            if debug_keys() {
                eprintln!(
                    "[keys] desktop lost keyboard focus; releasing {} held keys",
                    held.borrow().len()
                );
            }
            for scancode in held.borrow_mut().take_all() {
                send(&s, Command::Key { scancode, down: false });
            }
        }
    });
    area.add_controller(focus);

    let click = gtk::GestureClick::builder().button(0).build();
    let button_flag = |b: u32| match b {
        1 => ptr::BUTTON1,
        2 => ptr::BUTTON3, // GDK 2 = middle
        3 => ptr::BUTTON2, // GDK 3 = right
        _ => 0,
    };
    click.connect_pressed({
        let (s, area, view) = (session.clone(), area.clone(), view.clone());
        move |g, _, x, y| {
            area.grab_focus();
            let f = button_flag(g.current_button());
            if f != 0 {
                let (x, y) = view.to_remote(x, y);
                send(
                    &s,
                    Command::Mouse {
                        flags: ptr::DOWN | f,
                        x,
                        y,
                    },
                );
            }
        }
    });
    click.connect_released({
        let (s, view) = (session.clone(), view.clone());
        move |g, _, x, y| {
            let f = button_flag(g.current_button());
            if f != 0 {
                let (x, y) = view.to_remote(x, y);
                send(&s, Command::Mouse { flags: f, x, y });
            }
        }
    });
    area.add_controller(click);

    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let (s, view) = (session.clone(), view.clone());
        move |_, x, y| {
            let (x, y) = view.to_remote(x, y);
            send(&s, Command::Mouse { flags: ptr::MOVE, x, y })
        }
    });
    area.add_controller(motion);

    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    wheel.connect_scroll({
        let s = session.clone();
        move |_, _, dy| {
            // one notch = 120; negative (scroll down) is a 9-bit two's complement with the sign flag
            let flags = if dy > 0.0 {
                ptr::WHEEL | ptr::WHEEL_NEGATIVE | 0x88
            } else {
                ptr::WHEEL | 0x78
            };
            send(&s, Command::Mouse { flags, x: 0, y: 0 });
            glib::Propagation::Stop
        }
    });
    area.add_controller(wheel);

    // ---- resize (debounced) while "Match window" is on
    if dynamic {
        let generation = Rc::new(Cell::new(0u32));
        area.connect_resize({
            let (s, view) = (session.clone(), view.clone());
            move |_, w, h| {
                generation.set(generation.get() + 1);
                let (g, generation, s, view) = (generation.get(), generation.clone(), s.clone(), view.clone());
                glib::timeout_add_local_once(Duration::from_millis(150), move || {
                    if generation.get() == g && view.dynamic.get() && w > 0 && h > 0 {
                        send(
                            &s,
                            Command::Resize {
                                width: w as u32,
                                height: h as u32,
                            },
                        );
                    }
                });
            }
        });
    }

    // ---- clipboard (local → remote)
    if profile.redirect.clipboard {
        let clipboard = area.clipboard();
        clipboard.connect_changed({
            let s = session.clone();
            move |cb| {
                let s = s.clone();
                cb.read_text_async(gio::Cancellable::NONE, move |res| {
                    if let Ok(Some(t)) = res {
                        send(&s, Command::SetClipboardText(t.to_string()));
                    }
                });
            }
        });
    }

    // ---- toolbar
    cad.connect_clicked({
        let (s, area) = (session.clone(), area.clone());
        move |_| {
            area.grab_focus();
            // Ctrl (0x1D), Alt (0x38), Delete (extended 0x153): down in order, up reversed
            let keys = [0x1Du32, 0x38, 0x153];
            for k in keys {
                send(
                    &s,
                    Command::Key {
                        scancode: k,
                        down: true,
                    },
                );
            }
            for k in keys.iter().rev() {
                send(
                    &s,
                    Command::Key {
                        scancode: *k,
                        down: false,
                    },
                );
            }
        }
    });
    full.connect_clicked({
        let (w, area) = (window.clone(), area.clone());
        move |_| {
            w.set_fullscreened(!w.is_fullscreen());
            area.grab_focus();
        }
    });
    bye.connect_clicked({
        let s = session.clone();
        move |_| send(&s, Command::Disconnect)
    });

    // Dropping a Session joins its thread: do that off the UI thread so a pending prompt cannot deadlock.
    window.connect_close_request({
        let s = session.clone();
        move |_| {
            if let Some(sess) = s.borrow_mut().take() {
                std::thread::spawn(move || drop(sess));
            }
            glib::Propagation::Proceed
        }
    });

    // ---- events from the FreeRDP thread
    glib::spawn_future_local({
        let (session, surface, area, window, toasts, view) = (
            session.clone(),
            surface.clone(),
            area.clone(),
            window.clone(),
            toasts.clone(),
            view.clone(),
        );
        async move {
            while let Ok(ev) = rx.recv().await {
                let connected = matches!(ev, Event::Connected { .. });
                match ev {
                    Event::Connected { width, height } | Event::Resized { width, height } => {
                        view.size.set((width, height));
                        view.apply();
                        reset_surface(&session, &surface, width, height);
                        area.queue_draw();
                        area.grab_focus();
                        // The desktop starts at an estimated size; ask for the real window size once connected
                        // (the engine holds it until display control is up). Only on Connected: no resize loop.
                        let (w, h) = (area.width(), area.height());
                        if connected && view.dynamic.get() && w > 0 && h > 0 && (w as u32, h as u32) != (width, height)
                        {
                            send(
                                &session,
                                Command::Resize {
                                    width: w as u32,
                                    height: h as u32,
                                },
                            );
                        }
                    }
                    Event::Frame { x, y, w, h } => {
                        blit(&session, &surface, x, y, w, h);
                        area.queue_draw();
                    }
                    Event::ClipboardText(t) => area.clipboard().set_text(&t),
                    Event::Notice(m) => toasts.add_toast(adw::Toast::new(&m)),
                    Event::Prompt(Prompt::Certificate { info, reply }) => cert_dialog(&window, info, reply),
                    Event::Prompt(Prompt::Credentials {
                        username,
                        domain,
                        reply,
                    }) => credentials_dialog(&window, &username, &domain, reply),
                    Event::Disconnected { reason, error } => {
                        // the thread is finished: dropping joins immediately
                        drop(session.borrow_mut().take());
                        if error {
                            let w2 = window.clone();
                            let d = adw::AlertDialog::new(Some("Disconnected"), Some(&reason));
                            d.add_response("ok", "Close");
                            d.choose(&window, gio::Cancellable::NONE, move |_| w2.close());
                        } else {
                            window.close();
                        }
                        break;
                    }
                }
            }
        }
    });

    window.present();
    area.grab_focus();
}

/// New cairo surface sized to the remote desktop, filled from the current frame.
fn reset_surface(session: &Shared, surface: &Surface, w: u32, h: u32) {
    let Ok(s) = cairo::ImageSurface::create(cairo::Format::Rgb24, w as i32, h as i32) else {
        return;
    };
    *surface.borrow_mut() = Some(s);
    blit(session, surface, 0, 0, w, h);
}

/// Copy a dirty rectangle from the FreeRDP framebuffer (BGRX) into the cairo surface (Rgb24 = BGRX on little endian).
fn blit(session: &Shared, surface: &Surface, x: u32, y: u32, w: u32, h: u32) {
    let sess = session.borrow();
    let Some(sess) = sess.as_ref() else { return };
    let mut surf = surface.borrow_mut();
    let Some(surf) = surf.as_mut() else { return };
    let dst_stride = surf.stride() as usize;
    let (sw, sh) = (surf.width() as u32, surf.height() as u32);
    let frame = sess.frame();
    if frame.width != sw || frame.height != sh {
        return; // a Resized event for the new size is on its way
    }
    let src_stride = frame.stride as usize;
    let Ok(mut dst) = surf.data() else { return };
    let (x1, y1) = ((x + w).min(sw) as usize, (y + h).min(sh) as usize);
    for row in y as usize..y1 {
        let a = x as usize * 4;
        let b = x1 * 4;
        if b > a {
            dst[row * dst_stride + a..row * dst_stride + b]
                .copy_from_slice(&frame.data[row * src_stride + a..row * src_stride + b]);
        }
    }
}

fn dash(s: &str) -> &str {
    if s.is_empty() {
        "(none)"
    } else {
        s
    }
}

fn cert_dialog(parent: &adw::ApplicationWindow, info: CertInfo, reply: std::sync::mpsc::SyncSender<CertVerdict>) {
    let heading = if info.changed {
        "Certificate changed"
    } else {
        "Certificate not trusted"
    };
    let mut body = format!("{}:{}\n", info.host, info.port);
    for note in &info.notes {
        body.push_str(&format!("\n{note}"));
    }
    body.push_str(&format!(
        "\n\n{}Subject: {}\nIssuer: {}\nSHA-256: {}",
        if info.changed { "New certificate\n" } else { "" },
        dash(&info.subject),
        dash(&info.issuer),
        info.fingerprint
    ));
    if let Some(old) = &info.old {
        body.push_str(&format!(
            "\n\nStored certificate\nSubject: {}\nIssuer: {}\nSHA-256: {}\n\nThe server presented a different certificate than the one you trusted.",
            dash(&old.subject),
            dash(&old.issuer),
            old.fingerprint
        ));
    }
    body.push_str("\n\nCompare the fingerprint with the one on the server before continuing.");
    let d = adw::AlertDialog::new(Some(heading), Some(&body));
    d.add_response("reject", "Cancel");
    d.add_response("once", "Connect once");
    d.add_response("store", "Trust and remember");
    d.set_response_appearance(
        "store",
        if info.changed {
            adw::ResponseAppearance::Destructive
        } else {
            adw::ResponseAppearance::Suggested
        },
    );
    d.set_default_response(Some("reject"));
    d.set_close_response("reject");
    d.choose(parent, gio::Cancellable::NONE, move |resp| {
        let v = match resp.as_str() {
            "store" => CertVerdict::AcceptAndStore,
            "once" => CertVerdict::AcceptOnce,
            _ => CertVerdict::Reject,
        };
        let _ = reply.send(v);
    });
}

fn credentials_dialog(
    parent: &adw::ApplicationWindow,
    username: &str,
    domain: &str,
    reply: std::sync::mpsc::SyncSender<Option<Credentials>>,
) {
    let user = gtk::Entry::builder()
        .placeholder_text("Username")
        .text(username)
        .build();
    let pass = gtk::PasswordEntry::builder()
        .placeholder_text("Password")
        .show_peek_icon(true)
        .build();
    let dom = gtk::Entry::builder().placeholder_text("Domain").text(domain).build();
    let col = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    for w in [user.upcast_ref::<gtk::Widget>(), pass.upcast_ref(), dom.upcast_ref()] {
        col.append(w);
    }
    let d = adw::AlertDialog::new(Some("Sign in"), Some("The server asks for credentials."));
    d.set_extra_child(Some(&col));
    d.add_response("cancel", "Cancel");
    d.add_response("ok", "Connect");
    d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("ok"));
    d.set_close_response("cancel");
    d.choose(parent, gio::Cancellable::NONE, move |resp| {
        let creds = (resp == "ok").then(|| Credentials {
            username: user.text().to_string(),
            password: pass.text().to_string(),
            domain: dom.text().to_string(),
        });
        let _ = reply.send(creds);
    });
}
