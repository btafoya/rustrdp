//! Connection manager window: sidebar (All / Favorites / Recent / groups), search, host list
//! with reachability dots, new/edit/delete, connect.

use crate::{alert, editor, session_ui, SharedStore};
use adw::prelude::*;
use rdp_core::{probe, Filter, Profile, Reach};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::Duration,
};
use uuid::Uuid;

struct Ui {
    app: adw::Application,
    store: SharedStore,
    window: adw::ApplicationWindow,
    list: gtk::ListBox,
    sidebar: gtk::ListBox,
    title: gtk::Label,
    search: gtk::SearchEntry,
    delete_group: gtk::Button,
    filter: Cell<Filter>,
    /// Sidebar row index → filter. Rebuilt with the sidebar.
    sidebar_filters: RefCell<Vec<Filter>>,
    dots: RefCell<HashMap<Uuid, gtk::Label>>,
    reach: RefCell<HashMap<Uuid, Reach>>,
}

pub fn present(app: &adw::Application, store: SharedStore) {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let sidebar = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["navigation-sidebar"])
        .build();
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search name, host, tag")
        .hexpand(true)
        .build();
    let title = gtk::Label::builder()
        .label("All connections")
        .css_classes(["title-2"])
        .halign(gtk::Align::Start)
        .build();
    let delete_group = gtk::Button::builder().label("Delete group").sensitive(false).build();
    let add_group = gtk::Button::builder().label("Add group").build();
    let import = gtk::Button::builder().label("Import .rdp…").build();

    // sidebar
    let side_footer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    for b in [&add_group, &delete_group, &import] {
        side_footer.append(b);
    }
    let side_scroll = gtk::ScrolledWindow::builder()
        .child(&sidebar)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let side_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    side_box.append(&side_scroll);
    side_box.append(&side_footer);
    let side_header = adw::HeaderBar::builder()
        .show_end_title_buttons(false)
        .title_widget(&gtk::Label::new(Some("Connections")))
        .build();
    let side_view = adw::ToolbarView::new();
    side_view.add_top_bar(&side_header);
    side_view.set_content(Some(&side_box));

    // content
    let new_btn = gtk::Button::builder()
        .label("New connection")
        .css_classes(["suggested-action"])
        .build();
    let header = adw::HeaderBar::new();
    header.pack_start(&new_btn);
    header.set_title_widget(Some(&search));
    let clamp = adw::Clamp::builder()
        .maximum_size(900)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    let content_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    content_box.append(&title);
    content_box.append(&list);
    clamp.set_child(Some(&content_box));
    let scroll = gtk::ScrolledWindow::builder().child(&clamp).vexpand(true).build();
    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&header);
    content_view.set_content(Some(&scroll));

    let split = adw::OverlaySplitView::new();
    split.set_sidebar(Some(&side_view));
    split.set_content(Some(&content_view));
    split.set_min_sidebar_width(220.0);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("rustrdp")
        .default_width(1100)
        .default_height(700)
        .content(&split)
        .build();

    let ui = Rc::new(Ui {
        app: app.clone(),
        store,
        window: window.clone(),
        list,
        sidebar,
        title,
        search,
        delete_group,
        filter: Cell::new(Filter::All),
        sidebar_filters: RefCell::new(Vec::new()),
        dots: RefCell::new(HashMap::new()),
        reach: RefCell::new(HashMap::new()),
    });

    rebuild_sidebar(&ui);
    refresh(&ui);

    ui.search.connect_search_changed({
        let ui = ui.clone();
        move |_| refresh(&ui)
    });
    ui.sidebar.connect_row_selected({
        let ui = ui.clone();
        move |_, row| {
            let Some(row) = row else { return };
            let f = ui.sidebar_filters.borrow().get(row.index() as usize).copied();
            if let Some(f) = f {
                ui.filter.set(f);
                ui.delete_group.set_sensitive(matches!(f, Filter::Group(_)));
                refresh(&ui);
            }
        }
    });
    new_btn.connect_clicked({
        let ui = ui.clone();
        move |_| open_editor(&ui, None)
    });
    add_group.connect_clicked({
        let ui = ui.clone();
        move |_| prompt_new_group(&ui)
    });
    ui.delete_group.connect_clicked({
        let ui = ui.clone();
        move |_| {
            if let Filter::Group(g) = ui.filter.get() {
                ui.store.borrow_mut().remove_group(g);
                save(&ui);
                ui.filter.set(Filter::All);
                rebuild_sidebar(&ui);
                refresh(&ui);
            }
        }
    });
    import.connect_clicked({
        let ui = ui.clone();
        move |_| import_rdp(&ui)
    });

    // reachability every 60 s for what is on screen
    gtk::glib::timeout_add_seconds_local(60, {
        let ui = ui.clone();
        move || {
            probe_visible(&ui);
            gtk::glib::ControlFlow::Continue
        }
    });

    window.present();
}

