// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::i18n::i18n_f;
use adw::prelude::*;
use library::collections::{self, Album, Entry};

thread_local! {
    static LOCAL_COVER_SLOTS: (async_channel::Sender<()>, async_channel::Receiver<()>) =
        async_channel::bounded(4);
}

fn error(parent: &impl IsA<gtk::Widget>, text: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(i18n("Album Could Not Be Updated"))
        .body(text)
        .build();
    dialog.add_response("close", &i18n("Close"));
    AdwDialogExt::present(&dialog, Some(parent));
}
fn cover(album: &Album) -> gtk::Image {
    let image = gtk::Image::from_icon_name("hopea-media-optical-symbolic");
    image.set_pixel_size(64);
    if let Some(data) = album.cover.as_ref() {
        if let Some(pixbuf) =
            crate::utils::load_cover_texture(&glib::Bytes::from_owned(glib::base64_decode(data)))
        {
            image.set_paintable(Some(&gdk::Texture::for_pixbuf(&pixbuf)));
        }
    }
    image
}
fn dialog(title: &str) -> (adw::Dialog, gtk::Box, adw::HeaderBar) {
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(650)
        .content_height(680)
        .build();
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar.add_top_bar(&header);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_start(18);
    content.set_margin_end(18);
    content.set_margin_top(12);
    content.set_margin_bottom(18);
    toolbar.set_content(Some(&crate::dialogs::scroll(&content)));
    dialog.set_child(Some(&toolbar));
    (dialog, content, header)
}
fn list_view(model: &gio::ListStore, factory: &gtk::SignalListItemFactory) -> gtk::ScrolledWindow {
    let list = gtk::ListView::new(
        Some(gtk::NoSelection::new(Some(model.clone()))),
        Some(factory.clone()),
    );
    list.add_css_class("boxed-list");
    gtk::ScrolledWindow::builder()
        .child(&list)
        .min_content_height(180)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build()
}
fn fill(model: &gio::ListStore, album: &Album) {
    let items: Vec<_> = album
        .tracks
        .iter()
        .cloned()
        .map(glib::BoxedAnyObject::new)
        .collect();
    model.splice(0, model.n_items(), &items);
}
fn sync_name(album: &Rc<RefCell<Album>>, name: &adw::EntryRow) {
    album.borrow_mut().name = name.text().trim().to_string();
}

impl Window {
    fn collection_thumbnail(&self, entry: &Entry) -> gtk::Image {
        let image = gtk::Image::builder()
            .pixel_size(48)
            .width_request(48)
            .height_request(48)
            .valign(gtk::Align::Center)
            .css_classes(["track-thumbnail"])
            .build();
        image.set_widget_name("album-track-cover");
        crate::artwork::placeholder(&image);
        let started = Rc::new(Cell::new(false));
        image.connect_map(clone!(
            #[weak(rename_to = window)]
            self,
            #[strong]
            entry,
            move |image| {
                if started.replace(true) {
                    return;
                }
                let image = image.downgrade();
                let window = window.downgrade();
                let entry = entry.clone();
                glib::spawn_future_local(async move {
                    let mut texture = if let Some(uri) = entry.local_uri.clone() {
                        let (acquire, release) = LOCAL_COVER_SLOTS.with(|slots| slots.clone());
                        if acquire.send(()).await.is_err() {
                            return;
                        }
                        if image.upgrade().is_none() {
                            let _ = release.try_recv();
                            return;
                        }
                        let texture = Song::local_cover(uri).await;
                        let _ = release.try_recv();
                        texture
                    } else {
                        None
                    };
                    if texture.is_none() {
                        if let Some(window) = window.upgrade() {
                            // Do not launch a network search merely to draw a row.
                            if let Ok(track) = window.collection_track(&entry, false).await {
                                if track.has_cover {
                                    texture = library::thumbnails::load(track.id).await;
                                }
                            }
                        }
                    }
                    if let (Some(image), Some(texture)) = (image.upgrade(), texture) {
                        image.set_paintable(Some(&texture));
                    }
                });
            }
        ));
        image
    }

