// SPDX-FileCopyrightText: 2022  Emmanuele Bassi
// SPDX-License-Identifier: GPL-3.0-or-later

#[cfg(test)]
mod tests;

mod collections;
mod import_audition;
mod metadata;
mod song_drag;
mod song_editor;

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    str::FromStr,
    time::Instant,
};

use adw::{prelude::AnimationExt, subclass::prelude::*};
use glib::{clone, closure_local};
use gtk::{gdk, gio, glib, prelude::*, CompositeTemplate};
use hopea_common::api::Track;
use log::debug;

use crate::{
    audio::{AudioPlayer, RepeatMode, ReplayGainMode, Song},
    config::APPLICATION_ICON,
    drag_overlay::DragOverlay,
    i18n::{i18n, i18n_k, ni18n_f, ni18n_k},
    library,
    library_panel::LibraryPanel,
    playback_control::PlaybackControl,
    playlist_view::PlaylistView,
    queue_row::QueueRow,
    search::FuzzyFilter,
    song_cover::SongCover,
    song_details::SongDetails,
    sort::FuzzySorter,
    utils,
    volume_control::VolumeControl,
    waveform_view::WaveformView,
};

pub enum WindowMode {
    InitialView,
    MainView,
}

const ATTRIBUTE_HOST_PATH: &str = "xattr::document-portal.host-path";

mod imp {
    use glib::{ParamSpec, ParamSpecBoolean, ParamSpecEnum, Value};
    use once_cell::sync::Lazy;

    use super::*;

    #[derive(CompositeTemplate)]
    #[template(resource = "/io/hopea/Hopea/window.ui")]
    pub struct Window {
        // Template widgets
        #[template_child]
        pub drag_overlay: TemplateChild<DragOverlay>,
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub main_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub status_page: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub song_cover: TemplateChild<SongCover>,
        #[template_child]
        pub song_details: TemplateChild<SongDetails>,
        #[template_child]
        pub waveform_view: TemplateChild<WaveformView>,
        #[template_child]
        pub elapsed_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub remaining_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub playback_control: TemplateChild<PlaybackControl>,
        #[template_child]
        pub split_view: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub library_split: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub library_sheet: TemplateChild<adw::BottomSheet>,
        #[template_child]
        pub narrow_breakpoint: TemplateChild<adw::Breakpoint>,
        #[template_child]
        pub library_panel: TemplateChild<LibraryPanel>,
        #[template_child]
        pub import_progress_start: TemplateChild<gtk::Button>,
        #[template_child]
        pub import_progress_main: TemplateChild<gtk::Button>,
        #[template_child]
        pub for_you_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub my_music_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub player_header: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub audition_bar: TemplateChild<gtk::Box>,
        pub(super) audition: RefCell<Option<import_audition::State>>,
        #[template_child]
        pub mode_switch: TemplateChild<gtk::Box>,
        #[template_child]
        pub mode_switch_narrow: TemplateChild<gtk::Box>,
        #[template_child]
        pub cover_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub lyrics_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub lyrics_scroller: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub lyrics_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub lyrics_none_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub lyrics_online_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub lyrics_online_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub player_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub for_you_play_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub queue_mode_page: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub playlist_view: TemplateChild<PlaylistView>,
        #[template_child]
        pub add_folder_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub restore_playlist_button: TemplateChild<gtk::Button>,

        pub provider: gtk::CssProvider,
        pub settings: gio::Settings,

        pub playlist_shuffled: Cell<bool>,
        pub playlist_loading: Cell<bool>,
        pub pending_playlist_songs: RefCell<Vec<Song>>,
        pub pending_playlist_files: RefCell<std::collections::VecDeque<gio::File>>,
        pub playlist_visible: Cell<bool>,
        pub playlist_selection: Cell<bool>,
        pub playlist_search: Cell<bool>,
        pub replaygain_mode: Cell<ReplayGainMode>,

        pub playlist_filtermodel: RefCell<Option<gio::ListModel>>,

        pub notify_playing_id: RefCell<Option<glib::SignalHandlerId>>,
        pub notify_position_id: RefCell<Option<glib::SignalHandlerId>>,
        pub notify_song_id: RefCell<Option<glib::SignalHandlerId>>,
        pub notify_cover_id: RefCell<Option<glib::SignalHandlerId>>,
        pub notify_favorite_id: RefCell<Option<glib::SignalHandlerId>>,
        pub notify_nsongs_id: RefCell<Option<glib::SignalHandlerId>>,
        pub notify_current_id: RefCell<Option<glib::SignalHandlerId>>,
        pub notify_peaks_id: RefCell<Option<glib::SignalHandlerId>>,
        /// Playing recommendations ("For You") rather than the playlist
        pub in_for_you: Cell<bool>,
        /// The queue that isn't playing: the playlist or For You
        pub my_music_songs: RefCell<Vec<Song>>,
        pub for_you_songs: RefCell<Vec<Song>>,
        /// More recommendations are being fetched
        pub for_you_refilling: Cell<bool>,
        pub metadata_reviewing: Cell<bool>,
        pub for_you_checking: Cell<bool>,
        /// Tracks already asked to download ahead
        pub prefetched: RefCell<std::collections::HashSet<i64>>,
        /// Songs from other services are being looked for
        pub looking_for_wanted: Cell<bool>,
        /// The lyrics shown, their labels and the line being sung
        pub lyrics: RefCell<Option<crate::lyrics::Lyrics>>,
        pub lyrics_labels: RefCell<Vec<gtk::Label>>,
        pub lyrics_line: Cell<Option<usize>>,
        pub lyrics_scroll_animation: RefCell<Option<adw::TimedAnimation>>,
        /// Bumped for every lookup, so that a late answer is ignored
        pub lyrics_generation: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "HopeaWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);

            klass.install_action("win.return-to-import", None, |win, _, _| {
                win.finish_import_audition(true)
            });
            klass.install_action("win.play", None, move |win, _, _| {
                debug!("Window::win.play()");
                if let Some(p) = win.player() {
                    p.toggle_play();
                }
            });
            klass.install_action("win.seek-backwards", None, move |win, _, _| {
                debug!("Window::win.seek-backwards");
                if let Some(p) = win.player() {
                    p.seek_backwards();
                }
            });
            klass.install_action("win.seek-forward", None, move |win, _, _| {
                debug!("Window::win.seek-forward");
                if let Some(p) = win.player() {
                    p.seek_forward();
                }
            });
            klass.install_action("win.previous", None, move |win, _, _| {
                debug!("Window::win.previous()");
                if let Some(p) = win.player() {
                    p.skip_previous();
                }
            });
            klass.install_action("win.next", None, move |win, _, _| {
                debug!("Window::win.next()");
                if let Some(p) = win.player() {
                    p.skip_next();
                }
            });
            klass.install_action("queue.repeat-mode", None, move |win, _, _| {
                debug!("Window::queue.repeat()");
                if let Some(p) = win.player() {
                    p.toggle_repeat_mode();
                }
            });
            klass.install_action_async("queue.add-song", None, |win, _, _| async move {
                debug!("Window::win.add-song()");
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                let filter = gtk::FileFilter::new();
                gtk::FileFilter::set_name(&filter, Some(&i18n("Audio files")));
                filter.add_mime_type("audio/*");
                filters.append(&filter);

                let xdg_music = match glib::user_special_dir(glib::UserDirectory::Music) {
                    Some(p) => p,
                    None => glib::current_dir(),
                };

                let dialog = gtk::FileDialog::builder()
                    .accept_label(&i18n("_Add Song"))
                    .filters(&filters)
                    .initial_folder(&gio::File::for_path(&xdg_music))
                    .modal(true)
                    .title(&i18n("Open File"))
                    .build();

                if let Ok(files) = dialog.open_multiple_future(Some(&win)).await {
                    if files.n_items() == 0 {
                        win.add_toast(i18n("Unable to access files"));
                    } else {
                        win.add_files_to_queue(&files);
                    }
                }
            });
            klass.install_action_async("queue.add-folder", None, |win, _, _| async move {
                debug!("Window::win.add-folder()");
                let xdg_music = match glib::user_special_dir(glib::UserDirectory::Music) {
                    Some(p) => p,
                    None => glib::current_dir(),
                };

                let dialog = gtk::FileDialog::builder()
                    .accept_label(&i18n("_Add Folder"))
                    .initial_folder(&gio::File::for_path(&xdg_music))
                    .modal(true)
                    .title(&i18n("Open Folder"))
                    .build();

                if let Ok(files) = dialog.select_multiple_folders_future(Some(&win)).await {
                    if files.n_items() == 0 {
                        win.add_toast(i18n("Unable to access files"));
                    } else {
                        win.add_files_to_queue(&files);
                    }
                }
            });
            klass.install_action("queue.restore-playlist", None, move |win, _, _| {
                debug!("Window::queue.restore-playlist()");
                win.restore_playlist();
            });
            klass.install_action("win.copy", None, move |win, _, _| {
                debug!("Window::win.copy()");
                win.copy_song();
            });
            for (action, lyrics) in [("win.edit-cover", false), ("win.edit-lyrics", true)] {
                klass.install_action_async(action, None, move |win, _, _| async move {
                    if let Some(song) = win.player().and_then(|p| p.state().current_song()) {
                        win.edit_song(song, lyrics).await;
                    }
                });
            }
            klass.install_action_async("win.metadata", None, |win, _, _| async move {
                if let Some(song) = win.player().and_then(|p| p.state().current_song()) {
                    win.review_metadata(song, false).await;
                }
            });
            klass.install_action_async("win.open-directory", None, async move |win, _, _| {
                debug!("Window::win.open-directory()");
                win.open_song_directory().await
            });
            klass.install_action("win.shared-library", None, move |win, _, _| {
                debug!("Window::win.shared-library()");
                win.toggle_library();
            });
            // The shared music settings, also before joining and with the
            // panel hidden
            klass.install_action_async("win.library-settings", None, async move |win, _, _| {
                win.imp().library_panel.edit_settings().await;
            });
            klass.install_action_async("win.import-music", None, async move |win, _, _| {
                win.imp().library_panel.import_music().await;
            });
            klass.install_action_async("win.my-albums", None, async move |win, _, _| {
                win.show_collections().await;
            });
            klass.install_action("win.import-progress", None, |win, _, _| {
                win.imp().library_panel.show_import_job();
            });
            klass.install_action_async("win.toggle-favorite", None, async move |win, _, _| {
                debug!("Window::win.toggle-favorite()");
                win.toggle_current_favorite().await;
            });
            klass.install_action("queue.clear", None, move |win, _, _| {
                debug!("Window::queue.clear()");
                win.clear_queue();
            });
            klass.install_property_action("queue.toggle", "playlist-visible");
            klass.install_property_action("queue.shuffle", "playlist-shuffled");
            klass.install_property_action("queue.select", "playlist-selection");
            klass.install_property_action("queue.search", "playlist-search");
            klass.install_property_action("win.replaygain", "replaygain-mode");

            klass.install_action(
                "win.skip-to",
                Some(glib::VariantTy::UINT32),
                move |win, _, param| {
                    if let Some(pos) = param.and_then(u32::from_variant) {
                        if let Some(player) = win.player() {
                            player.skip_to(pos);
                            player.play();
                        }
                    }
                },
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }

