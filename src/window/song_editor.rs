// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::song_edits;
use adw::prelude::*;

impl Window {
    pub(super) async fn edit_song(&self, song: Song, lyrics_tab: bool) {
        let path = song_edits::path(&song);
        let read_path = path.clone();
        let edits = match gio::spawn_blocking(move || song_edits::read(&read_path)).await {
            Ok(Ok(edits)) => edits,
            _ => {
                self.add_toast(i18n("Unable to read saved song edits"));
                return;
            }
        };
        let original_lyrics = crate::lyrics::raw_on_this_computer(&song)
            .await
            .unwrap_or_default();
        let dialog = adw::Dialog::builder()
            .title(i18n("Edit Song"))
            .content_width(560)
            .content_height(640)
            .build();
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::builder()
            .show_end_title_buttons(false)
            .build();
        let cancel = gtk::Button::with_label(&i18n("Cancel"));
        let save = gtk::Button::with_label(&i18n("Save"));
        save.add_css_class("suggested-action");
        header.pack_start(&cancel);
        header.pack_end(&save);
        toolbar.add_top_bar(&header);
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .margin_start(18)
            .margin_end(18)
            .margin_top(12)
            .margin_bottom(18)
            .build();
        let title = gtk::Label::builder()
            .label(song.title())
            .use_markup(false)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["title-2"])
            .build();
        content.append(&title);
        let stack = gtk::Stack::builder()
            .vexpand(true)
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        let switch = gtk::StackSwitcher::builder()
            .stack(&stack)
            .halign(gtk::Align::Center)
            .build();
        content.append(&switch);
        content.append(&stack);
        let cover_page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .valign(gtk::Align::Center)
            .build();
        let preview = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Contain)
            .width_request(192)
            .height_request(192)
            .build();
        preview.set_paintable(song.cover_texture().as_ref());
        cover_page.append(&preview);
        let choose = gtk::Button::with_label(&i18n("Choose Image…"));
        choose.set_halign(gtk::Align::Center);
        cover_page.append(&choose);
        stack.add_titled(&cover_page, Some("cover"), &i18n("Cover"));
        let lyrics_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let help = gtk::Label::builder().label(i18n("Paste lyrics or edit the text below. Keep [mm:ss.xx] timestamps for synchronized lyrics."))
            .wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).xalign(0.0).css_classes(["dim-label"]).build();
        lyrics_page.append(&help);
        let buffer = gtk::TextBuffer::new(None);
        buffer.set_text(&original_lyrics);
        let text = gtk::TextView::builder()
            .buffer(&buffer)
            .wrap_mode(gtk::WrapMode::WordChar)
            .left_margin(12)
            .right_margin(12)
            .top_margin(12)
            .bottom_margin(12)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&text)
            .min_content_height(180)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .css_classes(["card"])
            .build();
        lyrics_page.append(&scroller);
        stack.add_titled(&lyrics_page, Some("lyrics"), &i18n("Lyrics"));
        stack.set_visible_child_name(if lyrics_tab { "lyrics" } else { "cover" });
        let status = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .use_markup(false)
            .xalign(0.0)
            .build();
        content.append(&status);
        toolbar.set_content(Some(&crate::dialogs::scroll(&content)));
        dialog.set_child(Some(&toolbar));
        let selected: Rc<RefCell<Option<glib::Bytes>>> = Rc::default();
        choose.connect_clicked(clone!(
            #[weak(rename_to=win)]
            self,
            #[strong]
            selected,
            #[weak]
            preview,
            #[weak]
            status,
            move |_| {
                glib::spawn_future_local(clone!(
                    #[strong]
                    selected,
                    async move {
                        match crate::artwork::choose(Some(win.upcast_ref())).await {
                            Ok(Some(bytes)) => {
                                if let Ok(texture) = gdk::Texture::from_bytes(&bytes) {
                                    preview.set_paintable(Some(&texture));
                                    selected.replace(Some(bytes));
                                    status.set_text("");
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                status.add_css_class("error");
                                status.set_text(&e);
                            }
                        }
                    }
                ));
            }
        ));
        cancel.connect_clicked(clone!(
            #[weak]
            dialog,
            move |_| {
                dialog.close();
            }
        ));
        save.connect_clicked(clone!(
            #[weak(rename_to=win)] self,
            #[weak] dialog,
            #[weak] content,
            #[weak] cancel,
            #[weak] status,
            #[strong] selected,
            #[strong] buffer,
            move |button| {
                let lyrics = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string();
                if let Err(error)=crate::lyrics::validate_for_song(&song,&lyrics) {
                    status.add_css_class("error"); status.set_text(&error); return;
                }
                let mut edits = edits.clone();
                if lyrics != original_lyrics { edits.lyrics = Some(lyrics); }
                let bytes = selected.borrow().clone().or_else(|| {
                    // Saving again after reconnecting also retries publication
                    // of an own cover already kept on this computer.
                    edits.cover.as_ref().map(|cover| glib::Bytes::from_owned(glib::base64_decode(cover)))
                });
                if let Some(bytes) = &bytes { edits.cover = Some(glib::base64_encode(bytes).to_string()); }
                button.set_sensitive(false);
                cancel.set_sensitive(false);
                dialog.set_can_close(false);
                content.set_sensitive(false);
                status.set_text(&i18n("Saving…"));
                glib::spawn_future_local(clone!(
                    #[strong] win, #[strong] dialog, #[strong] content, #[strong] cancel,
                    #[strong] status, #[strong] button, #[strong] song, #[strong] path,
                    async move {
                        let lyrics_to_publish = edits.lyrics.clone();
                        let result = song_edits::save(path, edits).await;
                        if let Err(e) = result {
                            status.add_css_class("error"); status.set_text(&e);
                        } else {
                            let mut publication_failed = false;
                            if let Some(text) = lyrics_to_publish {
                                publication_failed = crate::lyrics::publish(&song, &text).await.is_err();
                            }
                            if let Some(bytes) = bytes {
                                song.set_cover_bytes(&bytes);
                                if let Some(id) = song.library_id() {
                                    win.refresh_import_cover(id, &bytes);
                                    {
                                        publication_failed |= match library::client() {
                                            Some(client) => match client.add_cover(bytes).await {
                                                Ok(cover) => client.update_cover(id, &cover).await.is_err(),
                                                Err(_) => true,
                                            },
                                            None => true,
                                        };
                                    }
                                }
                            }
                            if win.player().and_then(|p| p.state().current_song()).is_some_and(|s| s.equals(&song)) && win.lyrics_shown() {
                                win.load_lyrics().await;
                            }
                            win.add_toast(if publication_failed { i18n("Saved on this device. Reconnect and save again to publish the cover and lyrics.") } else { i18n("Song changes saved") });
                            dialog.set_can_close(true);
                            dialog.close();
                        }
                        button.set_sensitive(true); cancel.set_sensitive(true);
                        content.set_sensitive(true); dialog.set_can_close(true);
                    }
                ));
            }
        ));
        AdwDialogExt::present(&dialog, Some(self));
    }
}

