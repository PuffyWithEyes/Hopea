// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::audio::PlaybackState;
use adw::prelude::*;

/// Run with a headless GTK display, memory settings and temporary XDG dirs.
#[test]
#[ignore = "requires a GTK display and isolated XDG directories; see docs/IMPORTING.md"]
fn navigation_preserves_playback_and_settings_open_without_search() {
    assert_eq!(std::env::var("GSETTINGS_BACKEND").as_deref(), Ok("memory"));
    assert!(glib::user_data_dir().starts_with("/tmp"));
    adw::init().unwrap();
    gtk::Settings::default()
        .unwrap()
        .set_gtk_enable_animations(false);
    gst::init().unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let resources = gio::Resource::load(root.join("_builddir/src/hopea.gresource")).unwrap();
    gio::resources_register(&resources);
    let context = glib::MainContext::default();
    let _guard = context.acquire().unwrap();
    let settings = utils::settings_manager();
    assert!(
        settings.boolean("game-mode"),
        "game mode defaults to enabled"
    );
    // The UI fixture must not inspect the user's real running games.
    settings.set_boolean("game-mode", false).unwrap();
    settings.set_boolean("library-enabled", false).unwrap();
    settings.set_boolean("library-rules-seen", true).unwrap();
    let app = crate::application::Application::new();
    // Never attach this test to a user's already running instance. A remote
    // GApplication does not initialize GTK's local application implementation.
    app.set_flags(app.flags() | gio::ApplicationFlags::NON_UNIQUE);
    app.register(gio::Cancellable::NONE).unwrap();
    assert!(!app.is_remote());
    assert!(
        app.actions_for_accel("<primary>a").is_empty(),
        "Ctrl+A must reach the focused text editor"
    );
    assert!(app
        .accels_for_action("queue.add-folder")
        .iter()
        .all(|key| key.as_str() != "<Primary>a"));
    assert!(!settings.boolean("game-mode-force"));
    settings.set_boolean("game-mode-force", true).unwrap();
    assert!(
        !crate::gaming::active(),
        "master switch still disables the override"
    );
    settings.set_boolean("game-mode", true).unwrap();
    assert!(
        !crate::gaming::active(),
        "retired manual setting must not force Game Mode"
    );
    settings.set_boolean("game-mode-force", false).unwrap();
    assert!(
        !crate::gaming::active(),
        "removing the override restores normal activity immediately"
    );
    settings.set_boolean("game-mode", false).unwrap();
    let window = Window::new(&app);
    assert!(
        !window.has_css_class("devel"),
        "development stripes must not be drawn"
    );
    assert!(!window.imp().import_progress_start.get_visible());
    assert!(!window.imp().import_progress_main.get_visible());
    assert!(
        gtk::IconTheme::for_display(&gtk::prelude::WidgetExt::display(&window))
            .has_icon(crate::config::APPLICATION_ICON)
    );
    let theme = gtk::IconTheme::for_display(&gtk::prelude::WidgetExt::display(&window));
    for line in include_str!("../hopea.gresource.xml").lines() {
        let Some(alias) = line
            .split("alias=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
        else {
            continue;
        };
        let Some(name) = alias
            .strip_suffix(".svg")
            .filter(|s| s.starts_with("hopea-"))
        else {
            continue;
        };
        assert!(theme.has_icon(name), "missing bundled icon: {}", name);
        let icon = theme.lookup_icon(
            name,
            &[],
            24,
            1,
            gtk::TextDirection::Ltr,
            gtk::IconLookupFlags::FORCE_SYMBOLIC,
        );
        assert!(
            icon.file()
                .is_some_and(|f| f.uri().starts_with("resource:///io/hopea/Hopea/icons/")),
            "{} must use the bundled artwork, not a host theme replacement",
            name
        );
    }
    if let Ok(path) = std::env::var("HOPEA_TEST_STARTUP_SCREENSHOT") {
        window.set_default_size(960, 760);
        window.imp().library_split.set_show_sidebar(false);
        window
            .imp()
            .main_stack
            .set_visible_child_name("initial-view");
        let paintable = gtk::WidgetPaintable::new(Some(&window));
        GtkWindowExt::present(&window);
        window.queue_draw();
        context.block_on(glib::timeout_future(std::time::Duration::from_millis(300)));
        let snapshot = gtk::Snapshot::new();
        snapshot.append_color(
            &gdk::RGBA::WHITE,
            &gtk::graphene::Rect::new(0.0, 0.0, window.width() as f32, window.height() as f32),
        );
        paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
        let node = snapshot
            .to_node()
            .or_else(|| {
                let snapshot = gtk::Snapshot::new();
                window.snapshot_child(&window.first_child().unwrap(), &snapshot);
                snapshot.to_node()
            })
            .expect("startup window must render");
        let texture = window.renderer().unwrap().render_texture(&node, None);
        texture.save_to_png(path).unwrap();
        window.imp().main_stack.set_visible_child_name("main-view");
    }
    let player = app.player();
    if let Some(directory) = std::env::var_os("HOPEA_LIVE_IMPORT_DIR") {
        let path = std::path::PathBuf::from(directory).join("selected.flac");
        let imported = Song::from_uri(&gio::File::for_path(path).uri()).unwrap();
        assert!(
            imported.cover_texture().is_some(),
            "real imported audio must load its saved cover"
        );
        player.state().set_current_song(Some(imported.clone()));
        assert_eq!(
            window.imp().song_cover.album_image().cover(),
            imported.cover_texture()
        );
        window.set_default_size(960, 760);
        window.imp().library_split.set_show_sidebar(false);
        window.imp().main_stack.set_visible_child_name("main-view");
        GtkWindowExt::present(&window);
        context.block_on(glib::timeout_future(std::time::Duration::from_millis(400)));
        if let Some(path) = std::env::var_os("HOPEA_TEST_REAL_COVER_SCREENSHOT") {
            let snapshot = gtk::Snapshot::new();
            window.snapshot_child(&window.first_child().unwrap(), &snapshot);
            window
                .renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
        eprintln!("Real imported cover is visible in the player");
    }
    let uri = gio::File::for_path(root.join("node/tests/fixtures/tone.wav")).uri();
    let song = Song::from_uri(&uri).unwrap();
    player.set_volume(0.0);
    player.preview(song.clone());
    assert!(
        player.queue().is_empty(),
        "preview must work with an empty playlist"
    );
    assert!(player.state().playing());
    assert!(
        format!("{:?}", crate::tray::TrayState::from_player(&player)).contains("available: true"),
        "tray transport must work with an empty queue"
    );
    player.toggle_play();
    assert!(
        !player.state().playing(),
        "an empty queue must not disable preview pause"
    );
    player.toggle_play();
    assert!(player.state().playing());
    player.skip_next();
    assert!(!player.state().playing());
    assert!(player.queue().is_empty());
    // End-of-stream on a standalone audition must remain silent.
    player.preview(song.clone());
    player.process_action(crate::audio::PlaybackAction::PlayNext);
    assert!(!player.state().playing());
    let next_uri = gio::File::for_path(root.join("node/tests/fixtures/tone.flac")).uri();
    let next = Song::from_uri(&next_uri).unwrap();
    player.preview_album(vec![song.clone(), next.clone()], 0);
    player.process_action(crate::audio::PlaybackAction::PlayNext);
    assert_eq!(player.state().current_song(), Some(next.clone()));
    assert!(player.state().playing());
    assert!(
        player.queue().is_empty(),
        "album audition must not populate Favorites"
    );
    player.process_action(crate::audio::PlaybackAction::PlayNext);
    assert!(
        !player.state().playing(),
        "end of album must not fall through to another queue"
    );
    player.skip_previous();
    assert_eq!(player.state().current_song(), Some(song.clone()));
    player.clear_queue();
    assert!(player.queue().add_song(&song));
    assert!(
        !player.queue().add_song(&song),
        "adding the same row twice must be a no-op"
    );
    let remote_track: hopea_common::api::Track = serde_json::from_value(serde_json::json!({
        "id": 999, "title": "Different display title", "artist": null,
        "album": null, "album_artist": null, "track_number": null, "year": null,
        "genre": null, "duration_ms": 1000, "format": "wav", "codec": "pcm",
        "sample_rate": 44100, "channels": 1, "size": 100,
        "sha256": song.content_hash().unwrap(), "has_cover": false,
        "uploader": "test", "uploaded_at": 0
    }))
    .unwrap();
    let remote = Song::from_remote(&remote_track, "http://127.0.0.1:1/audio/999", None);
    assert!(
        song.equals(&remote),
        "local and network versions must share content identity"
    );
    assert!(!player.queue().add_song(&remote));
    let restored = crate::audio::Queue::default();
    restored.add_songs(&[song.clone(), remote.clone(), song.clone()]);
    assert_eq!(
        restored.n_songs(),
        1,
        "restore must collapse existing duplicates"
    );
    restored.set_shuffled(true);
    restored.set_current_song(Some(song.clone()));
    restored.add_songs(&[remote]);
    assert_eq!(restored.n_songs(), 1);
    assert_eq!(restored.current_song(), Some(song.clone()));
    let mut rebuilt_track = remote_track.clone();
    rebuilt_track.sha256 = "a".repeat(64);
    let rebuilt = Song::from_remote(&rebuilt_track, "http://127.0.0.1:1/audio/1000", None);
    song.set_library(Some(rebuilt_track.id), true, false);
    assert!(
        song.equals(&rebuilt),
        "the sanitized shared copy must match its original"
    );
    assert!(!restored.add_song(&rebuilt));
    restored.add_songs(&[rebuilt]);
    assert_eq!(restored.n_songs(), 1);
    song.set_library(None, false, false);
    let fixture_dir = crate::paths::data_dir().join("duplicate-fixtures");
    let first_path = fixture_dir.join("first/same-title.wav");
    let second_path = fixture_dir.join("second/same-title.wav");
    std::fs::create_dir_all(first_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(second_path.parent().unwrap()).unwrap();
    let mut bytes = std::fs::read(root.join("node/tests/fixtures/tone.wav")).unwrap();
    std::fs::write(&first_path, &bytes).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&second_path, &bytes).unwrap();
    let first_uri = gio::File::for_path(&first_path).uri();
    let first = Song::from_uri(&first_uri).unwrap();
    let other = Song::from_uri(&gio::File::for_path(&second_path).uri()).unwrap();
    assert_eq!(first.uuid(), other.uuid(), "same display tags and filename");
    assert!(
        !first.equals(&other),
        "different audio must remain selectable"
    );
    let cached = Song::from_uri(&first_uri).unwrap();
    assert_eq!(cached.content_hash(), first.content_hash());
    std::fs::write(&first_path, bytes).unwrap();
    let changed = Song::from_uri(&first_uri).unwrap();
    assert_eq!(
        changed.content_hash(),
        other.content_hash(),
        "invalidate a changed file's cached hash"
    );
    let distinct = crate::audio::Queue::default();
    distinct.add_songs(&[first, other]);
    assert_eq!(distinct.n_songs(), 2);
    player.state().set_current_song(Some(song.clone()));
    player.state().set_playback_state(&PlaybackState::Playing);
    player.state().set_position(42);
    let second =
        Song::from_uri(&gio::File::for_path(root.join("node/tests/fixtures/tone.flac")).uri())
            .unwrap();
    player.queue().set_current_song(Some(song.clone()));
    player.preview(second.clone());
    assert_eq!(player.queue().n_songs(), 1);
    assert_eq!(player.queue().current_song(), Some(song.clone()));
    assert_eq!(player.state().current_song(), Some(second.clone()));
    player.skip_to(0);
    assert_eq!(
        player.state().current_song(),
        Some(song.clone()),
        "same queue index must leave preview"
    );
    assert!(!player.is_previewing());
    player.state().set_position(42);
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "windows",
        target_os = "macos"
    ))]
    {
        use crate::tray::{TrayEvent, TrayState};
        player.set_volume(0.0);
        player.queue().add_song(&second);
        player.queue().set_current_song(Some(song.clone()));
        player.state().set_position(0);
        app.handle_tray_event(TrayEvent::PlayPause);
        assert!(!player.state().playing());
        app.handle_tray_event(TrayEvent::Next);
        assert_eq!(player.state().current_song(), Some(second.clone()));
        let tray = TrayState::from_player(&player);
        assert!(format!("{tray:?}").contains(&second.title()));
        app.handle_tray_event(TrayEvent::Previous);
        assert_eq!(player.state().current_song(), Some(song.clone()));
        app.handle_tray_event(TrayEvent::PlayPause);
        assert!(player.state().playing());
        app.handle_tray_event(TrayEvent::PlayPause);
        player.queue().remove_song(&second);
        player.state().set_playback_state(&PlaybackState::Playing);
        player.state().set_position(42);
    }
    let reordered = crate::audio::Queue::default();
    // A paused song below the first row must move to the top without seeking
    // or briefly rebinding the highlighted row to another song.
    player.queue().add_song(&second);
    player.skip_to(1);
    player.pause();
    player.state().set_position(42);
    window.set_playlist_shuffled(true);
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(100)));
    assert_eq!(player.queue().song_at(0), Some(second.clone()));
    assert_eq!(player.queue().current_song_index(), Some(0));
    assert_eq!(player.state().current_song(), Some(second.clone()));
    assert_eq!(player.state().position(), 42);
    assert!(!player.state().playing());
    assert_eq!(
        window
            .imp()
            .playlist_view
            .queue_view()
            .model()
            .unwrap()
            .item(0)
            .and_downcast::<Song>(),
        Some(second.clone())
    );
    assert!(window.imp().playlist_view.scroll().vadjustment().value() < 1.0);
    window.set_playlist_shuffled(false);
    player.skip_to(0);
    player.queue().remove_song(&second);
    player.state().set_playback_state(&PlaybackState::Playing);
    player.state().set_position(42);
    reordered.add_songs(&[song.clone(), second.clone()]);
    reordered.set_current_song(Some(song.clone()));
    let reloads = Rc::new(Cell::new(0));
    let observed = reloads.clone();
    reordered.connect_notify_local(Some("current"), move |_, _| {
        observed.set(observed.get() + 1)
    });
    assert!(reordered.move_song(&song, &second, true));
    assert_eq!(reordered.song_at(0), Some(second.clone()));
    assert_eq!(reordered.current_song(), Some(song.clone()));
    assert_eq!(
        reloads.get(),
        0,
        "reordering must not reload the currently playing song"
    );
    assert!(reordered.move_song(&song, &second, false));
    assert_eq!(reordered.current_song_index(), Some(0));
    assert!(!reordered.move_song(&song, &song, true));
    reordered.set_shuffled(true);
    let before = reloads.get();
    assert!(reordered.move_song(&song, &second, true));
    assert!(reordered.is_shuffled());
    assert_eq!(reordered.song_at(1), Some(song.clone()));
    assert_eq!(reordered.current_song(), Some(song.clone()));
    assert_eq!(
        reloads.get(),
        before,
        "reordering a shuffled queue must preserve playback"
    );
    reordered.set_shuffled(false);
    assert_eq!(
        reordered.song_at(0),
        Some(song.clone()),
        "leaving Shuffle restores the underlying playlist"
    );
    // Cover updates reach the existing queue immediately, with no audio reload
    // or wait for the background import to finish.
    song.set_library(Some(42), true, true);
    let notified = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = notified.clone();
    song.connect_notify_local(Some("cover"), move |song, _| {
        assert!(song.cover_texture().is_some());
        assert!(crate::audio::CoverCache::global().try_lock().is_ok());
        seen.set(true);
    });
    let cover = glib::Bytes::from_static(include_bytes!("../../data/icons/cat-kz/64.png"));
    window.refresh_import_cover(42, &cover);
    assert!(notified.get());
    assert!(song.cover_texture().is_some());
    assert!(window.imp().song_cover.album_image().cover().is_some());
    let favorite = descendants(window.upcast_ref())
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .find(|button| button.action_name().as_deref() == Some("win.toggle-favorite"))
        .unwrap();
    assert_eq!(
        favorite.icon_name().as_deref(),
        Some("hopea-starred-symbolic")
    );
    assert!(favorite.has_css_class("suggested-action"));
    song.set_property("favorite", false);
    assert_eq!(
        favorite.icon_name().as_deref(),
        Some("hopea-non-starred-symbolic")
    );
    assert!(!favorite.has_css_class("suggested-action"));
    song.set_property("favorite", true);
    assert!(favorite.has_css_class("suggested-action"));

    // Exercise loading from disk, including the constructor used during queue
    // restoration, rather than only injecting pixels into an existing Song.
    let fixture_dir = glib::user_data_dir().join("cover-test");
    std::fs::create_dir_all(&fixture_dir).unwrap();
    let audio = fixture_dir.join("imported.wav");
    std::fs::copy(root.join("node/tests/fixtures/tone.wav"), &audio).unwrap();
    context
        .block_on(library::import_history::save_cover(&audio, &cover))
        .unwrap();
    let restored = Song::new(&gio::File::for_path(&audio).uri());
    assert!(
        restored.cover_texture().is_some(),
        "queue restoration must load the sidecar"
    );
    let white = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 16, 16).unwrap();
    white.fill(0xffffffff);
    let white_bytes = glib::Bytes::from_owned(white.save_to_bufferv("png", &[]).unwrap());
    context
        .block_on(library::import_history::save_cover(&audio, &white_bytes))
        .unwrap();
    let changed = Song::from_uri(&gio::File::for_path(&audio).uri()).unwrap();
    assert!(
        changed.cover_texture().is_some(),
        "a white image must not depend on palette extraction"
    );
    assert_ne!(
        restored.cover_uuid(),
        changed.cover_uuid(),
        "changed art must invalidate cached art"
    );
    // Late Song notifications must propagate through PlayerState to the large
    // cover without manually notifying the state or changing songs.
    song.set_cover_bytes(&white_bytes);
    assert_eq!(
        window.imp().song_cover.album_image().cover(),
        song.cover_texture()
    );
    song.set_cover_bytes(&cover);
    if let Ok(path) = std::env::var("HOPEA_TEST_COVER_SCREENSHOT") {
        window.set_default_size(960, 760);
        window.imp().library_split.set_show_sidebar(false);
        window.imp().main_stack.set_visible_child_name("main-view");
        GtkWindowExt::present(&window);
        context.block_on(glib::timeout_future(std::time::Duration::from_millis(300)));
        let snapshot = gtk::Snapshot::new();
        window.snapshot_child(&window.first_child().unwrap(), &snapshot);
        let texture = window
            .renderer()
            .unwrap()
            .render_texture(&snapshot.to_node().unwrap(), None);
        texture.save_to_png(path).unwrap();
    }
    let unchanged = || {
        assert_eq!(player.queue().n_songs(), 1);
        assert_eq!(player.state().current_song(), Some(song.clone()));
        assert_eq!(player.state().position(), 42);
        assert!(player.state().playing());
        assert!(!window.in_for_you());
    };
    window.imp().for_you_button.set_active(true);
    unchanged();
    assert_eq!(
        window.imp().player_stack.visible_child_name().as_deref(),
        Some("for-you")
    );
    window.imp().for_you_button.set_active(false);
    unchanged();
    assert_eq!(
        window.imp().player_stack.visible_child_name().as_deref(),
        Some("player")
    );
    // Even pressing Play without a joined network must keep the current song.
    window.imp().for_you_button.set_active(true);
    context.block_on(window.start_for_you());
    unchanged();
    window.imp().library_split.set_show_sidebar(false);
    GtkWindowExt::present(&window);
    gtk::prelude::WidgetExt::activate_action(&window, "win.library-settings", None).unwrap();
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(200)));
    let dialog = window
        .visible_dialog()
        .expect("settings must open with search hidden");
    assert_eq!(dialog.widget_name(), "settings-form");
    let game = descendants(dialog.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<adw::SwitchRow>().ok())
        .find(|row| row.title() == "Game Mode")
        .expect("Game Mode belongs in Settings");
    assert!(!game.is_active());
    game.set_active(true);
    assert!(!descendants(dialog.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<adw::SwitchRow>().ok())
        .any(|row| row.title() == "Keep Game Mode Active"));
    assert!(
        !settings.boolean("game-mode"),
        "unsaved changes must not affect the detector"
    );
    let popular = descendants(dialog.upcast_ref())
        .into_iter()
        .find(|w| w.widget_name() == "popular-metadata")
        .unwrap()
        .downcast::<adw::SwitchRow>()
        .unwrap();
    assert!(
        popular.is_active(),
        "most-shared metadata is enabled on first launch"
    );
    popular.set_active(false);
    assert!(
        utils::settings_manager().boolean("popular-metadata"),
        "Cancel must preserve the preference"
    );
    let language = descendants(dialog.upcast_ref())
        .into_iter()
        .find(|w| w.widget_name() == "language-choice")
        .unwrap()
        .downcast::<adw::ComboRow>()
        .unwrap();
    let languages = language
        .model()
        .unwrap()
        .downcast::<gtk::StringList>()
        .unwrap();
    let russian = (0..languages.n_items())
        .find(|&i| languages.string(i).as_deref() == Some("Русский"))
        .unwrap();
    assert_eq!(language.selected(), 0, "system language is the default");
    language.set_selected(russian);
    assert!(
        crate::language::preference().is_empty(),
        "selection is not saved before Save"
    );
    let invitation = descendants(dialog.upcast_ref())
        .into_iter()
        .find(|w| w.widget_name() == "language-invitation")
        .unwrap()
        .downcast::<gtk::Stack>()
        .unwrap();
    assert_eq!(
        invitation.transition_type(),
        gtk::StackTransitionType::Crossfade
    );
    gtk::Settings::default()
        .unwrap()
        .set_gtk_enable_animations(true);
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(3500)));
    assert_ne!(
        invitation.visible_child_name().as_deref(),
        Some("0"),
        "the invitation must cycle languages"
    );
    gtk::Settings::default()
        .unwrap()
        .set_gtk_enable_animations(false);
    context.block_on(crate::dialogs::test_fit(&window, &dialog));
    let footer_buttons: Vec<_> = descendants(dialog.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .filter(|b| matches!(b.label().as_deref(), Some("Cancel" | "Save")))
        .collect();
    assert_eq!(footer_buttons.len(), 2);
    let footer = footer_buttons[0].parent().unwrap();
    for width in [360, 800, 614, 390, 960, 614, 360] {
        window.set_default_size(width, 800);
        context.block_on(glib::timeout_future(std::time::Duration::from_millis(300)));
        let left = footer_buttons[0].compute_bounds(&footer).unwrap();
        let right = footer_buttons[1].compute_bounds(&footer).unwrap();
        assert!(
            (left.width() - right.width()).abs() <= 1.0,
            "footer actions must have equal widths after resize"
        );
        assert!(
            (left.y() - right.y()).abs() <= 1.0,
            "footer actions must stay on the same row"
        );
        assert!(
            left.x().abs() <= 1.0
                && (right.x() + right.width() - footer.width() as f32).abs() <= 1.0,
            "footer actions must fill both edges without an empty column after resize"
        );
    }
    assert!(!window.imp().library_split.shows_sidebar());
    let cache_open = descendants(dialog.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<adw::ButtonRow>().ok())
        .find(|row| row.title() == "Clear Cache…")
        .expect("cache cleanup belongs in Settings");
    let cache_root = crate::paths::cache_dir();
    std::fs::create_dir_all(cache_root.join("lyrics")).unwrap();
    std::fs::create_dir_all(cache_root.join("waveforms")).unwrap();
    std::fs::write(cache_root.join("lyrics/cleanup-test.lrc"), "lyrics").unwrap();
    std::fs::write(cache_root.join("waveforms/cleanup-test.json"), "[]").unwrap();
    cache_open.emit_by_name::<()>("activated", &[]);
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(300)));
    let cache = window
        .visible_dialog()
        .unwrap()
        .downcast::<adw::AlertDialog>()
        .unwrap();
    assert_eq!(cache.heading().as_deref(), Some("Clear Cache"));
    context.block_on(crate::dialogs::test_fit(&window, cache.upcast_ref()));
    let cache_widgets = descendants(cache.upcast_ref());
    let rows: Vec<_> = cache_widgets
        .iter()
        .filter_map(|w| w.downcast_ref::<adw::SwitchRow>())
        .collect();
    assert_eq!(rows.len(), 3);
    let clear = cache_widgets
        .iter()
        .filter_map(|w| w.downcast_ref::<gtk::Button>())
        .find(|b| b.label().as_deref() == Some("Clear Selected"))
        .unwrap();
    assert!(!clear.is_sensitive());
    rows.iter()
        .find(|row| row.title() == "Downloaded Lyrics")
        .unwrap()
        .set_active(true);
    assert!(clear.is_sensitive());
    clear.emit_clicked();
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(300)));
    assert!(!cache_root.join("lyrics/cleanup-test.lrc").exists());
    assert!(cache_root.join("waveforms/cleanup-test.json").exists());
    cache.close();
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(200)));
    dialog.close();
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(200)));
    assert!(
        !settings.boolean("game-mode"),
        "Cancel must preserve game mode"
    );
    assert!(!settings.boolean("game-mode-force"));
    assert!(
        crate::language::preference().is_empty(),
        "Cancel must preserve the language"
    );
    context.block_on(crate::accent::test_settings(&window));
    window.imp().library_panel.check_vpn_notice();
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(200)));
    let vpn_settings = window.visible_dialog().expect("VPN notice opens Settings");
    let vpn_row = descendants(vpn_settings.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<adw::SwitchRow>().ok())
        .find(|row| row.title() == "Share Only Through a VPN")
        .expect("VPN setting must be accessible from the notice");
    assert_eq!(
        vpn_row.is_active(),
        settings.boolean("library-only-protected")
    );
    vpn_settings.close();
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(200)));
    window.imp().library_panel.refresh_protection_notice();
    gtk::prelude::WidgetExt::activate_action(&window, "win.import-music", None).unwrap();
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(200)));
    let picker = window
        .visible_dialog()
        .unwrap()
        .downcast::<adw::AlertDialog>()
        .unwrap();
    fn descendants(widget: &gtk::Widget) -> Vec<gtk::Widget> {
        let mut result = vec![widget.clone()];
        let mut child = widget.first_child();
        while let Some(widget) = child {
            result.extend(descendants(&widget));
            child = widget.next_sibling();
        }
        result
    }
    context.block_on(crate::dialogs::test_fit(&window, picker.upcast_ref()));
    let button = descendants(&picker.extra_child().unwrap())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.label().as_deref() == Some(&i18n("Import Account…")))
        .expect("account import must be reachable from the main menu");
    button.emit_clicked();
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(400)));
    let form = window
        .visible_dialog()
        .unwrap()
        .downcast::<adw::AlertDialog>()
        .unwrap();
    context.block_on(crate::dialogs::test_fit(&window, form.upcast_ref()));
    let titles: Vec<_> = descendants(&form.extra_child().unwrap())
        .into_iter()
        .filter_map(|w| w.downcast::<adw::ButtonRow>().ok())
        .filter(|row| row.is_visible())
        .map(|row| row.title().to_string())
        .collect();
    assert!(titles.contains(&i18n("Sign In with Default Browser…")));
    assert!(!titles.iter().any(|title| title.contains("Firefox")));
    let fields = descendants(&form.extra_child().unwrap());
    let cover_switch = fields
        .iter()
        .filter_map(|w| w.downcast_ref::<adw::SwitchRow>())
        .find(|row| row.title() == i18n("Import Cover Art"))
        .unwrap();
    assert!(cover_switch.is_active());
    cover_switch.set_active(false);
    assert!(!cover_switch.is_active());
    let service = fields
        .iter()
        .find_map(|w| w.downcast_ref::<adw::ComboRow>())
        .unwrap();
    let secret = fields
        .iter()
        .find_map(|w| w.downcast_ref::<adw::PasswordEntryRow>())
        .unwrap();
    let account = fields
        .iter()
        .filter_map(|w| w.downcast_ref::<adw::EntryRow>())
        .find(|row| row.title() == i18n("VK Profile ID to Import"))
        .unwrap();
    assert!(
        !form.is_response_enabled("import"),
        "an empty form cannot be submitted"
    );
    account.set_text("123456789");
    assert!(
        !form.is_response_enabled("import"),
        "a profile ID is not a session"
    );
    secret.set_text(" \n ");
    assert!(!form.is_response_enabled("import"));
    secret.set_text("synthetic-session");
    assert!(form.is_response_enabled("import"));
    for invalid in ["", " ", "0", "-1", "123x", "https://vk.ru/id123456789"] {
        account.set_text(invalid);
        assert!(
            !form.is_response_enabled("import"),
            "invalid profile: {}",
            invalid
        );
    }
    account.set_text(" 123456789 ");
    assert!(form.is_response_enabled("import"));
    service.set_sensitive(false);
    assert!(
        !form.is_response_enabled("import"),
        "sign-in cannot overlap import"
    );
    service.set_sensitive(true);
    assert!(form.is_response_enabled("import"));
    secret.set_text("");
    assert!(!form.is_response_enabled("import"));
    // A direct response must also keep invalid input in the form.
    form.emit_by_name::<()>("response", &[&"import"]);
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(200)));
    assert_eq!(window.visible_dialog(), Some(form.clone().upcast()));
    assert_eq!(account.text().trim(), "123456789");
    for selected in [1, 2, 3, 0] {
        service.set_selected(selected);
        assert!(
            !form.is_response_enabled("import"),
            "switching services clears credentials"
        );
        secret.set_text("synthetic-session");
        assert!(form.is_response_enabled("import"));
    }
    if std::env::var("HOPEA_IMPORT_PYTHON").as_deref() == Ok("/nonexistent/hopea-test-python") {
        service.set_selected(2);
        let download = fields
            .iter()
            .find_map(|w| w.downcast_ref::<adw::SwitchRow>())
            .unwrap();
        assert!(download.is_sensitive(), "Spotify audio can be selected");
        assert!(download.is_active());
        download.set_active(false); // Metadata-only import remains available.
        secret.set_text("synthetic-token");
        form.emit_by_name::<()>("response", &[&"import"]);
        context.block_on(glib::timeout_future(std::time::Duration::from_millis(400)));
        let error = window
            .visible_dialog()
            .expect("startup errors must be visible with search hidden")
            .downcast::<adw::AlertDialog>()
            .unwrap();
        assert_eq!(
            error.heading().as_deref(),
            Some(i18n("Import Not Completed").as_str())
        );
        assert!(descendants(error.upcast_ref()).iter().any(|w| w
            .downcast_ref::<gtk::Label>()
            .is_some_and(|label| label
                .text()
                .contains("Unable to start the service importer")
                && label.has_css_class("error"))));
        assert!(!window.imp().import_progress_start.get_visible());
        assert!(!window.imp().import_progress_main.get_visible());
        error.close();
    }
    form.close();
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(200)));
    for label in ["Paste Link…", "Paste List…"] {
        gtk::prelude::WidgetExt::activate_action(&window, "win.import-music", None).unwrap();
        context.block_on(glib::timeout_future(std::time::Duration::from_millis(200)));
        let picker = window
            .visible_dialog()
            .unwrap()
            .downcast::<adw::AlertDialog>()
            .unwrap();
        descendants(&picker.extra_child().unwrap())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|button| button.label().as_deref() == Some(i18n(label).as_str()))
            .unwrap()
            .emit_clicked();
        context.block_on(glib::timeout_future(std::time::Duration::from_millis(400)));
        let input = window
            .visible_dialog()
            .unwrap()
            .downcast::<adw::AlertDialog>()
            .unwrap();
        context.block_on(crate::dialogs::test_fit(&window, input.upcast_ref()));
        assert!(!input.is_response_enabled("ok"));
        for child in descendants(&input.extra_child().unwrap()) {
            if let Some(entry) = child.downcast_ref::<gtk::Entry>() {
                entry.set_text("https://music.yandex.ru/album/123");
                assert!(input.is_response_enabled("ok"));
                entry.set_text("   ");
                assert!(!input.is_response_enabled("ok"));
            } else if let Some(view) = child.downcast_ref::<gtk::TextView>() {
                view.buffer().set_text("Artist - Song");
                assert!(input.is_response_enabled("ok"));
                view.buffer().set_text(" \n ");
                assert!(!input.is_response_enabled("ok"));
            }
        }
        input.close();
        context.block_on(glib::timeout_future(std::time::Duration::from_millis(200)));
    }
    context.block_on(window.import_panel().test_picker_and_background_controls());
    #[cfg(unix)]
    context.block_on(window.import_panel().test_comparison_playback());
    let panel: crate::library_panel::LibraryPanel = glib::Object::new();
    let list_window = adw::Window::builder()
        .default_width(420)
        .default_height(780)
        .content(&panel)
        .build();
    GtkWindowExt::present(&list_window);
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(300)));
    context.block_on(panel.test_welcome_layout(&list_window));
    context.block_on(panel.test_scroll_loading_and_recycling(&window));
    list_window.close();
    context.block_on(window.test_collection_editor());
    context.block_on(crate::library::import_albums::ImportedAlbums::test_persistence());
    let editor_window = Window::new(&app);
    GtkWindowExt::present(&editor_window);
    context.block_on(editor_window.test_song_editor());
    context.block_on(editor_window.test_metadata_choices());
    editor_window.close();
    let drag_window = Window::new(&app);
    context.block_on(drag_window.test_animated_reorder());
    drag_window.close();
    // Portrait navigation keeps its own center regardless of the number of
    // header actions. Rotating back must restore the same toggle widgets.
    window.imp().main_stack.set_visible_child_name("main-view");
    window.imp().library_split.set_show_sidebar(false);
    window.imp().split_view.set_show_sidebar(false);
    GtkWindowExt::present(&window);
    gtk::Settings::default()
        .unwrap()
        .set_gtk_enable_animations(false);
    for width in [360, 390, 560, 800, 390] {
        window.set_default_size(width, 780);
        for _ in 0..100 {
            if window.width() == width && window.imp().mode_switch.width() > 0 {
                break;
            }
            context.block_on(glib::timeout_future(std::time::Duration::from_millis(50)));
        }
        for importing in [false, true] {
            window.set_import_active(importing);
            context.block_on(glib::timeout_future(std::time::Duration::from_millis(100)));
            let imp = window.imp();
            if width <= 560 {
                assert_eq!(
                    imp.mode_switch.parent(),
                    Some(imp.mode_switch_narrow.get().upcast())
                );
                let bounds = imp.mode_switch.compute_bounds(&window).unwrap();
                let center = bounds.x() + bounds.width() / 2.0;
                assert!(
                    (center - window.width() as f32 / 2.0).abs() <= 1.0,
                    "navigation must remain centered at width {} with importing={}: center={}, window={}, content={}",
                    width,
                    importing, center, window.width(), imp.mode_switch_narrow.width()
                );
            } else {
                assert_eq!(
                    imp.player_header.title_widget(),
                    Some(imp.mode_switch.get().upcast())
                );
                assert!(!imp.mode_switch_narrow.is_visible());
            }
        }
    }
    window.set_import_active(false);
    if let Some(path) = std::env::var_os("HOPEA_TEST_PORTRAIT_SCREENSHOT") {
        context.block_on(glib::timeout_future(std::time::Duration::from_millis(1100)));
        let snapshot = gtk::Snapshot::new();
        snapshot.append_color(
            &gdk::RGBA::WHITE,
            &gtk::graphene::Rect::new(0.0, 0.0, window.width() as f32, window.height() as f32),
        );
        window.snapshot_child(&window.first_child().unwrap(), &snapshot);
        window
            .renderer()
            .unwrap()
            .render_texture(&snapshot.to_node().unwrap(), None)
            .save_to_png(path)
            .unwrap();
    }
    window.close();
}