        fn new() -> Self {
            Self {
                song_details: TemplateChild::default(),
                song_cover: TemplateChild::default(),
                split_view: TemplateChild::default(),
                library_split: TemplateChild::default(),
                library_sheet: TemplateChild::default(),
                narrow_breakpoint: TemplateChild::default(),
                library_panel: TemplateChild::default(),
                import_progress_start: TemplateChild::default(),
                import_progress_main: TemplateChild::default(),
                for_you_button: TemplateChild::default(),
                my_music_button: TemplateChild::default(),
                player_header: TemplateChild::default(),
                audition_bar: TemplateChild::default(),
                audition: RefCell::new(None),
                mode_switch: TemplateChild::default(),
                mode_switch_narrow: TemplateChild::default(),
                cover_stack: TemplateChild::default(),
                lyrics_stack: TemplateChild::default(),
                lyrics_scroller: TemplateChild::default(),
                lyrics_box: TemplateChild::default(),
                lyrics_none_label: TemplateChild::default(),
                lyrics_online_box: TemplateChild::default(),
                lyrics_online_button: TemplateChild::default(),
                player_stack: TemplateChild::default(),
                for_you_play_button: TemplateChild::default(),
                queue_mode_page: TemplateChild::default(),
                toast_overlay: TemplateChild::default(),
                drag_overlay: TemplateChild::default(),
                playback_control: TemplateChild::default(),
                waveform_view: TemplateChild::default(),
                elapsed_label: TemplateChild::default(),
                remaining_label: TemplateChild::default(),
                main_stack: TemplateChild::default(),
                status_page: TemplateChild::default(),
                add_folder_button: TemplateChild::default(),
                restore_playlist_button: TemplateChild::default(),
                playlist_view: TemplateChild::default(),
                playlist_shuffled: Cell::new(false),
                playlist_loading: Cell::new(false),
                pending_playlist_files: RefCell::default(),
                pending_playlist_songs: RefCell::default(),
                playlist_visible: Cell::new(true),
                playlist_selection: Cell::new(false),
                playlist_search: Cell::new(false),
                playlist_filtermodel: RefCell::default(),
                replaygain_mode: Cell::new(ReplayGainMode::default()),
                provider: gtk::CssProvider::new(),
                settings: utils::settings_manager(),
                notify_playing_id: RefCell::new(None),
                notify_position_id: RefCell::new(None),
                notify_song_id: RefCell::new(None),
                notify_cover_id: RefCell::new(None),
                notify_favorite_id: RefCell::new(None),
                notify_nsongs_id: RefCell::new(None),
                notify_current_id: RefCell::new(None),
                notify_peaks_id: RefCell::new(None),
                in_for_you: Cell::default(),
                my_music_songs: RefCell::default(),
                for_you_songs: RefCell::default(),
                for_you_refilling: Cell::default(),
                metadata_reviewing: Cell::default(),
                for_you_checking: Cell::default(),
                prefetched: RefCell::default(),
                looking_for_wanted: Cell::default(),
                lyrics: RefCell::default(),
                lyrics_labels: RefCell::default(),
                lyrics_line: Cell::default(),
                lyrics_scroll_animation: RefCell::default(),
                lyrics_generation: Cell::default(),
            }
        }
    }

    impl ObjectImpl for Window {
        fn properties() -> &'static [ParamSpec] {
            static PROPERTIES: Lazy<Vec<ParamSpec>> = Lazy::new(|| {
                vec![
                    ParamSpecBoolean::builder("playlist-shuffled").build(),
                    ParamSpecBoolean::builder("playlist-visible").build(),
                    ParamSpecBoolean::builder("playlist-selection").build(),
                    ParamSpecBoolean::builder("playlist-search").build(),
                    ParamSpecEnum::builder::<ReplayGainMode>("replaygain-mode").build(),
                ]
            });
            PROPERTIES.as_ref()
        }

        fn set_property(&self, _id: usize, value: &Value, pspec: &ParamSpec) {
            let obj = self.obj();
            match pspec.name() {
                "playlist-shuffled" => obj.set_playlist_shuffled(value.get::<bool>().unwrap()),
                "playlist-visible" => obj.set_playlist_visible(value.get::<bool>().unwrap()),
                "playlist-selection" => obj.set_playlist_selection(value.get::<bool>().unwrap()),
                "playlist-search" => obj.set_playlist_search(value.get::<bool>().unwrap()),
                "replaygain-mode" => obj.set_replaygain(value.get::<ReplayGainMode>().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &ParamSpec) -> Value {
            let obj = self.obj();
            match pspec.name() {
                "playlist-shuffled" => obj.playlist_shuffled().to_value(),
                "playlist-visible" => obj.playlist_visible().to_value(),
                "playlist-selection" => obj.playlist_selection().to_value(),
                "playlist-search" => obj.playlist_search().to_value(),
                "replaygain-mode" => obj.replaygain().to_value(),
                _ => unimplemented!(),
            }
        }
    }

    impl WidgetImpl for Window {}
    impl WindowImpl for Window {}
    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow, adw::ApplicationWindow,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl Window {
    pub fn set_import_active(&self, active: bool) {
        self.imp().import_progress_start.set_visible(active);
        self.imp().import_progress_main.set_visible(active);
    }

    pub fn show_import_result_toast(&self) {
        let toast = adw::Toast::builder()
            .title(i18n("Import finished"))
            .button_label(i18n("View Result"))
            .action_name("win.import-progress")
            .timeout(0)
            .build();
        self.imp().toast_overlay.add_toast(toast);
    }

    pub fn import_panel(&self) -> crate::library_panel::LibraryPanel {
        self.imp().library_panel.get()
    }

    pub fn new<P: IsA<gtk::Application>>(application: &P) -> Self {
        let win = glib::Object::builder::<Window>()
            .property("application", application)
            .build();

        win.setup_waveform();
        win.setup_actions();
        win.setup_playlist();
        win.setup_drop_target();
        win.setup_provider();
        win.bind_state();
        win.bind_queue();
        win.connect_signals();
        win.setup_library_sheet();
        win.setup_for_you();
        win.setup_lyrics();
        win.restore_window_state();
        win.set_import_active(LibraryPanel::import_is_running());
        win.set_initial_state();
        win.open_with_music();
        win.show_rules_once();

        win
    }

    pub(crate) fn player(&self) -> Option<Rc<AudioPlayer>> {
        if let Some(app) = self.application() {
            let player = app
                .downcast::<crate::application::Application>()
                .unwrap()
                .player();
            return Some(player);
        }

        None
    }

    fn setup_actions(&self) {
        let enable_recoloring = self.imp().settings.boolean("enable-recoloring");
        self.add_action_entries([gio::ActionEntry::builder("enable-recoloring")
            .state(enable_recoloring.to_variant())
            .activate(|this: &Window, action, _| {
                let state = action.state().unwrap();
                let action_state: bool = state.get().unwrap();
                let enable_recoloring = !action_state;
                action.set_state(&enable_recoloring.to_variant());

                this.imp()
                    .settings
                    .set_boolean("enable-recoloring", enable_recoloring)
                    .expect("Unable to store setting");
            })
            .build()])
    }

    fn setup_waveform(&self) {
        if let Some(player) = self.player() {
            let peaks = player.waveform_generator().peaks();
            self.imp().waveform_view.set_peaks(peaks);

            let notify_peaks_id = player.waveform_generator().connect_notify_local(
                Some("has-peaks"),
                clone!(
                    #[weak(rename_to = win)]
                    self,
                    move |gen, _| {
                        let peaks = gen.peaks();
                        win.imp().waveform_view.set_peaks(peaks);
                    }
                ),
            );
            self.imp().notify_peaks_id.replace(Some(notify_peaks_id));
        }
    }

    fn unbind_waveform(&self) {
        if let Some(player) = self.player() {
            if let Some(id) = self.imp().notify_peaks_id.take() {
                player.waveform_generator().disconnect(id);
            }
        }
    }

    fn restore_window_state(&self) {
        let settings = utils::settings_manager();
        let width = settings.int("window-width");
        let height = settings.int("window-height");
        self.set_default_size(width, height);
    }

    fn reset_queue(&self) {
        self.set_playlist_visible(false);
        self.set_playlist_shuffled(false);
        self.set_playlist_selection(false);
        self.update_style(None);
    }

    fn clear_queue(&self) {
        if let Some(p) = self.player() {
            p.clear_queue();
        }
    }

    fn playlist_visible(&self) -> bool {
        self.imp().playlist_visible.get()
    }

    fn set_playlist_visible(&self, visible: bool) {
        let visible = visible && !self.is_import_audition();
        let changed = visible != self.imp().playlist_visible.replace(visible);
        // Breakpoints can change the widget before this notification arrives.
        // Keep it synchronized even when our stored state was already false.
        if self.imp().split_view.shows_sidebar() != visible {
            self.imp().split_view.set_show_sidebar(visible);
        }
        if changed {
            self.notify("playlist-visible");
        }
    }

    fn playlist_shuffled(&self) -> bool {
        self.imp().playlist_shuffled.get()
    }

    fn set_playlist_shuffled(&self, shuffled: bool) {
        let imp = self.imp();

        if shuffled != imp.playlist_shuffled.replace(shuffled) {
            if let Some(player) = self.player() {
                let queue = player.queue();
                let view = imp.playlist_view.queue_view();
                // GtkListView otherwise keeps the old visible/focused row as
                // its anchor when every item changes position. That row may
                // now be at the very end of the shuffled list.
                let model = view.model();
                if shuffled {
                    view.set_model(gtk::SelectionModel::NONE);
                }
                queue.set_shuffled(shuffled);
                if shuffled {
                    view.set_model(model.as_ref());
                }
                if shuffled && queue.n_songs() > 0 {
                    view.scroll_to(0, gtk::ListScrollFlags::NONE, None);
                }
            }

            self.notify("playlist-shuffled");
        }
    }

    fn playlist_selection(&self) -> bool {
        self.imp().playlist_selection.get()
    }

    fn set_playlist_selection(&self, selection: bool) {
        let imp = self.imp();

        if selection != imp.playlist_selection.replace(selection) {
            if !selection {
                if let Some(player) = self.player() {
                    let queue = player.queue();
                    queue.unselect_all_songs();
                }
            }

            self.imp()
                .playlist_view
                .queue_actionbar()
                .set_revealed(selection);

            self.notify("playlist-selection");
        }
    }

    fn playlist_search(&self) -> bool {
        self.imp().playlist_search.get()
    }

    fn set_playlist_search(&self, search: bool) {
        let imp = self.imp();

        if search != imp.playlist_search.replace(search) {
            if search {
                self.set_playlist_visible(true);
            }
            imp.playlist_view.set_search(search);
            self.notify("playlist-search");
        }
    }

    /// Saving while background restoration runs must not drop unread favorites.
    fn store_personal_playlist(&self) {
        let mut songs = if self.in_for_you() {
            self.imp().my_music_songs.borrow().clone()
        } else if let Some(player) = self.player() {
            (0..player.queue().n_songs())
                .filter_map(|i| player.queue().song_at(i))
                .collect()
        } else {
            return;
        };
        songs.extend(self.imp().pending_playlist_songs.borrow().iter().cloned());
        let pending: Vec<_> = self
            .imp()
            .pending_playlist_files
            .borrow()
            .iter()
            .cloned()
            .collect();
        utils::store_songs_with_pending(&songs, &pending);
    }

    fn restore_playlist(&self) {
        if let Some(songs) = utils::load_cached_songs() {
            self.queue_songs_with_feedback(songs, false);
        }
    }

    /// Restore the saved playlist and leave search closed until explicitly opened.
    fn open_with_music(&self) {
        if self.player().is_some_and(|p| p.queue().is_empty()) && utils::has_cached_playlist() {
            self.restore_playlist();
        }
        self.imp().library_split.set_show_sidebar(false);
        self.imp().library_sheet.set_open(false);
    }

    /// The first time the app starts, explains what matters when sharing
    /// music with everyone: names others can find, and sharing back.
    fn show_rules_once(&self) {
        let settings = utils::settings_manager();
        if settings.boolean("library-rules-seen") {
            return;
        }
        glib::idle_add_local_once(clone!(
            #[weak(rename_to = win)]
            self,
            move || {
                let alert = adw::AlertDialog::builder()
                    .heading(i18n("Sharing Music Together"))
                    .body(i18n(
                        "Everyone here shares the music they keep, so a few things matter.\n\n• Name songs properly. Others find music only by its title and artist: set them in the song’s tags, or name the file “Artist - Title”. Songs whose title and artist together have fewer than 5 letters aren’t shared.\n\n• Don’t be greedy. What you download and star is shared from your computer in turn. Keep sharing on, also with the window closed (the app stays in the tray), and don’t limit sharing far below downloading.\n\n• Share only music, and only what you may share. Every file is checked, and anything that isn’t clean audio is refused.\n\n• Others see the address you share from. A VPN shows them its address instead: see Shared Music Settings.",
                    ))
                    .default_response("ok")
                    .close_response("ok")
                    .build();
                use adw::prelude::AlertDialogExt;
                alert.add_response("ok", &i18n("_Got It"));
                alert.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
                AlertDialogExt::connect_response(&alert, None, |_, _| {
                    if let Err(err) =
                        utils::settings_manager().set_boolean("library-rules-seen", true)
                    {
                        log::warn!("Unable to remember that the rules were shown: {err}");
                    }
                });
                adw::prelude::AdwDialogExt::present(&alert, Some(&win));
            }
        ));
    }

    /// Whether the shared music panel is in the bottom sheet, as on phones.
    fn library_in_sheet(&self) -> bool {
        self.imp().library_sheet.sheet().is_some()
    }

    /// Moves the shared music panel into the bottom sheet when the window is
    /// as narrow as a phone, and back to the right side when it widens.
    fn setup_library_sheet(&self) {
        let imp = self.imp();
        imp.narrow_breakpoint.connect_apply(clone!(
            #[weak(rename_to = win)]
            self,
            move |_| {
                let imp = win.imp();
                let panel = imp.library_panel.get();
                imp.library_split.set_show_sidebar(false);
                imp.library_split.set_sidebar(gtk::Widget::NONE);
                panel.add_css_class("in-sheet");
                imp.library_sheet.set_sheet(Some(&panel));
                imp.player_header.set_title_widget(gtk::Widget::NONE);
                imp.mode_switch_narrow.append(&imp.mode_switch.get());
                imp.player_header.set_show_title(false);
                imp.mode_switch.set_visible(true);
                win.enforce_import_audition();
            }
        ));
        imp.narrow_breakpoint.connect_unapply(clone!(
            #[weak(rename_to = win)]
            self,
            move |_| {
                let imp = win.imp();
                let panel = imp.library_panel.get();
                let was_open = imp.library_sheet.is_open();
                imp.library_sheet.set_open(false);
                imp.library_sheet.set_sheet(gtk::Widget::NONE);
                panel.remove_css_class("in-sheet");
                imp.library_split.set_sidebar(Some(&panel));
                imp.library_split.set_show_sidebar(was_open);
                imp.mode_switch_narrow.remove(&imp.mode_switch.get());
                imp.player_header.set_show_title(true);
                imp.player_header
                    .set_title_widget(Some(&imp.mode_switch.get()));
                win.enforce_import_audition();
            }
        ));
    }

    fn queue_songs(&self, queue: Vec<gio::File>) {
        self.queue_songs_with_feedback(queue, true);
    }

    fn queue_songs_with_feedback(&self, queue: Vec<gio::File>, feedback: bool) {
        if self.imp().playlist_loading.get() {
            let weak = self.downgrade();
            glib::spawn_future_local(async move {
                loop {
                    glib::timeout_future(std::time::Duration::from_millis(25)).await;
                    let Some(win) = weak.upgrade() else {
                        return;
                    };
                    if !win.imp().playlist_loading.get() {
                        win.queue_songs_with_feedback(queue, feedback);
                        break;
                    }
                }
            });
            return;
        }
        if queue.iter().any(|file| file.path().is_none()) {
            let window = self.clone();
            glib::spawn_future_local(async move {
                let mut local = Vec::with_capacity(queue.len());
                for file in queue {
                    match crate::document_file::local_audio(&file).await {
                        Ok(file) => local.push(file),
                        Err(_) => window.add_toast(i18n("Unable to read this audio document")),
                    }
                }
                window.queue_songs_with_feedback(local, feedback);
            });
            return;
        }
        if queue.is_empty() {
            if feedback {
                self.add_toast(i18n("No available song found"));
            }
            return;
        }

        self.switch_mode(WindowMode::MainView);

        // Keep additions ordered, but let playback and rendering continue while
        // file I/O runs on a worker. Publish the first song immediately, then
        // small batches instead of waiting for the entire playlist.
        self.action_set_enabled("queue.add-song", false);
        self.action_set_enabled("queue.add-folder", false);
        self.action_set_enabled("queue.clear", false);
        self.set_playlist_visible(true);
        self.imp()
            .pending_playlist_files
            .replace(queue.iter().cloned().collect());
        self.imp().playlist_loading.set(true);
        self.imp().playlist_view.begin_loading();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let now = Instant::now();
            let n_files = queue.len() as u32;
            let mut added = 0;
            let mut duplicates = 0;
            let mut batch = Vec::new();
            let mut last_added = None;
            for (index, file) in queue.into_iter().enumerate() {
                let loaded = Song::load_local(file.uri().to_string()).await;
                let Some(win) = weak.upgrade() else {
                    return;
                };
                let Some(player) = win.player() else {
                    return;
                };
                win.imp().pending_playlist_files.borrow_mut().pop_front();
                if let Ok(song) = loaded {
                    song.set_property("favorite", true);
                    let exists = if win.in_for_you() {
                        win.imp()
                            .my_music_songs
                            .borrow()
                            .iter()
                            .any(|s| s.equals(&song))
                    } else {
                        player.queue().contains(&song)
                    };
                    if exists || batch.iter().any(|s: &Song| s.equals(&song)) {
                        duplicates += 1;
                    } else {
                        batch.push(song);
                    }
                }
                win.imp()
                    .playlist_view
                    .update_loading(index as u32 + 1, n_files);
                if added == 0 || batch.len() >= 8 || index as u32 + 1 == n_files {
                    if !batch.is_empty() {
                        added += batch.len();
                        last_added = batch.last().cloned();
                        if win.in_for_you() {
                            win.imp().my_music_songs.borrow_mut().append(&mut batch);
                        } else {
                            let queue = player.queue();
                            let was_empty = queue.is_empty();
                            queue.add_songs(&batch);
                            batch.clear();
                            if was_empty && queue.current_song_index().is_none() {
                                player.skip_to(0);
                            }
                            if was_empty && feedback && n_files == 1 {
                                player.play();
                            }
                        }
                    }
                }
                win.imp().pending_playlist_songs.replace(batch.clone());
                // Give GTK an idle cycle even if the next read hits a hot cache.
                glib::timeout_future(std::time::Duration::from_millis(1)).await;
            }
            let Some(win) = weak.upgrade() else {
                return;
            };
            win.imp().playlist_loading.set(false);
            win.imp().playlist_view.end_loading();
            win.action_set_enabled("queue.add-song", true);
            win.action_set_enabled("queue.add-folder", true);
            win.action_set_enabled("queue.clear", true);
            debug!(
                "Total loading time for {} files: {} ms",
                n_files,
                now.elapsed().as_millis()
            );
            if added == 0 {
                if duplicates == 0 && feedback {
                    win.add_toast(i18n("No songs found"));
                }
                return;
            }
            if win.in_for_you() {
                win.store_personal_playlist();
            } else if let Some(player) = win.player() {
                win.store_personal_playlist();
                if feedback && added == 1 && player.queue().n_songs() > 1 {
                    if let Some(position) = (0..player.queue().n_songs())
                        .find(|&i| player.queue().song_at(i) == last_added)
                    {
                        win.add_skip_to_toast(i18n("Added a new song"), i18n("Play"), position);
                    }
                }
            }
            if feedback && added > 1 {
                win.add_toast(ni18n_f(
                    "Added one song",
                    "Added {} songs",
                    added as u32,
                    &[&added.to_string()],
                ));
            }
            win.sync_local_sharing().await;
        });
    }

