// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

mod account_jobs;
mod import_picker;
mod import_preview;
mod import_review;
mod import_warnings;
mod search_results;
#[cfg(not(target_os = "android"))]
mod service_sign_in;
mod sign_in_help;

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use adw::{prelude::*, subclass::prelude::*};
use glib::clone;
use gtk::{gdk, gio, glib, CompositeTemplate};
use hopea_common::{
    api::{
        DhtState, Me, Reachability, SearchFailure, SearchOrder, SearchReport, SpeedLimits, Track,
        MAX_SHARING_SLOWDOWN, MIN_LIMIT_BPS,
    },
    describe_non_audio, AudioFormat, SNIFF_LEN,
};
use log::{debug, warn};

use crate::{
    i18n::{i18n, i18n_f, i18n_k, ni18n_f},
    library::{self, LibraryClient, UploadCover},
    utils,
    window::Window,
};

/// Longest name shown next to shared songs, as the node accepts it
const MAX_NAME_CHARS: usize = 32;

/// Which list a row is shown in; rows behave slightly differently in each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKind {
    Search,
    Album,
}

/// Local form checks only; the service verifies the session during import.
fn account_import_problem(
    service: u32,
    secret: &str,
    account: &str,
    signing_in: bool,
) -> Option<String> {
    if signing_in {
        return Some(i18n("Finish signing in before starting the import."));
    }
    if secret.trim().is_empty() {
        return Some(i18n(
            "Sign in with your browser or enter your service’s session data or access token.",
        ));
    }
    let account = account.trim();
    if service == 0
        && !(account.starts_with(|c: char| matches!(c, '1'..='9'))
            && account.bytes().all(|c| c.is_ascii_digit()))
    {
        return Some(i18n(
            "Enter the numeric VK profile ID to import, for example 123456789.",
        ));
    }
    None
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/io/hopea/Hopea/library-panel.ui")]
    pub struct LibraryPanel {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub upload_bar: TemplateChild<gtk::Box>,
        #[template_child]
        pub title_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub switcher: TemplateChild<adw::ViewSwitcher>,
        #[template_child]
        pub menu_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub name_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub join_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub reach_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub vpn_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub connection_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub connection_description: TemplateChild<gtk::Label>,
        #[template_child]
        pub connection_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub connection_settings: TemplateChild<gtk::Button>,
        #[template_child]
        pub view_stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub search_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub results_list: TemplateChild<gtk::ListView>,
        #[template_child]
        pub results_scroll: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub albums_section: TemplateChild<gtk::Box>,
        #[template_child]
        pub albums_scroll: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub albums_list: TemplateChild<gtk::ListView>,
        #[template_child]
        pub page_spinner: TemplateChild<adw::Spinner>,
        #[template_child]
        pub network_status: TemplateChild<gtk::Box>,
        #[template_child]
        pub retry_page_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub empty_page: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub order_dropdown: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub navigation: TemplateChild<adw::NavigationView>,
        #[template_child]
        pub album_page: TemplateChild<adw::NavigationPage>,
        #[template_child]
        pub album_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub album_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub album_subtitle: TemplateChild<gtk::Label>,
        #[template_child]
        pub album_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub upload_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub upload_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub upload_progress: TemplateChild<gtk::ProgressBar>,

        pub me: RefCell<Option<Me>>,
        pub settings: once_cell::unsync::OnceCell<gio::Settings>,
        pub loaded: Cell<u32>,
        pub results: once_cell::unsync::OnceCell<gio::ListStore>,
        pub album_results: once_cell::unsync::OnceCell<gio::ListStore>,
        pub page_loading: Cell<bool>,
        pub page_offset: Cell<u32>,
        pub has_more: Cell<bool>,
        // Incremented for every search, so that late responses to an older
        // query are ignored
        pub generation: Cell<u64>,
        /// Whether the favorites were put in the playlist since the app started
        pub favorites_in_playlist: Cell<bool>,
        pub availability_loading: Cell<bool>,
        /// The album shown on the album page: title and artist
        pub album: RefCell<Option<(String, Option<String>)>>,
        pub album_tracks: RefCell<Vec<Track>>,
        pub album_generation: Cell<u64>,
        /// A batch of uploads or an album download is running
        pub busy: Cell<bool>,
        /// The star buttons shown, by track, to keep them all current
        pub stars: RefCell<Vec<(i64, glib::WeakRef<gtk::Button>)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LibraryPanel {
        const NAME: &'static str = "HopeaLibraryPanel";
        type Type = super::LibraryPanel;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);

            klass.install_action_async("library.upload", None, |dialog, _, _| async move {
                dialog.choose_uploads().await;
            });
            klass.install_action_async("library.leave", None, |dialog, _, _| async move {
                dialog.leave().await;
            });
            klass.install_action_async("library.import", None, |panel, _, _| async move {
                panel.import_music().await;
            });
            klass.install_action_async("library.settings", None, |panel, _, _| async move {
                panel.edit_settings().await;
            });
            klass.install_action_async("library.upload-folder", None, |panel, _, _| async move {
                panel.choose_folder_upload().await;
            });
            klass.install_action_async("library.play-album", None, |panel, _, _| async move {
                let tracks = panel.imp().album_tracks.borrow().clone();
                panel.play_all(tracks).await;
            });
            klass.install_action_async("library.download-album", None, |panel, _, _| async move {
                panel.download_album().await;
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for LibraryPanel {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_results();
            obj.connect_signals();
            obj.start();
        }
    }

    impl WidgetImpl for LibraryPanel {
        fn map(&self) {
            self.parent_map();
            self.obj().refresh_protection_notice();
            // Reachability may have changed since the panel was last shown
            self.obj().refresh_me();
        }
    }
    impl BinImpl for LibraryPanel {}
}

glib::wrapper! {
    pub struct LibraryPanel(ObjectSubclass<imp::LibraryPanel>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl LibraryPanel {
    fn window(&self) -> Option<Window> {
        self.root().and_downcast::<Window>()
    }

    /// Updates every star shown for a track, after it was added to the
    /// favorites or removed from them elsewhere.
    pub fn set_star_state(&self, id: i64, favorite: bool) {
        let imp = self.imp();
        self.update_result_star(id, favorite);
        imp.stars.borrow_mut().retain(|(track, star)| {
            let Some(star) = star.upgrade() else {
                return false;
            };
            if *track == id {
                set_star(&star, favorite);
            }
            true
        });
    }

    /// Puts the keyboard focus in the search entry, if it is shown.
    pub fn focus_search(&self) {
        let imp = self.imp();
        if imp.stack.visible_child_name().as_deref() == Some("library") {
            imp.view_stack.set_visible_child_name("search");
            imp.search_entry.grab_focus();
        }
    }

    /// Updates what is shown about this node, such as whether others can
    /// reach it.
    fn refresh_me(&self) {
        let Some(client) = library::client() else {
            return;
        };
        glib::spawn_future_local(clone!(
            #[weak(rename_to = this)]
            self,
            async move {
                if let Ok(me) = client.me().await {
                    this.imp()
                        .reach_banner
                        .set_revealed(me.reachability == Reachability::Unreachable);
                    this.imp().me.replace(Some(me));
                }
            }
        ));
    }

    fn settings(&self) -> gio::Settings {
        self.imp()
            .settings
            .get_or_init(utils::settings_manager)
            .clone()
    }

    fn connect_signals(&self) {
        let imp = self.imp();

        imp.vpn_banner.connect_button_clicked(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                glib::spawn_future_local(async move { this.edit_settings_at_privacy(true).await });
            }
        ));
        imp.connection_settings.connect_clicked(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                glib::spawn_future_local(async move { this.edit_settings_at_privacy(true).await });
            }
        ));
        imp.stack.connect_visible_child_name_notify(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| this.refresh_protection_notice()
        ));
        self.settings().connect_changed(
            Some("library-only-protected"),
            clone!(
                #[weak(rename_to = this)]
                self,
                move |_, _| this.refresh_protection_notice()
            ),
        );
        glib::timeout_add_local(
            std::time::Duration::from_secs(2),
            clone!(
                #[weak(rename_to = this)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    if this.is_mapped() {
                        this.refresh_protection_notice();
                    }
                    glib::ControlFlow::Continue
                }
            ),
        );

        imp.join_button.connect_clicked(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                glib::spawn_future_local(async move { this.join().await });
            }
        ));

        imp.name_row.connect_entry_activated(clone!(
            #[weak]
            imp,
            move |_| imp.join_button.emit_clicked()
        ));

        imp.search_entry.connect_search_changed(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| this.search()
        ));

        imp.retry_page_button.connect_clicked(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| this.load_page(this.imp().page_offset.get())
        ));

        imp.order_dropdown.connect_selected_notify(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| this.search()
        ));
    }

    fn toast(&self, message: &str) {
        // Messages may contain titles and text from other people: never
        // parse markup
        let toast = adw::Toast::builder()
            .title(message)
            .use_markup(false)
            .build();
        self.imp().toast_overlay.add_toast(toast);
    }

    /// A persistent explanation, visible immediately on opening the panel,
    /// including before joining and on an album page. This never changes the
    /// listener's VPN preference or tries to join the network.
    pub(crate) fn refresh_protection_notice(&self) {
        let required = self.settings().boolean("library-only-protected");
        self.update_protection_notice(required, &library::protection::check());
    }

    fn update_protection_notice(
        &self,
        required: bool,
        protection: &library::protection::Protection,
    ) {
        use library::protection::Protection;
        let imp = self.imp();
        let usable = protection.sharing_ipv4_only();
        let (title, description, icon, color) = match protection {
            Protection::Offline => (i18n("You’re Offline"), i18n("Connect to the internet to discover and share music."), "hopea-network-offline-symbolic", "warning"),
            _ if required && usable == Some(true) => (i18n("VPN Connected"), i18n("Ready to share over IPv4. Hopea won’t use IPv6 for sharing."), "hopea-network-vpn-symbolic", "success"),
            _ if usable == Some(false) => (i18n("VPN Connected"), i18n("Your connection is ready. Join the network to start sharing."), "hopea-network-vpn-symbolic", "success"),
            Protection::Partial { .. } if required => (i18n("Check Your VPN"), i18n("Enable IPv4 protection in your VPN, or change the VPN requirement in Settings."), "hopea-network-vpn-symbolic", "warning"),
            _ if required => (i18n("Waiting for Your VPN"), i18n("Turn on your VPN to start sharing, or change the VPN requirement in Settings."), "hopea-network-vpn-symbolic", "warning"),
            _ => (i18n("Ready to Connect"), i18n("Connect directly with other listeners to find and share songs."), "hopea-network-workgroup-symbolic", "accent"),
        };
        imp.connection_title.set_text(&title);
        imp.connection_description.set_text(&description);
        imp.connection_icon.set_icon_name(Some(icon));
        imp.connection_icon.set_css_classes(&[color]);
        imp.vpn_banner.set_title(&description);
        // The welcome card already explains the connection; do not repeat a
        // long banner above it, which also steals room in narrow layouts.
        imp.vpn_banner.set_revealed(
            required
                && usable.is_none()
                && imp.stack.visible_child_name().as_deref() != Some("welcome"),
        );
    }

    #[cfg(test)]
    pub(crate) async fn test_welcome_layout(&self, window: &adw::Window) {
        let color_scheme = adw::StyleManager::default().color_scheme();
        if std::env::var_os("HOPEA_TEST_CONNECTION_SCREENSHOT").is_some() {
            adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        }
        let settings = self.settings();
        let required = settings.boolean("library-only-protected");
        settings
            .set_boolean("library-only-protected", true)
            .unwrap();
        window.set_default_size(360, 740);
        glib::timeout_future_seconds(2).await;
        assert!(
            self.measure(gtk::Orientation::Horizontal, -1).0 <= 360,
            "welcome must fit a narrow panel"
        );
        for label in [
            &self.imp().connection_title.get(),
            &self.imp().connection_description.get(),
        ] {
            let bounds = label.compute_bounds(self).unwrap();
            assert!(bounds.x() >= 0.0 && bounds.x() + bounds.width() <= self.width() as f32 + 1.0);
        }
        if let Some(path) = std::env::var_os("HOPEA_TEST_CONNECTION_SCREENSHOT") {
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gdk::RGBA::new(0.15, 0.15, 0.15, 1.0),
                &gtk::graphene::Rect::new(0.0, 0.0, self.width() as f32, self.height() as f32),
            );
            self.snapshot_child(&self.child().unwrap(), &snapshot);
            window
                .renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
        settings
            .set_boolean("library-only-protected", required)
            .unwrap();
        adw::StyleManager::default().set_color_scheme(color_scheme);
    }

    #[cfg(test)]
    pub(crate) fn check_vpn_notice(&self) {
        use library::protection::{Protection, Status};
        let banner = &self.imp().vpn_banner;
        let disconnected = Protection::Exposed(Some("wifi".into()));
        self.update_protection_notice(true, &disconnected);
        assert!(!banner.is_revealed(), "welcome explains VPN inline");
        assert_eq!(self.imp().connection_title.text(), "Waiting for Your VPN");
        self.update_protection_notice(false, &disconnected);
        assert!(!banner.is_revealed());
        let partial = Protection::Partial {
            tunnels: vec!["Meta".into()],
            exposed_families: vec!["IPv6"],
        };
        self.update_protection_notice(true, &partial);
        assert!(!banner.is_revealed());
        assert_eq!(self.imp().connection_title.text(), "VPN Connected");
        assert!(self.imp().connection_description.text().contains("IPv4"));
        let (protected, message) = protection_text(&Status {
            protection: partial,
            tools: vec!["Clash"],
            system_proxy: true,
        });
        assert!(protected);
        assert!(message.contains("IPv4"));
        self.update_protection_notice(true, &Protection::Protected(vec!["Meta".into()]));
        assert!(
            !banner.is_revealed(),
            "connecting the VPN clears the warning"
        );
        self.update_protection_notice(true, &Protection::Offline);
        assert!(!banner.is_revealed());
        assert_eq!(self.imp().connection_title.text(), "You’re Offline");
        self.imp().connection_settings.emit_clicked();
    }

    fn start(&self) {
        if let Some(client) = library::client() {
            glib::spawn_future_local(clone!(
                #[weak(rename_to = this)]
                self,
                async move { this.finish_joining(client).await }
            ));
            return;
        }

        // Joined before, but not at this start (as on phones): joining
        // again is one button away
        let settings = self.settings();
        if !settings.boolean("library-enabled") || !library::wanted() {
            self.show_welcome();
            return;
        }
        if !library::allowed(&settings) {
            self.show_welcome();
            self.refresh_protection_notice();
            return;
        }

        // The node starts with the app; it may still be starting
        self.imp().stack.set_visible_child_name("loading");
        glib::spawn_future_local(clone!(
            #[weak(rename_to = this)]
            self,
            async move {
                match library::ensure_started().await {
                    Ok(client) => this.finish_joining(client).await,
                    Err(err) => {
                        this.toast(&err);
                        this.show_welcome();
                    }
                }
            }
        ));
    }

    /// Follows the node starting or stopping by itself, as when a VPN goes
    /// off or comes back.
    pub fn connection_changed(&self) {
        match library::client() {
            Some(client) => {
                glib::spawn_future_local(clone!(
                    #[weak(rename_to = this)]
                    self,
                    async move { this.finish_joining(client).await }
                ));
            }
            None => {
                let imp = self.imp();
                imp.me.replace(None);
                self.reset_search();
                self.show_welcome();
            }
        }
    }

    fn show_welcome(&self) {
        let imp = self.imp();
        imp.upload_bar.set_visible(false);
        imp.menu_button.set_visible(false);
        imp.switcher.set_visible(false);
        imp.title_label.set_visible(true);
        imp.join_button.set_sensitive(true);
        imp.name_row
            .set_text(&self.settings().string("library-name"));
        imp.stack.set_visible_child_name("welcome");
    }

    async fn join(&self) {
        let imp = self.imp();
        let name: String = imp
            .name_row
            .text()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_NAME_CHARS)
            .collect();
        let settings = self.settings();
        if let Err(err) = settings.set_string("library-name", &name) {
            warn!("Unable to store the library name: {err}");
        }

        library::set_wanted(true);
        if !library::allowed(&settings) {
            // The app joins by itself once a VPN is on
            if let Err(err) = settings.set_boolean("library-enabled", true) {
                warn!("Unable to enable the library: {err}");
            }
            self.refresh_protection_notice();
            return;
        }
        imp.join_button.set_sensitive(false);
        imp.stack.set_visible_child_name("loading");
        match library::ensure_started().await {
            Ok(client) => {
                if let Err(err) = settings.set_boolean("library-enabled", true) {
                    warn!("Unable to enable the library: {err}");
                }
                self.finish_joining(client).await;
            }
            Err(err) => {
                self.toast(&i18n_f("Unable to start sharing: {}", &[&err]));
                self.show_welcome();
            }
        }
    }

    async fn finish_joining(&self, client: LibraryClient) {
        let me = match client.me().await {
            Ok(me) => me,
            Err(err) => {
                self.toast(&err.to_string());
                self.show_welcome();
                return;
            }
        };
        let imp = self.imp();
        imp.reach_banner
            .set_revealed(me.reachability == Reachability::Unreachable);
        imp.me.replace(Some(me));

        imp.upload_bar.set_visible(false);
        imp.menu_button.set_visible(true);
        imp.title_label.set_visible(false);
        imp.stack.set_visible_child_name("library");
        if self.is_mapped() {
            imp.search_entry.grab_focus();
        }
        self.search();

        if let Some(window) = self.window() {
            window.look_for_wanted();
        }
        if !imp.favorites_in_playlist.replace(true) {
            if let Some(window) = self.window() {
                window.sync_local_sharing().await;
            }
            match fetch_favorites(&client).await {
                Ok(tracks) => {
                    if let Some(window) = self.window() {
                        window.show_favorites_in_playlist(tracks).await;
                    }
                }
                Err(err) => warn!("Unable to load favorites: {err}"),
            }
        }
    }

    /// Imports downloaded audio, or song lists to look for on the network.
    pub async fn import_music(&self) {
        let group = adw::PreferencesGroup::builder()
            .description(i18n(
                "Downloaded audio files are checked, added to your library and shared. For exported song lists, the app looks for the songs on the network and stars the ones people share. The others are looked for again later.",
            ))
            .build();
        let alert = adw::AlertDialog::builder()
            .heading(i18n("Bring Your Music Over"))
            .extra_child(&group)
            .close_response("close")
            .build();
        alert.add_response("close", &i18n("_Close"));

        #[derive(Clone, Copy)]
        enum Source {
            Account,
            AudioFiles,
            AudioFolder,
            File,
            YandexLink,
            List,
        }
        let rows: [(String, String, &[(Source, String)]); 6] = [
            (
                i18n("Your Online Library"),
                i18n("Read your saved songs, playlists and albums from VK, Yandex Music, Spotify or YouTube Music. Download available audio, or look for the songs on the sharing network."),
                &[(Source::Account, i18n("Import Account…"))],
            ),
            (
                i18n("Downloaded Music, Including VK"),
                i18n("If you downloaded songs with a browser extension or from a music service, choose the audio files or their folder. Subfolders are included. Extract ZIP archives first. These songs will be shared with other listeners."),
                &[(Source::AudioFiles, i18n("Choose Songs…")), (Source::AudioFolder, i18n("Choose Folder…"))],
            ),
            (
                "Spotify".to_string(),
                i18n("In Spotify: Account → Privacy settings → Download your data. When the email comes, choose YourLibrary.json or a Playlist file from it. A CSV from Exportify works too."),
                &[(Source::File, i18n("Choose File…"))],
            ),
            (
                "YouTube Music".to_string(),
                i18n("On takeout.google.com, export “YouTube and YouTube Music”, then choose “music library songs.csv”, or a playlist file, from it."),
                &[(Source::File, i18n("Choose File…"))],
            ),
            (
                i18n("Yandex Music"),
                i18n("Paste the link to a playlist or an album. A playlist has to be public, also “My favorites”: make it so in its settings."),
                &[(Source::YandexLink, i18n("Paste Link…"))],
            ),
            (
                i18n("VK Music, Apple Music, Deezer and Others"),
                i18n("Export your library to a file with TuneMyMusic or Soundiiz (choose CSV), or Apple’s privacy export, or paste lines of “Artist - Title”."),
                &[(Source::File, i18n("Choose File…")), (Source::List, i18n("Paste List…"))],
            ),
        ];
        let (picked_tx, picked_rx) = async_channel::bounded::<Source>(1);
        for (title, subtitle, buttons) in rows.iter() {
            let row = adw::ActionRow::builder()
                .title(title.as_str())
                .subtitle(subtitle.as_str())
                .subtitle_lines(0)
                .build();
            let box_ = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .valign(gtk::Align::Center)
                .build();
            for (source, label) in buttons.iter() {
                let button = gtk::Button::builder().label(label.as_str()).build();
                let source = *source;
                button.connect_clicked(clone!(
                    #[strong]
                    picked_tx,
                    #[weak]
                    alert,
                    move |_| {
                        let _ = picked_tx.try_send(source);
                        alert.close();
                    }
                ));
                box_.append(&button);
            }
            row.add_suffix(&box_);
            group.add(&row);
        }
        alert.choose_future(Some(self)).await;
        let Ok(source) = picked_rx.try_recv() else {
            return;
        };

        let songs = match source {
            Source::Account => {
                self.import_account().await;
                return;
            }
            Source::AudioFiles => {
                self.choose_uploads().await;
                return;
            }
            Source::AudioFolder => {
                self.choose_folder_upload().await;
                return;
            }
            Source::File => {
                let dialog = gtk::FileDialog::builder()
                    .accept_label(i18n("_Import"))
                    .modal(true)
                    .title(i18n("Choose an Exported File"))
                    .build();
                let window = self.window();
                let Ok(file) = dialog.open_future(window.as_ref()).await else {
                    return;
                };
                library::import::read_file(&file).await
            }
            Source::YandexLink => {
                let Some(link) = self.ask_text(&i18n("Yandex Music Link"), false).await else {
                    return;
                };
                library::import::from_yandex(&link).await
            }
            Source::List => {
                let Some(list) = self.ask_text(&i18n("Songs, One per Line"), true).await else {
                    return;
                };
                Ok(library::import::from_text(&list))
            }
        };
        let songs = match songs {
            Ok(songs) if !songs.is_empty() => songs,
            Ok(_) => {
                self.toast(&i18n("No songs were found in it"));
                return;
            }
            Err(err) => {
                self.toast(&err);
                return;
            }
        };
        let total = songs.len();
        let added = library::import::add(songs);
        self.toast(&if added == 0 {
            i18n("All of these songs are already being looked for")
        } else {
            ni18n_f(
                "Looking for {} song on the network; when someone shares it, it gets a star",
                "Looking for {} songs on the network; the ones someone shares get a star",
                added as u32,
                &[&added.to_string()],
            )
        });
        log::debug!("Imported {total} songs, {added} new");
        if let Some(window) = self.window() {
            window.look_for_wanted();
        }
    }

    #[cfg(target_os = "android")]
    async fn sign_in_default_browser(
        &self,
        service: &str,
    ) -> Result<Option<(String, String)>, String> {
        let url = match service {
            "vk" => "https://vk.ru/audio",
            "yandex" => "https://music.yandex.ru/",
            "spotify" => "https://open.spotify.com/",
            _ => "https://music.youtube.com/library",
        };
        // The Android activity tries a non-browser handler before falling back
        // to the browser. Other apps' private sessions cannot be read here.
        gtk::UriLauncher::new(url)
            .launch_future(self.window().as_ref())
            .await
            .map_err(|_| i18n("Unable to open the music service"))?;
        Ok(None)
    }

    #[cfg(not(target_os = "android"))]
    async fn sign_in_default_browser(
        &self,
        service: &str,
    ) -> Result<Option<(String, String)>, String> {
        if matches!(service, "spotify" | "yandex") {
            return self.sign_in_service(service).await;
        }
        library::browser_import::require_browser_portal().await?;
        let url = if service == "vk" {
            "https://vk.ru/audio"
        } else {
            "https://music.youtube.com/library"
        };
        gtk::UriLauncher::new(url)
            .launch_future(self.window().as_ref())
            .await
            .map_err(|_| i18n("Unable to open the default browser"))?;
        // The portal knows the host's browser; AppInfo inside Flatpak does
        // not. Never block opening the browser on cookie-reader detection.
        if library::browser_import::sandboxed() {
            let prompt = adw::AlertDialog::builder()
                .heading(i18n("Continue in Your Browser"))
                .body(i18n("Your default browser has been opened. After signing in, paste the service’s session data into the import window. This Flatpak cannot read the browser’s saved sessions automatically."))
                .close_response("close")
                .build();
            prompt.add_response("close", &i18n("_Back to Import"));
            prompt.add_response("manual", &i18n("Manual Sign-In Instructions"));
            if prompt.choose_future(self.window().as_ref()).await == "manual" {
                self.show_sign_in_help(if service == "vk" { 0 } else { 3 })
                    .await;
            }
            return Ok(None);
        }
        let browser = library::browser_import::default_browser()?;
        let prompt = adw::AlertDialog::builder()
            .heading(i18n("Sign In in Your Browser"))
            .body(i18n("Sign in in a normal browser window and open your music library. When you press Continue, the importer reads the browser’s saved cookies locally and uses only this music service’s session. Private windows are not supported. If you use several browser profiles, the saved session may come from a different profile; sign in using the browser’s default profile."))
            .close_response("cancel").build();
        prompt.add_response("cancel", &i18n("_Cancel"));
        prompt.add_response("continue", &i18n("_Continue"));
        prompt.set_response_appearance("continue", adw::ResponseAppearance::Suggested);
        if prompt.choose_future(self.window().as_ref()).await != "continue" {
            return Ok(None);
        }
        library::service_import::browser_session(service, &browser)
            .await
            .map(Some)
    }

    async fn import_start_error(&self, message: &str) {
        let alert = adw::AlertDialog::builder()
            .heading(i18n("Import Could Not Start"))
            .body(message)
            .body_use_markup(false)
            .close_response("close")
            .build();
        alert.add_response("close", &i18n("_Close"));
        alert.choose_future(self.window().as_ref()).await;
    }

    async fn import_account(&self) {
        use library::service_import::Request;
        if self.imp().busy.get() {
            self.import_start_error(&i18n("Wait for the current transfer to finish"))
                .await;
            return;
        }
        let group = adw::PreferencesGroup::new();
        let services = gtk::StringList::new(&["VK", "Yandex Music", "Spotify", "YouTube Music"]);
        let service = adw::ComboRow::builder()
            .title(i18n("Music Service"))
            .model(&services)
            .build();
        let secret = adw::PasswordEntryRow::builder()
            .title(i18n("Session Cookie or Access Token"))
            .build();
        let account = adw::EntryRow::builder()
            .title(i18n("VK Profile ID to Import"))
            .build();
        let default_browser_button = adw::ButtonRow::builder()
            .title(if cfg!(target_os = "android") {
                i18n("Open Service App…")
            } else {
                i18n("Sign In with Default Browser…")
            })
            .start_icon_name("hopea-web-browser-symbolic")
            .build();
        if cfg!(target_os = "android") {
            default_browser_button.set_tooltip_text(Some(&i18n("Opens the installed service app, or your browser if it is unavailable. This does not grant Hopea access to the account.")));
        }
        default_browser_button.connect_activated(clone!(
            #[weak(rename_to = panel)]
            self,
            #[weak]
            service,
            #[weak]
            secret,
            #[weak]
            account,
            move |button| {
                if !service.is_sensitive() {
                    return;
                }
                button.set_sensitive(false);
                service.set_sensitive(false);
                let (button, panel, service, secret, account) = (
                    button.clone(),
                    panel.clone(),
                    service.clone(),
                    secret.clone(),
                    account.clone(),
                );
                glib::spawn_future_local(async move {
                    let name = match service.selected() {
                        0 => "vk",
                        1 => "yandex",
                        2 => "spotify",
                        _ => "youtube",
                    };
                    match panel.sign_in_default_browser(name).await {
                        Ok(Some((credential, id))) => {
                            secret.set_text(&credential);
                            if !id.is_empty() && account.text().trim().is_empty() {
                                account.set_text(&id);
                            }
                        }
                        Ok(None) => {
                            secret.grab_focus();
                        }
                        Err(error) => {
                            let alert = adw::AlertDialog::builder()
                                .heading(i18n("Browser Sign-In Unavailable"))
                                .body(error)
                                .body_use_markup(false)
                                .build();
                            alert.add_response("close", &i18n("_Close"));
                            alert.add_response("manual", &i18n("Manual Sign-In Instructions"));
                            if alert.choose_future(panel.window().as_ref()).await == "manual" {
                                panel.show_sign_in_help(service.selected()).await;
                            }
                        }
                    }
                    button.set_sensitive(true);
                    service.set_sensitive(true);
                });
            }
        ));
        let download = adw::SwitchRow::builder()
            .title(i18n("Download and Share Available Audio"))
            .subtitle(i18n("Files are checked before sharing. Songs without audio are looked for on the network."))
            .active(true)
            .build();
        let covers = adw::SwitchRow::builder()
            .title(i18n("Import Cover Art"))
            .subtitle(i18n(
                "Turn off to import audio without downloading or updating cover images.",
            ))
            .active(true)
            .build();
        let hint = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .max_width_chars(60)
            .build();
        let update = clone!(
            #[weak]
            secret,
            #[weak]
            account,
            #[weak]
            download,
            #[weak]
            hint,
            #[weak]
            default_browser_button,
            move |service: &adw::ComboRow| {
                default_browser_button.set_visible(true);
                account.set_visible(service.selected() == 0);
                secret.set_text("");
                download.set_sensitive(true);
                download.set_active(true);
                hint.set_text(&match service.selected() {
                    0 => i18n("Sign in with your browser, then enter the profile ID whose music you want to import. This can be your own profile or another profile whose audio you can access. Signing in keeps the ID you entered."),
                    1 => i18n("Sign in with your browser and confirm the displayed device code, or enter a Yandex Music OAuth token. The importer reads liked songs, playlists and saved albums. Audio availability depends on your account."),
                    2 => i18n("Sign in with your browser using your Spotify application’s public Client ID, or enter an OAuth access token with user-library-read, playlist-read-private, playlist-read-collaborative and streaming permissions. Direct audio import uses librespot and requires Premium; some accounts are incompatible with Spotify's current audio access. Turn off audio download to import song names only."),
                    _ => i18n("Sign in with your browser, or paste the JSON session headers used by ytmusicapi. The importer reads library songs, likes, playlists and albums."),
                });
            }
        );
        update(&service);
        service.connect_selected_notify(update);
        group.add(&service);
        group.add(&default_browser_button);
        group.add(&account);
        group.add(&secret);
        let manual = adw::ButtonRow::builder()
            .title(i18n("Manual Sign-In Instructions"))
            .build();
        manual.connect_activated(clone!(
            #[weak(rename_to = panel)]
            self,
            #[weak]
            service,
            move |_| {
                let selected = service.selected();
                glib::spawn_future_local(async move { panel.show_sign_in_help(selected).await });
            }
        ));
        group.add(&manual);
        group.add(&download);
        group.add(&covers);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.append(&group);
        content.append(&hint);
        let validation = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .max_width_chars(60)
            .build();
        content.append(&validation);
        let dialog = adw::AlertDialog::builder()
            .heading(i18n("Import Your Online Library"))
            .body(i18n("Sign-in data is used for this import only and is not saved. Already imported files are reused when you run it again."))
            .extra_child(&content)
            .close_response("cancel")
            .build();
        #[cfg(target_os = "android")]
        dialog.set_body(&i18n("Account import is not available in this experimental Android build. You can open the service app here, or add audio files using Share Songs."));
        dialog.add_response("cancel", &i18n("_Cancel"));
        dialog.add_response("import", &i18n("_Import"));
        dialog.set_response_appearance("import", adw::ResponseAppearance::Suggested);
        let validate = Rc::new(clone!(
            #[weak]
            dialog,
            #[weak]
            service,
            #[weak]
            secret,
            #[weak]
            account,
            #[weak]
            validation,
            move || {
                let problem = account_import_problem(
                    service.selected(),
                    &secret.text(),
                    &account.text(),
                    !service.is_sensitive(),
                );
                dialog.set_response_enabled(
                    "import",
                    problem.is_none() && !cfg!(target_os = "android"),
                );
                validation.set_text(problem.as_deref().unwrap_or(""));
                validation.set_visible(problem.is_some());
            }
        ));
        validate();
        secret.connect_changed(clone!(
            #[strong]
            validate,
            move |_| validate()
        ));
        account.connect_changed(clone!(
            #[strong]
            validate,
            move |_| validate()
        ));
        service.connect_selected_notify(clone!(
            #[strong]
            validate,
            move |_| validate()
        ));
        service.connect_sensitive_notify(clone!(
            #[strong]
            validate,
            move |_| validate()
        ));
        loop {
            if dialog.clone().choose_future(self.window().as_ref()).await != "import" {
                secret.set_text("");
                return;
            }
            // Keep the form and its data if a response is triggered without
            // using the button (for example through accessibility tooling).
            if account_import_problem(
                service.selected(),
                &secret.text(),
                &account.text(),
                !service.is_sensitive(),
            )
            .is_none()
            {
                break;
            }
            validate();
        }
        let credential = secret.text().trim().to_string();
        secret.set_text("");
        let service_name = ["vk", "yandex", "spotify", "youtube"][service.selected() as usize];
        let wants_audio = download.is_active();
        let request = Request {
            service: service_name.into(),
            secret: credential,
            account: account.text().trim().to_string(),
            download: wants_audio,
            covers: covers.is_active(),
            browser: None,
            browser_profile: None,
            browse: true,
            directory: crate::paths::data_dir().join("imports").join(service_name),
        };
        self.begin_account_import(request).await;
    }

    /// Asks for a line of text, or several.
    async fn ask_text(&self, title: &str, many_lines: bool) -> Option<String> {
        let alert = adw::AlertDialog::builder()
            .heading(title)
            .close_response("cancel")
            .default_response("ok")
            .build();
        alert.add_response("cancel", &i18n("_Cancel"));
        alert.add_response("ok", &i18n("_Import"));
        alert.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        alert.set_response_enabled("ok", false);
        let text = if many_lines {
            let view = gtk::TextView::builder()
                .wrap_mode(gtk::WrapMode::WordChar)
                .top_margin(6)
                .bottom_margin(6)
                .left_margin(6)
                .right_margin(6)
                .build();
            let scrolled = gtk::ScrolledWindow::builder()
                .child(&view)
                .min_content_height(200)
                .css_classes(["card"])
                .build();
            alert.set_extra_child(Some(&scrolled));
            view.buffer().connect_changed(clone!(
                #[weak]
                alert,
                move |buffer| {
                    let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                    alert.set_response_enabled("ok", !text.trim().is_empty());
                }
            ));
            if alert.choose_future(Some(self)).await != "ok" {
                return None;
            }
            let buffer = view.buffer();
            buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string()
        } else {
            let entry = gtk::Entry::builder().activates_default(true).build();
            alert.set_extra_child(Some(&entry));
            entry.connect_changed(clone!(
                #[weak]
                alert,
                move |entry| alert.set_response_enabled("ok", !entry.text().trim().is_empty())
            ));
            if alert.choose_future(Some(self)).await != "ok" {
                return None;
            }
            entry.text().to_string()
        };
        let text: String = text
            .chars()
            .take(library::import::MAX_IMPORT_BYTES)
            .collect();
        (!text.trim().is_empty()).then_some(text)
    }

    /// The shared music settings: whether to join the network at startup,
    /// speed limits, and what unstarring a song does.
    ///
    /// Sharing can be limited to at most seven times slower than
    /// downloading, which the dialog enforces as values change (and the
    /// node again, whatever it is sent).
    pub async fn edit_settings(&self) {
        self.edit_settings_at_privacy(false).await;
    }

    async fn edit_settings_at_privacy(&self, focus_privacy: bool) {
        let settings = self.settings();

        let join_row = adw::SwitchRow::builder()
            .title(i18n("Join the Network at Startup"))
            .subtitle(i18n(
                "Off at first on phones and tablets, to save battery and mobile data",
            ))
            .active(library::joins_at_start(&settings))
            .build();
        let startup_group = adw::PreferencesGroup::new();
        startup_group.add(&join_row);

        let protected_row = adw::SwitchRow::builder()
            .title(i18n("Share Only Through a VPN"))
            .subtitle(i18n(
                "Join the network only while a VPN is on, and leave it as soon as the VPN goes off",
            ))
            .active(settings.boolean("library-only-protected"))
            .build();
        let indicator = gtk::Image::builder()
            .icon_name("hopea-media-record-symbolic")
            .valign(gtk::Align::Center)
            .build();
        let status_row = adw::ActionRow::builder()
            .title(i18n("Your Connection"))
            .subtitle_lines(0)
            .use_markup(false)
            .build();
        status_row.add_prefix(&indicator);
        let show_status = clone!(
            #[weak]
            indicator,
            #[weak]
            status_row,
            #[upgrade_or]
            glib::ControlFlow::Break,
            move || {
                let (protected, text) = protection_text(&library::protection::status());
                indicator.set_css_classes(&[if protected {
                    "protection-on"
                } else {
                    "protection-off"
                }]);
                indicator.set_tooltip_text(Some(&if protected {
                    i18n("Through a VPN")
                } else {
                    i18n("Connection Is Not Fully Protected")
                }));
                status_row.set_subtitle(&text);
                glib::ControlFlow::Continue
            }
        );
        show_status();
        // The VPN may come or go while the dialog is open
        glib::timeout_add_local(std::time::Duration::from_secs(2), show_status);
        let privacy_group = adw::PreferencesGroup::builder()
            .title(i18n("Privacy"))
            .description(i18n(
                "Others on the network see the address you share from. A VPN shows them its address instead.",
            ))
            .build();
        privacy_group.add(&protected_row);
        privacy_group.add(&status_row);

        let unliked_group = adw::PreferencesGroup::builder()
            .title(i18n("Favorites and Sharing"))
            .description(i18n("Stars add songs to your personal playlist. Removing a star keeps the audio and albums on this computer, and they continue to be shared while sharing is enabled."))
            .build();

        let import_row = adw::ButtonRow::builder()
            .title(i18n("Bring Your Music Over…"))
            .start_icon_name("hopea-document-open-symbolic")
            .build();
        import_row.connect_activated(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                glib::spawn_future_local(async move { this.import_music().await });
            }
        ));
        let import_group = adw::PreferencesGroup::builder()
            .title(i18n("Your Music Elsewhere"))
            .description(i18n(
                "From Spotify, YouTube Music, Yandex Music, VK Music and other services",
            ))
            .build();
        import_group.add(&import_row);
        let cache_row = adw::ButtonRow::builder()
            .title(i18n("Clear Cache…"))
            .start_icon_name("hopea-edit-clear-all-symbolic")
            .build();
        cache_row.connect_activated(clone!(
            #[weak(rename_to=panel)]
            self,
            move |row| {
                let cover = panel
                    .window()
                    .and_then(|w| w.player())
                    .and_then(|p| p.state().current_song())
                    .and_then(|s| s.cover_cache());
                glib::spawn_future_local(clone!(
                    #[strong]
                    row,
                    async move {
                        crate::cache_cleanup::show(&row, cover).await;
                    }
                ));
            }
        ));
        let cache_group = adw::PreferencesGroup::builder()
            .title(i18n("Storage"))
            .build();
        cache_group.add(&cache_row);
        let min_kib = MIN_LIMIT_BPS / 1024;
        let spin = |value: u32| {
            let row =
                adw::SpinRow::with_range(f64::from(min_kib), f64::from(u32::MAX / 1024), 64.0);
            row.set_value(f64::from(value.max(min_kib)));
            row
        };

        let download_switch = adw::SwitchRow::builder()
            .title(i18n("Limit Downloading"))
            .build();
        let download_row = spin(settings.uint("library-download-limit"));
        download_row.set_title(&i18n("Downloading, KiB/s"));
        let upload_switch = adw::SwitchRow::builder()
            .title(i18n("Limit Sharing"))
            .build();
        let upload_row = spin(settings.uint("library-upload-limit"));
        upload_row.set_title(&i18n("Sharing, KiB/s"));

        download_switch.set_active(settings.uint("library-download-limit") > 0);
        upload_switch.set_active(settings.uint("library-upload-limit") > 0);

        // Keep the dialog within the rule at all times
        let enforce = clone!(
            #[weak]
            download_switch,
            #[weak]
            download_row,
            #[weak]
            upload_switch,
            #[weak]
            upload_row,
            move || {
                let limited = download_switch.is_active();
                download_row.set_sensitive(limited);
                upload_switch.set_sensitive(limited);
                if !limited {
                    upload_switch.set_active(false);
                }
                upload_row.set_sensitive(upload_switch.is_active());
                let floor = (download_row.value() / f64::from(MAX_SHARING_SLOWDOWN))
                    .ceil()
                    .max(f64::from(min_kib));
                upload_row.adjustment().set_lower(floor);
                if upload_row.value() < floor {
                    upload_row.set_value(floor);
                }
            }
        );
        enforce();
        download_switch.connect_active_notify(clone!(
            #[strong]
            enforce,
            move |_| enforce()
        ));
        upload_switch.connect_active_notify(clone!(
            #[strong]
            enforce,
            move |_| enforce()
        ));
        download_row.connect_value_notify(clone!(
            #[strong]
            enforce,
            move |_| enforce()
        ));

        let group = adw::PreferencesGroup::builder()
            .title(i18n("Speed Limits"))
            .description(i18n_k(
                // Translators: `{times}` is replaced by a number
                "Sharing can be limited to at most {times} times slower than downloading, so that everyone gets music about as fast as they share it.",
                &[("times", &MAX_SHARING_SLOWDOWN.to_string())],
            ))
            .build();
        group.add(&download_switch);
        group.add(&download_row);
        group.add(&upload_switch);
        group.add(&upload_row);
        let game_row = adw::SwitchRow::builder()
            .title(i18n("Game Mode"))
            .subtitle(crate::gaming::description())
            .active(settings.boolean("game-mode"))
            .build();
        group.add(&game_row);
        let game_force = adw::SwitchRow::builder()
            .title(i18n("Keep Game Mode Active"))
            .subtitle(i18n("Use reduced activity until you turn this off, including for games that are not detected automatically."))
            .active(settings.boolean("game-mode-force"))
            .sensitive(game_row.is_active())
            .build();
        game_row
            .bind_property("active", &game_force, "sensitive")
            .sync_create()
            .build();
        group.add(&game_force);
        #[cfg(target_os = "android")]
        {
            let access = adw::ActionRow::builder()
                .title(i18n("Allow Game Detection…"))
                .subtitle(i18n("Open Android Usage Access settings. App activity stays on this device and is not saved or shared."))
                .activatable(true)
                .build();
            access.connect_activated(clone!(
                #[weak(rename_to=panel)]
                self,
                move |_| {
                    if !crate::android::open_usage_settings() {
                        panel.toast(&i18n("Unable to open Usage Access settings"));
                    }
                }
            ));
            group.add(&access);
        }
        glib::timeout_add_seconds_local(
            2,
            clone!(
                #[weak]
                game_row,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    game_row.set_subtitle(&crate::gaming::description());
                    glib::ControlFlow::Continue
                }
            ),
        );

        let groups = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .build();
        let accent = crate::accent::Settings::new(self);
        let appearance = crate::artwork::Settings::new(self);
        let language = crate::language::Settings::new();
        groups.append(&language.group);
        groups.append(&accent.group);
        groups.append(&appearance.group);
        groups.append(&startup_group);
        groups.append(&privacy_group);
        groups.append(&group);
        groups.append(&unliked_group);
        groups.append(&import_group);
        groups.append(&cache_group);
        if !crate::dialogs::confirm_form(
            self.window().as_ref(),
            &i18n("Shared Music Settings"),
            &groups,
            focus_privacy.then_some(protected_row.upcast_ref()),
        )
        .await
        {
            return;
        }

        if let Err(message) = accent.apply() {
            self.toast(&message);
            return;
        }
        if let Err(message) = appearance.apply() {
            self.toast(&message);
            return;
        }
        if let Err(message) = language.apply() {
            self.toast(&message);
            return;
        }
        let mut stored = Vec::new();
        if join_row.is_active() != library::joins_at_start(&settings) {
            let value = if join_row.is_active() { "yes" } else { "no" };
            stored.push(settings.set_string("library-join-at-start", value));
        }
        stored.push(settings.set_boolean("library-only-protected", protected_row.is_active()));
        stored.push(settings.set_boolean("game-mode", game_row.is_active()));
        stored.push(settings.set_boolean("game-mode-force", game_force.is_active()));
        for result in stored {
            if let Err(err) = result {
                warn!("Unable to store a setting: {err}");
            }
        }

        let kib = |switch: &adw::SwitchRow, row: &adw::SpinRow| {
            switch.is_active().then(|| row.value() as u32)
        };
        let limits = SpeedLimits {
            download: kib(&download_switch, &download_row).map(|k| k.saturating_mul(1024)),
            upload: kib(&upload_switch, &upload_row).map(|k| k.saturating_mul(1024)),
        }
        .fair();
        for (key, value) in [
            ("library-download-limit", limits.download),
            ("library-upload-limit", limits.upload),
        ] {
            if let Err(err) = settings.set_uint(key, value.unwrap_or(0) / 1024) {
                warn!("Unable to store {key}: {err}");
            }
        }
        if let Some(client) = library::client() {
            match client.set_limits(limits).await {
                Ok(_) => self.toast(&i18n("Settings saved")),
                Err(err) => self.toast(&err.to_string()),
            }
        }
    }

    async fn leave(&self) {
        let alert = adw::AlertDialog::builder()
            .heading(i18n("Leave the Network?"))
            .body(i18n(
                "The songs you share will no longer be available to others, and you won’t be able to search theirs. Everything stays on this computer for when you join again.",
            ))
            .close_response("cancel")
            .default_response("cancel")
            .build();
        alert.add_response("cancel", &i18n("_Cancel"));
        alert.add_response("leave", &i18n("_Leave"));
        alert.set_response_appearance("leave", adw::ResponseAppearance::Destructive);
        if alert.choose_future(Some(self)).await != "leave" {
            return;
        }

        library::set_wanted(false);
        self.reset_search();
        if let Err(err) = self.settings().set_boolean("library-enabled", false) {
            warn!("Unable to disable the library: {err}");
        }
        self.imp().stack.set_visible_child_name("loading");
        // Stopping waits for the node to close its files; do it off the
        // main thread so the window stays responsive
        let _ = gio::spawn_blocking(library::stop_node).await;
        library::set_client(None);
        let imp = self.imp();
        imp.me.replace(None);
        self.reset_search();
        self.show_welcome();
    }

    /// The order of results the listener picked.
    fn order(&self) -> SearchOrder {
        if self.imp().order_dropdown.selected() == 1 {
            SearchOrder::Popular
        } else {
            SearchOrder::BestMatch
        }
    }

    fn search(&self) {
        self.imp().loaded.set(0);
        self.imp().has_more.set(false);
        self.imp().results_scroll.vadjustment().set_value(0.0);
        self.load_page(0);
    }

    /// The first page performs one bounded network search. Later pages read
    /// the collected results without starting another network search.
    fn load_page(&self, offset: u32) {
        let imp = self.imp();
        if offset > 0 && imp.page_loading.get() {
            return;
        }
        let query = imp.search_entry.text().trim().to_string();
        if query.is_empty() {
            self.reset_search();
            imp.empty_page.set_title(&i18n("Start Searching"));
            imp.empty_page.set_description(Some(&i18n(
                "Start a search to discover songs, albums and artists.",
            )));
            imp.search_stack.set_visible_child_name("empty");
            return;
        }
        let Some(client) = library::client() else {
            self.show_welcome();
            return;
        };
        let generation = imp.generation.get().wrapping_add(1);
        imp.generation.set(generation);
        imp.page_loading.set(true);
        imp.page_offset.set(offset);
        imp.page_spinner.set_visible(true);
        imp.retry_page_button.set_visible(false);
        imp.network_status.set_visible(offset == 0);
        if offset == 0 {
            self.clear_results();
            imp.search_stack.set_visible_child_name("results");
        }
        glib::spawn_future_local(clone!(
            #[weak(rename_to=this)]
            self,
            async move {
                if offset == 0 {
                    glib::timeout_future(NETWORK_SEARCH_DELAY).await;
                }
                if this.imp().generation.get() != generation {
                    return;
                }
                let result = client
                    .discover(&query, offset, offset == 0, this.order())
                    .await;
                if this.imp().generation.get() != generation {
                    return;
                }
                this.imp().network_status.set_visible(false);
                match result {
                    Ok(page) => {
                        this.show_page(&query, offset, &page.tracks, page.total);
                        if let Some(problem) = page.report.as_ref().and_then(search_problem) {
                            if page.total == 0 {
                                this.imp()
                                    .empty_page
                                    .set_title(&i18n("The Network Couldn’t Be Searched"));
                                this.imp().empty_page.set_description(Some(&problem));
                            } else {
                                this.toast(&problem);
                            }
                        }
                        this.finish_page_load(true);
                    }
                    Err(error) => {
                        this.toast(&error.to_string());
                        this.finish_page_load(false);
                    }
                }
            }
        ));
    }

    fn show_page(&self, query: &str, offset: u32, tracks: &[Track], total: u64) {
        let imp = self.imp();
        if offset == 0 {
            self.clear_results();
        }
        self.append_album_results(tracks);
        for track in tracks {
            self.append_result(track);
        }
        if offset == 0 && !tracks.is_empty() {
            // Reset the ListView anchor after replacing its model contents.
            // Setting only the adjustment before loading lets GTK restore the
            // previous end-of-list position when the new rows are allocated.
            imp.results_list
                .scroll_to(0, gtk::ListScrollFlags::NONE, None);
            if imp
                .album_results
                .get()
                .is_some_and(|model| model.n_items() > 0)
            {
                imp.albums_list
                    .scroll_to(0, gtk::ListScrollFlags::NONE, None);
            }
        }
        let loaded = if offset == 0 {
            tracks.len() as u32
        } else {
            offset + tracks.len() as u32
        };
        if !tracks.is_empty() || offset == 0 {
            imp.loaded.set(loaded);
        }
        imp.has_more
            .set(u64::from(loaded) < total && !tracks.is_empty());

        if loaded == 0 {
            if query.is_empty() {
                imp.empty_page.set_title(&i18n("Search Everyone’s Music"));
                imp.empty_page.set_description(Some(&i18n(
                    "Find new songs and albums. Music already on this computer is hidden from search.",
                )));
            } else {
                imp.empty_page.set_title(&i18n("No Songs Found"));
                imp.empty_page.set_description(Some(&i18n(
                    "Nobody who is online right now has songs matching all of these words",
                )));
            }
            imp.search_stack.set_visible_child_name("empty");
        } else {
            imp.search_stack.set_visible_child_name("results");
        }
    }

    pub(crate) fn album_track_row(&self, track: &Track, tracks: &[Track]) -> adw::ActionRow {
        let row = self.track_row(track, ListKind::Album);
        // The activation handler uses this immutable album snapshot, not a later search.
        let album = tracks.to_vec();
        let position = album.iter().position(|t| t.id == track.id).unwrap_or(0);
        row.connect_activated(clone!(
            #[weak(rename_to=panel)]
            self,
            move |_| {
                if let Some(win) = panel.window() {
                    win.preview_remote_album(&album, position);
                }
            }
        ));
        row
    }

    fn track_row(&self, track: &Track, kind: ListKind) -> adw::ActionRow {
        let mut details = Vec::new();
        if let Some(artist) = track.artist.as_deref() {
            details.push(artist.to_string());
        }
        if let Some(album) = track.album.as_deref().filter(|_| kind != ListKind::Album) {
            details.push(album.to_string());
        }
        details.push(utils::format_time((track.duration_ms / 1000) as i64));
        details.push(if track.mine {
            i18n("shared by you")
        } else {
            i18n_k(
                // Translators: `{name}` is replaced by the name a person chose
                // followed by a short code identifying them
                "shared by {name}",
                &[("name", &track.uploader)],
            )
        });
        // How popular it is: of two songs with the same name, people go
        // for the one more people have
        if track.sources > 1 {
            details.push(ni18n_f(
                "on {} computer",
                "on {} computers",
                track.sources,
                &[&track.sources.to_string()],
            ));
        }

        // Titles come from other people's files: always plain text, never
        // markup, and without a file extension if one was left in them
        let shown_title = AudioFormat::strip_extension(&track.title);
        let title = match track.track_number {
            Some(n) if kind == ListKind::Album => format!("{n}. {shown_title}"),
            _ => shown_title.to_string(),
        };
        let subtitle = details.join(" · ");
        let row = adw::ActionRow::builder().activatable(true).build();
        row.add_prefix(&track_text(&title, &format_label(track), &subtitle));
        // Prefixes are added from the right: the picture goes left of the text
        row.add_prefix(&thumbnail(track));
        row.upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "{title}, {}, {subtitle}",
                format_label(track)
            ))]);
        row.set_tooltip_text(Some(&i18n("Listen without adding to your playlist")));
        row.connect_activated(clone!(
            #[weak(rename_to = this)]
            self,
            #[strong]
            track,
            move |_| {
                if let Some(window) = this.window() {
                    if kind != ListKind::Album {
                        window.preview_remote_track(&track);
                    }
                }
            }
        ));

        let star = gtk::Button::builder()
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        set_star(&star, track.favorite);
        let mut stars = self.imp().stars.borrow_mut();
        // Rows of earlier searches are gone: forget their stars
        if stars.len() >= 256 {
            stars.retain(|(_, star)| star.upgrade().is_some());
        }
        stars.push((track.id, star.downgrade()));
        drop(stars);
        star.connect_clicked(clone!(
            #[weak(rename_to = this)]
            self,
            #[strong]
            track,
            move |star| {
                glib::spawn_future_local(clone!(
                    #[weak]
                    this,
                    #[weak]
                    star,
                    #[strong]
                    track,
                    async move { this.toggle_favorite(&track, &star).await }
                ));
            }
        ));
        row.add_suffix(&star);

        row
    }

    /// Puts a track in the library or takes it out. Taking it out waits a
    /// few seconds for the listener to change their mind (see
    /// [`Window::take_out_of_library`]).
    async fn toggle_favorite(&self, track: &Track, star: &gtk::Button) {
        let (Some(client), Some(window)) = (library::client(), self.window()) else {
            return;
        };
        // The button's icon holds the current state, which may have changed
        // since the row was built
        if star.icon_name().as_deref() == Some("hopea-starred-symbolic") {
            window.take_out_of_library(track.id, &track.title, track.mine);
            return;
        }
        star.set_sensitive(false);
        let result = client.set_favorite(track.id, true).await;
        star.set_sensitive(true);
        match result {
            Ok(updated) => {
                window.set_library_state(updated.id, updated.favorite);
                // The library is always in the playlist
                window.add_remote_track(&updated, false).await;
                window.offer_metadata(&updated);
                self.toast(&i18n_k(
                    // Translators: `{title}` is replaced by the title of a song
                    "Added “{title}” to your library and the playlist",
                    &[("title", &track.title)],
                ));
            }
            Err(err) => self.toast(&err.to_string()),
        }
    }

    async fn choose_uploads(&self) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&i18n("Supported audio files")));
        for ext in AudioFormat::all_extensions() {
            filter.add_suffix(ext);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .accept_label(i18n("_Share"))
            .filters(&filters)
            .default_filter(&filter)
            .modal(true)
            .title(i18n("Share Songs"))
            .build();

        let window = self.window();
        let Ok(files) = dialog.open_multiple_future(window.as_ref()).await else {
            return;
        };
        let files: Vec<gio::File> = files.iter::<gio::File>().flatten().collect();
        self.share_selected_files(files).await;
    }

    /// Shares every supported song in a folder and the folders inside it,
    /// such as the discs of an album.
    async fn choose_folder_upload(&self) {
        let dialog = gtk::FileDialog::builder()
            .accept_label(i18n("_Share"))
            .modal(true)
            .title(i18n("Share a Music Folder"))
            .build();
        let window = self.window();
        let Ok(folder) = dialog.select_folder_future(window.as_ref()).await else {
            return;
        };
        let files = match collect_audio_files(&folder).await {
            Ok(files) => files,
            Err(err) => {
                self.toast(&i18n_f("Unable to read the folder: {}", &[err.message()]));
                return;
            }
        };
        if files.is_empty() {
            self.toast(&i18n("There are no supported songs in this folder"));
            return;
        }
        self.share_selected_files(files).await;
    }

    /// Import is also available before joining, from the welcome page and
    /// the main menu. Use the usual join path so VPN preferences still apply.
    async fn share_selected_files(&self, files: Vec<gio::File>) {
        if files.is_empty() {
            return;
        }
        if self.imp().busy.get() {
            self.toast(&i18n("Wait for the current transfer to finish"));
            return;
        }
        if library::client().is_none() {
            self.join().await;
        } else if self.imp().me.borrow().is_none() {
            if let Some(client) = library::client() {
                self.finish_joining(client).await;
            }
        }
        if library::client().is_none() || self.imp().me.borrow().is_none() {
            return;
        }
        let Some(cover) = self.ask_cover(files.len()).await else {
            return;
        };
        self.upload_files(files, cover).await;
    }

    /// Shares files, with the cover art chosen for them; songs without
    /// one, and without a picture of their own, get their folder's.
    async fn upload_files(&self, files: Vec<gio::File>, cover: UploadCover) {
        if self.imp().busy.replace(true) {
            self.toast(&i18n("Wait for the current transfer to finish"));
            return;
        }
        let total = files.len();
        let (mut shared, mut duplicates, mut failed) = (0, 0, Vec::new());
        let mut folder_covers = HashMap::new();
        for (i, file) in files.iter().enumerate() {
            let batch = (total > 1).then_some((i + 1, total));
            let mut cover = cover.clone();
            if cover.cover.is_none() {
                cover.fallback = self.folder_cover(file, &mut folder_covers).await;
            }
            let result = self.upload(file, batch, &cover).await;
            if let Upload::Shared(ref track) | Upload::Duplicate(ref track) = result {
                if let Some(window) = self.window() {
                    window.add_shared_file(file, track.id);
                }
            }
            match result {
                Upload::Shared(track) if total == 1 => self.toast(&i18n_k(
                    // Translators: `{title}` is replaced by the title of a song
                    "Sharing “{title}”",
                    &[("title", &track.title)],
                )),
                Upload::Duplicate(track) if total == 1 => self.toast(&i18n_k(
                    // Translators: `{title}` is replaced by the title of a song
                    "You already share “{title}”",
                    &[("title", &track.title)],
                )),
                Upload::Failed(reason) if total == 1 => self.toast(&reason),
                Upload::Shared(_) => shared += 1,
                Upload::Duplicate(_) => duplicates += 1,
                Upload::Failed(reason) => failed.push(reason),
            }
        }
        self.imp().upload_revealer.set_reveal_child(false);
        self.imp().busy.set(false);

        if total > 1 {
            let mut summary = ni18n_f(
                "Sharing {} song",
                "Sharing {} songs",
                shared as u32,
                &[&shared.to_string()],
            );
            if duplicates > 0 {
                summary.push_str(&ni18n_f(
                    "; {} was already shared",
                    "; {} were already shared",
                    duplicates as u32,
                    &[&duplicates.to_string()],
                ));
            }
            if !failed.is_empty() {
                summary.push_str(&ni18n_f(
                    "; {} was refused",
                    "; {} were refused",
                    failed.len() as u32,
                    &[&failed.len().to_string()],
                ));
            }
            self.toast(&summary);
            if let Some(first) = failed.first() {
                self.toast(first);
            }
        }

        self.imp().search_entry.set_text("");
        self.imp().view_stack.set_visible_child_name("search");
        self.search();
    }

    /// Shares one file from this computer, as when its song is starred in
    /// the playlist, with its folder's picture if it has none of its own.
    pub async fn ensure_collection_sharing(&self) -> Result<(), String> {
        if !library::allowed(&self.settings()) {
            return Err(i18n("Connect to the VPN required by your sharing settings"));
        }
        if library::client().is_none() {
            self.join().await;
        }
        if self.imp().me.borrow().is_none() {
            if let Some(client) = library::client() {
                self.finish_joining(client).await;
            }
        }
        if self.imp().me.borrow().is_none() {
            return Err(i18n("Unable to join Shared Music"));
        }
        Ok(())
    }

    pub async fn share_file(&self, file: &gio::File) -> Result<Track, String> {
        if self.imp().busy.replace(true) {
            return Err(i18n("Wait for the current transfer to finish"));
        }
        let cover = UploadCover {
            cover: None,
            fallback: self.folder_cover(file, &mut HashMap::new()).await,
        };
        let result = self.upload(file, None, &cover).await;
        self.imp().busy.set(false);
        match result {
            Upload::Shared(track) | Upload::Duplicate(track) => Ok(track),
            Upload::Failed(reason) => Err(reason),
        }
    }

    /// Asks for cover art for songs about to be shared. `None` if the
    /// listener changed their mind.
    async fn ask_cover(&self, count: usize) -> Option<UploadCover> {
        let alert = adw::AlertDialog::builder()
            .heading(i18n("Add Cover Art?"))
            .body(ni18n_f(
                "Choose a picture for this song. Without one, it keeps its own picture, or the one in its folder.",
                "Choose a picture for these {} songs. Without one, each keeps its own picture, or the one in its folder.",
                count as u32,
                &[&count.to_string()],
            ))
            .close_response("cancel")
            .default_response("choose")
            .build();
        alert.add_response("cancel", &i18n("_Cancel"));
        alert.add_response("skip", &i18n("Share _Without"));
        alert.add_response("choose", &i18n("_Choose Picture…"));
        alert.set_response_appearance("choose", adw::ResponseAppearance::Suggested);
        match alert.choose_future(Some(self)).await.as_str() {
            "skip" => return Some(UploadCover::default()),
            "choose" => {}
            _ => return None,
        }

        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&i18n("Pictures")));
        for ext in ["png", "jpg", "jpeg"] {
            filter.add_suffix(ext);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .accept_label(i18n("_Use Picture"))
            .filters(&filters)
            .default_filter(&filter)
            .modal(true)
            .title(i18n("Choose Cover Art"))
            .build();
        let window = self.window();
        let file = dialog.open_future(window.as_ref()).await.ok()?;
        match send_picture(&file).await {
            Ok(cover) => Some(UploadCover {
                cover: Some(cover),
                fallback: None,
            }),
            Err(err) => {
                self.toast(&err);
                None
            }
        }
    }

    /// The picture in a file's folder, such as `cover.jpg`, sent for its
    /// upload, looked up once per folder.
    async fn folder_cover(
        &self,
        file: &gio::File,
        found: &mut HashMap<String, Option<String>>,
    ) -> Option<String> {
        let folder = file.parent()?.uri().to_string();
        if let Some(cover) = found.get(&folder) {
            return cover.clone();
        }
        let cover = match folder_picture(file).await {
            Some(picture) => send_picture(&picture)
                .await
                .inspect_err(|err| debug!("Not using the folder picture: {err}"))
                .ok(),
            None => None,
        };
        found.insert(folder, cover.clone());
        cover
    }

    /// Validates and shares one file; `batch` is its place in a batch.
    async fn upload(
        &self,
        file: &gio::File,
        batch: Option<(usize, usize)>,
        cover: &UploadCover,
    ) -> Upload {
        let (Some(client), Some(me)) = (library::client(), self.imp().me.borrow().clone()) else {
            return Upload::Failed(i18n("Not connected"));
        };

        let info = match file
            .query_info_future(
                "standard::display-name,standard::size",
                gio::FileQueryInfoFlags::NONE,
                glib::Priority::DEFAULT,
            )
            .await
        {
            Ok(info) => info,
            Err(err) => {
                return Upload::Failed(i18n_f("Unable to read the file: {}", &[err.message()]))
            }
        };
        let name = info.display_name().to_string();
        let size = info.size().max(0) as u64;

        if let Err(reason) = precheck(file, &name, size, me.max_upload_bytes).await {
            return Upload::Failed(format!("{name}: {reason}"));
        }

        let imp = self.imp();
        imp.upload_label.set_text(&match batch {
            Some((n, total)) => i18n_k(
                // Translators: `{n}` and `{total}` are numbers, `{name}` a file name
                "Sharing {n} of {total}: “{name}”…",
                &[
                    ("n", &n.to_string()),
                    ("total", &total.to_string()),
                    ("name", &name),
                ],
            ),
            None => i18n_k(
                // Translators: `{name}` is replaced by a file name
                "Checking and sharing “{name}”…",
                &[("name", &name)],
            ),
        });
        imp.upload_progress.set_fraction(0.0);
        imp.upload_revealer.set_reveal_child(true);

        let progress = imp.upload_progress.downgrade();
        let result = client
            .upload(file, &name, size, cover, move |sent| {
                if let Some(progress) = progress.upgrade() {
                    progress.set_fraction(sent as f64 / size.max(1) as f64);
                }
            })
            .await;
        if batch.is_none() {
            imp.upload_revealer.set_reveal_child(false);
        }

        if let Ok(response) = &result {
            crate::lyrics::publish_file(
                &client,
                response.track.clone(),
                Some(file.uri().to_string()),
            )
            .await;
        }
        match result {
            Ok(response) if response.duplicate => Upload::Duplicate(response.track),
            Ok(response) => {
                debug!("Shared track {}", response.track.id);
                Upload::Shared(response.track)
            }
            Err(err) => Upload::Failed(format!("{name}: {err}")),
        }
    }

    /// Shows an album's page: what is known here right away, then what
    /// other people's computers have.
    fn open_album(&self, album: String, artist: Option<String>) {
        let Some(client) = library::client() else {
            return;
        };
        let imp = self.imp();
        let generation = imp.album_generation.get() + 1;
        imp.album_generation.set(generation);
        imp.album.replace(Some((album.clone(), artist.clone())));
        imp.album_tracks.replace(Vec::new());
        imp.album_list.remove_all();
        imp.album_title.set_text(&album);
        imp.album_subtitle
            .set_text(artist.as_deref().unwrap_or_default());
        imp.album_page.set_title(&album);
        imp.album_stack.set_visible_child_name("loading");
        let album_content = self.window().map(|win| win.begin_remote_album(&album));

        glib::spawn_future_local(clone!(
            #[weak(rename_to = this)]
            self,
            #[strong]
            album_content,
            async move {
                for network in [false, true] {
                    let result = client.album(&album, artist.as_deref(), network).await;
                    if this.imp().album_generation.get() != generation {
                        return;
                    }
                    match result {
                        Ok(page) if !page.tracks.is_empty() || network => {
                            if let (Some(win), Some(content)) =
                                (this.window(), album_content.as_ref())
                            {
                                win.fill_remote_album(content, page.tracks.clone());
                            }
                            this.show_album(page.tracks);
                        }
                        Ok(_) => {}
                        Err(err) => {
                            this.toast(&err.to_string());
                            if network {
                                this.show_album(Vec::new());
                                if let (Some(win), Some(content)) =
                                    (this.window(), album_content.as_ref())
                                {
                                    win.fill_remote_album(content, Vec::new());
                                }
                            }
                        }
                    }
                }
            }
        ));
    }

    fn show_album(&self, tracks: Vec<Track>) {
        let imp = self.imp();
        imp.album_list.remove_all();
        for track in &tracks {
            imp.album_list.append(&self.album_track_row(track, &tracks));
        }
        let artist = imp
            .album
            .borrow()
            .as_ref()
            .and_then(|(_, a)| a.clone())
            .unwrap_or_default();
        let minutes = tracks.iter().map(|t| t.duration_ms).sum::<u64>() / 60_000;
        let count = ni18n_f(
            "{} song",
            "{} songs",
            tracks.len() as u32,
            &[&tracks.len().to_string()],
        );
        let length = ni18n_f(
            "{} minute",
            "{} minutes",
            minutes as u32,
            &[&minutes.to_string()],
        );
        imp.album_subtitle.set_text(
            &vec![artist, count, length]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
        );
        imp.album_stack
            .set_visible_child_name(if tracks.is_empty() {
                "empty"
            } else {
                "results"
            });
        imp.album_tracks.replace(tracks);
    }

    async fn play_all(&self, tracks: Vec<Track>) {
        if let Some(window) = self.window() {
            window.preview_remote_album(&tracks, 0);
            window.hide_library_if_overlaid();
        }
    }

    /// Downloads every song of the album shown, waits until each one is here
    /// and verified, and saves the files to a folder the listener picks.
    /// The songs also become favorites, so they stay in the app.
    pub(crate) async fn save_remote_album(&self, tracks: Vec<Track>) {
        let Some(first) = tracks.first() else {
            return;
        };
        let album = first.album.clone().unwrap_or_else(|| i18n("Album"));
        let artist = first.album_artist.clone().or_else(|| first.artist.clone());
        self.download_album_tracks(tracks, album, artist).await;
    }

    async fn download_album(&self) {
        let tracks = self.imp().album_tracks.borrow().clone();
        let Some((album, artist)) = self.imp().album.borrow().clone() else {
            return;
        };
        self.download_album_tracks(tracks, album, artist).await;
    }

    async fn download_album_tracks(
        &self,
        tracks: Vec<Track>,
        album: String,
        artist: Option<String>,
    ) {
        let Some(client) = library::client() else {
            return;
        };
        let imp = self.imp();
        if tracks.is_empty() {
            return;
        }
        if imp.busy.get() {
            self.toast(&i18n("Wait for the current transfer to finish"));
            return;
        }

        let dialog = gtk::FileDialog::builder()
            .accept_label(i18n("_Save Here"))
            .modal(true)
            .title(i18n("Download the Album To"))
            .build();
        if let Some(music) = glib::user_special_dir(glib::UserDirectory::Music) {
            dialog.set_initial_folder(Some(&gio::File::for_path(music)));
        }
        let window = self.window();
        let Ok(parent) = dialog.select_folder_future(window.as_ref()).await else {
            return;
        };
        let folder_name = match artist.as_deref() {
            Some(artist) => safe_file_name(&format!("{artist} - {album}")),
            None => safe_file_name(&album),
        };
        let folder = parent.child(folder_name);
        if let Err(err) = folder.make_directory_with_parents(gio::Cancellable::NONE) {
            if !err.matches(gio::IOErrorEnum::Exists) {
                self.toast(&i18n_f("Unable to create the folder: {}", &[err.message()]));
                return;
            }
        }

        imp.busy.set(true);
        imp.upload_progress.set_fraction(0.0);
        imp.upload_revealer.set_reveal_child(true);
        let label = |text: String| imp.upload_label.set_text(&text);
        label(i18n_k(
            // Translators: `{album}` is replaced by the title of an album
            "Downloading “{album}”…",
            &[("album", &album)],
        ));

        // Ask for all of them at once, so they download side by side
        for track in &tracks {
            if let Err(err) = client.prefetch(track.id).await {
                warn!("Unable to fetch track {}: {err}", track.id);
            }
        }

        let mut collection = library::collections::Album::new();
        collection.name = album.clone();
        let total = tracks.len();
        let mut saved = 0;
        let mut failed = Vec::new();
        for (i, track) in tracks.iter().enumerate() {
            label(i18n_k(
                // Translators: `{n}` and `{total}` are numbers, `{title}` a song
                "Downloading {n} of {total}: “{title}”…",
                &[
                    ("n", &(i + 1).to_string()),
                    ("total", &total.to_string()),
                    ("title", &track.title),
                ],
            ));
            if let Err(err) = wait_until_local(&client, track.id).await {
                failed.push(format!("{}: {err}", track.title));
                continue;
            }
            let number = track
                .track_number
                .map(|n| format!("{n:02} - "))
                .unwrap_or_default();
            let name = safe_file_name(&format!(
                "{number}{}.{}",
                track.title,
                track.format.canonical_extension()
            ));
            let file = folder.child(name);
            match client.save_file(track.id, &file).await {
                Ok(()) => {
                    let mut upload_cover = UploadCover::default();
                    if track.has_cover {
                        if let Ok(bytes) = client.cover(track.id).await {
                            if collection.cover.is_none() {
                                collection.cover =
                                    library::collections::normalize_cover(&bytes).ok();
                            }
                            if let Some(path) = file.path() {
                                let _ = library::import_history::save_cover(&path, &bytes).await;
                            }
                            upload_cover.cover = client.add_cover(bytes).await.ok();
                        }
                    }
                    let shared = match self.upload(&file, None, &upload_cover).await {
                        Upload::Shared(t) | Upload::Duplicate(t) => t,
                        Upload::Failed(reason) => {
                            failed.push(reason);
                            track.clone()
                        }
                    };
                    library::shared_files::remember(&file.uri(), shared.id);
                    collection.tracks.push(library::collections::Entry {
                        title: track.title.clone(),
                        artist: track.artist.clone(),
                        node_id: Some(shared.id),
                        sha256: Some(shared.sha256),
                        local_uri: Some(file.uri().to_string()),
                        ..Default::default()
                    });
                    saved += 1;
                }
                Err(err) => failed.push(format!("{}: {err}", track.title)),
            }
            imp.upload_progress
                .set_fraction((i + 1) as f64 / total as f64);
        }
        // The cover art goes with it, as music players expect it
        if let Some(track) = tracks.iter().find(|t| t.has_cover) {
            if let Ok(jpeg) = client.cover(track.id).await {
                let cover = folder.child("cover.jpg");
                if let Err((_, err)) = cover
                    .replace_contents_future(jpeg.to_vec(), None, false, gio::FileCreateFlags::NONE)
                    .await
                {
                    debug!("Unable to save the album cover: {err}");
                }
            }
        }
        if !collection.tracks.is_empty() {
            if let Err(reason) = library::collections::save(collection).await {
                failed.push(reason);
            }
        }
        imp.upload_revealer.set_reveal_child(false);
        imp.busy.set(false);

        self.toast(&ni18n_f(
            "Saved {} song to the album folder",
            "Saved {} songs to the album folder",
            saved as u32,
            &[&saved.to_string()],
        ));
        if let Some(first) = failed.first() {
            self.toast(first);
        }
    }
}