/// Exercise a genuinely scrollable queue, rather than two rows that both fit.
#[test]
#[ignore = "requires a GTK display and isolated XDG directories; see docs/IMPORTING.md"]
fn progressive_playlist_and_shuffle_start_at_top() {
    assert_eq!(std::env::var("GSETTINGS_BACKEND").as_deref(), Ok("memory"));
    assert!(glib::user_data_dir().starts_with("/tmp"));
    adw::init().unwrap();
    gtk::Settings::default()
        .unwrap()
        .set_gtk_enable_animations(false);
    gst::init().unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    gio::resources_register(
        &gio::Resource::load(root.join("_builddir/src/hopea.gresource")).unwrap(),
    );
    let context = glib::MainContext::default();
    let _guard = context.acquire().unwrap();
    let settings = utils::settings_manager();
    settings.set_boolean("library-enabled", false).unwrap();
    settings.set_boolean("library-rules-seen", true).unwrap();
    settings.set_boolean("game-mode", false).unwrap();
    let app = crate::application::Application::new();
    app.set_flags(app.flags() | gio::ApplicationFlags::NON_UNIQUE);
    app.register(gio::Cancellable::NONE).unwrap();
    let window = Window::new(&app);
    window.set_default_size(1100, 600);
    GtkWindowExt::present(&window);
    let directory = crate::paths::data_dir().join("startup-fixtures");
    std::fs::create_dir_all(&directory).unwrap();
    let original = std::fs::read(root.join("node/tests/fixtures/tone.wav")).unwrap();
    let files: Vec<_> = (0u16..200)
        .map(|i| {
            let mut bytes = original.clone();
            let len = bytes.len();
            bytes[len - 2..].copy_from_slice(&i.to_le_bytes());
            let path = directory.join(format!("track-{i:03}.wav"));
            std::fs::write(&path, bytes).unwrap();
            gio::File::for_path(path)
        })
        .collect();
    let mut input = files.clone();
    input.push(files[10].clone()); // Duplicate within the same load.
    input.push(gio::File::for_path(directory.join("missing.wav")));
    window.queue_songs_with_feedback(input, false);
    let player = window.player().unwrap();
    let deadline = Instant::now() + std::time::Duration::from_secs(30);
    context.block_on(async {
        while player.queue().is_empty() && Instant::now() < deadline {
            glib::timeout_future(std::time::Duration::from_millis(5)).await;
        }
    });
    assert!(player.queue().n_songs() > 0 && player.queue().n_songs() < 200);
    assert!(
        window.imp().playlist_loading.get(),
        "first rows appear before indexing finishes"
    );
    window.store_personal_playlist();
    let saved_during_load = utils::load_cached_songs().unwrap();
    assert!(
        files
            .iter()
            .all(|file| saved_during_load.iter().any(|saved| saved.equal(file))),
        "saving during restoration must preserve unread favorites"
    );
    let scroll = window.imp().playlist_view.scroll();
    let adjustment = scroll.vadjustment();
    context.block_on(async {
        while (player.queue().n_songs() < 50 || adjustment.upper() <= adjustment.page_size())
            && Instant::now() < deadline
        {
            glib::timeout_future(std::time::Duration::from_millis(5)).await;
        }
    });
    assert!(
        window.imp().playlist_loading.get(),
        "scroll during loading, not afterwards"
    );
    assert!(scroll.is_sensitive());
    assert!(scroll.emit_by_name::<bool>("scroll-child", &[&gtk::ScrollType::PageForward, &false]));
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(40)));
    assert!(
        adjustment.value() > 100.0,
        "loading must accept user scrolling: {}",
        adjustment.value()
    );
    context.block_on(async {
        while window.imp().playlist_loading.get() && Instant::now() < deadline {
            glib::timeout_future(std::time::Duration::from_millis(10)).await;
        }
    });
    assert!(!window.imp().playlist_loading.get());
    assert!(
        adjustment.value() > 100.0,
        "later batches must not reset the user's scroll position"
    );
    assert_eq!(player.queue().n_songs(), 200);
    assert_eq!(utils::load_cached_songs().unwrap().len(), 200);
    for (i, file) in files.iter().enumerate() {
        assert!(player.queue().song_at(i as u32).unwrap().file().equal(file));
    }
    player.skip_to(125);
    player.pause();
    let current = player.state().current_song().unwrap();
    let view = window.imp().playlist_view.queue_view();
    view.scroll_to(199, gtk::ListScrollFlags::FOCUS, None);
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(250)));
    let adjustment = window.imp().playlist_view.scroll().vadjustment();
    assert!(
        adjustment.value() > 100.0,
        "fixture must start scrolled down"
    );
    player.state().set_position(42);
    window.set_playlist_shuffled(true);
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(250)));
    assert_eq!(player.queue().song_at(0), Some(current.clone()));
    assert_eq!(player.state().current_song(), Some(current));
    assert_eq!(player.state().position(), 42);
    assert!(!player.state().playing());
    assert!(
        adjustment.value() < 1.0,
        "shuffle must scroll to the beginning: {}",
        adjustment.value()
    );
    window.queue_songs_with_feedback(
        vec![
            files[10].clone(),
            gio::File::for_path(directory.join("missing.wav")),
        ],
        false,
    );
    context.block_on(async {
        while window.imp().playlist_loading.get() && Instant::now() < deadline {
            glib::timeout_future(std::time::Duration::from_millis(10)).await;
        }
    });
    assert!(
        !window.imp().playlist_loading.get(),
        "all duplicate/invalid load must finish"
    );
    assert_eq!(player.queue().n_songs(), 200);
    // Joining the node restores favorites through a separate path from the
    // cached playlist. It must yield while local files are still being read.
    player.clear_queue();
    let tracks = files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let id = index as i64 + 1;
            library::shared_files::remember(&file.uri(), id);
            serde_json::from_value(serde_json::json!({
                "id": id, "title": format!("Song {index}"), "artist": null, "album": null,
                "album_artist": null, "track_number": null, "year": null, "genre": null,
                "duration_ms": 1000, "format": "wav", "codec": "pcm", "sample_rate": 44100,
                "channels": 1, "size": 100, "sha256": format!("{id:064x}"),
                "has_cover": false, "uploader": "fixture", "uploaded_at": 0,
                "favorite": true, "mine": true, "local": true
            }))
            .unwrap()
        })
        .collect();
    let finished = std::rc::Rc::new(std::cell::Cell::new(false));
    let task_window = window.clone();
    let task_finished = finished.clone();
    let loading = glib::spawn_future_local(async move {
        task_window.show_favorites_in_playlist(tracks).await;
        task_finished.set(true);
    });
    context.block_on(async {
        while (player.queue().n_songs() < 50 || adjustment.upper() <= adjustment.page_size())
            && Instant::now() < deadline
        {
            glib::timeout_future(std::time::Duration::from_millis(5)).await;
        }
    });
    assert!(
        !finished.get(),
        "favorites must not block until the full catalogue has loaded"
    );
    assert!(scroll.emit_by_name::<bool>("scroll-child", &[&gtk::ScrollType::PageForward, &false]));
    context.block_on(glib::timeout_future(std::time::Duration::from_millis(40)));
    assert!(
        adjustment.value() > 100.0,
        "favorites must accept scrolling during loading"
    );
    context.block_on(loading).unwrap();
    assert_eq!(player.queue().n_songs(), 200);
    assert!(
        adjustment.value() > 100.0,
        "favorites must preserve the user's scroll position"
    );
    window.destroy();
}