    fn add_files_to_queue(&self, model: &gio::ListModel) {
        let mut queue: Vec<gio::File> = vec![];

        for pos in 0..model.n_items() {
            let file = model.item(pos).unwrap().downcast::<gio::File>().unwrap();

            if let Ok(info) = file.query_info(
                "standard::name,standard::display-name,standard::type,standard::content-type",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            ) {
                match info.file_type() {
                    gio::FileType::Regular => {
                        if let Some(content_type) = info.content_type() {
                            if gio::content_type_is_mime_type(&content_type, "audio/*") {
                                debug!("Adding file '{}' to the queue", file.uri());
                                queue.push(file);
                            }
                        }
                    }
                    gio::FileType::Directory => {
                        debug!("Adding folder '{}' to the queue", file.uri());
                        let files = utils::load_files_from_folder(&file, true);
                        queue.extend(files);
                    }
                    _ => (),
                }
            }
        }

        self.queue_songs(queue);
    }

    // Bind the PlayerState to the UI
    fn bind_state(&self) {
        let imp = self.imp();
        if let Some(player) = self.player() {
            let state = player.state();

            // Use the PlayerState:playing property to control the play/pause button
            self.update_play_button();
            let notify_playing_id = state.connect_notify_local(
                Some("playing"),
                clone!(
                    #[weak(rename_to = win)]
                    self,
                    move |_, _| {
                        win.set_playlist_selection(false);
                        win.update_play_button();
                    }
                ),
            );
            imp.notify_playing_id.replace(Some(notify_playing_id));

            // Update the position labels
            self.update_position_labels();
            let notify_position_id = state.connect_notify_local(
                Some("position"),
                clone!(
                    #[weak(rename_to = win)]
                    self,
                    move |_, _| {
                        win.update_position_labels();
                        win.update_lyrics_line();
                    }
                ),
            );
            imp.notify_position_id.replace(Some(notify_position_id));

            // Update the UI
            self.update_song();
            let notify_song_id = state.connect_notify_local(
                Some("song"),
                clone!(
                    #[weak(rename_to = win)]
                    self,
                    move |_, _| {
                        win.update_song();
                    }
                ),
            );
            imp.notify_song_id.replace(Some(notify_song_id));

            // Update the cover, if any is available
            self.update_cover();
            let notify_cover_id = state.connect_notify_local(
                Some("cover"),
                clone!(
                    #[weak(rename_to = win)]
                    self,
                    move |_, _| {
                        win.update_cover();
                    }
                ),
            );
            imp.notify_cover_id.replace(Some(notify_cover_id));
            let notify_favorite_id = state.connect_notify_local(
                Some("favorite"),
                clone!(
                    #[weak(rename_to = win)]
                    self,
                    move |_, _| {
                        win.update_favorite_button();
                    }
                ),
            );
            imp.notify_favorite_id.replace(Some(notify_favorite_id));

            // Bind the song properties to the UI
            state
                .bind_property("title", &imp.song_details.get().title_label(), "label")
                .sync_create()
                .build();
            state
                .bind_property("artist", &imp.song_details.get().artist_label(), "label")
                .sync_create()
                .build();
            state
                .bind_property("album", &imp.song_details.get().album_label(), "label")
                .sync_create()
                .build();
            state
                .bind_property(
                    "volume",
                    &imp.playback_control.get().volume_control(),
                    "volume",
                )
                .sync_create()
                .build();
        }
    }

    fn unbind_state(&self) {
        if let Some(player) = self.player() {
            let state = player.state();

            if let Some(id) = self.imp().notify_playing_id.take() {
                state.disconnect(id);
            }
            if let Some(id) = self.imp().notify_position_id.take() {
                state.disconnect(id);
            }
            if let Some(id) = self.imp().notify_song_id.take() {
                state.disconnect(id);
            }
            if let Some(id) = self.imp().notify_cover_id.take() {
                state.disconnect(id);
            }
            if let Some(id) = self.imp().notify_favorite_id.take() {
                state.disconnect(id);
            }
        }
    }

    // Bind the Queue to the UI
    fn bind_queue(&self) {
        if let Some(player) = self.player() {
            let queue = player.queue();

            let notify_nsongs_id = queue.connect_notify_local(
                Some("n-songs"),
                clone!(
                    #[weak(rename_to = win)]
                    self,
                    move |queue, _| {
                        debug!("queue.n_songs() = {}", queue.n_songs());
                        if queue.is_empty() {
                            if !win.player().is_some_and(|p| p.is_previewing()) {
                                win.set_initial_state();
                                win.reset_queue();
                            }
                        } else {
                            win.action_set_enabled("queue.toggle", true);
                            win.action_set_enabled("queue.shuffle", queue.n_songs() > 1);

                            win.action_set_enabled("win.play", true);
                            win.action_set_enabled("win.previous", true);
                            win.action_set_enabled("win.next", queue.n_songs() > 1);
                        }

                        if queue.n_songs() == 1 {
                            if let Some(p) = win.player() {
                                if !p.is_previewing() {
                                    p.skip_next();
                                }
                            }
                        }

                        win.update_playlist_time();
                    }
                ),
            );
            self.imp().notify_nsongs_id.replace(Some(notify_nsongs_id));

            queue.connect_notify_local(
                Some("repeat-mode"),
                clone!(
                    #[weak(rename_to = win)]
                    self,
                    move |queue, _| {
                        win.imp()
                            .playback_control
                            .set_repeat_mode(queue.repeat_mode());
                    }
                ),
            );

            let notify_current_id = queue.connect_notify_local(
                Some("current"),
                clone!(
                    #[weak(rename_to = win)]
                    self,
                    move |queue, _| {
                        if queue.is_last_song() {
                            match queue.repeat_mode() {
                                RepeatMode::Consecutive => {
                                    win.action_set_enabled("win.next", false);
                                }
                                _ => {
                                    win.action_set_enabled("win.next", true);
                                }
                            }
                        } else {
                            win.action_set_enabled("win.next", true);
                        }
                    }
                ),
            );
            self.imp()
                .notify_current_id
                .replace(Some(notify_current_id));
        }
    }