/// Sends a picture for the uploads that follow (see [`UploadCover`]).
async fn send_picture(file: &gio::File) -> Result<String, String> {
    let client = library::client().ok_or_else(|| i18n("Not connected"))?;
    let bytes = crate::utils::read_file_limited(file, MAX_COVER_FILE_BYTES)
        .await
        .map_err(|err| err.message().to_string())?;
    if bytes.len() > MAX_COVER_FILE_BYTES {
        return Err(i18n("The picture is too large"));
    }
    client
        .add_cover(glib::Bytes::from_owned(bytes))
        .await
        .map_err(|err| err.to_string())
}

/// A small picture of a track's cover art, fetched when its row is first
/// shown (see [`library::thumbnails`]).
fn thumbnail(track: &Track) -> gtk::Box {
    sized_thumbnail(track, 40)
}

fn sized_thumbnail(track: &Track, size: i32) -> gtk::Box {
    let frame = gtk::Box::builder()
        .width_request(size)
        .height_request(size)
        .valign(gtk::Align::Center)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["track-thumbnail"])
        .build();
    let placeholder = gtk::Image::builder()
        .pixel_size(size)
        .hexpand(true)
        .halign(gtk::Align::Center)
        .build();
    crate::artwork::placeholder(&placeholder);
    frame.append(&placeholder);
    if !track.has_cover {
        return frame;
    }
    let show = move |frame: &gtk::Box, texture: &gdk::Texture| {
        while let Some(child) = frame.first_child() {
            frame.remove(&child);
        }
        frame.append(
            &gtk::Image::builder()
                .paintable(texture)
                .pixel_size(size)
                .build(),
        );
    };
    if let Some(texture) = library::thumbnails::cached(track.id) {
        show(&frame, &texture);
        return frame;
    }
    let id = track.id;
    let handler = Rc::new(RefCell::new(None));
    let mapped = frame.connect_map(clone!(
        #[strong]
        handler,
        move |frame| {
            if let Some(handler) = handler.borrow_mut().take() {
                frame.disconnect(handler);
            }
            let frame = frame.downgrade();
            glib::spawn_future_local(async move {
                if let Some(texture) = library::thumbnails::load(id).await {
                    if let Some(frame) = frame.upgrade() {
                        show(&frame, &texture);
                    }
                }
            });
        }
    ));
    handler.replace(Some(mapped));
    frame
}