    pub(super) fn setup_collection_shelf(&self) {
        let view = self.imp().playlist_view.get();
        let model = view.albums();
        view.albums_list()
            .set_model(Some(&gtk::NoSelection::new(Some(model.clone()))));
        model.connect_items_changed(clone!(
            #[weak]
            view,
            move |m, _, _, _| view.albums_scroll().set_visible(m.n_items() > 0)
        ));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(clone!(
            #[weak(rename_to = win)]
            self,
            move |_, obj| {
                let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
                let object = item
                    .item()
                    .unwrap()
                    .downcast::<glib::BoxedAnyObject>()
                    .unwrap();
                let album = object.borrow::<Album>();
                let card = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(4)
                    .width_request(144)
                    .margin_start(6)
                    .margin_end(6)
                    .margin_bottom(6)
                    .build();
                card.set_tooltip_text(Some(&album.name));
                let picture = if album.cover.is_none() && !album.tracks.is_empty() {
                    win.collection_thumbnail(&album.tracks[0])
                } else {
                    cover(&album)
                };
                picture.set_pixel_size(128);
                picture.set_size_request(128, 128);
                picture.add_css_class("track-thumbnail");
                card.append(&picture);
                for text in [
                    album.name.clone(),
                    i18n_f("{} songs", &[&album.tracks.len().to_string()]),
                ] {
                    card.append(
                        &gtk::Label::builder()
                            .label(text)
                            .use_markup(false)
                            .xalign(0.0)
                            .max_width_chars(18)
                            .ellipsize(gtk::pango::EllipsizeMode::End)
                            .build(),
                    );
                }
                item.set_child(Some(&card));
            }
        ));
        factory.connect_unbind(|_, object| {
            object
                .downcast_ref::<gtk::ListItem>()
                .unwrap()
                .set_child(gtk::Widget::NONE);
        });
        view.albums_list().set_factory(Some(&factory));
        view.albums_list().connect_activate(clone!(
            #[weak(rename_to=win)]
            self,
            #[weak]
            model,
            move |_, position| {
                let Some(item) = model.item(position).and_downcast::<glib::BoxedAnyObject>() else {
                    return;
                };
                win.open_collection(item.borrow::<Album>().clone(), model.clone());
            }
        ));
        view.new_album_button().connect_clicked(clone!(
            #[weak(rename_to=win)]
            self,
            #[weak]
            model,
            move |_| {
                win.edit_collection(&win, Album::new(), model.clone());
            }
        ));
        view.open_album_button().connect_clicked(clone!(
            #[weak(rename_to=win)]
            self,
            #[weak]
            model,
            move |_| {
                glib::spawn_future_local(clone!(
                    #[strong]
                    win,
                    #[strong]
                    model,
                    async move {
                        let chooser = gtk::FileDialog::builder()
                            .title(i18n("Open Hopea Album"))
                            .build();
                        if let Ok(file) = chooser.open_future(Some(&win)).await {
                            if let Some(path) = file.path() {
                                match collections::open(path).await {
                                    Ok(album) => win.edit_collection(&win, album, model),
                                    Err(e) => error(&win, &e),
                                }
                            }
                        }
                    }
                ));
            }
        ));
        view.connect_map(clone!(
            #[weak(rename_to = win)]
            self,
            move |_| win.queue_collection_refresh()
        ));
        // Imports save through the same directory as the editor. Watch their
        // atomic renames too, so a visible shelf updates without reopening it.
        glib::spawn_future_local(clone!(
            #[weak(rename_to = win)]
            self,
            async move {
                let directory = gio::spawn_blocking(|| {
                    let path = crate::paths::data_dir().join("collections");
                    std::fs::create_dir_all(&path).map(|_| path)
                })
                .await;
                if let Ok(Ok(path)) = directory {
                    let file = gio::File::for_path(path);
                    match file.monitor_directory(
                        gio::FileMonitorFlags::WATCH_MOVES,
                        None::<&gio::Cancellable>,
                    ) {
                        Ok(monitor) => {
                            monitor.connect_changed(clone!(
                                #[weak]
                                win,
                                move |_, file, other, _| {
                                    let manifest = |file: &gio::File| {
                                        file.path().is_some_and(|path| {
                                            path.extension().and_then(|ext| ext.to_str())
                                                == Some("json")
                                        })
                                    };
                                    if manifest(file) || other.is_some_and(manifest) {
                                        win.queue_collection_refresh();
                                    }
                                }
                            ));
                            win.imp().playlist_view.monitor_albums(monitor);
                        }
                        Err(reason) => log::warn!("Unable to watch personal albums: {reason}"),
                    }
                }
                win.queue_collection_refresh();
            }
        ));
    }

    fn queue_collection_refresh(&self) {
        let weak = self.downgrade();
        self.imp().playlist_view.queue_album_refresh(move || {
            if let Some(win) = weak.upgrade() {
                glib::spawn_future_local(async move {
                    if let Err(reason) = win.refresh_collection_shelf().await {
                        log::warn!("Unable to refresh personal albums: {reason}");
                    }
                });
            }
        });
    }

    async fn refresh_collection_shelf(&self) -> Result<(), String> {
        let view = self.imp().playlist_view.get();
        let revision = view.next_album_revision();
        let albums = collections::list().await?;
        if view.album_revision() == revision {
            let items: Vec<_> = albums.into_iter().map(glib::BoxedAnyObject::new).collect();
            let model = view.albums();
            model.splice(0, model.n_items(), &items);
        }
        Ok(())
    }

    pub(super) async fn show_collections(&self) {
        self.switch_mode(WindowMode::MainView);
        self.imp().playlist_view.show_playlist();
        self.set_playlist_visible(true);
        self.hide_library_if_overlaid();
        if let Err(reason) = self.refresh_collection_shelf().await {
            error(self, &reason);
        }
    }

    fn open_collection(&self, album: Album, albums: gio::ListStore) {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let page = self.imp().playlist_view.show_page(&album.name, &content);
        let actions = crate::dialogs::actions();
        actions.set_margin_start(12);
        actions.set_margin_end(12);
        let play = gtk::Button::with_label(&i18n("Play Album"));
        play.add_css_class("suggested-action");
        let edit = gtk::Button::with_label(&i18n("Edit Album"));
        actions.append(&play);
        actions.append(&edit);
        content.append(&actions);
        let album = Rc::new(RefCell::new(album));
        edit.connect_clicked(clone!(
            #[weak(rename_to=win)]
            self,
            #[weak]
            page,
            #[strong]
            album,
            #[strong]
            albums,
            move |_| {
                win.edit_collection(&page, album.borrow().clone(), albums.clone());
            }
        ));
        // Returning from the editor refreshes the browser without losing Back history.
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        fill(&model, &album.borrow());
        albums.connect_items_changed(clone!(
            #[weak]
            page,
            #[weak]
            model,
            #[weak]
            album,
            move |albums, _, _, _| {
                let id = album.borrow().id.clone();
                let updated = albums
                    .iter::<glib::BoxedAnyObject>()
                    .flatten()
                    .map(|a| a.borrow::<Album>().clone())
                    .find(|a| a.id == id);
                if let Some(updated) = updated {
                    page.set_title(&updated.name);
                    fill(&model, &updated);
                    album.replace(updated);
                } else {
                    model.remove_all();
                }
            }
        ));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(clone!(
            #[weak(rename_to=win)]
            self,
            move |_, object| {
                let item = object.downcast_ref::<gtk::ListItem>().unwrap();
                let entry = item
                    .item()
                    .unwrap()
                    .downcast::<glib::BoxedAnyObject>()
                    .unwrap();
                let entry = entry.borrow::<Entry>();
                let row = adw::ActionRow::builder()
                    .title(&entry.title)
                    .subtitle(entry.artist.as_deref().unwrap_or(""))
                    .title_lines(2)
                    .subtitle_lines(1)
                    .use_markup(false)
                    .build();
                row.add_prefix(&win.collection_thumbnail(&entry));
                item.set_child(Some(&row));
            }
        ));
        let scroll = list_view(&model, &factory);
        let list = scroll.child().unwrap().downcast::<gtk::ListView>().unwrap();
        list.set_single_click_activate(true);
        list.connect_activate(clone!(
            #[weak(rename_to=win)]
            self,
            #[strong]
            album,
            move |_, position| {
                let snapshot = album.borrow().clone();
                let win = win.clone();
                glib::spawn_future_local(async move {
                    if let Err(reason) =
                        win.play_collection_from(&snapshot, position as usize).await
                    {
                        win.add_toast(reason);
                    }
                });
            }
        ));
        play.connect_clicked(clone!(
            #[weak(rename_to=win)]
            self,
            #[strong]
            album,
            move |_| {
                let snapshot = album.borrow().clone();
                let win = win.clone();
                glib::spawn_future_local(async move {
                    if let Err(reason) = win.play_collection(&snapshot).await {
                        win.add_toast(reason);
                    }
                });
            }
        ));
        content.append(&scroll);
        self.set_playlist_visible(true);
    }

