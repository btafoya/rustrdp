//! rustrdp: GTK4/libadwaita RDP client. Needs libgtk-4-dev and libadwaita-1-dev to build.

mod editor;
mod manager;
mod session_ui;

use adw::prelude::*;
use rdp_core::Store;
use std::{cell::RefCell, rc::Rc};

pub const APP_ID: &str = "com.briantafoya.rustrdp";

pub type SharedStore = Rc<RefCell<Store>>;

/// Modal-ish message with a single "OK".
pub fn alert(parent: &impl IsA<gtk::Widget>, heading: &str, body: &str) {
    let d = adw::AlertDialog::new(Some(heading), Some(body));
    d.add_response("ok", "OK");
    d.set_default_response(Some("ok"));
    d.set_close_response("ok");
    d.choose(parent, gtk::gio::Cancellable::NONE, |_| {});
}

fn main() -> gtk::glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        // Window/taskbar icon: looked up by name in the icon theme (hicolor/scalable/apps, installed by the packaging).
        gtk::Window::set_default_icon_name(APP_ID);
        // A corrupt profiles file must stop the app, never be overwritten by an empty store.
        match Store::open(Store::default_path()) {
            Ok(store) => manager::present(app, Rc::new(RefCell::new(store))),
            Err(e) => {
                let w = adw::ApplicationWindow::builder()
                    .application(app)
                    .default_width(480)
                    .default_height(200)
                    .build();
                w.present();
                let d = adw::AlertDialog::new(
                    Some("Cannot read profiles"),
                    Some(&format!("{e}\n\nThe file was left untouched.")),
                );
                d.add_response("quit", "Quit");
                let app = app.clone();
                d.choose(&w, gtk::gio::Cancellable::NONE, move |_| app.quit());
            }
        }
    });
    app.run()
}