/// How sharing one file went.
enum Upload {
    Shared(Track),
    Duplicate(Track),
    Failed(String),
}

/// Whether the connection hides this computer's address, and in words what
/// goes through what, with what to do if it doesn't.
fn protection_text(status: &library::protection::Status) -> (bool, String) {
    use library::protection::Protection;

    match &status.protection {
        Protection::Ipv4Only(through) => (true, i18n_k(
            "VPN connected through {names}. Hopea uses IPv4 for sharing.",
            &[("names", &through.join(", "))],
        )),
        Protection::Partial { tunnels, .. } if status.protection.sharing_ipv4_only() == Some(true) => (true, i18n_k(
            "VPN connected through {names}. With VPN-only sharing enabled, Hopea uses IPv4 and disables IPv6 for sharing.",
            &[("names", &tunnels.join(", "))],
        )),
        Protection::Protected(through) => (
            true,
            i18n_k(
                // Translators: `{names}` is replaced by network interfaces or tools, such as "wg0"
                "Through {names}: others see the VPN’s address, not yours",
                &[("names", &through.join(", "))],
            ),
        ),
        Protection::Offline => (false, i18n("Not connected to the internet")),
        Protection::Partial { tunnels, exposed_families } => (
            false,
            i18n_k(
                "VPN is active through {names}, but {families} bypasses it. Enable protection for {families} in your VPN’s TUN settings. Sharing stays paused while “Share Only Through a VPN” is enabled.",
                &[("names", &tunnels.join(", ")), ("families", &exposed_families.join(", "))],
            ),
        ),
        Protection::Exposed(_) if status.tools.contains(&"Tor") => (
            false,
            i18n(
                "Tor is on, but this app’s traffic doesn’t go through it, and can’t: BitTorrent needs UDP, which Tor doesn’t carry, and the Tor Project asks people not to share files over Tor. Use a VPN instead.",
            ),
        ),
        Protection::Exposed(_) if !status.tools.is_empty() => (
            false,
            i18n_k(
                // Translators: `{tools}` is replaced by program names, such as "Clash"
                "{tools} is on, but this app’s traffic doesn’t go through it: others see your address. Turn on its TUN mode (in Clash, “TUN Mode”; in v2rayN, sing-box, NekoBox or Hiddify, “TUN” or “VPN mode”) so that it carries all traffic, or use a VPN.",
                &[("tools", &status.tools.join(", "))],
            ),
        ),
        Protection::Exposed(_) if status.system_proxy => (
            false,
            i18n(
                "A proxy is set, but this app’s traffic doesn’t go through proxies: others see your address. Use a VPN, or a proxy app’s TUN mode.",
            ),
        ),
        Protection::Exposed(_) => (false, i18n("No VPN is on: others see your address")),
    }
}

