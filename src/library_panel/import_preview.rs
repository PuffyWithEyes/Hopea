// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::audio::Song;
#[cfg(test)]
use gst::prelude::*;
use library::service_import::{CatalogTrack, Event, Worker};

pub(super) struct Preview {
    worker: Option<Worker>,
    spotify: Option<library::spotify_audio::SpotifyAudio>,
    active_window: RefCell<Option<glib::WeakRef<Window>>>,
    on_play: RefCell<Option<Box<dyn Fn(Song, &Window)>>>,
    pub artwork: Rc<Artwork>,
    pub label: gtk::Label,
    pub busy: Cell<bool>,
    unavailable: RefCell<std::collections::HashSet<String>>,
    closed: Cell<bool>,
    generation: Cell<u64>,
}
impl Preview {
    pub fn new(
        worker: Option<Worker>,
        spotify: Option<library::spotify_audio::SpotifyAudio>,
    ) -> Rc<Self> {
        Rc::new(Self {
            worker,
            spotify,
            active_window: RefCell::new(None),
            on_play: RefCell::new(None),
            artwork: Artwork::new(),
            label: gtk::Label::builder().wrap(true).xalign(0.0).build(),
            busy: Cell::new(false),
            unavailable: RefCell::new(Default::default()),
            closed: Cell::new(false),
            generation: Cell::new(0),
        })
    }
    pub fn on_play(&self, callback: impl Fn(Song, &Window) + 'static) {
        self.on_play.replace(Some(Box::new(callback)));
    }
    pub fn unavailable(&self, key: &str) -> bool {
        self.unavailable.borrow().contains(key)
    }
    pub fn stop(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        let window = self
            .active_window
            .borrow_mut()
            .take()
            .and_then(|w| w.upgrade());
        if let Some(window) = window {
            window.finish_import_audition(!self.closed.get());
        }
        self.label.set_text("");
    }
    pub fn close(&self) {
        self.closed.set(true);
        self.stop();
    }
    pub fn button(
        self: &Rc<Self>,
        track: CatalogTrack,
        existing_id: Option<i64>,
        parent: Option<&Window>,
    ) -> gtk::Button {
        let button = gtk::Button::builder()
            .icon_name("hopea-media-playback-start-symbolic")
            .tooltip_text(i18n("Listen before importing"))
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let preview = self.clone();
        let parent = parent.map(|w| w.downgrade());
        button.connect_clicked(move |button| {
            if preview.closed.get() {
                return;
            }
            // Only incoming audio uses the shared importer stream. A slow
            // download must not block listening to the existing library copy.
            let prepares_incoming = existing_id.is_none();
            if prepares_incoming && preview.busy.get() {
                preview.label.set_text(&i18n("An incoming preview is still downloading. You can listen to the library copy while you wait."));
                return;
            }
            let (preview, track, parent) = (preview.clone(), track.clone(), parent.clone());
            preview.stop();
            if prepares_incoming {
                preview.busy.set(true);
            }
            button.set_sensitive(false);
            let labeled = button.widget_name().starts_with("import-play-");
            if labeled {
                button.set_child(Some(&gtk::Label::new(Some(&i18n("Preparing…")))));
            }
            let button = button.downgrade();
            let generation = preview.generation.get();
            preview.label.remove_css_class("import-unavailable");
            preview.label.set_text(&i18n(
                "Preparing preview… This file is not shared until you import it.",
            ));
            glib::spawn_future_local(async move {
                let result = preview.prepare(&track, existing_id, generation).await;
                if prepares_incoming {
                    preview.busy.set(false);
                }
                if let Some(button) = button.upgrade() {
                    button.set_sensitive(true);
                    if prepares_incoming {
                        let mut ancestor = button.parent();
                        while let Some(widget) = ancestor {
                            if widget.is::<adw::ActionRow>() || widget.widget_name() == "import-incoming-track" {
                                if preview.unavailable(&track.key) {
                                    widget.add_css_class("import-unavailable");
                                } else {
                                    widget.remove_css_class("import-unavailable");
                                }
                                break;
                            }
                            ancestor = widget.parent();
                        }
                    }
                    if labeled {
                        button.set_child(Some(&adw::ButtonContent::builder()
                            .icon_name("hopea-media-playback-start-symbolic")
                            .label(i18n("Play")).build()));
                    }
                    if let Err(reason) = &result {
                        button.set_tooltip_text(Some(reason));
                    }
                }
                if preview.closed.get() || preview.generation.get() != generation {
                    return;
                }
                match result {
                    Ok(uri) => {
                        let Some(window) = parent.and_then(|w| w.upgrade()) else {
                            preview.label.set_text(&i18n("Open the player window to listen to this song"));
                            return;
                        };
                        let song = Song::from_import(&track, &uri);
                        preview.active_window.replace(Some(window.downgrade()));
                        if let Some(callback) = preview.on_play.borrow().as_ref() {
                            callback(song.clone(), &window);
                        } else {
                            window.start_import_audition(song.clone(), Box::new(|_| {}));
                        }
                        // Reuse the chooser's cache. Late artwork only updates
                        // this song, never a newer audition or the saved queue.
                        let artwork = preview.artwork.clone();
                        let cover_url = track.cover.clone().or_else(|| track.collections.iter().find_map(|c| c.cover.clone()));
                        glib::spawn_future_local(async move {
                            let cover = if let Some(id) = existing_id {
                                if let Some(client) = library::client() { client.cover(id).await.ok() } else { None }
                            } else if let Some(url) = cover_url { artwork.covers.get(&url).await.ok() } else { None };
                            if let Some(cover) = cover { song.set_cover_bytes(&cover); }
                        });
                        preview.label.set_text(&format!(
                            "{} — {}",
                            track.artist.as_deref().unwrap_or(""),
                            track.title
                        ));
                    }
                    Err(reason) => {
                        if preview.unavailable(&track.key) {
                            preview.label.add_css_class("import-unavailable");
                        }
                        preview.label.set_text(&reason);
                    }
                }
            });
        });
        button
    }
    async fn prepare(
        &self,
        track: &CatalogTrack,
        existing_id: Option<i64>,
        generation: u64,
    ) -> Result<String, String> {
        if let Some(id) = existing_id {
            if let Some(uri) = library::shared_files::uri_for(id) {
                if gio::File::for_uri(&uri).query_exists(gio::Cancellable::NONE) {
                    return Ok(uri);
                }
            }
            return library::client()
                .map(|c| c.audio_uri(id))
                .ok_or_else(|| i18n("Connect to your library to listen to this copy"));
        }
        let worker = self
            .worker
            .as_ref()
            .ok_or_else(|| i18n("Preview is unavailable for this song"))?;
        let path = if let Some(spotify) = &self.spotify {
            spotify.download(track).await?
        } else {
            worker.command(serde_json::json!({"key":track.key})).await?;
            loop {
                match worker.next().await? {
                    Some(Event::Downloaded { key, path }) if key == track.key => break path,
                    Some(Event::Progress { message }) if self.generation.get() == generation => {
                        self.label.set_text(&message)
                    }
                    Some(Event::Error { message }) => return Err(message),
                    Some(_) => continue,
                    None => return Err(i18n("The importer stopped before the preview was ready")),
                }
            }
        }
        .ok_or_else(|| {
            self.unavailable.borrow_mut().insert(track.key.clone());
            i18n("The service did not provide audio for this song")
        })?;
        self.unavailable.borrow_mut().remove(&track.key);
        if !worker.accepts(&path) {
            return Err(i18n("The importer returned an unexpected file"));
        }
        Ok(gio::File::for_path(path).uri().to_string())
    }
}
#[cfg(all(test, unix))]
impl LibraryPanel {
    pub async fn test_comparison_playback(&self) {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let directory = glib::user_cache_dir().join("preview-test");
        std::fs::create_dir_all(&directory).unwrap();
        let incoming = directory.join("incoming.flac");
        std::fs::write(
            &incoming,
            include_bytes!("../../node/tests/fixtures/tone.flac"),
        )
        .unwrap();
        let existing = directory.join("existing.wav");
        std::fs::write(
            &existing,
            include_bytes!("../../node/tests/fixtures/tone.wav"),
        )
        .unwrap();
        let response = directory.join("response.jsonl");
        std::fs::write(
            &response,
            format!(
                "{}\n{}\n",
                serde_json::json!({"event":"progress","message":"Preparing test audio"}),
                serde_json::json!({"event":"downloaded","key":"incoming","path":incoming})
            ),
        )
        .unwrap();
        let process = gio::Subprocess::newv(
            &[
                std::ffi::OsStr::new("sh"),
                std::ffi::OsStr::new("-c"),
                std::ffi::OsStr::new("while IFS= read -r command; do sleep 0.3; cat \"$1\"; done"),
                std::ffi::OsStr::new("preview-test"),
                response.as_os_str(),
            ],
            gio::SubprocessFlags::STDIN_PIPE | gio::SubprocessFlags::STDOUT_PIPE,
        )
        .unwrap();
        let preview = Preview::new(Some(Worker::test_process(process, directory)), None);
        let sink = gst::ElementFactory::make("fakesink")
            .property("sync", true)
            .property("signal-handoffs", true)
            .build()
            .unwrap();
        let buffers = Arc::new(AtomicUsize::new(0));
        let seen = buffers.clone();
        sink.connect("handoff", false, move |_| {
            seen.fetch_add(1, Ordering::Relaxed);
            None
        });
        let window = self.window().unwrap();
        let main_player = window.player().unwrap();
        main_player.stop();
        glib::timeout_future(std::time::Duration::from_millis(100)).await;
        main_player.test_audio_sink(&sink);
        let queue_count = main_player.queue().n_songs();
        let track: CatalogTrack = serde_json::from_value(serde_json::json!({
            "key":"incoming", "title":"Incoming", "artist":"Test", "collections":[]
        }))
        .unwrap();
        let existing_uri = gio::File::for_path(&existing).uri();
        library::shared_files::remember(&existing_uri, 991199);
        let right = preview.button(track.clone(), None, self.window().as_ref());
        let left = preview.button(track, Some(991199), self.window().as_ref());
        right.emit_clicked();
        assert!(preview.busy.get());
        left.emit_clicked();
        for _ in 0..150 {
            if buffers.load(Ordering::Relaxed) > 0 {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            buffers.load(Ordering::Relaxed) > 0,
            "the existing copy must play while incoming audio is preparing"
        );
        assert_eq!(
            main_player
                .state()
                .current_song()
                .map(|s| s.uri())
                .as_deref(),
            Some(existing_uri.as_str())
        );
        while preview.busy.get() {
            glib::timeout_future(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(
            main_player
                .state()
                .current_song()
                .map(|s| s.uri())
                .as_deref(),
            Some(existing_uri.as_str()),
            "an older preparation must not replace the selected copy"
        );
        preview.stop();
        buffers.store(0, Ordering::Relaxed);
        right.emit_clicked();
        for _ in 0..150 {
            if !preview.busy.get() && buffers.load(Ordering::Relaxed) > 0 {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(20)).await;
        }
        assert!(!preview.busy.get());
        assert!(
            buffers.load(Ordering::Relaxed) > 0,
            "incoming audio must play after progress events"
        );
        assert_eq!(
            main_player
                .state()
                .current_song()
                .map(|s| s.uri())
                .as_deref(),
            Some(gio::File::for_path(&incoming).uri().as_str())
        );
        assert!(left.is_sensitive() && right.is_sensitive());
        assert!(window.is_import_audition());
        assert_eq!(main_player.queue().n_songs(), queue_count);
        let app = window
            .application()
            .unwrap()
            .downcast::<crate::application::Application>()
            .unwrap();
        app.handle_tray_event(crate::tray::TrayEvent::PlayPause);
        assert!(
            !main_player.state().playing(),
            "tray pauses the audible import preview"
        );
        app.handle_tray_event(crate::tray::TrayEvent::PlayPause);
        assert!(main_player.state().playing());
        let playing = main_player.state().current_song();
        app.handle_tray_event(crate::tray::TrayEvent::Next);
        assert!(!main_player.state().playing());
        assert_eq!(
            main_player.state().current_song(),
            playing,
            "Next cannot escape into the saved playlist"
        );
        preview.close();
        assert!(!window.is_import_audition());
        library::shared_files::forget(991199);
    }
}

// A chooser owns its artwork cache; signed CDN URLs never reach disk or logs.
// Four slow requests cannot block every other image behind one global mutex.
#[cfg(test)]
thread_local! {
    static TEST_COVERS: RefCell<std::collections::HashMap<String, Result<gdk::Texture, String>>> = RefCell::default();
}
pub(super) struct Artwork {
    pub covers: library::import_history::CoverCache,
    slots: (async_channel::Sender<()>, async_channel::Receiver<()>),
    cache: RefCell<std::collections::HashMap<String, Result<gdk::Texture, String>>>,
    order: RefCell<std::collections::VecDeque<String>>,
}
impl Artwork {
    pub fn new() -> Rc<Self> {
        #[cfg(test)]
        let cache = TEST_COVERS.with(|c| c.borrow().clone());
        #[cfg(not(test))]
        let cache = Default::default();
        Rc::new(Self {
            covers: Default::default(),
            slots: async_channel::bounded(4),
            cache: RefCell::new(cache),
            order: RefCell::new(Default::default()),
        })
    }

    #[cfg(test)]
    pub fn seed_test_cover(url: &str) {
        let texture = gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!(
            "../../data/icons/cat-kz/64.png"
        )))
        .unwrap();
        TEST_COVERS.with(|cache| {
            cache.borrow_mut().insert(url.into(), Ok(texture));
        });
    }

    pub fn image(self: &Rc<Self>, url: Option<&str>, size: i32) -> gtk::Overlay {
        let overlay = gtk::Overlay::builder()
            .width_request(size)
            .height_request(size)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .css_classes(["track-thumbnail"])
            .build();
        overlay.set_overflow(gtk::Overflow::Hidden);
        let placeholder = gtk::Image::builder()
            .icon_name("hopea-folder-music-symbolic")
            .pixel_size(size / 2)
            .build();
        overlay.set_child(Some(&placeholder));
        let Some(url) = url.filter(|s| !s.trim().is_empty()) else {
            overlay.set_tooltip_text(Some(&i18n("The service did not provide a cover")));
            return overlay;
        };
        let picture = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Cover)
            .build();
        picture.set_widget_name("import-cover-picture");
        overlay.add_overlay(&picture);
        let started = Rc::new(Cell::new(false));
        picture.connect_map(clone!(
            #[strong(rename_to=art)]
            self,
            #[strong]
            started,
            #[strong(rename_to=url)]
            url.to_string(),
            #[weak]
            placeholder,
            move |picture| {
                if started.replace(true) {
                    return;
                }
                let weak = picture.downgrade();
                let art = art.clone();
                let url = url.clone();
                let placeholder = placeholder.downgrade();
                let started = started.clone();
                glib::spawn_future_local(async move {
                    if art.slots.0.send(()).await.is_err() {
                        return;
                    }
                    let Some(picture) = weak.upgrade() else {
                        let _ = art.slots.1.try_recv();
                        return;
                    };
                    if !picture.is_mapped() {
                        let _ = art.slots.1.try_recv();
                        started.set(false);
                        return;
                    }
                    drop(picture);
                    let cached = art.cache.borrow().get(&url).cloned();
                    let result = match cached {
                        Some(result) => result,
                        None => {
                            let result = art.fetch(&url).await;
                            let mut cache = art.cache.borrow_mut();
                            if cache.insert(url.clone(), result.clone()).is_none() {
                                art.order.borrow_mut().push_back(url);
                            }
                            while cache.len() > 256 {
                                if let Some(old) = art.order.borrow_mut().pop_front() {
                                    cache.remove(&old);
                                }
                            }
                            result
                        }
                    };
                    let _ = art.slots.1.try_recv();
                    if let Some(picture) = weak.upgrade() {
                        match result {
                            Ok(texture) => {
                                picture.set_paintable(Some(&texture));
                                if let Some(placeholder) = placeholder.upgrade() {
                                    placeholder.set_opacity(0.0);
                                }
                            }
                            Err(message) => picture.set_tooltip_text(Some(&message)),
                        }
                    }
                });
            }
        ));
        overlay
    }

    async fn fetch(&self, url: &str) -> Result<gdk::Texture, String> {
        let bytes = self.covers.get(url).await?;
        let pixels = gio::spawn_blocking(move || {
            let stream = gio::MemoryInputStream::from_bytes(&bytes);
            let pixbuf = gdk_pixbuf::Pixbuf::from_stream_at_scale(
                &stream,
                160,
                160,
                true,
                gio::Cancellable::NONE,
            )
            .ok()?;
            Some((
                pixbuf.read_pixel_bytes(),
                pixbuf.width(),
                pixbuf.height(),
                pixbuf.rowstride() as usize,
                pixbuf.has_alpha(),
            ))
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| i18n("Unable to prepare cover image"))?;
        let (bytes, width, height, stride, alpha) = pixels;
        Ok(gdk::MemoryTexture::new(
            width,
            height,
            if alpha {
                gdk::MemoryFormat::R8g8b8a8
            } else {
                gdk::MemoryFormat::R8g8b8
            },
            &bytes,
            stride,
        )
        .upcast())
    }
}
