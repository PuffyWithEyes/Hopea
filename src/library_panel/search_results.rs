// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[derive(Clone)]
enum ResultItem {
    Song(Track),
}

#[derive(Clone)]
struct AlbumResult {
    title: String,
    artist: Option<String>,
    cover: Track,
}

impl AlbumResult {
    fn key(&self) -> (String, String) {
        // Preserve accents and punctuation: different releases must not be
        // merged merely because their names are similar.
        (
            self.title.trim().to_lowercase(),
            self.artist.as_deref().unwrap_or("").trim().to_lowercase(),
        )
    }
}

impl LibraryPanel {
    pub(super) fn setup_results(&self) {
        let imp = self.imp();
        self.setup_album_results();
        glib::timeout_add_seconds_local(
            30,
            clone!(
                #[weak(rename_to=panel)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    panel.refresh_result_availability();
                    glib::ControlFlow::Continue
                }
            ),
        );
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        imp.results.set(model.clone()).unwrap();
        let selection = gtk::NoSelection::new(Some(model));
        imp.results_list.set_model(Some(&selection));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            item.set_activatable(false);
            item.set_child(Some(&gtk::Box::new(gtk::Orientation::Vertical, 0)));
        });
        factory.connect_bind(clone!(
            #[weak(rename_to = panel)]
            self,
            move |_, object| {
                let item = object.downcast_ref::<gtk::ListItem>().unwrap();
                panel.populate_result(item);
            }
        ));
        factory.connect_unbind(|_, object| {
            let slot = object
                .downcast_ref::<gtk::ListItem>()
                .unwrap()
                .child()
                .unwrap()
                .downcast::<gtk::Box>()
                .unwrap();
            while let Some(child) = slot.first_child() {
                slot.remove(&child);
            }
        });
        imp.results_list.set_factory(Some(&factory));
        let adjustment = imp.results_scroll.vadjustment();
        adjustment.connect_value_changed(clone!(
            #[weak(rename_to = panel)]
            self,
            move |_| panel.maybe_load_next_page()
        ));
        adjustment.connect_changed(clone!(
            #[weak(rename_to = panel)]
            self,
            move |_| panel.maybe_load_next_page()
        ));
        imp.results_scroll.connect_map(clone!(
            #[weak(rename_to = panel)]
            self,
            move |_| panel.maybe_load_next_page()
        ));
    }

    fn refresh_result_availability(&self) {
        let imp = self.imp();
        if !self.is_mapped()
            || imp.stack.visible_child_name().as_deref() != Some("library")
            || imp.view_stack.visible_child_name().as_deref() != Some("search")
            || imp.page_loading.get()
            || imp.availability_loading.get()
        {
            return;
        }
        let query = imp.active_query.borrow().clone();
        if query.is_empty() {
            return;
        }
        let Some(client) = library::client() else {
            return;
        };
        let ids: Vec<i64> = imp
            .results
            .get()
            .unwrap()
            .iter::<glib::BoxedAnyObject>()
            .flatten()
            .map(|item| {
                let ResultItem::Song(t) = &*item.borrow::<ResultItem>();
                t.id
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        let generation = imp.generation.get();
        imp.availability_loading.set(true);
        glib::spawn_future_local(clone!(
            #[weak(rename_to=panel)]
            self,
            async move {
                let result = client.available_tracks(&query, &ids).await;
                panel.imp().availability_loading.set(false);
                if panel.imp().generation.get() != generation {
                    return;
                }
                if let Ok(available) = result {
                    let checked: std::collections::HashSet<_> = ids.into_iter().collect();
                    let available: std::collections::HashSet<_> = available.into_iter().collect();
                    let model = panel.imp().results.get().unwrap();
                    let before = model.n_items();
                    for index in (0..model.n_items()).rev() {
                        let item = model
                            .item(index)
                            .unwrap()
                            .downcast::<glib::BoxedAnyObject>()
                            .unwrap();
                        let ResultItem::Song(track) = &*item.borrow::<ResultItem>();
                        if checked.contains(&track.id) && !available.contains(&track.id) {
                            model.remove(index);
                        }
                    }
                    let removed = before - model.n_items();
                    if removed > 0 {
                        panel
                            .imp()
                            .loaded
                            .set(panel.imp().loaded.get().saturating_sub(removed));
                        let tracks: Vec<_> = model
                            .iter::<glib::BoxedAnyObject>()
                            .flatten()
                            .map(|item| {
                                let ResultItem::Song(t) = &*item.borrow::<ResultItem>();
                                t.clone()
                            })
                            .collect();
                        panel.imp().album_results.get().unwrap().remove_all();
                        panel.append_album_results(&tracks);
                        if tracks.is_empty() {
                            panel
                                .imp()
                                .empty_page
                                .set_title(&i18n("No Songs Available Now"));
                            panel.imp().empty_page.set_description(Some(&i18n("Try another search. Songs already on this computer and songs with no reachable sources are hidden.")));
                            panel.imp().search_stack.set_visible_child_name("empty");
                        }
                    }
                }
            }
        ));
    }

    fn populate_result(&self, item: &gtk::ListItem) {
        let slot = item.child().unwrap().downcast::<gtk::Box>().unwrap();
        while let Some(child) = slot.first_child() {
            slot.remove(&child);
        }
        let Some(object) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let ResultItem::Song(track) = object.borrow::<ResultItem>().clone();
        let content = result_box(&self.track_row(&track, ListKind::Search));
        let clamp = adw::Clamp::builder()
            .maximum_size(560)
            .child(&content)
            .margin_start(12)
            .margin_end(12)
            .build();
        slot.append(&clamp);
    }

    pub(super) fn clear_results(&self) {
        if let Some(model) = self.imp().results.get() {
            model.remove_all();
        }
        if let Some(model) = self.imp().album_results.get() {
            model.remove_all();
        }
        self.imp().albums_section.set_visible(false);
        self.imp().albums_scroll.hadjustment().set_value(0.0);
        self.imp().results_scroll.vadjustment().set_value(0.0);
    }

    pub(super) fn reset_search(&self) {
        let imp = self.imp();
        imp.generation.set(imp.generation.get().wrapping_add(1));
        imp.loaded.set(0);
        imp.has_more.set(false);
        imp.page_loading.set(false);
        imp.page_spinner.set_visible(false);
        imp.retry_page_button.set_visible(false);
        imp.network_status.set_visible(false);
        self.clear_results();
    }

    pub(super) fn append_result(&self, track: &Track) {
        self.push_result(ResultItem::Song(track.clone()));
    }

    /// Reconcile snapshots without clearing/rebuilding already visible rows or
    /// scrolling to the top every time another peer answers.
    pub(super) fn sync_live_results(&self, tracks: &[Track]) {
        let model = self.imp().results.get().unwrap();
        if model.n_items() == 0 {
            let items: Vec<_> = tracks
                .iter()
                .map(|track| glib::BoxedAnyObject::new(ResultItem::Song(track.clone())))
                .collect();
            model.splice(0, 0, &items);
            self.append_album_results(tracks);
            if !items.is_empty() {
                self.imp()
                    .results_list
                    .scroll_to(0, gtk::ListScrollFlags::NONE, None);
            }
            return;
        }
        for (position, track) in tracks.iter().enumerate() {
            let position = position as u32;
            let existing = (position..model.n_items()).find_map(|index| {
                let object = model.item(index)?.downcast::<glib::BoxedAnyObject>().ok()?;
                let same = matches!(&*object.borrow::<ResultItem>(), ResultItem::Song(t) if t.id == track.id);
                same.then_some((index, object))
            });
            match existing {
                Some((index, object)) => {
                    let changed = matches!(&*object.borrow::<ResultItem>(), ResultItem::Song(old) if old != track);
                    if index != position {
                        model.remove(index);
                        model.insert(position, &object);
                    }
                    if changed {
                        model.splice(
                            position,
                            1,
                            &[glib::BoxedAnyObject::new(ResultItem::Song(track.clone()))],
                        );
                    }
                }
                None => model.insert(
                    position,
                    &glib::BoxedAnyObject::new(ResultItem::Song(track.clone())),
                ),
            }
        }
        if model.n_items() > tracks.len() as u32 {
            model.splice(
                tracks.len() as u32,
                model.n_items() - tracks.len() as u32,
                &[] as &[glib::BoxedAnyObject],
            );
        }
        let albums = self.imp().album_results.get().unwrap();
        let keys: std::collections::HashSet<_> = tracks
            .iter()
            .filter_map(|track| {
                let title = track.album.as_ref()?.trim();
                if title.is_empty() {
                    return None;
                }
                Some((
                    title.to_lowercase(),
                    track
                        .album_artist
                        .as_ref()
                        .filter(|s| !s.trim().is_empty())
                        .or(track.artist.as_ref())
                        .map(|s| s.trim().to_lowercase())
                        .unwrap_or_default(),
                ))
            })
            .collect();
        for index in (0..albums.n_items()).rev() {
            let item = albums
                .item(index)
                .unwrap()
                .downcast::<glib::BoxedAnyObject>()
                .unwrap();
            if !keys.contains(&item.borrow::<AlbumResult>().key()) {
                albums.remove(index);
            }
        }
        self.append_album_results(tracks);
    }

    fn push_result(&self, item: ResultItem) {
        self.imp()
            .results
            .get()
            .unwrap()
            .append(&glib::BoxedAnyObject::new(item));
    }

    pub(super) fn append_album_results(&self, tracks: &[Track]) {
        let model = self.imp().album_results.get().unwrap();
        let mut known: std::collections::HashMap<_, _> = model
            .iter::<glib::BoxedAnyObject>()
            .flatten()
            .enumerate()
            .map(|(index, item)| (item.borrow::<AlbumResult>().key(), index as u32))
            .collect();
        for track in tracks {
            let Some(title) = track
                .album
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
            else {
                continue;
            };
            let album = AlbumResult {
                title: title.into(),
                artist: track
                    .album_artist
                    .clone()
                    .filter(|a| !a.trim().is_empty())
                    .or_else(|| track.artist.clone()),
                cover: track.clone(),
            };
            let key = album.key();
            if let Some(&index) = known.get(&key) {
                let item = model
                    .item(index)
                    .unwrap()
                    .downcast::<glib::BoxedAnyObject>()
                    .unwrap();
                let missing_cover = !item.borrow::<AlbumResult>().cover.has_cover;
                if missing_cover && track.has_cover {
                    // A later page may be the first one with this album's art.
                    model.splice(index, 1, &[glib::BoxedAnyObject::new(album)]);
                }
            } else {
                known.insert(key, model.n_items());
                model.append(&glib::BoxedAnyObject::new(album));
            }
        }
        self.imp().albums_section.set_visible(model.n_items() > 0);
    }

    fn setup_album_results(&self) {
        let imp = self.imp();
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        imp.album_results.set(model.clone()).unwrap();
        imp.albums_list
            .set_model(Some(&gtk::NoSelection::new(Some(model))));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(|_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            let object = item
                .item()
                .unwrap()
                .downcast::<glib::BoxedAnyObject>()
                .unwrap();
            let album = object.borrow::<AlbumResult>();
            let card = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .width_request(144)
                .margin_start(6)
                .margin_end(6)
                .margin_bottom(6)
                .build();
            card.set_tooltip_text(Some(&format!(
                "{}\n{}",
                album.title,
                album.artist.as_deref().unwrap_or("")
            )));
            card.append(&sized_thumbnail(&album.cover, 128));
            for text in [&album.title, album.artist.as_deref().unwrap_or("")] {
                card.append(
                    &gtk::Label::builder()
                        .label(text)
                        .xalign(0.0)
                        .max_width_chars(18)
                        .ellipsize(gtk::pango::EllipsizeMode::End)
                        .build(),
                );
            }
            item.set_child(Some(&card));
        });
        factory.connect_unbind(|_, object| {
            object
                .downcast_ref::<gtk::ListItem>()
                .unwrap()
                .set_child(gtk::Widget::NONE);
        });
        imp.albums_list.set_factory(Some(&factory));
        imp.albums_list.connect_activate(clone!(
            #[weak(rename_to = panel)]
            self,
            move |_, position| {
                let Some(item) = panel
                    .imp()
                    .album_results
                    .get()
                    .and_then(|model| model.item(position))
                    .and_downcast::<glib::BoxedAnyObject>()
                else {
                    return;
                };
                let album = item.borrow::<AlbumResult>().clone();
                panel.open_album(album.title, album.artist);
            }
        ));
    }

    pub(super) fn update_result_star(&self, id: i64, favorite: bool) {
        let Some(model) = self.imp().results.get() else {
            return;
        };
        if favorite {
            let before = model.n_items();
            for index in (0..model.n_items()).rev() {
                let item = model
                    .item(index)
                    .unwrap()
                    .downcast::<glib::BoxedAnyObject>()
                    .unwrap();
                let ResultItem::Song(track) = &*item.borrow::<ResultItem>();
                if track.id == id {
                    model.remove(index);
                }
            }
            let removed = before - model.n_items();
            self.imp()
                .loaded
                .set(self.imp().loaded.get().saturating_sub(removed));
        }
        for item in model.iter::<glib::BoxedAnyObject>().flatten() {
            let ResultItem::Song(track) = &mut *item.borrow_mut::<ResultItem>();
            if track.id == id {
                track.favorite = favorite;
            }
        }
    }

    pub(super) fn finish_page_load(&self, success: bool) {
        let imp = self.imp();
        imp.page_loading.set(false);
        imp.page_spinner.set_visible(false);
        imp.retry_page_button.set_visible(!success);
        if success {
            // Let ListView allocate the appended rows before deciding whether
            // the viewport needs another page.
            glib::timeout_add_local_once(
                std::time::Duration::from_millis(50),
                clone!(
                    #[weak(rename_to = panel)]
                    self,
                    move || panel.maybe_load_next_page()
                ),
            );
        }
    }

    fn maybe_load_next_page(&self) {
        let imp = self.imp();
        if imp.page_loading.get()
            || !imp.has_more.get()
            || imp.retry_page_button.is_visible()
            || !imp.results_scroll.is_mapped()
        {
            return;
        }
        let adjustment = imp.results_scroll.vadjustment();
        let page = adjustment.page_size();
        // A newly populated ListView can briefly retain the empty viewport's
        // adjustment. That is not evidence that the user reached page end.
        if page > 0.0
            && adjustment.upper() > page
            && adjustment.upper() - adjustment.value() - page <= page * 0.75
        {
            self.load_page(imp.loaded.get());
        }
    }
}