/// How long typing must pause before the network is searched
const NETWORK_SEARCH_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

/// Search result rows shown at most

/// What kept a network search from finding anything, and what may help, if
/// something did. Nobody having matching songs is not a problem.
fn search_problem(report: &SearchReport) -> Option<String> {
    if report.dht == DhtState::NoContact && report.nodes_answered == 0 {
        return Some(i18n(
            "This computer can’t reach the BitTorrent network (DHT), which is how it finds others. Something blocks UDP traffic: a firewall, a VPN, or the network itself (as in some offices, schools and hotels). Try another network, or allow UDP for the app. Right after the app starts this can also take a minute.",
        ));
    }
    if report.nodes_asked > 0 && report.nodes_answered == 0 {
        let count = report.nodes_asked.to_string();
        return Some(match report.failure {
            Some(SearchFailure::Timeout) => ni18n_f(
                "{} potential source was found but didn’t answer in time. Its connection may be slow or busy: try again in a moment.",
                "{} potential sources were found but none answered in time. Their connections may be slow or busy: try again in a moment.",
                report.nodes_asked,
                &[&count],
            ),
            Some(SearchFailure::BadAnswer) => ni18n_f(
                "{} potential source sent an answer this version can’t read. One of you may need to update the app.",
                "{} potential sources sent answers this version can’t read. Some of you may need to update the app.",
                report.nodes_asked,
                &[&count],
            ),
            _ => ni18n_f(
                "{} potential source couldn’t be reached. Check that both apps are sharing and the local network allows devices to connect. Across the internet, a reachable source or relay is needed.",
                "{} potential sources couldn’t be reached. Check that both apps are sharing and the local network allows devices to connect. Across the internet, a reachable source or relay is needed.",
                report.nodes_asked,
                &[&count],
            ),
        });
    }
    if report.timed_out && report.nodes_answered == 0 {
        return Some(i18n(
            "The search took too long and nobody answered in time. Your connection may be slow: try again.",
        ));
    }
    None
}