    fn unbind_queue(&self) {
        if let Some(player) = self.player() {
            let queue = player.queue();
            if let Some(id) = self.imp().notify_nsongs_id.take() {
                queue.disconnect(id);
            }
            if let Some(id) = self.imp().notify_current_id.take() {
                queue.disconnect(id);
            }
        }
    }

    fn connect_signals(&self) {
        self.imp().split_view.connect_notify_local(
            Some("collapsed"),
            clone!(
                #[weak(rename_to = win)]
                self,
                move |split_view, _| {
                    win.set_playlist_visible(split_view.shows_sidebar());
                    win.imp()
                        .playlist_view
                        .back_button()
                        .set_visible(split_view.is_collapsed());
                }
            ),
        );

        self.imp().split_view.connect_notify_local(
            Some("show-sidebar"),
            clone!(
                #[weak(rename_to = win)]
                self,
                move |split_view, _| {
                    win.set_playlist_visible(split_view.shows_sidebar());
                }
            ),
        );

        self.imp().waveform_view.connect_closure(
            "position-changed",
            false,
            closure_local!(
                #[watch(rename_to = win)]
                self,
                move |_wv: WaveformView, position: f64| {
                    debug!("New position: {}", position);
                    if let Some(player) = win.player() {
                        let state = player.state();
                        if state.current_song().is_some() {
                            player.seek_position_rel(position);
                            player.play();
                        }
                    }
                }
            ),
        );

        self.imp()
            .playback_control
            .volume_control()
            .connect_closure(
                "volume-changed",
                false,
                closure_local!(
                    #[watch(rename_to = win)]
                    self,
                    move |_vc: VolumeControl, volume: f64| {
                        debug!("Volume changed: {}", volume);
                        if let Some(p) = win.player() {
                            p.set_volume(volume);
                        }
                    }
                ),
            );

        self.imp()
            .playlist_view
            .queue_select_all_button()
            .connect_clicked(clone!(
                #[weak(rename_to = win)]
                self,
                move |_| {
                    if win.playlist_search() {
                        if let Some(ref model) = *win.imp().playlist_filtermodel.borrow() {
                            for idx in 0..model.n_items() {
                                let item = model.item(idx).unwrap();
                                let song = item.downcast_ref::<Song>().unwrap();
                                song.set_selected(true);
                            }
                        }
                    } else if let Some(player) = win.player() {
                        let queue = player.queue();
                        for idx in 0..queue.n_songs() {
                            let song = queue.song_at(idx).unwrap();
                            song.set_selected(true);
                        }
                    }
                }
            ));

        self.imp()
            .playlist_view
            .queue_remove_button()
            .connect_clicked(clone!(
                #[weak(rename_to = win)]
                self,
                move |_| {
                    if let Some(player) = win.player() {
                        let queue = player.queue();
                        let mut remove_songs: Vec<Song> = Vec::new();
                        // Collect all songs to be removed first, since we can't
                        // remove objects from the model while we're iterating it
                        for idx in 0..queue.n_songs() {
                            let song = queue.song_at(idx).unwrap();
                            if song.selected() {
                                remove_songs.push(song);
                            }
                        }

                        for song in remove_songs {
                            win.remove_song(&song);
                        }

                        // Store the current state of the playlist
                        if !win.in_for_you() {
                            win.store_personal_playlist();
                        }
                    }
                }
            ));

        self.imp()
            .playlist_view
            .playlist_searchbar()
            .connect_notify_local(
                Some("search-mode-enabled"),
                clone!(
                    #[weak(rename_to = win)]
                    self,
                    move |searchbar, _| {
                        win.set_playlist_search(searchbar.is_search_mode());
                    }
                ),
            );

        self.imp().settings.connect_changed(
            Some("enable-recoloring"),
            clone!(
                #[weak(rename_to = this)]
                self,
                move |settings, _| {
                    debug!(
                        "GSettings:enable-recoloring: {}",
                        settings.boolean("enable-recoloring")
                    );
                    if let Some(player) = this.player() {
                        let state = player.state();
                        this.update_style(state.current_song().as_ref());
                    }
                }
            ),
        );
        let _dummy = self.imp().settings.boolean("enable-recoloring");

        self.connect_close_request(move |window| {
            window.finish_import_audition(false);
            debug!("Saving window state");
            let width = window.default_size().0;
            let height = window.default_size().1;

            let settings = utils::settings_manager();
            settings
                .set_int("window-width", width)
                .expect("Unable to store window-width");
            settings
                .set_int("window-height", height)
                .expect("Unable to stop window-height");

            window.unbind_queue();
            window.unbind_state();
            window.unbind_waveform();

            glib::Propagation::Proceed
        });

        self.imp()
            .playlist_view
            .playlist_searchbar()
            .set_key_capture_widget(Some(self.upcast_ref::<gtk::Widget>()));
    }

    // The initial state of the playback actions
    fn set_initial_state(&self) {
        if let Some(player) = self.player() {
            let queue = player.queue();
            self.action_set_enabled("win.play", !queue.is_empty());
            self.action_set_enabled("win.previous", !queue.is_empty());
            self.action_set_enabled("win.next", !queue.is_last_song());

            self.action_set_enabled("queue.toggle", !queue.is_empty());
            self.action_set_enabled("queue.shuffle", queue.n_songs() > 1);
            self.action_set_enabled("win.replaygain", player.replaygain_available());

            let replaygain = self.imp().settings.enum_("replay-gain").into();
            self.set_replaygain(replaygain);

            // Manually set player state, because set_replaygain
            // only updates player state when the value changes.
            player.set_replaygain(replaygain);

            self.imp()
                .playback_control
                .set_repeat_mode(queue.repeat_mode());
            self.set_playlist_shuffled(queue.is_shuffled());

            // Manually update the icon on the initial empty state
            // to avoid generating the UI definition file at build
            // time
            self.imp().status_page.set_icon_name(Some(APPLICATION_ICON));

            if utils::has_cached_playlist() {
                self.imp().restore_playlist_button.set_visible(true);
                self.imp()
                    .restore_playlist_button
                    .add_css_class("suggested-action");
                self.imp()
                    .add_folder_button
                    .remove_css_class("suggested-action");
            } else {
                self.imp().restore_playlist_button.set_visible(false);
                self.imp()
                    .restore_playlist_button
                    .remove_css_class("suggested-action");
                self.imp()
                    .add_folder_button
                    .add_css_class("suggested-action");
            }

            // There is no start screen in this fork: the player is always
            // shown, with the shared music next to it
            self.switch_mode(WindowMode::MainView);
        }
    }

    fn setup_playlist(&self) {
        let imp = self.imp();

        self.setup_queue_reorder();
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(clone!(
            #[weak(rename_to = win)]
            self,
            move |_, item| {
                let row = QueueRow::default();
                let list_item = item.downcast_ref::<gtk::ListItem>().unwrap();
                list_item.set_child(Some(&row));

                row.connect_notify_local(
                    Some("selected"),
                    clone!(
                        #[weak]
                        win,
                        move |_, _| {
                            win.update_selected_count();
                        }
                    ),
                );

                win.bind_property("playlist-selection", &row, "selection-mode")
                    .sync_create()
                    .build();

                list_item
                    .bind_property("item", &row, "song")
                    .sync_create()
                    .build();

                list_item
                    .property_expression("item")
                    .chain_property::<Song>("artist")
                    .bind(&row, "song-artist", gtk::Widget::NONE);
                list_item
                    .property_expression("item")
                    .chain_property::<Song>("title")
                    .bind(&row, "song-title", gtk::Widget::NONE);
                list_item
                    .property_expression("item")
                    .chain_property::<Song>("cover")
                    .bind(&row, "song-cover", gtk::Widget::NONE);
                list_item
                    .property_expression("item")
                    .chain_property::<Song>("playing")
                    .bind(&row, "playing", gtk::Widget::NONE);
                list_item
                    .property_expression("item")
                    .chain_property::<Song>("selected")
                    .bind(&row, "selected", gtk::Widget::NONE);
                list_item
                    .property_expression("item")
                    .chain_property::<Song>("status")
                    .bind(&row, "song-status", gtk::Widget::NONE);
                list_item
                    .property_expression("item")
                    .chain_property::<Song>("progress")
                    .bind(&row, "song-progress", gtk::Widget::NONE);
            }
        ));
        imp.playlist_view
            .queue_view()
            .set_factory(Some(&factory.upcast::<gtk::ListItemFactory>()));

        if let Some(player) = self.player() {
            let queue = player.queue();

            let filter = FuzzyFilter::new();
            let filter_model =
                gtk::FilterListModel::new(Some(queue.model().clone()), Some(filter.clone()));
            let sorter = FuzzySorter::new();
            let sorter_model = gtk::SortListModel::new(Some(filter_model), Some(sorter.clone()));
            let selection = gtk::NoSelection::new(Some(sorter_model.clone()));
            imp.playlist_view
                .queue_view()
                .set_model(Some(selection.upcast_ref::<gtk::SelectionModel>()));
            imp.playlist_view.queue_view().connect_activate(clone!(
                #[weak(rename_to = win)]
                self,
                #[weak]
                selection,
                move |_, pos| {
                    let song = selection
                        .upcast::<gio::ListModel>()
                        .item(pos)
                        .unwrap()
                        .downcast::<Song>()
                        .unwrap();

                    if let Some(player) = win.player() {
                        let queue = player.queue();

                        let mut real_pos = None;
                        for i in 0..queue.model().n_items() {
                            if let Some(item) = queue.model().item(i) {
                                let s = item.downcast::<Song>().unwrap();
                                if s.equals(&song) {
                                    real_pos = Some(i);
                                    break;
                                }
                            }
                        }

                        if let Some(real_pos) = real_pos {
                            if win.playlist_selection() {
                                queue.select_song_at(real_pos);
                            } else if queue.current_song_index() != Some(real_pos)
                                || player.is_previewing()
                            {
                                player.skip_to(real_pos);
                                player.play();
                            } else if !player.state().playing() {
                                player.play();
                            }
                        }
                    }
                }
            ));

            imp.playlist_filtermodel
                .replace(Some(sorter_model.upcast::<gio::ListModel>()));

            imp.playlist_view
                .playlist_searchentry()
                .bind_property("text", &filter, "search")
                .sync_create()
                .build();
            imp.playlist_view
                .playlist_searchentry()
                .bind_property("text", &sorter, "search")
                .sync_create()
                .build();
            imp.playlist_view
                .playlist_searchentry()
                .connect_search_changed(clone!(
                    #[weak(rename_to = win)]
                    self,
                    move |_| {
                        if let Some(adjustment) = win.imp().playlist_view.queue_view().vadjustment()
                        {
                            adjustment.set_value(0.0);
                        }
                    }
                ));
        }
    }

    fn setup_drop_target(&self) {
        let drop_target = gtk::DropTarget::builder()
            .name("file-drop-target")
            .actions(gdk::DragAction::COPY)
            .formats(&gdk::ContentFormats::for_type(gdk::FileList::static_type()))
            .build();

        drop_target.connect_drop(clone!(
            #[weak(rename_to = win)]
            self,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                if let Ok(file_list) = value.get::<gdk::FileList>() {
                    if file_list.files().is_empty() {
                        win.add_toast(i18n("Unable to access dropped files"));
                        return false;
                    }

                    let model = gio::ListStore::new::<gio::File>();
                    for f in file_list.files() {
                        model.append(&f);
                    }
                    win.add_files_to_queue(model.upcast_ref::<gio::ListModel>());
                    return true;
                }

                false
            }
        ));