fn save(ui: &Rc<Ui>) {
    if let Err(e) = ui.store.borrow().save() {
        alert(&ui.window, "Could not save profiles", &e.to_string());
    }
}

fn rebuild_sidebar(ui: &Rc<Ui>) {
    while let Some(c) = ui.sidebar.first_child() {
        ui.sidebar.remove(&c);
    }
    let mut filters = vec![Filter::All, Filter::Favorites, Filter::Recent];
    let mut labels = vec!["All connections".to_string(), "Favorites".into(), "Recent".into()];
    for g in ui.store.borrow().groups() {
        filters.push(Filter::Group(g.id));
        labels.push(g.name.clone());
    }
    for l in &labels {
        let lbl = gtk::Label::builder()
            .label(l.as_str())
            .halign(gtk::Align::Start)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(6)
            .build();
        ui.sidebar.append(&lbl);
    }
    let selected = filters.iter().position(|f| *f == ui.filter.get()).unwrap_or(0);
    *ui.sidebar_filters.borrow_mut() = filters;
    if let Some(row) = ui.sidebar.row_at_index(selected as i32) {
        ui.sidebar.select_row(Some(&row));
    }
}

fn refresh(ui: &Rc<Ui>) {
    while let Some(c) = ui.list.first_child() {
        ui.list.remove(&c);
    }
    ui.dots.borrow_mut().clear();
    let query = ui.search.text().to_string();
    let filter = ui.filter.get();
    let profiles: Vec<Profile> = ui.store.borrow().list(filter, &query).into_iter().cloned().collect();
    let group_name = |id: Option<Uuid>| {
        id.and_then(|g| {
            ui.store
                .borrow()
                .groups()
                .iter()
                .find(|x| x.id == g)
                .map(|x| x.name.clone())
        })
    };
    ui.title.set_label(&match filter {
        Filter::All => "All connections".to_string(),
        Filter::Favorites => "Favorites".into(),
        Filter::Recent => "Recent".into(),
        Filter::Group(g) => group_name(Some(g)).unwrap_or_default(),
    });

    if profiles.is_empty() {
        let page = adw::StatusPage::builder()
            .icon_name("computer-symbolic")
            .title(if query.is_empty() {
                "No connections yet"
            } else {
                "No matches"
            })
            .description(if query.is_empty() {
                "Use “New connection” or import an .rdp file."
            } else {
                "Try a different search."
            })
            .build();
        ui.list.append(&page);
        return;
    }
    for p in profiles {
        ui.list.append(&build_row(ui, &p));
    }
    probe_visible(ui);
}

fn build_row(ui: &Rc<Ui>, p: &Profile) -> adw::ActionRow {
    let user = if p.username.is_empty() {
        String::new()
    } else {
        format!(" · {}", p.username)
    };
    let row = adw::ActionRow::builder()
        .title(p.name.as_str())
        .subtitle(format!("{}:{}{}", p.host, p.port, user))
        .build();

    let dot = gtk::Label::builder()
        .label("●")
        .css_classes(["dim-label"])
        .tooltip_text("Checking…")
        .build();
    row.add_prefix(&dot);
    if let Some(r) = ui.reach.borrow().get(&p.id) {
        apply_reach(&dot, *r);
    }
    ui.dots.borrow_mut().insert(p.id, dot);

    let star = gtk::Button::builder()
        .icon_name(if p.favorite {
            "starred-symbolic"
        } else {
            "non-starred-symbolic"
        })
        .tooltip_text("Favorite")
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .build();
    let edit = gtk::Button::builder()
        .icon_name("document-edit-symbolic")
        .tooltip_text("Edit")
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .build();
    let connect = gtk::Button::builder()
        .label("Connect")
        .css_classes(["suggested-action"])
        .valign(gtk::Align::Center)
        .build();
    row.add_suffix(&star);
    row.add_suffix(&edit);
    row.add_suffix(&connect);

    let id = p.id;
    star.connect_clicked({
        let ui = ui.clone();
        move |_| {
            let updated = ui.store.borrow().get(id).cloned();
            if let Some(mut p) = updated {
                p.favorite = !p.favorite;
                ui.store.borrow_mut().upsert(p);
                save(&ui);
                refresh(&ui);
            }
        }
    });
    edit.connect_clicked({
        let ui = ui.clone();
        move |_| {
            let p = ui.store.borrow().get(id).cloned();
            open_editor(&ui, p);
        }
    });
    connect.connect_clicked({
        let ui = ui.clone();
        move |_| connect_to(&ui, id)
    });
    row
}