/// Largest picture sent as cover art
const MAX_COVER_FILE_BYTES: usize = 16 * 1024 * 1024;
/// Names of the picture of an album folder, in order of preference
const FOLDER_PICTURES: [&str; 4] = ["cover", "folder", "front", "album"];

/// The picture in the folder of `file`, such as `cover.jpg`, if any.
async fn folder_picture(file: &gio::File) -> Option<gio::File> {
    let dir = file.parent()?;
    let children = dir
        .enumerate_children_future(
            "standard::name,standard::type",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            glib::Priority::DEFAULT,
        )
        .await
        .ok()?;
    let mut found: Option<(usize, gio::File)> = None;
    while let Ok(infos) = children
        .next_files_future(64, glib::Priority::DEFAULT)
        .await
    {
        if infos.is_empty() {
            break;
        }
        for info in infos {
            if info.file_type() != gio::FileType::Regular {
                continue;
            }
            let name = info.name().to_string_lossy().to_lowercase();
            let Some((stem, ext)) = name.rsplit_once('.') else {
                continue;
            };
            if !matches!(ext, "jpg" | "jpeg" | "png") {
                continue;
            }
            if let Some(rank) = FOLDER_PICTURES.iter().position(|p| *p == stem) {
                if found.as_ref().is_none_or(|(r, _)| rank < *r) {
                    found = Some((rank, dir.child(info.name())));
                }
            }
        }
    }
    found.map(|(_, f)| f)
}