    pub(crate) fn begin_remote_album(&self, title: &str) -> gtk::Box {
        self.switch_mode(WindowMode::MainView);
        self.set_playlist_visible(true);
        self.hide_library_if_overlaid();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let spinner = gtk::Spinner::builder().spinning(true).vexpand(true).build();
        content.append(&spinner);
        self.imp().playlist_view.show_page(title, &content);
        self.set_playlist_visible(true);
        self.hide_library_if_overlaid();
        content
    }

    pub(crate) fn fill_remote_album(&self, content: &gtk::Box, tracks: Vec<Track>) {
        while let Some(child) = content.first_child() {
            content.remove(&child);
        }
        if tracks.is_empty() {
            let empty = gtk::Label::new(Some(&i18n("No available songs in this album")));
            empty.set_wrap(true);
            content.append(&empty);
            return;
        }
        let play = gtk::Button::with_label(&i18n("Play Album"));
        play.add_css_class("suggested-action");
        play.set_margin_start(12);
        play.set_margin_end(12);
        content.append(&play);
        play.connect_clicked(clone!(
            #[weak(rename_to=win)]
            self,
            #[strong]
            tracks,
            move |_| win.preview_remote_album(&tracks, 0)
        ));
        let save = gtk::Button::with_label(&i18n("Save Album…"));
        save.set_margin_start(12);
        save.set_margin_end(12);
        save.connect_clicked(clone!(
            #[weak(rename_to=win)]
            self,
            #[strong]
            tracks,
            move |_| {
                let panel = win.imp().library_panel.get();
                let tracks = tracks.clone();
                glib::spawn_future_local(async move {
                    panel.save_remote_album(tracks).await;
                });
            }
        ));
        content.append(&save);
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        for track in &tracks {
            model.append(&glib::BoxedAnyObject::new(track.clone()));
        }
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(clone!(
            #[weak(rename_to=win)]
            self,
            #[strong]
            tracks,
            move |_, object| {
                let item = object.downcast_ref::<gtk::ListItem>().unwrap();
                let track = item
                    .item()
                    .unwrap()
                    .downcast::<glib::BoxedAnyObject>()
                    .unwrap();
                let row = win
                    .imp()
                    .library_panel
                    .album_track_row(&track.borrow::<Track>(), &tracks);
                // ActionRow activates through its ListBox parent. ListView
                // virtualizes the surrounding boxes, just as in search results.
                let list = gtk::ListBox::builder()
                    .selection_mode(gtk::SelectionMode::None)
                    .build();
                list.append(&row);
                item.set_activatable(false);
                item.set_child(Some(&list));
            }
        ));
        content.append(&list_view(&model, &factory));
    }

    pub(crate) fn preview_remote_album(&self, tracks: &[Track], start: usize) {
        let (Some(player), Some(client)) = (self.player(), library::client()) else {
            return;
        };
        let songs = tracks
            .iter()
            .map(|track| {
                let song = Song::from_remote(track, &client.audio_uri(track.id), None);
                song
            })
            .collect();
        player.preview_album(songs, start);
        self.switch_mode(WindowMode::MainView);
    }