fn apply_reach(dot: &gtk::Label, r: Reach) {
    match r {
        Reach::Online { rtt_ms } => {
            dot.set_css_classes(&["success"]);
            dot.set_tooltip_text(Some(&format!("Online · {rtt_ms} ms")));
        }
        Reach::Offline => {
            dot.set_css_classes(&["dim-label"]);
            dot.set_tooltip_text(Some("Offline"));
        }
    }
}

/// One short-lived thread per visible host; results come back over a channel.
fn probe_visible(ui: &Rc<Ui>) {
    let targets: Vec<(Uuid, String, u16)> = ui
        .dots
        .borrow()
        .keys()
        .filter_map(|id| ui.store.borrow().get(*id).map(|p| (p.id, p.host.clone(), p.port)))
        .collect();
    if targets.is_empty() {
        return;
    }
    let (tx, rx) = async_channel::unbounded();
    for (id, host, port) in targets {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking((id, probe(&host, port, Duration::from_secs(3))));
        });
    }
    drop(tx);
    let ui = ui.clone();
    gtk::glib::spawn_future_local(async move {
        while let Ok((id, r)) = rx.recv().await {
            ui.reach.borrow_mut().insert(id, r);
            if let Some(dot) = ui.dots.borrow().get(&id) {
                apply_reach(dot, r);
            }
        }
    });
}

fn open_editor(ui: &Rc<Ui>, profile: Option<Profile>) {
    let ui2 = ui.clone();
    editor::present(
        &ui.window,
        ui.store.clone(),
        profile,
        Rc::new(move || {
            save(&ui2);
            rebuild_sidebar(&ui2);
            refresh(&ui2);
        }),
    );
}

fn connect_to(ui: &Rc<Ui>, id: Uuid) {
    let Some(p) = ui.store.borrow().get(id).cloned() else {
        return;
    };
    // Blocking D-Bus call to the keyring; short, and only when the user clicked Connect.
    let password = match rdp_core::secrets::get_password(id) {
        Ok(pw) => pw,
        Err(e) => {
            alert(
                &ui.window,
                "Keyring unavailable",
                &format!("{e}\n\nYou will be asked for the password."),
            );
            None
        }
    };
    ui.store.borrow_mut().mark_connected(id);
    save(ui);
    session_ui::open(&ui.app, p, password);
}

fn prompt_new_group(ui: &Rc<Ui>) {
    let entry = gtk::Entry::builder()
        .placeholder_text("Group name")
        .activates_default(true)
        .build();
    let d = adw::AlertDialog::new(Some("New group"), None);
    d.set_extra_child(Some(&entry));
    d.add_response("cancel", "Cancel");
    d.add_response("add", "Add");
    d.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("add"));
    d.set_close_response("cancel");
    let ui = ui.clone();
    d.clone()
        .choose(&ui.window.clone(), gtk::gio::Cancellable::NONE, move |resp| {
            let name = entry.text().trim().to_string();
            if resp == "add" && !name.is_empty() {
                ui.store.borrow_mut().add_group(&name);
                save(&ui);
                rebuild_sidebar(&ui);
            }
        });
}

fn import_rdp(ui: &Rc<Ui>) {
    let dialog = gtk::FileDialog::builder().title("Import .rdp file").build();
    let ui = ui.clone();
    let parent = ui.window.clone();
    dialog.open(Some(&parent), gtk::gio::Cancellable::NONE, move |res| {
        let Ok(file) = res else { return };
        let Some(path) = file.path() else { return };
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Imported".into());
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => return alert(&ui.window, "Could not read file", &e.to_string()),
        };
        match rdp_core::rdpfile::parse(&name, &text) {
            Ok(p) => {
                ui.store.borrow_mut().upsert(p);
                save(&ui);
                refresh(&ui);
            }
            Err(e) => alert(&ui.window, "Not a usable .rdp file", &e),
        }
    });
}