/// Songs taken from one folder, at most
const MAX_FOLDER_FILES: usize = 500;
/// How deep folders are looked into, for albums split into discs
const MAX_FOLDER_DEPTH: usize = 3;
/// How long a download may take before giving up on it
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Every favorite, all pages of them.
async fn fetch_favorites(client: &LibraryClient) -> Result<Vec<Track>, library::ClientError> {
    let mut offset = 0;
    let mut tracks = Vec::new();
    loop {
        let page = client.favorites(offset).await?;
        let n = page.tracks.len() as u32;
        tracks.extend(page.tracks);
        offset += n;
        if n == 0 || u64::from(offset) >= page.total {
            return Ok(tracks);
        }
    }
}

/// Waits until the node has a song in full and verified.
async fn wait_until_local(client: &LibraryClient, id: i64) -> Result<(), String> {
    let start = std::time::Instant::now();
    loop {
        match client.track(id).await {
            Ok(track) if track.local => return Ok(()),
            Ok(_) => {}
            Err(err) => return Err(err.to_string()),
        }
        if start.elapsed() > DOWNLOAD_TIMEOUT {
            return Err(i18n("nobody who has it sent it in time"));
        }
        glib::timeout_future(std::time::Duration::from_secs(1)).await;
    }
}

/// The supported songs in `folder` and the folders inside it, sorted by
/// path so that an album keeps its order.
async fn collect_audio_files(folder: &gio::File) -> Result<Vec<gio::File>, glib::Error> {
    let mut found = Vec::new();
    let mut pending = vec![(folder.clone(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let children = dir
            .enumerate_children_future(
                "standard::name,standard::type",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                glib::Priority::DEFAULT,
            )
            .await?;
        loop {
            let infos = children
                .next_files_future(64, glib::Priority::DEFAULT)
                .await?;
            if infos.is_empty() {
                break;
            }
            for info in infos {
                let child = dir.child(info.name());
                match info.file_type() {
                    gio::FileType::Directory if depth < MAX_FOLDER_DEPTH => {
                        pending.push((child, depth + 1))
                    }
                    gio::FileType::Regular
                        if AudioFormat::from_file_name(&info.name().to_string_lossy())
                            .is_some() =>
                    {
                        found.push(child)
                    }
                    _ => {}
                }
                if found.len() >= MAX_FOLDER_FILES {
                    break;
                }
            }
        }
    }
    found.sort_by_key(|f| f.path());
    found.truncate(MAX_FOLDER_FILES);
    Ok(found)
}

/// A file name made from text other people wrote: no path separators,
/// control characters or characters other systems reject, not hidden, and
/// not too long.
pub(crate) fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    let short: String = trimmed.chars().take(120).collect();
    if short.is_empty() {
        "Untitled".to_string()
    } else {
        short
    }
}

/// What a row shows of a track: the title, and under it the format in an
/// accent coloured frame, then the details.
fn track_text(title: &str, format: &str, subtitle: &str) -> gtk::Box {
    let title = gtk::Label::builder()
        .label(title)
        .use_markup(false)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let badge = gtk::Label::builder()
        .label(format)
        .use_markup(false)
        .valign(gtk::Align::Center)
        .css_classes(["format-badge"])
        .build();
    let subtitle = gtk::Label::builder()
        .label(subtitle)
        .use_markup(false)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["dim-label", "caption"])
        .build();
    let line = gtk::Box::builder().spacing(6).build();
    line.append(&badge);
    line.append(&subtitle);
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    text.append(&title);
    text.append(&line);
    text
}