fn result_box(row: &adw::ActionRow) -> gtk::Widget {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    list.append(row);
    list.upcast()
}

#[cfg(test)]
impl LibraryPanel {
    pub async fn test_scroll_loading_and_recycling(&self, window: &Window) {
        use std::{
            io::{BufRead, Write},
            sync::{
                atomic::{AtomicBool, Ordering},
                Arc, Mutex,
            },
        };
        let fixture = |id: i64| Track {
            id,
            title: format!("Test song {id}"),
            artist: Some("Test artist".into()),
            album: None,
            album_artist: None,
            track_number: None,
            year: None,
            genre: None,
            duration_ms: 1000,
            format: AudioFormat::Flac,
            codec: "flac".into(),
            sample_rate: 44100,
            channels: 2,
            size: 1024,
            sha256: format!("{id:064x}"),
            has_cover: false,
            uploader: "Test".into(),
            uploaded_at: 0,
            mine: false,
            local: false,
            favorite: false,
            sources: 1,
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let requested = offsets.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let favorites = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let favorite_requests = favorites.clone();
        let server = std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
                        break;
                    }
                }
                if line.starts_with("PUT ") && line.contains("/favorite ") {
                    favorite_requests.fetch_add(1, Ordering::Relaxed);
                    let mut track = fixture(99999);
                    track.favorite = true;
                    let body = serde_json::to_string(&track).unwrap();
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                    continue;
                }
                if line.contains("/audio ") {
                    let bytes = include_bytes!("../../node/tests/fixtures/tone.wav");
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len());
                    let _ = stream.write_all(bytes);
                    continue;
                }
                if line.contains("/cover ") {
                    let bytes = include_bytes!("../../data/icons/cat-kz/256.png");
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len());
                    let _ = stream.write_all(bytes);
                    continue;
                }
                if line.contains("/album?") {
                    let mut track = fixture(1);
                    track.has_cover = true;
                    let body = serde_json::json!({"tracks":[track],"total":1}).to_string();
                    let _ = write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                    continue;
                }
                if line.contains("/available?ids=") {
                    let ids: Vec<i64> = line
                        .split("ids=")
                        .nth(1)
                        .unwrap()
                        .split(['&', ' '])
                        .next()
                        .unwrap()
                        .split(',')
                        .filter_map(|id| id.parse().ok())
                        .filter(|id| *id != 2)
                        .collect();
                    let body = serde_json::to_string(&ids).unwrap();
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                    continue;
                }
                if line.contains("local=true") {
                    let body = r#"{"tracks":[],"total":0}"#;
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                    continue;
                }
                if !line.contains("offset=") {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    continue;
                }
                let offset: u32 = line
                    .split("offset=")
                    .nth(1)
                    .unwrap()
                    .split(['&', ' '])
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap();
                // Local snapshots of page zero are not peer searches/pages.
                if !line.contains("network=false&") || offset > 0 {
                    requested.lock().unwrap().push(offset);
                }
                let tracks: Vec<_> = (offset..offset + 50)
                    .map(|id| fixture(i64::from(id) + 1))
                    .collect();
                let body = serde_json::json!({"tracks":tracks,"total":1000}).to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
        });
        let previous = library::client();
        library::set_client(Some(
            LibraryClient::new(&format!("http://{address}"), "test").unwrap(),
        ));
        let player = window.player().unwrap();
        let previous_song = player.state().current_song();
        let previous_index = player.queue().current_song_index();
        let before = player.queue().n_songs();
        let audition = fixture(99999);
        let row = window.import_panel().track_row(&audition, ListKind::Search);
        fn buttons(widget: &gtk::Widget) -> Vec<gtk::Button> {
            let mut result = Vec::new();
            if let Some(button) = widget.downcast_ref::<gtk::Button>() {
                result.push(button.clone());
            }
            let mut child = widget.first_child();
            while let Some(widget) = child {
                result.extend(buttons(&widget));
                child = widget.next_sibling();
            }
            result
        }
        let actions = buttons(row.upcast_ref());
        assert_eq!(
            actions.len(),
            1,
            "search rows must have only the star action"
        );
        let star = &actions[0];
        assert_eq!(
            star.icon_name().as_deref(),
            Some("hopea-non-starred-symbolic")
        );
        row.emit_by_name::<()>("activated", &[]);
        assert_eq!(
            player.queue().n_songs(),
            before,
            "clicking a result must only preview"
        );
        assert_eq!(player.queue().current_song_index(), previous_index);
        assert_eq!(favorites.load(Ordering::Relaxed), 0);
        assert_eq!(
            player.state().current_song().unwrap().library_id(),
            Some(audition.id)
        );
        assert!(player.is_previewing());
        let preview = player.state().current_song().unwrap();
        star.emit_clicked();
        for _ in 0..100 {
            if player.queue().n_songs() == before + 1 {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(favorites.load(Ordering::Relaxed), 1);
        assert_eq!(
            player.queue().n_songs(),
            before + 1,
            "star must add to My Music"
        );
        assert_eq!(
            player.state().current_song(),
            Some(preview.clone()),
            "starring must not restart preview"
        );
        assert!(preview.property::<bool>("favorite"));
        assert_eq!(star.icon_name().as_deref(), Some("hopea-starred-symbolic"));
        window.add_remote_track(&audition, false).await;
        assert_eq!(
            player.queue().n_songs(),
            before + 1,
            "saving twice must not duplicate"
        );
        let saved = (0..player.queue().n_songs())
            .filter_map(|i| player.queue().song_at(i))
            .find(|song| song.library_id() == Some(audition.id))
            .unwrap();
        player.queue().remove_song(&saved);
        player.stop();
        if let Some(index) = previous_index {
            player.skip_to(index);
        } else {
            player.clear_queue();
        }
        if let Some(song) = previous_song {
            player.set_current_song(Some(song));
        }
        let imp = self.imp();
        imp.stack.set_visible_child_name("library");
        imp.view_stack.set_visible_child_name("search");
        imp.search_entry.set_text("");
        self.search();
        assert_eq!(imp.loaded.get(), 0);
        assert_eq!(
            imp.search_stack.visible_child_name().as_deref(),
            Some("empty")
        );
        assert!(imp
            .empty_page
            .description()
            .unwrap()
            .contains("albums and artists"));
        assert!(
            offsets.lock().unwrap().is_empty(),
            "empty search must not fetch old tracks"
        );
        imp.search_entry.set_text("test");
        for _ in 0..100 {
            if imp.loaded.get() == 50 && !imp.page_loading.get() {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(imp.loaded.get(), 50);
        let scroll = imp.results_scroll.vadjustment();
        for _ in 0..100 {
            if scroll.page_size() > 0.0 && scroll.upper() > scroll.page_size() {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        scroll.set_value(scroll.upper() - scroll.page_size());
        for _ in 0..100 {
            if imp.loaded.get() == 100 && !imp.page_loading.get() {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(imp.loaded.get(), 100, "scrolling must fetch the next page");
        assert_eq!(
            *offsets.lock().unwrap(),
            vec![0, 50],
            "one request per page"
        );
        // Wait for the second page to be allocated before recording position.
        glib::timeout_future(std::time::Duration::from_millis(200)).await;
        scroll.set_value((scroll.upper() - scroll.page_size()) / 2.0);
        glib::timeout_future(std::time::Duration::from_millis(100)).await;
        let generation = imp.generation.get();
        let position = scroll.value();
        for text in ["test ", "te st", "\ttest\u{a0}"] {
            imp.search_entry.set_text(text);
            glib::timeout_future(std::time::Duration::from_millis(350)).await;
            assert_eq!(
                imp.generation.get(),
                generation,
                "whitespace must not restart discovery"
            );
            assert_eq!(imp.loaded.get(), 100);
            assert!(
                (scroll.value() - position).abs() < 1.0,
                "whitespace must not reset scrolling"
            );
        }
        assert_eq!(*offsets.lock().unwrap(), vec![0, 50]);
        // The following pages are injected directly, as they would be while
        // a search request owns the loading guard. Do not let intermediate
        // one-song fixtures trigger unrelated network pagination.
        imp.page_loading.set(true);
        let mut first = fixture(1);
        first.album = Some("Hybrid Theory".into());
        first.album_artist = Some("Linkin Park".into());
        self.show_page("", 0, &[first.clone()], 3);
        let mut same = fixture(2);
        same.album = Some("HYBRID THEORY".into());
        same.album_artist = Some("LINKIN PARK".into());
        let mut other = fixture(3);
        other.album = Some("Hybrid Theory".into());
        other.album_artist = Some("Another artist".into());
        self.show_page("", 1, &[same, other], 3);
        assert_eq!(
            imp.results.get().unwrap().n_items(),
            3,
            "song list contains only songs"
        );
        assert_eq!(
            imp.album_results.get().unwrap().n_items(),
            2,
            "albums merge across pages, but different artists stay separate"
        );
        assert!(imp.albums_section.is_visible());
        assert_eq!(imp.albums_list.orientation(), gtk::Orientation::Horizontal);
        assert_eq!(imp.results_list.orientation(), gtk::Orientation::Vertical);
        imp.albums_list.emit_by_name::<()>("activate", &[&0u32]);
        assert_eq!(
            imp.album.borrow().as_ref(),
            Some(&("Hybrid Theory".into(), Some("Linkin Park".into())))
        );
        for _ in 0..100 {
            if imp.album_tracks.borrow().len() == 1 {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(
            imp.album_tracks.borrow().len(),
            1,
            "opening a card must load the album contents"
        );
        // The scrolling fixture is hosted in a plain AdwWindow; navigation
        // belongs to the real Hopea window, so exercise it there as well.
        window
            .import_panel()
            .open_album("Hybrid Theory".into(), Some("Linkin Park".into()));
        fn has_art(widget: &gtk::Widget) -> bool {
            if widget
                .downcast_ref::<gtk::Image>()
                .is_some_and(|image| image.paintable().is_some())
            {
                return true;
            }
            let mut child = widget.first_child();
            while let Some(widget) = child {
                if has_art(&widget) {
                    return true;
                }
                child = widget.next_sibling();
            }
            false
        }
        for _ in 0..100 {
            if has_art(window.test_album_page().unwrap().upcast_ref()) {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            has_art(window.test_album_page().unwrap().upcast_ref()),
            "shared album rows must show actual cover art"
        );
        fn album_row(w: &gtk::Widget) -> Option<adw::ActionRow> {
            if let Some(row) = w.downcast_ref::<adw::ActionRow>() {
                return Some(row.clone());
            }
            let mut child = w.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                if let Some(row) = album_row(&widget) {
                    return Some(row);
                }
            }
            None
        }
        let row = album_row(window.test_album_page().unwrap().upcast_ref()).unwrap();
        assert!(
            row.parent().is_some_and(|p| p.is::<gtk::ListBox>()),
            "album rows need an activating list parent"
        );
        let queued = player.queue().n_songs();
        row.emit_by_name::<()>("activated", &[]);
        assert_eq!(player.queue().n_songs(), queued);
        assert_eq!(player.state().current_song().unwrap().library_id(), Some(1));
        assert_eq!(player.album_preview_next(), Some(false));
        player.stop();
        // Starting a new search clears the model; subsequent live snapshots
        // deliberately preserve its scroll position.
        self.reset_search();
        imp.page_loading.set(true);
        self.show_page("new query", 0, &[fixture(4)], 1);
        assert_eq!(imp.album_results.get().unwrap().n_items(), 0);
        assert!(
            !imp.albums_section.is_visible(),
            "a new search must clear the old albums"
        );
        let albums: Vec<_> = (1..=30)
            .map(|id| {
                let mut track = fixture(id);
                track.album = Some(format!("Test album {id}"));
                track.has_cover = true;
                track
            })
            .collect();
        self.reset_search();
        imp.page_loading.set(true);
        self.show_page("", 0, &albums, 30);
        let horizontal = imp.albums_scroll.hadjustment();
        for _ in 0..100 {
            if horizontal.page_size() > 0.0 && horizontal.upper() > horizontal.page_size() {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            horizontal.upper() > horizontal.page_size(),
            "album cards must scroll horizontally"
        );
        assert!(
            imp.results_scroll.height() > 100,
            "album strip must leave space for songs"
        );
        for _ in 0..100 {
            if library::thumbnails::cached(1).is_some() {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            library::thumbnails::cached(1).is_some(),
            "album artwork must load from the library"
        );
        for _ in 0..100 {
            if imp.results_scroll.vadjustment().value() < 1.0 {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            imp.results_scroll.vadjustment().value() < 1.0,
            "replacing search results must return to the first song"
        );
        if let Ok(path) = std::env::var("HOPEA_TEST_SEARCH_SCREENSHOT") {
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gdk::RGBA::WHITE,
                &gtk::graphene::Rect::new(0.0, 0.0, self.width() as f32, self.height() as f32),
            );
            self.snapshot_child(&self.first_child().unwrap(), &snapshot);
            self.native()
                .unwrap()
                .renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
        horizontal.set_value(horizontal.upper() - horizontal.page_size());
        assert!(horizontal.value() > 0.0);
        let tracks: Vec<_> = (1..=5000).map(fixture).collect();
        self.reset_search();
        imp.page_loading.set(true);
        self.show_page("", 0, &tracks, 5000);
        scroll.set_value(0.0);
        for _ in 0..100 {
            if scroll.upper() > 10000.0 && scroll.value() < 1.0 {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        fn rows(widget: &gtk::Widget) -> Vec<adw::ActionRow> {
            let mut found = Vec::new();
            if let Some(row) = widget.downcast_ref::<adw::ActionRow>() {
                found.push(row.clone());
            }
            let mut child = widget.first_child();
            while let Some(next) = child {
                found.extend(rows(&next));
                child = next.next_sibling();
            }
            found
        }
        fn contains_label(widget: &gtk::Widget, text: &str) -> bool {
            if widget
                .downcast_ref::<gtk::Label>()
                .is_some_and(|label| label.text() == text)
            {
                return true;
            }
            let mut child = widget.first_child();
            while let Some(next) = child {
                if contains_label(&next, text) {
                    return true;
                }
                child = next.next_sibling();
            }
            false
        }
        let visible = rows(imp.results_list.upcast_ref());
        assert!(visible.len() < 500, "5000 songs must not create 5000 rows");
        assert!(imp.results_scroll.is_mapped());
        drop(visible);
        imp.results_list
            .scroll_to(4999, gtk::ListScrollFlags::NONE, None);
        scroll.set_value(scroll.upper() - scroll.page_size());
        for _ in 0..100 {
            if contains_label(imp.results_list.upcast_ref(), "Test song 5000") {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            contains_label(imp.results_list.upcast_ref(), "Test song 5000"),
            "scrolling must bind rows from the end of the model"
        );
        assert!(rows(imp.results_list.upcast_ref()).len() < 500);
        self.set_star_state(1, true);
        assert_eq!(imp.results.get().unwrap().n_items(), 4999);
        assert_eq!(
            imp.loaded.get(),
            4999,
            "next page must not skip a song after removing a saved result"
        );
        assert!(imp.results.get().unwrap().iter::<glib::BoxedAnyObject>().flatten()
            .all(|object| matches!(&*object.borrow::<ResultItem>(), ResultItem::Song(track) if track.id != 1)),
            "a song just saved to Favorites must disappear from discovery");
        scroll.set_value(0.0);
        glib::timeout_future(std::time::Duration::from_millis(150)).await;
        assert!(rows(imp.results_list.upcast_ref()).len() < 500);
        imp.page_loading.set(false);
        self.refresh_result_availability();
        for _ in 0..200 {
            if !imp.availability_loading.get() {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(20)).await;
        }
        assert!(!imp.availability_loading.get());
        assert_eq!(
            imp.results.get().unwrap().n_items(),
            4998,
            "periodic refresh must remove only the offline song"
        );
        assert_eq!(imp.loaded.get(), 4998);
        self.clear_results();
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
        library::set_client(previous);
    }
}
