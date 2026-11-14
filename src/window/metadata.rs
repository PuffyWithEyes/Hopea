// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use adw::prelude::*;
use hopea_common::api::MetadataVariant;

struct ReviewGuard(Window);
impl Drop for ReviewGuard {
    fn drop(&mut self) {
        self.0.imp().metadata_reviewing.set(false);
    }
}

impl Window {
    pub(super) fn refresh_current_metadata(&self) {
        if self.is_import_audition()
            || self.visible_dialog().is_some()
            || !utils::settings_manager().boolean("popular-metadata")
        {
            return;
        }
        if let Some(song) = self
            .player()
            .and_then(|p| p.state().current_song())
            .filter(|s| s.library_id().is_some())
        {
            glib::spawn_future_local(clone!(
                #[weak(rename_to=win)]
                self,
                async move {
                    win.review_metadata(song, true).await;
                }
            ));
        }
    }
    pub(crate) fn offer_metadata(&self, track: &Track) {
        let Some(client) = library::client() else {
            return;
        };
        let song = Song::from_remote(track, &client.audio_uri(track.id), None);
        glib::spawn_future_local(clone!(
            #[weak(rename_to=win)]
            self,
            async move {
                win.review_metadata(song, true).await;
            }
        ));
    }

    async fn apply_metadata(&self, song: &Song, variant: &MetadataVariant) -> Result<(), String> {
        let client =
            library::client().ok_or_else(|| i18n("Reconnect to choose covers and lyrics"))?;
        let id = song
            .library_id()
            .ok_or_else(|| i18n("Share this song first"))?;
        let bytes = client
            .metadata_data(id, &variant.hash)
            .await
            .map_err(|e| e.to_string())?;
        if variant.kind == "lyrics" {
            let text = std::str::from_utf8(&bytes).map_err(|_| i18n("Lyrics could not be read"))?;
            crate::lyrics::validate_for_song(song, text)?;
        }
        if !variant.selected {
            client
                .select_metadata(id, &variant.hash)
                .await
                .map_err(|e| e.to_string())?;
        }
        let path = crate::song_edits::path(song);
        let read = path.clone();
        let mut edits = gio::spawn_blocking(move || crate::song_edits::read(&read))
            .await
            .map_err(|_| i18n("Unable to read saved song edits"))??;
        let changed = if variant.kind == "cover" {
            let cover = glib::base64_encode(&bytes).to_string();
            let changed = edits.cover.as_deref() != Some(cover.as_str());
            edits.cover = Some(cover);
            changed
        } else {
            let text =
                String::from_utf8(bytes.to_vec()).map_err(|_| i18n("Lyrics could not be read"))?;
            let changed = edits.lyrics.as_deref() != Some(text.as_str());
            edits.lyrics = Some(text);
            changed
        };
        if changed {
            crate::song_edits::save(path, edits).await?;
        }
        if variant.kind == "cover" && (changed || !song.has_cover_art()) {
            song.set_cover_bytes(&bytes);
            self.refresh_import_cover(id, &bytes);
        }
        if self
            .player()
            .and_then(|p| p.state().current_song())
            .is_some_and(|s| s.library_id() == Some(id))
            && self.lyrics_shown()
            && changed
            && variant.kind == "lyrics"
        {
            self.load_lyrics().await;
        }
        Ok(())
    }