#[cfg(test)]
impl Window {
    pub(super) async fn test_song_editor(&self) {
        fn children(w: &gtk::Widget) -> Vec<gtk::Widget> {
            let mut result = vec![w.clone()];
            let mut child = w.first_child();
            while let Some(w) = child {
                child = w.next_sibling();
                result.extend(children(&w));
            }
            result
        }
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("node/tests/fixtures/tone.wav");
        let before = std::fs::read(&fixture).unwrap();
        let song = Song::from_uri(&gio::File::for_path(&fixture).uri()).unwrap();
        self.player()
            .unwrap()
            .state()
            .set_current_song(Some(song.clone()));
        self.action_set_enabled("win.edit-lyrics", true);
        gtk::prelude::WidgetExt::activate_action(self, "win.edit-lyrics", None).unwrap();
        let wait = || glib::timeout_future(std::time::Duration::from_millis(300));
        wait().await;
        let editor = self
            .visible_dialog()
            .expect("the track menu opens the editor");
        assert_eq!(editor.title(), "Edit Song");
        crate::dialogs::test_fit(self, &editor).await;
        let buffer = children(editor.upcast_ref())
            .into_iter()
            .find_map(|w| w.downcast::<gtk::TextView>().ok())
            .unwrap()
            .buffer();
        let lrc = "[00:00.10]First line\n[00:00.50]Second line";
        buffer.set_text(lrc);
        let save = children(editor.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.label().as_deref() == Some("Save"))
            .unwrap();
        save.emit_clicked();
        wait().await;
        assert_eq!(
            crate::lyrics::raw_on_this_computer(&song).await.as_deref(),
            Some(lrc)
        );
        assert_eq!(
            std::fs::read(&fixture).unwrap(),
            before,
            "edits must not change audio or its hash"
        );
        self.edit_song(song.clone(), true).await;
        wait().await;
        let reopened = self.visible_dialog().unwrap();
        let buffer = children(reopened.upcast_ref())
            .into_iter()
            .find_map(|w| w.downcast::<gtk::TextView>().ok())
            .unwrap()
            .buffer();
        assert_eq!(
            buffer.text(&buffer.start_iter(), &buffer.end_iter(), true),
            lrc
        );
        buffer.set_text("unsaved changes");
        reopened.close();
        wait().await;
        assert_eq!(
            crate::lyrics::raw_on_this_computer(&song).await.as_deref(),
            Some(lrc)
        );
        let image =
            hopea_node::ingest::reencode_cover(include_bytes!("../../data/icons/cat-kz/64.png"))
                .unwrap();
        let mut edits = song_edits::read(&song_edits::path(&song)).unwrap();
        edits.cover = Some(glib::base64_encode(&image).to_string());
        song_edits::save(song_edits::path(&song), edits)
            .await
            .unwrap();
        let restored = Song::new(&song.uri());
        assert!(restored.has_cover_art());
        assert!(song_edits::path(&song).starts_with(crate::paths::data_dir()));
        let player = self.player().unwrap();
        let state = player.state();
        let previous = state.current_song();
        state.set_current_song(Some(song));
        self.imp().cover_stack.set_visible_child_name("lyrics");
        self.set_lyrics(crate::lyrics::parse(lrc).unwrap());
        state.set_position_ms(499);
        assert_eq!(self.imp().lyrics_line.get(), Some(0));
        state.set_position_ms(500);
        assert_eq!(self.imp().lyrics_line.get(), Some(1));
        state.set_position_ms(125);
        assert_eq!(
            self.imp().lyrics_line.get(),
            Some(0),
            "seeking back preserves millisecond precision"
        );
        let settings = gtk::Settings::default().unwrap();
        let animations = settings.is_gtk_enable_animations();
        settings.set_gtk_enable_animations(true);
        self.set_default_size(900, 700);
        self.imp().main_stack.set_visible_child_name("main-view");
        self.imp().player_stack.set_visible_child_name("player");
        self.imp().library_split.set_show_sidebar(false);
        self.set_playlist_visible(false);
        let long_lrc = (0..40)
            .map(|i| format!("[00:{i:02}.00]Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        state.set_position_ms(0);
        self.set_lyrics(crate::lyrics::parse(&long_lrc).unwrap());
        glib::timeout_future_seconds(2).await;
        assert!(self.imp().lyrics_scroller.is_mapped());
        let scroll = self.imp().lyrics_scroller.vadjustment();
        let start = scroll.value();
        state.set_position_ms(20000);
        let animation = self.imp().lyrics_scroll_animation.borrow().clone().unwrap();
        assert_eq!(animation.duration(), 320);
        assert!(animation.value_to() > start + 10.0);
        assert_eq!(
            scroll.value(),
            start,
            "a line change must start an animation, not teleport"
        );
        for _ in 0..40 {
            if (scroll.value() - animation.value_to()).abs() < 1.0 {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
        }
        assert!((scroll.value() - animation.value_to()).abs() < 1.0);
        let current = scroll.value();
        state.set_position_ms(5000);
        let backward = self.imp().lyrics_scroll_animation.borrow().clone().unwrap();
        assert_eq!(
            backward.value_from(),
            current,
            "seeking starts from the current scroll position"
        );
        assert!(backward.value_to() < current);
        settings.set_gtk_enable_animations(animations);
        self.imp().cover_stack.set_visible_child_name("cover");
        state.set_current_song(previous);
    }
}