    fn edit_collection(
        &self,
        parent: &impl IsA<gtk::Widget>,
        initial: Album,
        albums: gio::ListStore,
    ) {
        let album = Rc::new(RefCell::new(initial));
        let (editor, content, header) = dialog(&i18n("Edit Album"));
        let save = gtk::Button::with_label(&i18n("Save"));
        save.add_css_class("suggested-action");
        header.pack_end(&save);
        header.set_show_end_title_buttons(false);
        let cancel = gtk::Button::with_label(&i18n("Cancel"));
        header.pack_start(&cancel);
        cancel.connect_clicked(clone!(
            #[weak]
            editor,
            move |_| {
                editor.close();
            }
        ));
        let name = adw::EntryRow::builder()
            .title(i18n("Album Name"))
            .text(&album.borrow().name)
            .build();
        let group = adw::PreferencesGroup::new();
        group.add(&name);
        let picture = cover(&album.borrow());
        picture.set_size_request(80, 80);
        picture.add_css_class("album-editor-cover");
        let cover_button = gtk::Button::builder()
            .child(&picture)
            .tooltip_text(i18n("Choose album cover"))
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        cover_button.update_property(&[gtk::accessible::Property::Label(&i18n(
            "Choose album cover",
        ))]);
        group.set_hexpand(true);
        group.set_valign(gtk::Align::Center);
        let identity = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        identity.append(&cover_button);
        identity.append(&group);
        content.append(&identity);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let heading = gtk::Label::builder()
            .label(i18n("Songs"))
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["heading"])
            .build();
        actions.append(&heading);
        let add = gtk::Button::with_label(&i18n("Add Songs"));
        actions.append(&add);
        let play = gtk::Button::with_label(&i18n("Play Album"));
        let share = gtk::Button::with_label(&i18n("Share Album…"));
        content.append(&actions);
        let menu = gtk::MenuButton::builder()
            .icon_name("hopea-view-more-symbolic")
            .tooltip_text(i18n("Album Actions"))
            .build();
        let popover = gtk::Popover::new();
        let bottom = gtk::Box::new(gtk::Orientation::Vertical, 4);
        for button in [&play, &share] {
            button.add_css_class("flat");
            bottom.append(button);
            button.connect_clicked(clone!(
                #[weak]
                popover,
                move |_| popover.popdown()
            ));
        }
        popover.set_child(Some(&bottom));
        menu.set_popover(Some(&popover));
        header.pack_end(&menu);
        let saved_id = album.borrow().id.clone();
        let is_saved = (0..albums.n_items()).any(|i| {
            albums
                .item(i)
                .unwrap()
                .downcast::<glib::BoxedAnyObject>()
                .unwrap()
                .borrow::<Album>()
                .id
                == saved_id
        });
        {
            let delete = gtk::Button::builder()
                .label(i18n("Delete Album…"))
                .visible(is_saved)
                .css_classes(["flat", "destructive-action"])
                .build();
            bottom.append(&delete);
            delete.connect_clicked(clone!(
                #[weak]
                popover,
                move |_| popover.popdown()
            ));
            albums.connect_items_changed(clone!(
                #[weak]
                delete,
                #[strong]
                saved_id,
                move |albums, _, _, _| {
                    delete.set_visible((0..albums.n_items()).any(|i| {
                        albums
                            .item(i)
                            .unwrap()
                            .downcast::<glib::BoxedAnyObject>()
                            .unwrap()
                            .borrow::<Album>()
                            .id
                            == saved_id
                    }));
                }
            ));
            delete.connect_clicked(clone!(
                #[weak(rename_to=win)]
                self,
                #[weak]
                editor,
                #[strong]
                albums,
                #[strong]
                saved_id,
                move |_| {
                    glib::spawn_future_local(clone!(
                        #[strong]
                        win,
                        #[strong]
                        editor,
                        #[strong]
                        albums,
                        #[strong]
                        saved_id,
                        async move {
                            let prompt = adw::AlertDialog::builder()
                                .heading(i18n("Delete Album?"))
                                .body(i18n("Only this album will be removed. Its songs will stay in your music and keep being shared."))
                                .default_response("cancel")
                                .close_response("cancel")
                                .build();
                            prompt.add_response("cancel", &i18n("Cancel"));
                            prompt.add_response("delete", &i18n("Delete Album"));
                            prompt.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
                            if prompt.choose_future(Some(&editor)).await != "delete" {
                                return;
                            }
                            editor.set_can_close(false);
                            editor.child().unwrap().set_sensitive(false);
                            let result = collections::remove(saved_id.clone()).await;
                            editor.set_can_close(true);
                            editor.child().unwrap().set_sensitive(true);
                            match result {
                                Ok(()) => {
                                    if let Some(pos) = (0..albums.n_items()).find(|&i| {
                                        albums.item(i).unwrap().downcast::<glib::BoxedAnyObject>()
                                            .unwrap().borrow::<Album>().id == saved_id
                                    }) {
                                        albums.remove(pos);
                                    }
                                    editor.close();
                                }
                                Err(e) => error(&win, &e),
                            }
                        }
                    ));
                }
            ));
        }
        let status = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .xalign(0.0)
            .build();
        status.add_css_class("dim-label");
        content.append(&status);
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        fill(&model, &album.borrow());
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(clone!(
            #[weak(rename_to = window)]
            self,
            #[strong]
            album,
            #[weak]
            model,
            move |_, obj| {
                let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
                let entry = item
                    .item()
                    .unwrap()
                    .downcast::<glib::BoxedAnyObject>()
                    .unwrap()
                    .borrow::<Entry>()
                    .clone();
                let row = adw::ActionRow::builder()
                    .title(&entry.title)
                    .subtitle(if entry.node_id.is_none() && entry.local_uri.is_none() {
                        format!(
                            "{} · {}",
                            entry.artist.as_deref().unwrap_or(""),
                            i18n("Not downloaded")
                        )
                    } else {
                        entry.artist.clone().unwrap_or_default()
                    })
                    .use_markup(false)
                    .title_lines(1)
                    .subtitle_lines(1)
                    .build();
                let position = gtk::Label::builder()
                    .label((item.position() + 1).to_string())
                    .width_chars(2)
                    .css_classes(["dim-label"])
                    .build();
                row.add_prefix(&position);
                row.add_prefix(&window.collection_thumbnail(&entry));
                let menu = gtk::MenuButton::builder()
                    .icon_name("hopea-view-more-symbolic")
                    .tooltip_text(i18n("Song Actions"))
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .build();
                let popover = gtk::Popover::new();
                let options = gtk::Box::new(gtk::Orientation::Vertical, 4);
                popover.set_child(Some(&options));
                menu.set_popover(Some(&popover));
                row.add_suffix(&menu);
                for (icon, tooltip, delta) in [
                    ("hopea-go-up-symbolic", "Move Up", -1i32),
                    ("hopea-go-down-symbolic", "Move Down", 1),
                    ("hopea-list-remove-symbolic", "Remove from Album", 0),
                ] {
                    let button = gtk::Button::builder()
                        .child(
                            &adw::ButtonContent::builder()
                                .icon_name(icon)
                                .label(i18n(tooltip))
                                .build(),
                        )
                        .build();
                    button.set_tooltip_text(Some(&i18n(tooltip)));
                    button.set_valign(gtk::Align::Center);
                    button.add_css_class("flat");
                    options.append(&button);
                    let pos = item.position() as usize;
                    button.set_sensitive(
                        delta == 0
                            || if delta < 0 {
                                pos > 0
                            } else {
                                pos + 1 < album.borrow().tracks.len()
                            },
                    );
                    button.connect_clicked(clone!(
                        #[strong]
                        album,
                        #[weak]
                        model,
                        #[weak]
                        item,
                        #[weak]
                        popover,
                        move |_| {
                            popover.popdown();
                            let pos = item.position() as usize;
                            let mut a = album.borrow_mut();
                            if pos >= a.tracks.len() {
                                return;
                            }
                            if delta == 0 {
                                a.tracks.remove(pos);
                            } else {
                                let dest = pos as i32 + delta;
                                if dest < 0 || dest as usize >= a.tracks.len() {
                                    return;
                                }
                                a.tracks.swap(pos, dest as usize);
                            }
                            let snapshot = a.clone();
                            drop(a);
                            fill(&model, &snapshot);
                        }
                    ));
                }
                item.set_child(Some(&row));
            }
        ));
        content.append(&list_view(&model, &factory));
        let empty = gtk::Label::new(Some(&i18n(
            "No songs yet. Use Add Songs to choose from your music.",
        )));
        empty.set_wrap(true);
        empty.set_visible(model.n_items() == 0);
        content.append(&empty);
        model.connect_items_changed(clone!(
            #[weak]
            empty,
            move |m, _, _, _| empty.set_visible(m.n_items() == 0)
        ));
        play.set_sensitive(model.n_items() > 0);
        share.set_sensitive(model.n_items() > 0 && !name.text().trim().is_empty());
        model.connect_items_changed(clone!(
            #[weak]
            play,
            #[weak]
            share,
            #[weak]
            name,
            move |model, _, _, _| {
                play.set_sensitive(model.n_items() > 0);
                share.set_sensitive(model.n_items() > 0 && !name.text().trim().is_empty());
            }
        ));
        name.connect_changed(clone!(
            #[weak]
            share,
            #[weak]
            model,
            move |name| {
                share.set_sensitive(model.n_items() > 0 && !name.text().trim().is_empty());
            }
        ));
        save.set_sensitive(!name.text().trim().is_empty());
        name.connect_changed(clone!(
            #[weak]
            save,
            move |name| save.set_sensitive(!name.text().trim().is_empty())
        ));
        save.connect_clicked(clone!(
            #[weak(rename_to=win)]
            self,
            #[weak]
            editor,
            #[weak]
            name,
            #[strong]
            album,
            #[strong]
            albums,
            move |button| {
                sync_name(&album, &name);
                let saved = album.borrow().clone();
                button.set_sensitive(false);
                editor.set_can_close(false);
                editor.child().unwrap().set_sensitive(false);
                glib::spawn_future_local(clone!(
                    #[strong]
                    win,
                    #[strong]
                    editor,
                    #[strong]
                    albums,
                    #[weak]
                    button,
                    async move {
                        let result = collections::save(saved.clone()).await;
                        editor.set_can_close(true);
                        editor.child().unwrap().set_sensitive(true);
                        match result {
                            Ok(()) => {
                                let pos = (0..albums.n_items()).find(|&i| {
                                    albums
                                        .item(i)
                                        .unwrap()
                                        .downcast::<glib::BoxedAnyObject>()
                                        .unwrap()
                                        .borrow::<Album>()
                                        .id
                                        == saved.id
                                });
                                let item = glib::BoxedAnyObject::new(saved);
                                if let Some(pos) = pos {
                                    albums.splice(pos, 1, &[item]);
                                } else {
                                    albums.append(&item);
                                }
                                editor.close();
                                win.sync_local_sharing().await;
                            }
                            Err(e) => error(&win, &e),
                        }
                        button.set_sensitive(true);
                    }
                ));
            }
        ));
        cover_button.connect_clicked(clone!(
            #[weak(rename_to=win)]
            self,
            #[weak]
            picture,
            #[strong]
            album,
            move |_| {
                glib::spawn_future_local(clone!(
                    #[strong]
                    win,
                    #[strong]
                    album,
                    #[strong]
                    picture,
                    async move {
                        let chooser = gtk::FileDialog::builder()
                            .title(i18n("Choose Album Cover"))
                            .build();
                        let Ok(file) = chooser.open_future(Some(&win)).await else {
                            return;
                        };
                        let Some(path) = file.path() else {
                            return;
                        };
                        let result = gio::spawn_blocking(move || {
                            use std::io::Read;
                            let mut bytes = Vec::new();
                            std::fs::File::open(path)
                                .map_err(|_| "Unable to open cover")?
                                .take(collections::MAX_FILE as u64 + 1)
                                .read_to_end(&mut bytes)
                                .map_err(|_| "Unable to read cover")?;
                            collections::normalize_cover(&bytes)
                        })
                        .await;
                        match result {
                            Ok(Ok(data)) => {
                                if let Ok(texture) = gdk::Texture::from_bytes(
                                    &glib::Bytes::from_owned(glib::base64_decode(&data)),
                                ) {
                                    picture.set_paintable(Some(&texture));
                                    album.borrow_mut().cover = Some(data);
                                }
                            }
                            _ => error(&win, &i18n("Choose a valid PNG or JPEG smaller than 8 MB")),
                        }
                    }
                ));
            }
        ));
        add.connect_clicked(clone!(
            #[weak(rename_to=win)]
            self,
            #[weak]
            editor,
            #[strong]
            album,
            #[weak]
            model,
            move |_| {
                win.choose_collection_songs(&editor, album.clone(), model.clone());
            }
        ));
        play.connect_clicked(clone!(
            #[weak(rename_to=win)]
            self,
            #[strong]
            album,
            #[weak]
            status,
            move |button| {
                let album = album.borrow().clone();
                button.set_sensitive(false);
                glib::spawn_future_local(clone!(
                    #[strong]
                    win,
                    #[weak]
                    button,
                    #[strong]
                    status,
                    async move {
                        status.set_text(&i18n("Finding album songs…"));
                        match win.play_collection(&album).await {
                            Ok(()) => status.set_text(""),
                            Err(e) => status.set_text(&e),
                        }
                        button.set_sensitive(true);
                    }
                ));
            }
        ));
        share.connect_clicked(clone!(
            #[weak(rename_to=win)] self,
            #[strong] album,
            #[strong] albums,
            #[weak] name,
            #[weak] status,
            #[weak] content,
            #[weak] save,
            #[weak] bottom,
            move |_| {
                sync_name(&album, &name);
                let snapshot = album.borrow().clone();
                if let Err(e) = snapshot.validate() { error(&win, &e); return; }
                content.set_sensitive(false);
                bottom.set_sensitive(false);
                save.set_sensitive(false);
                glib::spawn_future_local(clone!(
                    #[strong] win,
                    #[strong] album,
                    #[strong] albums,
                    #[strong] status,
                    #[strong] content,
                    #[strong] save,
                    #[strong] bottom,
                    async move {
                        let result = win.share_collection(snapshot, &status).await;
                        match result {
                            Ok(Some(updated)) => {
                                let pos = (0..albums.n_items()).find(|&i| albums.item(i).unwrap().downcast::<glib::BoxedAnyObject>().unwrap().borrow::<Album>().id == updated.id);
                                let item = glib::BoxedAnyObject::new(updated.clone());
                                if let Some(pos) = pos { albums.splice(pos, 1, &[item]); } else { albums.append(&item); }
                                album.replace(updated);
                                status.set_text(&i18n("Album saved. Its songs are being shared. Send the album file to another Hopea user."));
                                status.remove_css_class("error");
                                status.add_css_class("success");
                            }
                            Ok(None) => {}
                            Err(e) => { status.set_text(&e); status.remove_css_class("success"); status.add_css_class("error"); }
                        }
                        content.set_sensitive(true);
                        bottom.set_sensitive(true);
                        save.set_sensitive(true);
                    }
                ));
            }
        ));
        AdwDialogExt::present(&editor, Some(parent));
    }

    fn choose_collection_songs(
        &self,
        parent: &adw::Dialog,
        album: Rc<RefCell<Album>>,
        editor_model: gio::ListStore,
    ) {
        let (dialog, content, header) = dialog(&i18n("Add Songs"));
        let add = gtk::Button::with_label(&i18n("Add Selected"));
        add.add_css_class("suggested-action");
        header.pack_end(&add);
        let all = gtk::CheckButton::with_label(&i18n("Select All"));
        content.append(&all);
        let mut songs = self.imp().my_music_songs.borrow().clone();
        if !self.in_for_you() {
            if let Some(player) = self.player() {
                songs.extend(
                    (0..player.queue().n_songs()).filter_map(|i| player.queue().song_at(i)),
                );
            }
        }
        let mut seen = std::collections::HashSet::new();
        songs.retain(|s| seen.insert(s.uri()));
        let entries: Vec<Entry> = songs
            .iter()
            .map(|s| Entry {
                source_key: None,
                title: s.title(),
                artist: s.known_artist(),
                node_id: s.library_id(),
                local_uri: s.file().path().map(|_| s.uri()),
                sha256: None,
            })
            .collect();
        let entries = Rc::new(entries);
        let selected = Rc::new(RefCell::new(std::collections::HashSet::<usize>::new()));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        for i in 0..entries.len() {
            model.append(&glib::BoxedAnyObject::new(i));
        }
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(clone!(
            #[weak(rename_to = window)]
            self,
            #[strong]
            entries,
            #[strong]
            selected,
            move |_, obj| {
                let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
                let i = *item
                    .item()
                    .unwrap()
                    .downcast::<glib::BoxedAnyObject>()
                    .unwrap()
                    .borrow::<usize>();
                let row = adw::ActionRow::builder()
                    .title(&entries[i].title)
                    .subtitle(entries[i].artist.as_deref().unwrap_or(""))
                    .use_markup(false)
                    .build();
                let check = gtk::CheckButton::new();
                check.set_active(selected.borrow().contains(&i));
                row.add_prefix(&check);
                row.add_prefix(&window.collection_thumbnail(&entries[i]));
                row.set_activatable_widget(Some(&check));
                check.connect_toggled(clone!(
                    #[strong]
                    selected,
                    move |check| {
                        if check.is_active() {
                            selected.borrow_mut().insert(i);
                        } else {
                            selected.borrow_mut().remove(&i);
                        }
                    }
                ));
                item.set_child(Some(&row));
            }
        ));
        content.append(&list_view(&model, &factory));
        if entries.is_empty() {
            content.append(&gtk::Label::new(Some(&i18n(
                "Add or import music into My Music first.",
            ))));
        }
        all.connect_toggled(clone!(
            #[strong]
            selected,
            #[strong]
            entries,
            #[weak]
            model,
            move |check| {
                *selected.borrow_mut() = if check.is_active() {
                    (0..entries.len()).collect()
                } else {
                    Default::default()
                };
                let rows: Vec<_> = (0..entries.len()).map(glib::BoxedAnyObject::new).collect();
                model.splice(0, model.n_items(), &rows);
            }
        ));
        add.connect_clicked(clone!(
            #[strong]
            album,
            #[strong]
            entries,
            #[strong]
            selected,
            #[weak]
            dialog,
            #[strong]
            editor_model,
            move |_| {
                let mut album = album.borrow_mut();
                for (i, e) in entries
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| selected.borrow().contains(i))
                {
                    let _ = i;
                    if !album.tracks.iter().any(|t| {
                        (e.local_uri.is_some() && t.local_uri == e.local_uri)
                            || (e.node_id.is_some() && t.node_id == e.node_id)
                    }) {
                        album.tracks.push(e.clone());
                    }
                }
                let snapshot = album.clone();
                drop(album);
                fill(&editor_model, &snapshot);
                dialog.close();
            }
        ));
        AdwDialogExt::present(&dialog, Some(parent));
    }

    async fn collection_track(&self, entry: &Entry, network: bool) -> Result<Track, String> {
        let client = library::client()
            .ok_or_else(|| i18n("Join Shared Music to find this album’s songs"))?;
        if let Some(id) = entry.node_id {
            if let Ok(t) = client.track(id).await {
                if entry
                    .sha256
                    .as_ref()
                    .map_or(t.title == entry.title, |hash| *hash == t.sha256)
                {
                    return Ok(t);
                }
            }
        }
        let Some(hash) = &entry.sha256 else {
            return Err(i18n("This song is not shared yet"));
        };
        let query = format!("{} {}", entry.artist.as_deref().unwrap_or(""), entry.title);
        let mut offset = 0;
        loop {
            let page = client
                .search(
                    &query,
                    offset,
                    network && offset == 0,
                    hopea_common::api::SearchOrder::default(),
                )
                .await
                .map_err(|_| i18n("Unable to search for album songs"))?;
            if let Some(t) = page.tracks.iter().find(|t| t.sha256 == *hash) {
                return Ok(t.clone());
            }
            offset += page.tracks.len() as u32;
            if page.tracks.is_empty() || u64::from(offset) >= page.total {
                break;
            }
        }
        Err(i18n_f(
            "Song unavailable: {}. Its owner may be offline.",
            &[&entry.title],
        ))
    }
    async fn play_collection(&self, album: &Album) -> Result<(), String> {
        self.play_collection_from(album, 0).await
    }

    async fn play_collection_from(&self, album: &Album, start: usize) -> Result<(), String> {
        if album.tracks.is_empty() {
            return Err(i18n("Add songs to the album first"));
        }
        let mut songs = Vec::new();
        for entry in &album.tracks {
            if let Some(uri) = &entry.local_uri {
                if let Ok(song) = Song::from_uri(uri) {
                    if let Some(client) = library::client() {
                        if let Some(id) = entry.node_id.or_else(|| song.library_id()) {
                            if let Ok(track) = client.track(id).await {
                                song.set_library(Some(id), track.mine, track.favorite);
                            }
                        }
                    }
                    songs.push(song);
                    continue;
                }
            }
            let track = self.collection_track(entry, true).await?;
            let client = library::client().ok_or_else(|| i18n("Not connected"))?;
            let cover = if track.has_cover {
                client.cover(track.id).await.ok()
            } else {
                None
            };
            songs.push(Song::from_remote(
                &track,
                &client.audio_uri(track.id),
                cover.as_ref(),
            ));
        }
        if let Some(player) = self.player() {
            player.preview_album(songs, start);
        }
        Ok(())
    }
    async fn share_collection(
        &self,
        mut album: Album,
        status: &gtk::Label,
    ) -> Result<Option<Album>, String> {
        if album.tracks.is_empty() {
            return Err(i18n("Add songs before sharing the album"));
        }
        let chooser = gtk::FileDialog::builder()
            .title(i18n("Share Hopea Album"))
            .initial_name(format!(
                "{}.hopea-album.json",
                crate::library_panel::safe_file_name(&album.name)
            ))
            .build();
        let Ok(destination) = chooser.save_future(Some(self)).await else {
            return Ok(None);
        };
        self.imp().library_panel.ensure_collection_sharing().await?;
        let client = library::client().ok_or_else(|| i18n("Not connected"))?;
        let total = album.tracks.len();
        for (index, entry) in album.tracks.iter_mut().enumerate() {
            status.remove_css_class("error");
            status.set_text(&i18n_f(
                "Sharing {} of {}: {}",
                &[&(index + 1).to_string(), &total.to_string(), &entry.title],
            ));
            let track = if let Some(uri) = &entry.local_uri {
                let file = gio::File::for_uri(uri);
                let track = self.imp().library_panel.share_file(&file).await?;
                self.mark_shared_file(&file.uri(), track.id);
                track
            } else {
                self.collection_track(entry, true).await?
            };
            if !track.mine {
                client
                    .prefetch(track.id)
                    .await
                    .map_err(|_| i18n("Unable to keep the album song for sharing"))?;
            }
            // A manifest is not reported as seeded before every remote song is
            // downloaded and verified by the node.
            let start = Instant::now();
            while !client
                .track(track.id)
                .await
                .map_err(|_| i18n("Unable to check song availability"))?
                .local
            {
                if start.elapsed().as_secs() > 300 {
                    return Err(i18n("A song is still downloading. Try sharing the album again when it is available."));
                }
                glib::timeout_future_seconds(1).await;
            }
            entry.node_id = Some(track.id);
            entry.sha256 = Some(track.sha256);
            self.imp().library_panel.refresh_shared_music().await;
        }
        let bytes = album.export()?;
        destination
            .replace_contents_future(
                bytes,
                None,
                false,
                gio::FileCreateFlags::REPLACE_DESTINATION,
            )
            .await
            .map_err(|_| i18n("Unable to save the shared album file"))?;
        collections::save(album.clone()).await?;
        Ok(Some(album))
    }
}