    pub(crate) async fn review_metadata(&self, song: Song, after_add: bool) {
        if self.imp().metadata_reviewing.replace(true) {
            return;
        }
        let _guard = ReviewGuard(self.clone());
        let (Some(client), Some(id)) = (library::client(), song.library_id()) else {
            if !after_add {
                self.add_toast(i18n("Share this song first"));
            }
            return;
        };
        let variants = match client.metadata_variants(id, true).await {
            Ok(v) => v,
            Err(e) => {
                if !after_add {
                    self.add_toast(e.to_string());
                }
                return;
            }
        };
        if after_add && utils::settings_manager().boolean("popular-metadata") {
            for kind in ["cover", "lyrics"] {
                // Stable ties retain the listener's choice; don't flip artwork on every refresh.
                if let Some(best) = variants
                    .iter()
                    .filter(|v| v.kind == kind)
                    .max_by_key(|v| (v.peers, v.selected))
                {
                    if let Err(e) = self.apply_metadata(&song, best).await {
                        self.add_toast(e);
                    }
                }
            }
            return;
        }
        if after_add
            && !["cover", "lyrics"]
                .iter()
                .any(|kind| variants.iter().filter(|v| v.kind == *kind).count() > 1)
        {
            return;
        }
        if variants.is_empty() {
            if !after_add {
                self.add_toast(i18n("No shared covers or lyrics found"));
            }
            return;
        }
        // Do not cover an active import/editor. Keep the choice accessible afterwards.
        if after_add && (self.visible_dialog().is_some() || self.is_import_audition()) {
            let toast = adw::Toast::builder()
                .title(i18n("Alternative covers or lyrics are available"))
                .button_label(i18n("Choose…"))
                .timeout(0)
                .build();
            toast.connect_button_clicked(clone!(
                #[weak(rename_to=win)]
                self,
                #[strong]
                song,
                move |_| {
                    glib::spawn_future_local(clone!(
                        #[strong]
                        win,
                        #[strong]
                        song,
                        async move {
                            win.review_metadata(song, false).await;
                        }
                    ));
                }
            ));
            self.imp().toast_overlay.add_toast(toast);
            return;
        }
        let dialog = adw::Dialog::builder()
            .title(i18n("Choose Cover and Lyrics"))
            .content_width(560)
            .content_height(650)
            .build();
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        toolbar.add_top_bar(&header);
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .margin_start(16)
            .margin_end(16)
            .margin_top(10)
            .margin_bottom(16)
            .build();
        let title = gtk::Label::builder()
            .label(song.title())
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["title-2"])
            .build();
        content.append(&title);
        let help=gtk::Label::builder().label(i18n("One audio track, different covers and lyrics. Counts show recently confirmed peers sharing each variant."))
            .wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).xalign(0.0).css_classes(["dim-label"]).build();
        content.append(&help);
        if utils::settings_manager().boolean("popular-metadata") {
            let hint = gtk::Label::builder()
                .label(i18n(
                    "To keep a manual choice, turn off automatic selection in Settings.",
                ))
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build();
            content.append(&hint);
        }
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .hhomogeneous(false)
            .vhomogeneous(false)
            .build();
        let switch = gtk::StackSwitcher::builder()
            .stack(&stack)
            .halign(gtk::Align::Center)
            .build();
        content.append(&switch);
        content.append(&stack);
        let choices: Rc<RefCell<Vec<(gtk::CheckButton, MetadataVariant)>>> = Rc::default();
        // Selected lyrics are rendered in full, without interpreting markup.
        let preview = gtk::TextBuffer::new(None);
        for (kind, label) in [("cover", i18n("Cover")), ("lyrics", i18n("Lyrics"))] {
            let page = gtk::Box::new(gtk::Orientation::Vertical, 10);
            let mut group: Option<gtk::CheckButton> = None;
            for variant in variants.iter().filter(|v| v.kind == kind) {
                let row = gtk::Box::builder()
                    .orientation(gtk::Orientation::Horizontal)
                    .spacing(12)
                    .margin_start(8)
                    .margin_end(8)
                    .margin_top(8)
                    .margin_bottom(8)
                    .build();
                let check = gtk::CheckButton::new();
                check.set_group(group.as_ref());
                if group.is_none() {
                    group = Some(check.clone());
                }
                check.set_active(variant.selected);
                row.append(&check);
                let data = client.metadata_data(id, &variant.hash).await.ok();
                if kind == "cover" {
                    let image = gtk::Picture::builder()
                        .width_request(96)
                        .height_request(96)
                        .can_shrink(true)
                        .content_fit(gtk::ContentFit::Contain)
                        .build();
                    if let Some(data) = &data {
                        if let Some(p) = crate::utils::load_cover_texture(data) {
                            image.set_paintable(Some(&gdk::Texture::for_pixbuf(&p)));
                        }
                    }
                    row.append(&image);
                } else {
                    let text = data
                        .as_ref()
                        .and_then(|b| std::str::from_utf8(b).ok())
                        .unwrap_or_default()
                        .to_string();
                    if check.is_active() {
                        preview.set_text(&text);
                    }
                    check.connect_toggled(clone!(
                        #[strong]
                        preview,
                        move |check| {
                            if check.is_active() {
                                preview.set_text(&text);
                            }
                        }
                    ));
                }
                let info = gtk::Label::builder()
                    .label(i18n_k(
                        "{count} peers",
                        &[("count", &variant.peers.to_string())],
                    ))
                    .wrap(true)
                    .hexpand(true)
                    .xalign(0.0)
                    .build();
                row.append(&info);
                let button = gtk::Button::builder()
                    .child(&row)
                    .css_classes(["flat"])
                    .build();
                button.connect_clicked(clone!(
                    #[weak]
                    check,
                    move |_| check.set_active(true)
                ));
                page.append(&button);
                choices.borrow_mut().push((check, variant.clone()));
            }
            if kind == "lyrics" {
                let text = gtk::TextView::builder()
                    .buffer(&preview)
                    .editable(false)
                    .cursor_visible(false)
                    .wrap_mode(gtk::WrapMode::WordChar)
                    .left_margin(10)
                    .right_margin(10)
                    .top_margin(10)
                    .bottom_margin(10)
                    .build();
                let scroll = gtk::ScrolledWindow::builder()
                    .child(&text)
                    .min_content_height(200)
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .build();
                page.append(&scroll);
                let play = gtk::Button::with_label(&i18n("Play and Compare Lyrics"));
                page.append(&play);
                play.connect_clicked(clone!(
                    #[weak(rename_to=win)]
                    self,
                    #[weak]
                    dialog,
                    #[strong]
                    song,
                    #[strong]
                    preview,
                    move |_| {
                        let lrc = preview.text(&preview.start_iter(), &preview.end_iter(), true);
                        dialog.close();
                        let back = dialog.clone();
                        let weak = win.downgrade();
                        win.start_import_audition(
                            song.clone(),
                            Box::new(move |resume| {
                                if resume {
                                    if let Some(win) = weak.upgrade() {
                                        AdwDialogExt::present(&back, Some(&win));
                                    }
                                }
                            }),
                        );
                        if let Some(audition) = win.imp().audition.borrow_mut().as_mut() {
                            audition.lyrics = Some(lrc.to_string());
                        }
                        if let Some(lyrics) = crate::lyrics::parse(&lrc) {
                            win.set_lyrics(lyrics);
                            win.imp().cover_stack.set_visible_child_name("lyrics");
                        }
                    }
                ));
            }
            stack.add_titled(&page, Some(kind), &label);
        }
        let status = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .xalign(0.0)
            .build();
        content.append(&status);
        let save = gtk::Button::with_label(&i18n("Use Selected"));
        save.add_css_class("suggested-action");
        content.append(&save);
        save.connect_clicked(clone!(
            #[weak(rename_to=win)]
            self,
            #[weak]
            dialog,
            #[weak]
            status,
            #[strong]
            song,
            #[strong]
            choices,
            move |button| {
                let selected = choices
                    .borrow()
                    .iter()
                    .filter(|(c, _)| c.is_active())
                    .map(|(_, v)| v.clone())
                    .collect::<Vec<_>>();
                button.set_sensitive(false);
                glib::spawn_future_local(clone!(
                    #[strong]
                    win,
                    #[strong]
                    dialog,
                    #[strong]
                    button,
                    #[strong]
                    status,
                    #[strong]
                    song,
                    async move {
                        for v in selected {
                            if let Err(e) = win.apply_metadata(&song, &v).await {
                                status.set_text(&e);
                                status.add_css_class("error");
                                button.set_sensitive(true);
                                return;
                            }
                        }
                        dialog.close();
                    }
                ));
            }
        ));
        toolbar.set_content(Some(&crate::dialogs::scroll(&content)));
        dialog.set_child(Some(&toolbar));
        AdwDialogExt::present(&dialog, Some(self));
    }
}

