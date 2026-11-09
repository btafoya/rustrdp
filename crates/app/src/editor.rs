//! Profile editor dialog. Pages are stacked in one scrolling view (General, Display,
//! Redirection, Gateway, Advanced) instead of tabs: one Save applies everything.

use crate::{alert, SharedStore};
use adw::prelude::*;
use rdp_core::{Display, DisplayMode, Gateway, Profile, SharedFolder, ViewMode};
use std::{cell::RefCell, rc::Rc};
use uuid::Uuid;

pub fn present(parent: &adw::ApplicationWindow, store: SharedStore, existing: Option<Profile>, on_saved: Rc<dyn Fn()>) {
    let is_new = existing.is_none();
    let p = existing.unwrap_or_else(|| Profile::new("", ""));
    let groups: Vec<(Uuid, String)> = store.borrow().groups().iter().map(|g| (g.id, g.name.clone())).collect();

    let page = adw::PreferencesPage::new();

    // --- General
    let g = adw::PreferencesGroup::builder().title("Profile").build();
    let name = adw::EntryRow::builder().title("Name").text(p.name.as_str()).build();
    let mut group_names = vec!["No group".to_string()];
    group_names.extend(groups.iter().map(|(_, n)| n.clone()));
    let group_model = gtk::StringList::new(&group_names.iter().map(String::as_str).collect::<Vec<_>>());
    let group = adw::ComboRow::builder().title("Group").model(&group_model).build();
    group.set_selected(
        p.group
            .and_then(|id| groups.iter().position(|(g, _)| *g == id))
            .map_or(0, |i| i as u32 + 1),
    );
    let tags = adw::EntryRow::builder()
        .title("Tags (comma separated)")
        .text(p.tags.join(", "))
        .build();
    g.add(&name);
    g.add(&group);
    g.add(&tags);
    page.add(&g);

    let g = adw::PreferencesGroup::builder().title("Server").build();
    let host = adw::EntryRow::builder().title("Computer").text(p.host.as_str()).build();
    let port = adw::EntryRow::builder().title("Port").text(p.port.to_string()).build();
    let domain = adw::EntryRow::builder().title("Domain").text(p.domain.as_str()).build();
    for r in [&host, &port, &domain] {
        g.add(r);
    }
    page.add(&g);

    let g = adw::PreferencesGroup::builder()
        .title("Credentials")
        .description("The password is stored in the system keyring, never in the profile file.")
        .build();
    let user = adw::EntryRow::builder()
        .title("Username")
        .text(p.username.as_str())
        .build();
    let have_pw = !is_new && rdp_core::secrets::get_password(p.id).ok().flatten().is_some();
    let pw = adw::PasswordEntryRow::builder()
        .title(if have_pw {
            "Password (saved, type to replace)"
        } else {
            "Password"
        })
        .build();
    g.add(&user);
    g.add(&pw);
    page.add(&g);

    // --- Display
    let g = adw::PreferencesGroup::builder().title("Display").build();
    let mode_model = gtk::StringList::new(&["Match window (dynamic)", "Fixed size", "Fullscreen"]);
    let mode = adw::ComboRow::builder().title("Resolution").model(&mode_model).build();
    mode.set_selected(match p.display.mode {
        DisplayMode::Dynamic => 0,
        DisplayMode::Fixed => 1,
        DisplayMode::Fullscreen => 2,
    });
    let width = adw::EntryRow::builder()
        .title("Width (fixed size)")
        .text(p.display.width.to_string())
        .build();
    let height = adw::EntryRow::builder()
        .title("Height (fixed size)")
        .text(p.display.height.to_string())
        .build();
    let scale = adw::EntryRow::builder()
        .title("Scale % (100–500)")
        .text(p.display.scale.to_string())
        .build();
    let view_model = gtk::StringList::new(&["Actual size (scrollbars)", "Fit window to session", "Scale to window"]);
    let view_mode = adw::ComboRow::builder()
        .title("View (fixed size / fullscreen)")
        .model(&view_model)
        .build();
    view_mode.set_selected(match p.display.view {
        ViewMode::Actual => 0,
        ViewMode::FitWindow => 1,
        ViewMode::Scale => 2,
    });
    g.add(&mode);
    g.add(&view_mode);
    for r in [&width, &height, &scale] {
        g.add(r);
    }
    page.add(&g);

    // --- Redirection
    let g = adw::PreferencesGroup::builder()
        .title("Redirection")
        .description("Nothing is shared unless listed. Shared folders are read/write for the remote side.")
        .build();
    let clip = adw::SwitchRow::builder()
        .title("Clipboard (text)")
        .active(p.redirect.clipboard)
        .build();
    let audio = adw::SwitchRow::builder()
        .title("Play remote audio here")
        .active(p.redirect.audio)
        .build();
    let printers = adw::SwitchRow::builder()
        .title("Printers")
        .active(p.redirect.printers)
        .build();
    for r in [&clip, &audio, &printers] {
        g.add(r);
    }
    page.add(&g);

    let folders = Rc::new(RefCell::new(p.redirect.folders.clone()));
    let folder_group = adw::PreferencesGroup::builder().title("Shared folders").build();
    let add_folder = gtk::Button::builder()
        .label("Add folder")
        .valign(gtk::Align::Center)
        .build();
    folder_group.set_header_suffix(Some(&add_folder));
    let folder_rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));
    page.add(&folder_group);
    let render: Rc<dyn Fn()> = {
        let (folders, folder_group, folder_rows) = (folders.clone(), folder_group.clone(), folder_rows.clone());
        let me: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let me2 = me.clone();
        let f: Rc<dyn Fn()> = Rc::new(move || {
            for r in folder_rows.borrow_mut().drain(..) {
                folder_group.remove(&r);
            }
            for (i, f) in folders.borrow().iter().enumerate() {
                let row = adw::ActionRow::builder()
                    .title(f.name.as_str())
                    .subtitle(f.path.as_str())
                    .build();
                let rm = gtk::Button::builder()
                    .icon_name("user-trash-symbolic")
                    .tooltip_text("Remove")
                    .css_classes(["flat"])
                    .valign(gtk::Align::Center)
                    .build();
                row.add_suffix(&rm);
                let (folders, me) = (folders.clone(), me2.clone());
                rm.connect_clicked(move |_| {
                    folders.borrow_mut().remove(i);
                    if let Some(r) = me.borrow().as_ref() {
                        r();
                    }
                });
                folder_group.add(&row);
                folder_rows.borrow_mut().push(row);
            }
        });
        *me.borrow_mut() = Some(f.clone());
        f
    };
    render();

    // --- Gateway + RemoteApp
    let g = adw::PreferencesGroup::builder()
        .title("Gateway")
        .description("RD Gateway host, empty for a direct connection.")
        .build();
    let gw = p.gateway.clone().unwrap_or_default();
    let gw_host = adw::EntryRow::builder()
        .title("Gateway host")
        .text(gw.host.as_str())
        .build();
    let gw_user = adw::EntryRow::builder()
        .title("Gateway username")
        .text(gw.username.as_str())
        .build();
    let gw_domain = adw::EntryRow::builder()
        .title("Gateway domain")
        .text(gw.domain.as_str())
        .build();
    for r in [&gw_host, &gw_user, &gw_domain] {
        g.add(r);
    }
    page.add(&g);

    let g = adw::PreferencesGroup::builder().title("RemoteApp").description("Program alias or path, e.g. ||excel. Empty opens the full desktop. Window placement follows the compositor on Wayland.").build();
    let app = adw::EntryRow::builder()
        .title("Program")
        .text(p.remote_app.clone().unwrap_or_default())
        .build();
    g.add(&app);
    page.add(&g);

    // --- Advanced
    let g = adw::PreferencesGroup::builder()
        .title("Advanced FreeRDP settings")
        .description("One NAME=VALUE per line, e.g. FreeRDP_IgnoreCertificate=TRUE. Applied last.")
        .build();
    let text = gtk::TextView::builder()
        .monospace(true)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();
    text.buffer().set_text(
        &p.overrides
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let frame = gtk::ScrolledWindow::builder()
        .child(&text)
        .min_content_height(90)
        .css_classes(["card"])
        .build();
    g.add(&frame);
    page.add(&g);

    // --- window
    let save = gtk::Button::builder()
        .label("Save")
        .css_classes(["suggested-action"])
        .build();
    let header = adw::HeaderBar::new();
    header.pack_end(&save);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(if is_new { "New connection" } else { "Edit connection" })
        .content_width(560)
        .content_height(760)
        .child(&view)
        .build();

    if !is_new {
        let delete = gtk::Button::builder()
            .label("Delete")
            .css_classes(["destructive-action"])
            .build();
        header.pack_start(&delete);
        let (store, on_saved, dialog, id) = (store.clone(), on_saved.clone(), dialog.clone(), p.id);
        delete.connect_clicked(move |_| {
            store.borrow_mut().remove(id);
            let _ = rdp_core::secrets::delete_password(id);
            on_saved();
            dialog.close();
        });
    }

    add_folder.connect_clicked({
        let (folders, render, parent) = (folders.clone(), render.clone(), parent.clone());
        move |_| {
            let fd = gtk::FileDialog::builder().title("Share folder").build();
            let (folders, render) = (folders.clone(), render.clone());
            fd.select_folder(Some(&parent), gtk::gio::Cancellable::NONE, move |res| {
                let Ok(file) = res else { return };
                let Some(path) = file.path() else { return };
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "share".into());
                folders.borrow_mut().push(SharedFolder {
                    name,
                    path: path.to_string_lossy().into_owned(),
                });
                render();
            });
        }
    });

    save.connect_clicked({
        let dialog = dialog.clone();
        let parent = parent.clone();
        move |_| {
            let fail = |msg: &str| alert(&dialog, "Cannot save", msg);
            if name.text().trim().is_empty() || host.text().trim().is_empty() {
                return fail("Name and computer are required.");
            }
            let Ok(port_n) = port.text().trim().parse::<u16>() else {
                return fail("Port must be 1–65535.");
            };
            let (Ok(w), Ok(h), Ok(sc)) = (
                width.text().trim().parse::<u32>(),
                height.text().trim().parse::<u32>(),
                scale.text().trim().parse::<u32>(),
            ) else {
                return fail("Width, height and scale must be numbers.");
            };
            let buf = text.buffer();
            let raw = buf.text(&buf.start_iter(), &buf.end_iter(), false);
            let mut overrides = std::collections::BTreeMap::new();
            for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let Some((k, v)) = line.split_once('=') else {
                    return fail(&format!("Advanced setting “{line}” needs NAME=VALUE."));
                };
                overrides.insert(k.trim().to_string(), v.trim().to_string());
            }
            let mut out = p.clone();
            out.name = name.text().trim().to_string();
            out.group = match group.selected() {
                0 => None,
                i => groups.get(i as usize - 1).map(|(id, _)| *id),
            };
            out.tags = tags
                .text()
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
            out.host = host.text().trim().to_string();
            out.port = port_n;
            out.domain = domain.text().trim().to_string();
            out.username = user.text().trim().to_string();
            out.display = Display {
                mode: match mode.selected() {
                    1 => DisplayMode::Fixed,
                    2 => DisplayMode::Fullscreen,
                    _ => DisplayMode::Dynamic,
                },
                width: w,
                height: h,
                scale: sc.clamp(100, 500),
                view: match view_mode.selected() {
                    1 => ViewMode::FitWindow,
                    2 => ViewMode::Scale,
                    _ => ViewMode::Actual,
                },
            };
            out.redirect.clipboard = clip.is_active();
            out.redirect.audio = audio.is_active();
            out.redirect.printers = printers.is_active();
            out.redirect.folders = folders.borrow().clone();
            out.gateway = (!gw_host.text().trim().is_empty()).then(|| Gateway {
                host: gw_host.text().trim().to_string(),
                username: gw_user.text().trim().to_string(),
                domain: gw_domain.text().trim().to_string(),
            });
            out.remote_app = Some(app.text().trim().to_string()).filter(|s| !s.is_empty());
            out.overrides = overrides;

            let pw_text = pw.text();
            if !pw_text.is_empty() {
                if let Err(e) = rdp_core::secrets::set_password(out.id, &pw_text) {
                    return alert(&parent, "Could not store the password", &e.to_string());
                }
            }
            store.borrow_mut().upsert(out);
            on_saved();
            dialog.close();
        }
    });

    dialog.present(Some(parent));
}