#[cfg(test)]
impl Window {
    pub(crate) fn test_album_page(&self) -> Option<adw::NavigationPage> {
        self.imp().playlist_view.visible_page()
    }
    pub(super) async fn test_collection_editor(&self) {
        fn widgets(w: &gtk::Widget) -> Vec<gtk::Widget> {
            let mut out = vec![w.clone()];
            let mut child = w.first_child();
            while let Some(w) = child {
                child = w.next_sibling();
                out.extend(widgets(&w));
            }
            out
        }
        fn click(dialog: &impl IsA<gtk::Widget>, label: &str) {
            let button = widgets(dialog.as_ref())
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Button>().ok())
                .find(|b| b.label().as_deref() == Some(label))
                .unwrap_or_else(|| panic!("missing {}", label));
            assert!(button.is_sensitive());
            button.emit_clicked();
        }
        let wait = || glib::timeout_future(std::time::Duration::from_millis(250));
        let audio = glib::user_data_dir().join("album-cover-fixture.wav");
        let mut bytes = include_bytes!("../../node/tests/fixtures/tone.wav").to_vec();
        *bytes.last_mut().unwrap() ^= 2;
        std::fs::write(&audio, bytes).unwrap();
        library::import_history::save_cover(
            &audio,
            &glib::Bytes::from_static(include_bytes!("../../data/icons/cat-kz/64.png")),
        )
        .await
        .unwrap();
        self.player()
            .unwrap()
            .queue()
            .add_song(&Song::from_uri(&gio::File::for_path(&audio).uri()).unwrap());
        self.show_collections().await;
        wait().await;
        assert!(
            self.visible_dialog().is_none(),
            "browsing albums must keep the playlist visible"
        );
        let manager = self.imp().playlist_view.visible_page().unwrap();
        assert_eq!(manager.tag().as_deref(), Some("playlist"));
        let list = self.imp().playlist_view.albums_list();
        assert_eq!(list.orientation(), gtk::Orientation::Horizontal);
        assert!(list.is_single_click_activate());
        click(&manager, "New Album");
        wait().await;
        let editor = self.visible_dialog().unwrap();
        assert_eq!(editor.title(), "Edit Album");
        crate::dialogs::test_fit(self, &editor).await;
        let cover_control = widgets(editor.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.tooltip_text().as_deref() == Some("Choose album cover"))
            .expect("the cover itself is an accessible button");
        assert!(cover_control.child().unwrap().is::<gtk::Image>());
        assert!(cover_control.label().is_none());
        let name = widgets(editor.upcast_ref())
            .into_iter()
            .find_map(|w| w.downcast::<adw::EntryRow>().ok())
            .unwrap();
        let save = widgets(editor.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.label().as_deref() == Some("Save"))
            .unwrap();
        assert!(!save.is_sensitive());
        name.set_text("Evening <mix>");
        click(&editor, "Add Songs");
        wait().await;
        let picker = self.visible_dialog().unwrap();
        crate::dialogs::test_fit(self, &picker).await;
        assert_eq!(picker.title(), "Add Songs");
        let select = widgets(picker.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::CheckButton>().ok())
            .find(|c| c.label().as_deref() == Some("Select All"))
            .unwrap();
        select.set_active(true);
        click(&picker, "Add Selected");
        wait().await;
        assert!(widgets(editor.upcast_ref())
            .iter()
            .any(|w| w.downcast_ref::<adw::ActionRow>().is_some()));
        for _ in 0..40 {
            if widgets(editor.upcast_ref())
                .iter()
                .filter(|w| w.widget_name() == "album-track-cover")
                .filter_map(|w| w.downcast_ref::<gtk::Image>())
                .any(|image| image.paintable().is_some())
            {
                break;
            }
            wait().await;
        }
        assert!(
            widgets(editor.upcast_ref())
                .iter()
                .filter(|w| w.widget_name() == "album-track-cover")
                .filter_map(|w| w.downcast_ref::<gtk::Image>())
                .any(|image| image.paintable().is_some()),
            "My Albums must display per-track artwork from disk"
        );
        if let Some(path) = std::env::var_os("HOPEA_TEST_ALBUM_SCREENSHOT") {
            glib::timeout_future_seconds(2).await;
            let canvas = editor.child().unwrap();
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gdk::RGBA::WHITE,
                &gtk::graphene::Rect::new(0.0, 0.0, canvas.width() as f32, canvas.height() as f32),
            );
            let mut child = canvas.first_child();
            while let Some(w) = child {
                child = w.next_sibling();
                canvas.snapshot_child(&w, &snapshot);
            }
            self.renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
        click(&editor, "Save");
        wait().await;
        let saved = collections::list().await.unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Evening <mix>");
        assert!(!saved[0].tracks.is_empty());
        assert!(self.imp().playlist_view.albums_scroll().is_visible());
        list.emit_by_name::<()>("activate", &[&0u32]);
        wait().await;
        assert!(self.visible_dialog().is_none());
        let album_page = self.imp().playlist_view.visible_page().unwrap();
        if let Some(path) = std::env::var_os("HOPEA_TEST_INLINE_ALBUM_SCREENSHOT") {
            self.set_playlist_visible(true);
            GtkWindowExt::present(self);
            glib::timeout_future(std::time::Duration::from_millis(400)).await;
            let canvas = self.imp().playlist_view.get();
            assert!(canvas.is_mapped(), "album browser must actually be visible");
            assert!(
                album_page.is_mapped(),
                "opened album must actually be visible"
            );
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gdk::RGBA::parse("#242424").unwrap(),
                &gtk::graphene::Rect::new(0.0, 0.0, canvas.width() as f32, canvas.height() as f32),
            );
            canvas.snapshot_child(&canvas.first_child().unwrap(), &snapshot);
            self.renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
        click(&album_page, "Edit Album");
        wait().await;
        let editor = self.visible_dialog().unwrap();
        let name = widgets(editor.upcast_ref())
            .into_iter()
            .find_map(|w| w.downcast::<adw::EntryRow>().ok())
            .unwrap();
        assert_eq!(name.text(), "Evening <mix>");
        name.set_text("Late evening");
        click(&editor, "Save");
        wait().await;
        assert_eq!(collections::list().await.unwrap()[0].name, "Late evening");
        list.emit_by_name::<()>("activate", &[&0u32]);
        wait().await;
        assert!(self.visible_dialog().is_none());
        let album_page = self.imp().playlist_view.visible_page().unwrap();
        if let Some(path) = std::env::var_os("HOPEA_TEST_INLINE_ALBUM_SCREENSHOT") {
            self.set_playlist_visible(true);
            GtkWindowExt::present(self);
            glib::timeout_future(std::time::Duration::from_millis(400)).await;
            let canvas = self.imp().playlist_view.get();
            assert!(canvas.is_mapped(), "album browser must actually be visible");
            assert!(
                album_page.is_mapped(),
                "opened album must actually be visible"
            );
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gdk::RGBA::parse("#242424").unwrap(),
                &gtk::graphene::Rect::new(0.0, 0.0, canvas.width() as f32, canvas.height() as f32),
            );
            canvas.snapshot_child(&canvas.first_child().unwrap(), &snapshot);
            self.renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
        click(&album_page, "Edit Album");
        wait().await;
        let editor = self.visible_dialog().unwrap();
        let menu = widgets(editor.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::MenuButton>().ok())
            .find(|b| b.tooltip_text().as_deref() == Some("Album Actions"))
            .unwrap();
        menu.popup();
        wait().await;
        click(&editor, "Delete Album…");
        wait().await;
        let prompt = self.visible_dialog().unwrap();
        click(&prompt, "Cancel");
        wait().await;
        assert_eq!(collections::list().await.unwrap().len(), 1);
        menu.popup();
        wait().await;
        click(&editor, "Delete Album…");
        wait().await;
        let prompt = self.visible_dialog().unwrap();
        click(&prompt, "Delete Album");
        wait().await;
        assert!(collections::list().await.unwrap().is_empty());
        assert_eq!(list.model().unwrap().n_items(), 0);
        self.imp().playlist_view.show_playlist();
        assert_eq!(
            self.imp()
                .playlist_view
                .visible_page()
                .unwrap()
                .tag()
                .as_deref(),
            Some("playlist")
        );
        wait().await;
        assert!(!self.imp().playlist_view.albums_scroll().is_visible());