#[cfg(test)]
impl Window {
    pub(super) async fn test_metadata_choices(&self) {
        use std::io::{BufRead, Write};
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        };
        fn descendants(w: &gtk::Widget) -> Vec<gtk::Widget> {
            let mut result = vec![w.clone()];
            let mut next = w.first_child();
            while let Some(child) = next {
                next = child.next_sibling();
                result.extend(descendants(&child));
            }
            result
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let selected = Arc::new(Mutex::new(Vec::<String>::new()));
        let requests = selected.clone();
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
                let body: Vec<u8> = if line.contains("/variants?") {
                    serde_json::to_vec(
                        &(1..=4)
                            .map(|i| MetadataVariant {
                                hash: format!("{i:064x}"),
                                kind: if i <= 2 { "cover" } else { "lyrics" }.into(),
                                size: 10,
                                selected: i % 2 == 1,
                                peers: if i % 2 == 0 { 3 } else { 1 },
                            })
                            .collect::<Vec<_>>(),
                    )
                    .unwrap()
                } else if line.contains("/variants/") {
                    let hash = line
                        .split("/variants/")
                        .nth(1)
                        .unwrap()
                        .split(' ')
                        .next()
                        .unwrap();
                    if line.starts_with("PUT") {
                        requests.lock().unwrap().push(hash.into());
                        Vec::new()
                    } else if hash.ends_with('1') || hash.ends_with('2') {
                        include_bytes!("../../data/icons/cat-kz/64.png").to_vec()
                    } else if hash.ends_with('3') {
                        b"[00:00.00]First text".to_vec()
                    } else {
                        b"[00:00.00]Popular text".to_vec()
                    }
                } else if line.contains("local=true") {
                    br#"{"tracks":[],"total":0}"#.to_vec()
                } else {
                    b"null".to_vec()
                };
                let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                let _ = stream.write_all(&body);
            }
        });
        let previous = library::client();
        library::set_client(Some(
            library::LibraryClient::new(&format!("http://{addr}"), "test").unwrap(),
        ));
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("node/tests/fixtures/tone.wav");
        let song = Song::from_uri(&gio::File::for_path(fixture).uri()).unwrap();
        song.set_library(Some(98765), false, false);
        let settings = utils::settings_manager();
        let auto = settings.boolean("popular-metadata");
        settings.set_boolean("popular-metadata", true).unwrap();
        self.review_metadata(song.clone(), true).await;
        assert!(
            self.visible_dialog().is_none(),
            "automatic choice must not open a dialog"
        );
        assert_eq!(
            *selected.lock().unwrap(),
            vec![format!("{:064x}", 2), format!("{:064x}", 4)]
        );
        assert_eq!(
            crate::lyrics::raw_on_this_computer(&song).await.as_deref(),
            Some("[00:00.00]Popular text")
        );
        settings.set_boolean("popular-metadata", false).unwrap();
        self.review_metadata(song.clone(), true).await;
        glib::timeout_future(std::time::Duration::from_millis(300)).await;
        let dialog = self.visible_dialog().unwrap();
        assert_eq!(dialog.title(), "Choose Cover and Lyrics");
        crate::dialogs::test_fit(self, &dialog).await;
        let stack = descendants(dialog.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Stack>().ok())
            .find(|s| s.child_by_name("lyrics").is_some())
            .unwrap();
        stack.set_visible_child_name("lyrics");
        glib::timeout_future(std::time::Duration::from_millis(200)).await;
        let play = descendants(dialog.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.label().as_deref() == Some("Play and Compare Lyrics"))
            .unwrap();
        play.emit_clicked();
        glib::timeout_future(std::time::Duration::from_millis(500)).await;
        assert!(self.is_import_audition());
        assert!(self.visible_dialog().is_none());
        assert_eq!(
            self.imp()
                .audition
                .borrow()
                .as_ref()
                .unwrap()
                .lyrics
                .as_deref(),
            Some("[00:00.00]First text")
        );
        assert!(self.player().unwrap().is_previewing());
        gtk::prelude::WidgetExt::activate_action(self, "win.return-to-import", None).unwrap();
        glib::timeout_future(std::time::Duration::from_millis(600)).await;
        assert_eq!(
            self.visible_dialog().unwrap(),
            dialog,
            "Back restores the same choices"
        );
        dialog.close();
        glib::timeout_future(std::time::Duration::from_millis(200)).await;
        settings.set_boolean("popular-metadata", auto).unwrap();
        library::set_client(previous);
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }
}