        self.imp().drag_overlay.set_drop_target(&drop_target);
    }

    fn update_play_button(&self) {
        if let Some(player) = self.player() {
            let state = player.state();
            let play_button = self.imp().playback_control.play_button();
            if state.playing() {
                play_button.set_icon_name("hopea-media-playback-pause-symbolic");
            } else {
                play_button.set_icon_name("hopea-media-playback-start-symbolic");
            }
        }
    }

    fn update_position_labels(&self) {
        if let Some(player) = self.player() {
            let state = player.state();
            if state.current_song().is_some() {
                let elapsed = state.position();
                let duration = state.duration();
                let remaining = duration.checked_sub(elapsed).unwrap_or_default();
                self.set_song_time(Some(elapsed), Some(remaining));

                let position = state.position() as f64 / state.duration() as f64;
                self.set_song_position(position);
            } else {
                self.set_song_time(None, None);
                self.set_song_position(0.0);
            }
        }
    }

    fn update_song(&self) {
        if let Some(player) = self.player() {
            let state = player.state();
            self.scroll_playlist_to_song();
            self.action_set_enabled("win.play", state.current_song().is_some());
            if let Some(has_next) = player.album_preview_next() {
                self.action_set_enabled("win.next", has_next);
                self.action_set_enabled("win.previous", true);
            }
            self.action_set_enabled("win.edit-cover", state.current_song().is_some());
            self.action_set_enabled("win.edit-lyrics", state.current_song().is_some());
            self.update_playlist_time();
            self.update_title(state.current_song().as_ref());
            self.update_style(state.current_song().as_ref());
            self.update_favorite_button();
            if let Some(song) = state.current_song().filter(|s| s.is_mine()) {
                glib::spawn_future_local(async move {
                    if let Some(text) = crate::lyrics::raw_on_this_computer(&song).await {
                        let _ = crate::lyrics::publish(&song, &text).await;
                    }
                });
            }
            self.prepare_next_song();
            if !self.is_import_audition() && utils::settings_manager().boolean("popular-metadata") {
                if let Some(song) = state.current_song().filter(|s| s.library_id().is_some()) {
                    glib::spawn_future_local(clone!(
                        #[weak(rename_to=win)]
                        self,
                        async move {
                            win.review_metadata(song, true).await;
                        }
                    ));
                }
            }
            if let (Some(song), Some(client)) = (state.current_song(), library::client()) {
                if let Some(id) = song.remote_id().filter(|_| song.cover_texture().is_none()) {
                    glib::spawn_future_local(async move {
                        if let Ok(bytes) = client.cover(id).await {
                            song.set_cover_bytes(&bytes);
                        }
                    });
                }
            }
            if self.lyrics_shown() {
                glib::spawn_future_local(clone!(
                    #[weak(rename_to = win)]
                    self,
                    async move { win.load_lyrics().await }
                ));
            }
        }
    }

    /// Shows whether the song playing is in the library. The star is always
    /// there, and only unavailable while nothing is playing.
    fn update_favorite_button(&self) {
        let song = self.player().and_then(|p| p.state().current_song());
        let state = song.map(|s| s.property::<bool>("favorite"));
        self.imp().playback_control.set_favorite_state(state);
    }

    /// Puts the song playing in the library, or takes it out.
    async fn toggle_current_favorite(&self) {
        if self.is_import_audition() {
            return;
        }
        let Some(song) = self.player().and_then(|p| p.state().current_song()) else {
            return;
        };
        match song.library_id() {
            Some(id) if song.property::<bool>("favorite") => {
                self.take_out_of_library(id, &song.title(), song.is_mine())
            }
            _ => self.add_song_to_library(&song).await,
        }
    }

    /// Puts a playlist song in the library, where everything is kept on this
    /// computer and shared: a file from here is shared, a song from the
    /// network is downloaded and shared on.
    pub async fn add_song_to_library(&self, song: &Song) {
        let Some(client) = library::client() else {
            self.add_toast(i18n(
                "Join the network in Shared Music to keep and share songs",
            ));
            self.show_library();
            return;
        };
        if let Some(id) = song.library_id() {
            match client.set_favorite(id, true).await {
                Ok(track) => {
                    self.set_library_state(id, track.favorite);
                    self.add_remote_track(&track, false).await;
                    self.offer_metadata(&track);
                    self.add_toast(i18n_k(
                        // Translators: `{title}` is replaced by the title of a song
                        "Added “{title}” to your library",
                        &[("title", &song.title())],
                    ));
                }
                Err(err) => self.add_toast(err.to_string()),
            }
            return;
        }
        // A file on this computer: sharing it puts it in the library
        let file = song.file();
        if file.path().is_none() {
            return;
        }
        match self.imp().library_panel.share_file(&file).await {
            Ok(track) => {
                self.add_shared_file(&file, track.id);
                self.add_toast(i18n_k(
                    // Translators: `{title}` is replaced by the title of a song
                    "Added “{title}” to your library and sharing it",
                    &[("title", &song.title())],
                ));
            }
            Err(reason) => self.add_toast(reason),
        }
    }

    /// Adds an imported file to My Music after the node has accepted it.
    /// Importing while recommendations play must leave that queue alone.
    pub fn add_shared_file(&self, file: &gio::File, id: i64) {
        let file = file.clone();
        let win = self.clone();
        glib::spawn_future_local(async move {
            let Some(client) = library::client() else {
                return;
            };
            match client.set_favorite(id, true).await {
                Ok(_) => win.add_starred_file(&file, id),
                Err(error) => win.add_toast(error.to_string()),
            }
        });
    }

    fn add_starred_file(&self, file: &gio::File, id: i64) {
        self.mark_shared_file(&file.uri(), id);
        self.set_library_state(id, true);
        let Ok(song) = Song::from_uri(&file.uri()) else {
            return;
        };
        song.set_library(Some(id), true, true);
        if self.in_for_you() {
            let mut songs = self.imp().my_music_songs.borrow_mut();
            if let Some(existing) = songs.iter().find(|s| s.uri() == file.uri()) {
                existing.set_library(Some(id), true, true);
            } else {
                songs.push(song);
            }
        } else if let Some(player) = self.player() {
            let queue = player.queue();
            let queued = (0..queue.n_songs())
                .filter_map(|i| queue.song_at(i))
                .any(|s| s.uri() == file.uri());
            if !queued {
                self.switch_mode(WindowMode::MainView);
                queue.add_song(&song);
                self.store_personal_playlist();
            }
        }
    }

    /// Register music already added to Hopea. No scan of unrelated user folders.
    pub(crate) async fn sync_local_sharing(&self) {
        let Some(client) = library::client() else {
            return;
        };
        let mut songs = self.imp().my_music_songs.borrow().clone();
        if let Some(player) = self.player().filter(|_| !self.in_for_you()) {
            songs.extend((0..player.queue().n_songs()).filter_map(|i| player.queue().song_at(i)));
        }
        for song in songs {
            if song.file().path().is_none() {
                continue;
            }
            let track = if let Some(id) = song.library_id() {
                client.track(id).await.ok()
            } else {
                self.imp().library_panel.share_file(&song.file()).await.ok()
            };
            if let Some(track) = track {
                library::shared_files::remember(&song.uri(), track.id);
                let favorite = song.property::<bool>("favorite");
                if favorite && !track.favorite {
                    if let Err(err) = client.set_favorite(track.id, true).await {
                        self.add_toast(err.to_string());
                        continue;
                    }
                }
                song.set_library(Some(track.id), track.mine, favorite);
            }
        }
        // Albums can contain unstarred local files. Register them too, without
        // adding their songs to the personal playlist.
        if let Ok(albums) = library::collections::list().await {
            for mut album in albums {
                let mut changed = false;
                for entry in &mut album.tracks {
                    if entry.node_id.is_some() {
                        continue;
                    }
                    let Some(uri) = &entry.local_uri else {
                        continue;
                    };
                    let file = gio::File::for_uri(uri);
                    if let Ok(track) = self.imp().library_panel.share_file(&file).await {
                        library::shared_files::remember(uri, track.id);
                        entry.node_id = Some(track.id);
                        entry.sha256 = Some(track.sha256);
                        changed = true;
                    }
                }
                if changed {
                    if let Err(err) = library::collections::save(album).await {
                        self.add_toast(err);
                    }
                }
            }
        }
        self.update_favorite_button();
    }

    pub fn refresh_import_cover(&self, id: i64, bytes: &glib::Bytes) {
        library::thumbnails::invalidate(id);
        let mut songs = self.imp().my_music_songs.borrow().clone();
        songs.extend(self.imp().for_you_songs.borrow().iter().cloned());
        if let Some(player) = self.player() {
            songs.extend((0..player.queue().n_songs()).filter_map(|i| player.queue().song_at(i)));
            for song in songs.iter().filter(|s| s.library_id() == Some(id)) {
                song.set_cover_bytes(bytes);
            }
            if player
                .state()
                .current_song()
                .is_some_and(|s| s.library_id() == Some(id))
            {
                player.state().notify("cover");
            }
        }
    }

    /// Marks the playlist songs of a file shared from here as in the library.
    pub fn mark_shared_file(&self, uri: &str, id: i64) {
        library::shared_files::remember(uri, id);
        if let Some(player) = self.player() {
            if let Some(song) = player
                .state()
                .current_song()
                .filter(|song| song.uri() == uri)
            {
                song.set_library(Some(id), true, song.property::<bool>("favorite"));
            }
            let queue = player.queue();
            for song in (0..queue.n_songs()).filter_map(|i| queue.song_at(i)) {
                if song.uri() == uri {
                    song.set_library(Some(id), true, song.property::<bool>("favorite"));
                }
            }
        }
        self.update_favorite_button();
    }

    /// A star controls the personal playlist; local audio keeps being shared.
    pub fn take_out_of_library(&self, id: i64, title: &str, _mine: bool) {
        let win = self.clone();
        let title = title.to_string();
        glib::spawn_future_local(async move {
            let Some(client) = library::client() else {
                return;
            };
            match client.set_favorite(id, false).await {
                Ok(_) => {
                    win.set_library_state(id, false);
                    win.remove_library_songs(id);
                    win.store_personal_playlist();
                    let toast = adw::Toast::builder()
                        .title(i18n_k(
                            "Removed “{title}” from favorites",
                            &[("title", &title)],
                        ))
                        .use_markup(false)
                        .button_label(i18n("_Undo"))
                        .timeout(5)
                        .build();
                    toast.connect_button_clicked(clone!(
                        #[weak]
                        win,
                        move |_| {
                            let win = win.clone();
                            glib::spawn_future_local(async move {
                                if let Some(client) = library::client() {
                                    match client.set_favorite(id, true).await {
                                        Ok(track) => {
                                            win.set_library_state(id, true);
                                            win.add_remote_track(&track, false).await;
                                        }
                                        Err(error) => win.add_toast(error.to_string()),
                                    }
                                }
                            });
                        }
                    ));
                    win.imp().toast_overlay.add_toast(toast);
                }
                Err(error) => win.add_toast(error.to_string()),
            }
        });
    }

    pub fn remove_replaced_library_track(&self, id: i64) {
        self.set_library_state(id, false);
        self.remove_library_songs(id);
        self.imp()
            .my_music_songs
            .borrow_mut()
            .retain(|s| s.library_id() != Some(id));
        self.imp()
            .for_you_songs
            .borrow_mut()
            .retain(|s| s.library_id() != Some(id));
        self.store_personal_playlist();
    }

    /// Takes every song of a library track out of the playlist, but the one
    /// playing, which plays on.
    fn remove_library_songs(&self, id: i64) {
        let Some(player) = self.player() else {
            return;
        };
        let queue = player.queue();
        let songs: Vec<Song> = (0..queue.n_songs())
            .filter_map(|i| queue.song_at(i))
            .filter(|s| s.library_id() == Some(id))
            .collect();
        for song in songs {
            if !player.is_previewing()
                && player
                    .state()
                    .current_song()
                    .is_some_and(|s| s.equals(&song))
            {
                player.detach_current();
            }
            queue.remove_song(&song);
        }
        self.imp()
            .my_music_songs
            .borrow_mut()
            .retain(|s| s.library_id() != Some(id));
    }

    /// Shows a track's new place in or out of the library everywhere: its
    /// playlist songs, the star of the song playing, and the panel's stars.
    pub fn set_library_state(&self, id: i64, favorite: bool) {
        if let Some(song) = self
            .player()
            .and_then(|p| p.state().current_song())
            .filter(|s| s.library_id() == Some(id))
        {
            song.set_property("favorite", favorite);
        }
        if let Some(player) = self.player() {
            player.update_preview_favorite(id, favorite);
            let queue = player.queue();
            for song in (0..queue.n_songs()).filter_map(|i| queue.song_at(i)) {
                if song.library_id() == Some(id) {
                    song.set_property("favorite", favorite);
                }
            }
        }
        for song in self
            .imp()
            .my_music_songs
            .borrow()
            .iter()
            .chain(self.imp().for_you_songs.borrow().iter())
        {
            if song.library_id() == Some(id) {
                song.set_property("favorite", favorite);
            }
        }
        self.update_favorite_button();
        self.imp().library_panel.set_star_state(id, favorite);
    }

    /// Puts the library in the playlist, so it is there every time the app
    /// starts: files shared from here as they are, everything else from the
    /// network.
    pub async fn show_favorites_in_playlist(&self, tracks: Vec<Track>) {
        let Some(player) = self.player() else {
            return;
        };
        for track in &tracks {
            if !track.favorite {
                continue;
            }
            let original = library::shared_files::uri_for(track.id)
                .map(|uri| gio::File::for_uri(&uri))
                .filter(|f| f.query_exists(gio::Cancellable::NONE));
            let Some(file) = original else {
                self.add_remote_track(track, false).await;
                continue;
            };
            let queue = player.queue();
            let queued = (0..queue.n_songs())
                .filter_map(|i| queue.song_at(i))
                .any(|s| s.uri() == file.uri());
            if !queued {
                if let Ok(song) = Song::from_uri(&file.uri()) {
                    song.set_library(Some(track.id), track.mine, track.favorite);
                    self.switch_mode(WindowMode::MainView);
                    queue.add_song(&song);
                }
            }
        }
    }

    /// Follows the app joining or leaving the network by itself, as when a
    /// VPN goes off or comes back.
    pub fn library_connection_changed(&self, message: String) {
        self.imp().library_panel.connection_changed();
        self.add_toast(message);
    }

    /// The "My Music" and "For You" switch above the player. For You plays
    /// recommendations, in a queue of its own, so that the listener's own
    /// playlist is left as it was.
    fn setup_for_you(&self) {
        let imp = self.imp();
        glib::timeout_add_seconds_local(
            60,
            clone!(
                #[weak(rename_to=win)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    win.refresh_current_metadata();
                    glib::ControlFlow::Continue
                }
            ),
        );
        imp.settings.connect_changed(
            Some("popular-metadata"),
            clone!(
                #[weak(rename_to=win)]
                self,
                move |_, _| {
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(300),
                        move || win.refresh_current_metadata(),
                    );
                }
            ),
        );
        imp.for_you_button.connect_toggled(clone!(
            #[weak(rename_to = win)]
            self,
            move |button| win.set_for_you(button.is_active())
        ));
        imp.for_you_play_button.connect_clicked(clone!(
            #[weak(rename_to = win)]
            self,
            move |_| {
                if win.imp().for_you_button.is_active() {
                    glib::spawn_future_local(async move { win.start_for_you().await });
                } else {
                    win.switch_playback_queue(false);
                    if let Some(player) = win.player() {
                        player.play();
                    }
                    win.set_for_you(false);
                }
            }
        ));
        // Songs from other services not found yet, looked for again
        glib::timeout_add_seconds_local(
            60 * 60,
            clone!(
                #[weak(rename_to = win)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    win.look_for_wanted();
                    glib::ControlFlow::Continue
                }
            ),
        );
        // How songs from the network are coming along, for the playlist
        glib::timeout_add_seconds_local(
            15,
            clone!(
                #[weak(rename_to = win)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    glib::spawn_future_local(async move {
                        win.refresh_recommendation_availability().await;
                    });
                    glib::ControlFlow::Continue
                }
            ),
        );
        glib::timeout_add_seconds_local(
            2,
            clone!(
                #[weak(rename_to = win)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    win.refresh_transfers();
                    glib::ControlFlow::Continue
                }
            ),
        );
    }

    /// Whether the player plays recommendations rather than the listener's
    /// own playlist.
    fn in_for_you(&self) -> bool {
        self.imp().in_for_you.get()
    }

    /// Browsing a tab must not alter the queue or playback position.
    fn set_for_you(&self, for_you: bool) {
        let imp = self.imp();
        if !for_you {
            imp.playlist_view.show_playlist();
        }
        let showing_playing_queue =
            imp.in_for_you.get() == for_you && self.player().is_some_and(|p| !p.queue().is_empty());
        if for_you {
            imp.queue_mode_page.set_title(&i18n("Music for You"));
            imp.queue_mode_page.set_description(Some(&i18n("Songs picked from the genres and artists you star and listen to. Your current music keeps playing until you press Play.")));
            imp.for_you_play_button
                .set_tooltip_text(Some(&i18n("Play Music for You")));
        } else {
            imp.queue_mode_page.set_title(&i18n("Your Music"));
            imp.queue_mode_page.set_description(Some(&i18n("Press Play to return to your playlist. Your current music keeps playing while you browse.")));
            imp.for_you_play_button
                .set_tooltip_text(Some(&i18n("Play My Music")));
        }
        imp.player_stack.set_visible_child_name(
            if showing_playing_queue || !for_you && !self.in_for_you() {
                "player"
            } else {
                "for-you"
            },
        );
    }

    /// Replaces the playing queue only after an explicit Play request.
    fn switch_playback_queue(&self, for_you: bool) {
        let imp = self.imp();
        if imp.in_for_you.get() == for_you {
            return;
        }
        let Some(player) = self.player() else {
            return;
        };
        let queue = player.queue();
        let leaving: Vec<Song> = (0..queue.n_songs())
            .filter_map(|i| queue.song_at(i))
            .collect();
        player.stop();
        if for_you {
            imp.my_music_songs.replace(leaving);
        } else {
            imp.for_you_songs.replace(leaving);
        }
        imp.in_for_you.set(for_you);
        queue.clear();
        let coming = if for_you {
            imp.for_you_songs.take()
        } else {
            imp.my_music_songs.take()
        };
        queue.add_songs(&coming);
        if !coming.is_empty() {
            player.skip_to(0);
        }
    }

    /// Fills the For You queue with recommendations and starts playing.
    async fn start_for_you(&self) {
        let imp = self.imp();
        let Some(client) = library::client() else {
            self.add_toast(i18n(
                "Join the network in Shared Music to get music picked for you",
            ));
            self.show_library();
            return;
        };
        while imp.for_you_checking.get() {
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        self.refresh_recommendation_availability().await;
        if !imp.for_you_button.is_active() {
            return;
        }
        loop {
            if !imp.for_you_button.is_active() {
                return;
            }
            let first = imp.for_you_songs.borrow().first().cloned();
            let Some(id) = first.and_then(|s| s.remote_id()) else {
                break;
            };
            match client.prefetch(id).await {
                Ok(()) => break,
                Err(library::ClientError::Unavailable | library::ClientError::NotFound) => {
                    imp.for_you_songs
                        .borrow_mut()
                        .retain(|s| s.remote_id() != Some(id));
                }
                Err(error) => {
                    self.add_toast(error.to_string());
                    return;
                }
            }
        }
        if !imp.for_you_button.is_active() {
            return;
        }
        if !imp.for_you_songs.borrow().is_empty() {
            self.switch_playback_queue(true);
            if let Some(player) = self.player() {
                player.play();
            }
            self.set_for_you(true);
            return;
        }
        imp.for_you_play_button.set_sensitive(false);
        let tracks = self.more_for_you(&client).await;
        imp.for_you_play_button.set_sensitive(true);
        if !imp.for_you_button.is_active() {
            return;
        }
        if tracks.is_empty() {
            self.add_toast(i18n(
                "Nothing to recommend yet: star songs you like or listen to some, and music like them shows up here when someone who has it is online",
            ));
            return;
        }
        self.switch_playback_queue(true);
        imp.player_stack.set_visible_child_name("player");
        for track in tracks {
            if !self.in_for_you() || !imp.for_you_button.is_active() {
                break;
            }
            let play = self.player().is_some_and(|p| p.queue().is_empty());
            self.add_remote_track(&track, play).await;
        }
        self.set_for_you(imp.for_you_button.is_active());
    }

    /// Recommendations not in the queue yet, searching the network for new
    /// ones when the known ones run out.
    async fn refresh_recommendation_availability(&self) {
        let (Some(player), Some(client)) = (self.player(), library::client()) else {
            return;
        };
        let imp = self.imp();
        if player.is_previewing() || imp.for_you_checking.replace(true) {
            return;
        }
        let mut songs = imp.for_you_songs.borrow().clone();
        if self.in_for_you() {
            let queue = player.queue();
            songs.extend((0..queue.n_songs()).filter_map(|i| queue.song_at(i)));
        }
        let ids: Vec<_> = songs.iter().filter_map(|s| s.remote_id()).collect();
        if !ids.is_empty() {
            if let Ok(available) = client.available_recommendations(&ids).await {
                let unavailable: std::collections::HashSet<_> = ids
                    .into_iter()
                    .filter(|id| !available.contains(id))
                    .collect();
                imp.for_you_songs
                    .borrow_mut()
                    .retain(|s| !s.remote_id().is_some_and(|id| unavailable.contains(&id)));
                imp.prefetched
                    .borrow_mut()
                    .retain(|id| !unavailable.contains(id));
                if self.in_for_you() && !player.is_previewing() {
                    let queue = player.queue();
                    let mut removed: Vec<_> = (0..queue.n_songs())
                        .filter_map(|i| queue.song_at(i))
                        .filter(|s| s.remote_id().is_some_and(|id| unavailable.contains(&id)))
                        .collect();
                    removed.sort_by_key(|s| s.playing());
                    for song in removed {
                        player.remove_song(&song);
                    }
                    if queue.is_empty() {
                        player.stop();
                        self.set_for_you(true);
                    }
                }
            }
        }
        imp.for_you_checking.set(false);
    }

    async fn more_for_you(&self, client: &library::LibraryClient) -> Vec<Track> {
        let queued: std::collections::HashSet<i64> = self
            .player()
            .map(|p| {
                let queue = p.queue();
                (0..queue.n_songs())
                    .filter_map(|i| queue.song_at(i))
                    .filter_map(|s| s.library_id())
                    .collect()
            })
            .unwrap_or_default();
        for refresh in [false, true] {
            let fresh: Vec<Track> = match client.recommendations(refresh).await {
                Ok(page) => page
                    .tracks
                    .into_iter()
                    .filter(|t| !queued.contains(&t.id) && (t.local || t.sources > 0))
                    .take(FOR_YOU_BATCH)
                    .collect(),
                Err(err) => {
                    debug!("No recommendations: {err}");
                    Vec::new()
                }
            };
            if !fresh.is_empty() {
                return fresh;
            }
        }
        Vec::new()
    }

    /// Gets the next song ready while this one plays: a song from the
    /// network starts downloading, and For You fetches more songs before
    /// it runs out.
    fn prepare_next_song(&self) {
        let (Some(player), Some(client)) = (self.player(), library::client()) else {
            return;
        };
        if player.is_previewing() {
            return;
        }
        let queue = player.queue();
        let Some(current) = queue.current_song_index() else {
            return;
        };
        let next = if current + 1 < queue.n_songs() {
            queue.song_at(current + 1)
        } else if queue.repeat_mode() == RepeatMode::RepeatAll {
            queue.song_at(0)
        } else {
            None
        };
        if let Some(id) = next
            .filter(|s| s.remote_id().is_some() && s.progress() < 1.0)
            .and_then(|s| s.library_id())
        {
            if self.imp().prefetched.borrow_mut().insert(id) {
                let client = client.clone();
                let win = self.downgrade();
                glib::spawn_future_local(async move {
                    if let Err(err) = client.prefetch(id).await {
                        debug!("Unable to fetch track {id} ahead: {err}");
                        if let Some(win) = win.upgrade() {
                            win.imp().prefetched.borrow_mut().remove(&id);
                            if matches!(
                                err,
                                library::ClientError::Unavailable | library::ClientError::NotFound
                            ) && win.in_for_you()
                            {
                                if let Some(player) = win.player().filter(|p| !p.is_previewing()) {
                                    let queue = player.queue();
                                    let missing = (0..queue.n_songs())
                                        .filter_map(|i| queue.song_at(i))
                                        .find(|s| s.remote_id() == Some(id));
                                    if let Some(song) = missing {
                                        player.remove_song(&song);
                                    }
                                    if queue.is_empty() {
                                        player.stop();
                                        win.set_for_you(true);
                                    } else {
                                        win.prepare_next_song();
                                    }
                                }
                            }
                        }
                    }
                });
            }
        }

        let imp = self.imp();
        if self.in_for_you() && current + 2 >= queue.n_songs() && !imp.for_you_refilling.get() {
            imp.for_you_refilling.set(true);
            glib::spawn_future_local(clone!(
                #[weak(rename_to = win)]
                self,
                async move {
                    for track in win.more_for_you(&client).await {
                        if !win.in_for_you() {
                            break;
                        }
                        win.add_remote_track(&track, false).await;
                    }
                    win.imp().for_you_refilling.set(false);
                }
            ));
        }
    }

    /// Asks the node how the songs from the network in the playlist are
    /// coming along, for the playlist to show. Songs that are all here are
    /// only asked about while they play, to show their sharing.
    fn refresh_transfers(&self) {
        let (Some(player), Some(client)) = (self.player(), library::client()) else {
            return;
        };
        let queue = player.queue();
        let current = player.state().current_song();
        let songs: Vec<Song> = (0..queue.n_songs())
            .filter_map(|i| queue.song_at(i))
            .filter(|s| s.remote_id().is_some())
            .filter(|s| s.progress() < 1.0 || current.as_ref().is_some_and(|c| c.equals(s)))
            .take(MAX_TRANSFERS_ASKED)
            .collect();
        for song in songs {
            let Some(id) = song.remote_id() else {
                continue;
            };
            let client = client.clone();
            glib::spawn_future_local(async move {
                if let Ok(transfer) = client.transfer(id).await {
                    song.set_property("progress", f64::from(transfer.progress).clamp(0.0, 1.0));
                    song.set_property("status", transfer_text(&transfer));
                }
            });
        }
    }

    /// The lyrics view, in place of the cover art (see [`crate::lyrics`]).
    fn setup_lyrics(&self) {
        let action = gio::SimpleAction::new_stateful("lyrics", None, &false.to_variant());
        action.connect_activate(clone!(
            #[weak(rename_to = win)]
            self,
            move |action, _| {
                let show = !action
                    .state()
                    .and_then(|s| s.get::<bool>())
                    .unwrap_or(false);
                action.set_state(&show.to_variant());
                win.show_lyrics(show);
            }
        ));
        self.add_action(&action);
        self.imp().lyrics_online_button.connect_clicked(clone!(
            #[weak(rename_to = win)]
            self,
            move |_| {
                glib::spawn_future_local(async move { win.find_lyrics_online().await });
            }
        ));
    }

    fn lyrics_shown(&self) -> bool {
        self.imp().cover_stack.visible_child_name().as_deref() == Some("lyrics")
    }

    fn show_lyrics(&self, show: bool) {
        self.imp()
            .cover_stack
            .set_visible_child_name(if show { "lyrics" } else { "cover" });
        if show {
            glib::spawn_future_local(clone!(
                #[weak(rename_to = win)]
                self,
                async move { win.load_lyrics().await }
            ));
        }
    }

    fn show_no_lyrics(&self, title: &str, can_look_online: bool) {
        let imp = self.imp();
        imp.lyrics.replace(None);
        imp.lyrics_none_label.set_label(title);
        imp.lyrics_online_box.set_visible(can_look_online);
        imp.lyrics_stack.set_visible_child_name("none");
    }

    /// Shows the lyrics of the song playing that are on this computer.
    async fn load_lyrics(&self) {
        let imp = self.imp();
        let generation = imp.lyrics_generation.get() + 1;
        imp.lyrics_generation.set(generation);
        let Some(song) = self.player().and_then(|p| p.state().current_song()) else {
            self.show_no_lyrics(&i18n("Nothing Playing"), false);
            return;
        };
        let audition_text = imp
            .audition
            .borrow()
            .as_ref()
            .and_then(|a| a.lyrics.clone());
        if let Some(text) = audition_text {
            if let Some(lyrics) = crate::lyrics::parse(&text) {
                self.set_lyrics(lyrics);
            }
            return;
        }
        imp.lyrics_stack.set_visible_child_name("loading");
        let found = crate::lyrics::on_this_computer(&song).await;
        if imp.lyrics_generation.get() != generation {
            return;
        }
        match found {
            Some(lyrics) => self.set_lyrics(lyrics),
            None => self.show_no_lyrics(&i18n("No Lyrics for This Song"), true),
        }
    }

    /// Looks the lyrics of the song playing up online, as the listener asked.
    async fn find_lyrics_online(&self) {
        let imp = self.imp();
        let generation = imp.lyrics_generation.get() + 1;
        imp.lyrics_generation.set(generation);
        let Some(song) = self.player().and_then(|p| p.state().current_song()) else {
            return;
        };
        imp.lyrics_stack.set_visible_child_name("loading");
        let found = crate::lyrics::look_up_online(&song).await;
        if imp.lyrics_generation.get() != generation {
            return;
        }
        match found {
            Ok(Some(lyrics)) => self.set_lyrics(lyrics),
            Ok(None) => self.show_no_lyrics(&i18n("No Lyrics Found Online Either"), false),
            Err(err) => {
                self.add_toast(err);
                self.show_no_lyrics(&i18n("No Lyrics for This Song"), true);
            }
        }
    }

    fn set_lyrics(&self, lyrics: crate::lyrics::Lyrics) {
        let imp = self.imp();
        if let Some(animation) = imp.lyrics_scroll_animation.take() {
            animation.pause();
        }
        while let Some(child) = imp.lyrics_box.first_child() {
            imp.lyrics_box.remove(&child);
        }
        let labels: Vec<gtk::Label> = lyrics
            .lines
            .iter()
            .map(|line| {
                // Text from others: never markup
                let label = gtk::Label::builder()
                    .label(line.text.as_str())
                    .use_markup(false)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .justify(gtk::Justification::Center)
                    .build();
                imp.lyrics_box.append(&label);
                label
            })
            .collect();
        let synced = lyrics.synced();
        imp.lyrics_box.set_css_classes(if synced {
            &["lyrics", "synced"]
        } else {
            &["lyrics"]
        });
        imp.lyrics_labels.replace(labels);
        imp.lyrics_line.set(None);
        imp.lyrics.replace(Some(lyrics));
        imp.lyrics_scroller.vadjustment().set_value(0.0);
        imp.lyrics_stack.set_visible_child_name("lines");
        self.update_lyrics_line();
    }

    /// Lights up the line being sung, for synced lyrics, and keeps it in
    /// the middle of the view.
    fn update_lyrics_line(&self) {
        let imp = self.imp();
        if !self.lyrics_shown() {
            return;
        }
        let Some(position) = self.player().map(|p| p.state().position_ms()) else {
            return;
        };
        let line = imp
            .lyrics
            .borrow()
            .as_ref()
            .and_then(|l| l.line_at(position));
        if line == imp.lyrics_line.get() {
            return;
        }
        let labels = imp.lyrics_labels.borrow();
        if let Some(old) = imp.lyrics_line.get().and_then(|i| labels.get(i)) {
            old.remove_css_class("current");
        }
        imp.lyrics_line.set(line);
        let Some(label) = line.and_then(|i| labels.get(i)) else {
            return;
        };
        label.add_css_class("current");
        let adjustment = imp.lyrics_scroller.vadjustment();
        if let Some(point) = label.compute_point(
            &imp.lyrics_box.get(),
            &gtk::graphene::Point::new(0.0, label.height() as f32 / 2.0),
        ) {
            let target = f64::from(point.y()) - adjustment.page_size() / 2.0;
            self.scroll_lyrics_to(target);
        }
    }

    fn scroll_lyrics_to(&self, value: f64) {
        let imp = self.imp();
        if let Some(animation) = imp.lyrics_scroll_animation.take() {
            animation.pause();
        }
        let adjustment = imp.lyrics_scroller.vadjustment();
        let value = value.clamp(
            adjustment.lower(),
            (adjustment.upper() - adjustment.page_size()).max(adjustment.lower()),
        );
        let target = adw::CallbackAnimationTarget::new(clone!(
            #[weak]
            adjustment,
            move |value| adjustment.set_value(value)
        ));
        let animation = adw::TimedAnimation::builder()
            .widget(&imp.lyrics_scroller.get())
            .value_from(adjustment.value())
            .value_to(value)
            .duration(320)
            .easing(adw::Easing::EaseOutCubic)
            .target(&target)
            .build();
        animation.play();
        imp.lyrics_scroll_animation.replace(Some(animation));
    }

    /// Looks for the songs brought over from other services, a few seconds
    /// apart, and stars the ones someone shares. Runs once at a time; the
    /// ones not found are looked for again later.
    pub fn look_for_wanted(&self) {
        let imp = self.imp();
        if imp.looking_for_wanted.replace(true) {
            return;
        }
        glib::spawn_future_local(clone!(
            #[weak(rename_to = win)]
            self,
            async move {
                let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
                let due = library::import::due(now);
                let mut results = Vec::new();
                let mut found = 0;
                for song in due {
                    let Some(client) = library::client() else {
                        break;
                    };
                    let track = library::import::look_for(&client, &song).await;
                    if let Some(track) = &track {
                        if let Ok(starred) = client.set_favorite(track.id, true).await {
                            found += 1;
                            win.set_library_state(starred.id, starred.favorite);
                            win.add_remote_track(&starred, false).await;
                        }
                    }
                    results.push((song, track.is_some()));
                    if results.len() >= 10 {
                        library::import::record(&std::mem::take(&mut results), now);
                    }
                    glib::timeout_future(WANTED_SEARCH_GAP).await;
                }
                library::import::record(&results, now);
                win.imp().looking_for_wanted.set(false);
                if found > 0 {
                    win.add_toast(ni18n_f(
                        "Found {} of your songs on the network and starred it",
                        "Found {} of your songs on the network and starred them",
                        found,
                        &[&found.to_string()],
                    ));
                }
            }
        ));
    }

    /// Shows the shared music panel, wherever it is.
    fn show_library(&self) {
        let imp = self.imp();
        if self.library_in_sheet() {
            imp.library_sheet.set_open(true);
        } else {
            imp.library_split.set_show_sidebar(true);
        }
    }

    fn update_cover(&self) {
        if let Some(player) = self.player() {
            let state = player.state();
            let song_cover = self.imp().song_cover.get();
            if let Some(cover) = state.cover() {
                song_cover.album_image().set_cover(Some(&cover));
                song_cover.show_cover_image(true);
            } else {
                song_cover.album_image().set_cover(None);
                song_cover.show_cover_image(false);
            }
        }
    }

    fn update_playlist_time(&self) {
        if let Some(player) = self.player() {
            let queue = player.queue();
            let n_songs = queue.n_songs();
            if let Some(current) = queue.current_song_index() {
                let mut remaining_time = 0;
                for pos in 0..n_songs {
                    let song = queue.song_at(pos).unwrap();
                    if pos >= current {
                        remaining_time += song.duration();
                    }
                }

                let remaining_min = ((remaining_time - (remaining_time % 60)) / 60) as u32;
                let remaining_hrs = (remaining_min - (remaining_min % 60)) / 60;

                let remaining_str = if remaining_hrs > 0 {
                    // Translators: the `{}` must be left unmodified, and
                    // it will be replaced by the number of minutes remaining
                    // in the string "N hours M minutes remaining"
                    let minutes = ni18n_f(
                        "{} minute",
                        "{} minutes",
                        remaining_min % 60,
                        &[&(remaining_min % 60).to_string()],
                    );

                    // Translators: `{hours}` and `{minutes}` must be left
                    // unmodified, and they will be replaced by number of
                    // hours and by the translated number of minutes
                    // remaining, respectively
                    ni18n_k(
                        "{hours} hour {minutes} remaining",
                        "{hours} hours {minutes} remaining",
                        remaining_hrs,
                        &[("hours", &remaining_hrs.to_string()), ("minutes", &minutes)],
                    )
                } else {
                    ni18n_f(
                        // Translators: the '{}' must be left unmodified, and
                        // it will be replaced by the number of minutes remaining
                        // in the playlist
                        "{} minute remaining",
                        "{} minutes remaining",
                        remaining_min,
                        &[&remaining_min.to_string()],
                    )
                };

                self.imp()
                    .playlist_view
                    .queue_length_label()
                    .set_label(&remaining_str);
                self.imp()
                    .playlist_view
                    .queue_length_label()
                    .set_visible(true);
            } else {
                self.imp()
                    .playlist_view
                    .queue_length_label()
                    .set_visible(false);
            }
        }
    }

    fn scroll_playlist_to_song(&self) {
        let queue_view = self.imp().playlist_view.queue_view();
        if let Some(player) = self.player() {
            if let Some(current_idx) = player.queue().current_song_index() {
                debug!("Scrolling playlist to {}", current_idx);
                queue_view
                    .upcast_ref::<gtk::Widget>()
                    .activate_action("list.scroll-to-item", Some(&current_idx.to_variant()))
                    .expect("Failed to activate action");
            }
        }
    }

    fn setup_provider(&self) {
        let imp = self.imp();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &imp.provider, 400);
        }
    }

    fn update_style(&self, song: Option<&Song>) {
        let imp = self.imp();

        if !imp.settings.boolean("enable-recoloring") {
            imp.provider.load_from_string("");
            imp.main_stack.remove_css_class("main-window");
            return;
        }

        if let Some(song) = song {
            if let Some(bg_colors) = song.cover_palette() {
                let mut css = String::new();

                css.push_str(":root {");

                let n_colors = bg_colors.len();
                for (i, color) in bg_colors.iter().enumerate().take(n_colors) {
                    let s = format!("--background-color-{}: {};", i, color);
                    css.push_str(&s);
                }

                for i in n_colors + 1 - 1..5 {
                    css.push_str(&format!(
                        "--background-color-{}: var(--window-bg-color);",
                        i
                    ));
                }

                css.push_str("}");

                imp.provider.load_from_string(&css);
                if !imp.main_stack.has_css_class("main-window") {
                    imp.main_stack.add_css_class("main-window");
                }

                self.action_set_enabled("win.enable-recoloring", true);

                return;
            }
        }

        imp.provider.load_from_string("");
        imp.main_stack.remove_css_class("main-window");
        self.action_set_enabled("win.enable-recoloring", false);
    }

    fn update_title(&self, song: Option<&Song>) {
        if let Some(song) = song {
            self.set_title(Some(&format!("{} - {}", song.artist(), song.title())));
        } else {
            self.set_title(Some("Hopea"));
        }
    }

    fn update_selected_count(&self) {
        if let Some(player) = self.player() {
            let queue = player.queue();
            let n_selected = queue.n_selected_songs();

            let selected_str = if n_selected == 0 {
                i18n("No song selected")
            } else {
                ni18n_f(
                    // Translators: The '{}' must be left unmodified, and
                    // it is expanded to the number of songs selected
                    "{} song selected",
                    "{} songs selected",
                    n_selected,
                    &[&n_selected.to_string()],
                )
            };

            self.imp()
                .playlist_view
                .queue_selected_label()
                .set_label(&selected_str);
        }
    }

    pub fn open_files(&self, files: &[gio::File]) {
        if files.is_empty() {
            self.add_toast(i18n("Unable to access files"));
            return;
        }

        let model = gio::ListStore::new::<gio::File>();
        for f in files {
            model.append(f);
        }
        self.add_files_to_queue(model.upcast_ref::<gio::ListModel>());
    }

    pub fn remove_song(&self, song: &Song) {
        if !self.in_for_you() {
            if let Some(id) = song.library_id() {
                self.take_out_of_library(id, &song.title(), song.is_mine());
                return;
            }
            song.set_property("favorite", false);
        }
        if let Some(p) = self.player() {
            p.remove_song(song);
        }
        self.update_selected_count();
        self.update_playlist_time();
    }

    /// Shows or hides the shared music panel at the right of the window.
    fn toggle_library(&self) {
        if self.is_import_audition() {
            return;
        }
        let imp = self.imp();
        if self.library_in_sheet() {
            let open = !imp.library_sheet.is_open();
            imp.library_sheet.set_open(open);
            if open {
                imp.library_panel.refresh_protection_notice();
                imp.library_panel.focus_search();
            }
            return;
        }
        let show = !imp.library_split.shows_sidebar();
        imp.library_split.set_show_sidebar(show);
        if show {
            imp.library_panel.refresh_protection_notice();
            imp.library_panel.focus_search();
        }
    }

    /// Hides the shared music panel if it covers the rest of the window,
    /// so that what was just played can be seen.
    pub fn hide_library_if_overlaid(&self) {
        if self.library_in_sheet() {
            self.imp().library_sheet.set_open(false);
            return;
        }
        let split = &self.imp().library_split;
        if split.is_collapsed() {
            split.set_show_sidebar(false);
        }
    }

    /// Search audition uses the main transport without adding to either queue.
    pub fn preview_remote_track(&self, track: &Track) {
        let (Some(player), Some(client)) = (self.player(), library::client()) else {
            return;
        };
        let song = Song::from_remote(track, &client.audio_uri(track.id), None);
        player.preview(song.clone());
        if track.has_cover {
            let id = track.id;
            glib::spawn_future_local(async move {
                if let Ok(bytes) = client.cover(id).await {
                    song.set_cover_bytes(&bytes);
                }
            });
        }
        self.switch_mode(WindowMode::MainView);
    }

    /// Adds a song from the shared library to the playlist, and optionally
    /// starts playing it right away; playback starts while it streams.
    pub async fn add_remote_track(&self, track: &Track, play: bool) {
        if !track.favorite && !self.in_for_you() {
            if play {
                self.preview_remote_track(track);
            }
            return;
        }
        let (Some(player), Some(client)) = (self.player(), library::client()) else {
            return;
        };

        if self.in_for_you() && !track.local && (play || player.queue().is_empty()) {
            if let Err(error) = client.prefetch(track.id).await {
                debug!("Skipping unavailable recommendation {}: {error}", track.id);
                return;
            }
            if !self.in_for_you() {
                return;
            }
        }
        let cover = if track.has_cover {
            match client.cover(track.id).await {
                Ok(bytes) => Some(bytes),
                Err(err) => {
                    debug!("Unable to fetch cover art: {err}");
                    None
                }
            }
        } else {
            None
        };
        let song = Song::from_remote(track, &client.audio_uri(track.id), cover.as_ref());

        if !play && track.favorite && self.in_for_you() {
            let mut songs = self.imp().my_music_songs.borrow_mut();
            if let Some(existing) = songs.iter().find(|s| s.equals(&song)) {
                existing.set_library(Some(track.id), track.mine, track.favorite);
            } else {
                songs.push(song);
            }
            return;
        }

        let queue = player.queue();
        let was_empty = queue.is_empty();
        let find = |song: &Song| {
            (0..queue.n_songs()).find(|&i| queue.song_at(i).is_some_and(|s| s.equals(song)))
        };
        match find(&song).and_then(|i| queue.song_at(i)) {
            Some(existing) => existing.set_library(Some(track.id), track.mine, track.favorite),
            None => {
                queue.add_song(&song);
            }
        }
        let Some(pos) = find(&song) else {
            return;
        };

        self.switch_mode(WindowMode::MainView);
        if play || (was_empty && !player.is_previewing()) {
            player.skip_to(pos);
        }
        if play {
            player.play();
        }
    }

    pub fn add_toast(&self, msg: String) {
        let toast = adw::Toast::new(&msg);
        self.imp().toast_overlay.add_toast(toast);
    }

    pub fn add_skip_to_toast(&self, msg: String, button: String, pos: u32) {
        let toast = adw::Toast::new(&msg);
        toast.set_button_label(Some(&button));
        toast.set_action_name(Some("win.skip-to"));
        toast.set_action_target_value(Some(&pos.to_variant()));
        self.imp().toast_overlay.add_toast(toast);
    }

    fn copy_song(&self) {
        if let Some(player) = self.player() {
            let state = player.state();
            if let Some(song) = state.current_song() {
                let s = i18n_k(
                    // Translators: `{title}` and `{artist}` must be left
                    // untranslated; they will expand to the title and
                    // artist of the currently playing song, respectively
                    "Currently playing “{title}” by “{artist}”",
                    &[("title", &song.title()), ("artist", &song.artist())],
                );
                self.clipboard().set_text(&s);
            }
        }
    }

    async fn open_song_directory(&self) {
        if let Some(player) = self.player() {
            let state = player.state();

            if let Some(song) = state.current_song() {
                // Songs from the shared library have no local folder
                if song.remote_id().is_some() {
                    return;
                }

                let file = song.file();

                match file
                    .query_info_future(
                        &ATTRIBUTE_HOST_PATH,
                        gio::FileQueryInfoFlags::NONE,
                        glib::Priority::DEFAULT,
                    )
                    .await
                {
                    Err(err) => debug!("Unable to get host-path attribute: {err}"),
                    Ok(info) => {
                        let host_path = info
                            .attribute_as_string(&ATTRIBUTE_HOST_PATH)
                            .and_then(|x| PathBuf::from_str(x.as_str()).ok());

                        if let Some(path) = host_path.or_else(|| file.path()) {
                            let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
                            match launcher
                                .open_containing_folder_future(Some(
                                    self.upcast_ref::<gtk::Window>(),
                                ))
                                .await
                            {
                                Err(_e) => self.add_toast(i18n("Unable to open folder")),
                                Ok(()) => {}
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn switch_mode(&self, mode: WindowMode) {
        let stack = self.imp().main_stack.get();
        match mode {
            WindowMode::InitialView => {
                stack.set_visible_child_name("initial-view");
                self.set_default_widget(Some(&self.imp().add_folder_button.get()));
            }
            WindowMode::MainView => {
                stack.set_visible_child_name("main-view");
                self.set_default_widget(Some(&self.imp().playback_control.play_button()));
            }
        };
    }

    pub fn set_replaygain(&self, replaygain: ReplayGainMode) {
        let imp = self.imp();

        if replaygain != imp.replaygain_mode.replace(replaygain) {
            if let Some(p) = self.player() {
                p.set_replaygain(replaygain);
            }
            self.imp()
                .settings
                .set_enum("replay-gain", replaygain.into())
                .expect("Unable to store setting");

            self.notify("replaygain-mode");
        }
    }

    pub fn replaygain(&self) -> ReplayGainMode {
        self.imp().replaygain_mode.get()
    }

    pub fn set_song_time(&self, elapsed: Option<u64>, remaining: Option<u64>) {
        if let Some(elapsed) = elapsed {
            self.imp()
                .elapsed_label
                .set_text(&utils::format_time(elapsed as i64));
        } else {
            self.imp().elapsed_label.set_text("0:00");
        }

        if let Some(remaining) = remaining {
            self.imp()
                .remaining_label
                .set_text(&utils::format_remaining_time(remaining as i64));
        } else {
            self.imp().remaining_label.set_text("0:00");
        }
    }

    pub fn set_song_position(&self, position: f64) {
        self.imp().waveform_view.set_position(position);
    }
}

/// The folder songs taken out of the library are moved to: as set, or
/// "Unliked" in the music folder.
/// Time between two searches for songs from other services, to spare
/// the network
const WANTED_SEARCH_GAP: std::time::Duration = std::time::Duration::from_secs(3);
/// Recommendations queued at once in For You
const FOR_YOU_BATCH: usize = 20;
/// Songs asked about at once for the playlist's transfer status
const MAX_TRANSFERS_ASKED: usize = 20;

/// How a song from the network is coming along, in words.
fn transfer_text(transfer: &hopea_common::api::Transfer) -> String {
    let speed = |bps: u64| format!("{}/s", glib::format_size(bps));
    if transfer.progress >= 1.0 {
        if transfer.upload_bps > 0 {
            i18n_k(
                // Translators: `{speed}` is a speed, such as "1.2 MB/s"
                "On this computer · sharing ↑ {speed}",
                &[("speed", &speed(transfer.upload_bps))],
            )
        } else {
            i18n("On this computer")
        }
    } else if transfer.peers == 0 {
        i18n_k(
            // Translators: `{percent}` is a number from 0 to 100
            "{percent}% here · looking for people who have it",
            &[("percent", &((transfer.progress * 100.0) as u32).to_string())],
        )
    } else {
        ni18n_k(
            // Translators: `{percent}` is a number from 0 to 100, `{n}` a
            // count of people, `{speed}` a speed such as "1.2 MB/s"
            "Downloading {percent}% · from {n} person · ↓ {speed}",
            "Downloading {percent}% · from {n} people · ↓ {speed}",
            transfer.peers,
            &[
                ("percent", &((transfer.progress * 100.0) as u32).to_string()),
                ("n", &transfer.peers.to_string()),
                ("speed", &speed(transfer.download_bps)),
            ],
        )
    }
}