        // Account imports and other collection writers do not hold this view's
        // model. Their saved albums must appear while the playlist stays open.
        let mut imported = Vec::new();
        for n in 0..12 {
            let mut album = Album::new();
            album.name = format!("Imported album {n:02}");
            collections::save(album.clone()).await.unwrap();
            imported.push(album);
        }
        for _ in 0..40 {
            if list.model().unwrap().n_items() == imported.len() as u32 {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(
            list.model().unwrap().n_items(),
            imported.len() as u32,
            "saving imported albums must update the visible shelf"
        );
        wait().await;
        let horizontal = self.imp().playlist_view.albums_scroll().hadjustment();
        assert!(
            horizontal.upper() > horizontal.page_size(),
            "personal album cards must scroll horizontally"
        );
        assert_eq!(
            self.imp()
                .playlist_view
                .visible_page()
                .unwrap()
                .tag()
                .as_deref(),
            Some("playlist"),
            "refreshing albums must not replace the song list"
        );
        if let Some(path) = std::env::var_os("HOPEA_TEST_ALBUMS_SCREENSHOT") {
            let paintable = gtk::WidgetPaintable::new(Some(self));
            self.queue_draw();
            glib::timeout_future(std::time::Duration::from_millis(150)).await;
            let snapshot = gtk::Snapshot::new();
            paintable.snapshot(&snapshot, self.width() as f64, self.height() as f64);
            self.renderer()
                .unwrap()
                .render_texture(
                    &snapshot.to_node().expect("mapped album shelf must render"),
                    None,
                )
                .save_to_png(path)
                .unwrap();
        }
        for album in imported {
            collections::remove(album.id).await.unwrap();
        }
        for _ in 0..40 {
            if list.model().unwrap().n_items() == 0 {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(list.model().unwrap().n_items(), 0);
        assert!(!self.imp().playlist_view.albums_scroll().is_visible());
    }
}