/// The format shown for a track: the codec for MPEG-4 files, which can hold
/// either AAC or ALAC, the container otherwise.
fn format_label(track: &Track) -> String {
    match track.format {
        AudioFormat::Flac => "FLAC".to_string(),
        AudioFormat::Mp3 => "MP3".to_string(),
        AudioFormat::Mp4 => track.codec.to_uppercase(),
        AudioFormat::Ogg => "OGG".to_string(),
        AudioFormat::Wav => "WAV".to_string(),
    }
}

fn set_star(button: &gtk::Button, favorite: bool) {
    if favorite {
        button.set_icon_name("hopea-starred-symbolic");
        button.set_tooltip_text(Some(&i18n("Remove From Favorites")));
    } else {
        button.set_icon_name("hopea-non-starred-symbolic");
        button.set_tooltip_text(Some(&i18n("Add to Favorites")));
    }
}

/// Quick local checks so obviously wrong files are explained right away.
/// The node repeats these and does the full validation.
async fn precheck(file: &gio::File, name: &str, size: u64, max: u64) -> Result<(), String> {
    let Some(format) = AudioFormat::from_file_name(name) else {
        let accepted: Vec<String> = AudioFormat::all_extensions()
            .map(|e| format!(".{e}"))
            .collect();
        return Err(i18n_f(
            "This type of file is not accepted. Accepted types: {}",
            &[&accepted.join(", ")],
        ));
    };

    if size > max {
        return Err(i18n_f(
            "The file is larger than the {} MiB limit",
            &[&(max / (1024 * 1024)).to_string()],
        ));
    }

    let header = async {
        let stream = file.read_future(glib::Priority::DEFAULT).await?;
        let mut header = Vec::with_capacity(SNIFF_LEN);
        while header.len() < SNIFF_LEN {
            let chunk = stream
                .read_bytes_future(SNIFF_LEN - header.len(), glib::Priority::DEFAULT)
                .await?;
            if chunk.is_empty() {
                break;
            }
            header.extend_from_slice(&chunk);
        }
        Ok::<_, glib::Error>(header)
    }
    .await
    .map_err(|err| err.message().to_string())?;

    match AudioFormat::sniff(&header) {
        Some(actual) if actual == format => Ok(()),
        Some(actual) => Err(i18n_f(
            "The file name does not match its contents, which are {}",
            &[actual.display_name()],
        )),
        None => Err(match describe_non_audio(&header) {
            Some(what) => i18n_f("This is not an audio file; it looks like {}", &[what]),
            None => i18n("This is not an audio file"),
        }),
    }
}
