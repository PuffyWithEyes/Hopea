// SPDX-FileCopyrightText: 2022  Emmanuele Bassi
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::subclass::prelude::*;
use gtk::{gio, glib, prelude::*, CompositeTemplate};
use std::cell::{Cell, OnceCell, RefCell};

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/io/hopea/Hopea/playlist-view.ui")]
    pub struct PlaylistView {
        #[template_child]
        pub navigation: TemplateChild<adw::NavigationView>,
        #[template_child]
        pub playlist_page: TemplateChild<adw::NavigationPage>,
        #[template_child]
        pub albums_scroll: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub albums_list: TemplateChild<gtk::ListView>,
        #[template_child]
        pub new_album_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub open_album_button: TemplateChild<gtk::Button>,
        pub albums: OnceCell<gio::ListStore>,
        pub albums_monitor: RefCell<Option<gio::FileMonitor>>,
        pub album_revision: Cell<u64>,
        pub album_refresh: RefCell<Option<glib::SourceId>>,
        #[template_child]
        pub playlist_overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub playlist_scroll: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub back_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub queue_view: TemplateChild<gtk::ListView>,
        #[template_child]
        pub queue_length_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub queue_actionbar: TemplateChild<gtk::ActionBar>,
        #[template_child]
        pub queue_select_all_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub queue_remove_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub queue_selected_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub playlist_progress: TemplateChild<gtk::ProgressBar>,
        #[template_child]
        pub playlist_searchbar: TemplateChild<gtk::SearchBar>,
        #[template_child]
        pub playlist_searchentry: TemplateChild<gtk::SearchEntry>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlaylistView {
        const NAME: &'static str = "HopeaPlaylistView";
        type Type = super::PlaylistView;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);

            klass.set_layout_manager_type::<gtk::BinLayout>();
            klass.set_css_name("playlistview");
            klass.set_accessible_role(gtk::AccessibleRole::Group);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PlaylistView {
        fn dispose(&self) {
            if let Some(monitor) = self.albums_monitor.borrow_mut().take() {
                monitor.cancel();
            }
            if let Some(source) = self.album_refresh.borrow_mut().take() {
                source.remove();
            }
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            self.obj().setup_searchbar();
        }
    }

    impl WidgetImpl for PlaylistView {}
}

glib::wrapper! {
    pub struct PlaylistView(ObjectSubclass<imp::PlaylistView>)
        @extends gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible,
                    gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for PlaylistView {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl PlaylistView {
    pub fn albums(&self) -> gio::ListStore {
        self.imp()
            .albums
            .get_or_init(gio::ListStore::new::<glib::BoxedAnyObject>)
            .clone()
    }

    pub fn albums_list(&self) -> gtk::ListView {
        self.imp().albums_list.get()
    }

    pub fn albums_scroll(&self) -> gtk::ScrolledWindow {
        self.imp().albums_scroll.get()
    }

    pub fn new_album_button(&self) -> gtk::Button {
        self.imp().new_album_button.get()
    }

    pub fn open_album_button(&self) -> gtk::Button {
        self.imp().open_album_button.get()
    }

    pub fn monitor_albums(&self, monitor: gio::FileMonitor) {
        if let Some(previous) = self.imp().albums_monitor.replace(Some(monitor)) {
            previous.cancel();
        }
    }

    pub fn next_album_revision(&self) -> u64 {
        let revision = self.imp().album_revision.get().wrapping_add(1);
        self.imp().album_revision.set(revision);
        revision
    }

    pub fn album_revision(&self) -> u64 {
        self.imp().album_revision.get()
    }

    pub fn queue_album_refresh(&self, refresh: impl FnOnce() + 'static) {
        if let Some(source) = self.imp().album_refresh.borrow_mut().take() {
            source.remove();
        }
        let weak = self.downgrade();
        let source =
            glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                view.imp().album_refresh.borrow_mut().take();
                refresh();
            });
        self.imp().album_refresh.replace(Some(source));
    }

    #[cfg(test)]
    pub fn visible_page(&self) -> Option<adw::NavigationPage> {
        self.imp().navigation.visible_page()
    }

    pub fn show_page(&self, title: &str, content: &impl IsA<gtk::Widget>) -> adw::NavigationPage {
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        header.set_show_end_title_buttons(false);
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(content));
        let page = adw::NavigationPage::new(&toolbar, title);
        self.imp().navigation.push(&page);
        page
    }

    pub fn show_playlist(&self) {
        self.imp()
            .navigation
            .pop_to_page(&self.imp().playlist_page.get());
    }

    pub fn new() -> Self {
        Self::default()
    }

    fn setup_searchbar(&self) {
        let entry = self.imp().playlist_searchentry.get();
        self.imp().playlist_searchbar.connect_entry(&entry);
    }

    pub fn back_button(&self) -> gtk::Button {
        self.imp().back_button.get()
    }

    pub fn queue_actionbar(&self) -> gtk::ActionBar {
        self.imp().queue_actionbar.get()
    }

    pub fn queue_remove_button(&self) -> gtk::Button {
        self.imp().queue_remove_button.get()
    }

    pub fn queue_select_all_button(&self) -> gtk::Button {
        self.imp().queue_select_all_button.get()
    }

    pub fn queue_selected_label(&self) -> gtk::Label {
        self.imp().queue_selected_label.get()
    }

    pub fn drag_overlay(&self) -> gtk::Overlay {
        self.imp().playlist_overlay.get()
    }

    pub fn scroll(&self) -> gtk::ScrolledWindow {
        self.imp().playlist_scroll.get()
    }

    pub fn queue_view(&self) -> gtk::ListView {
        self.imp().queue_view.get()
    }

    pub fn queue_length_label(&self) -> gtk::Label {
        self.imp().queue_length_label.get()
    }

    pub fn playlist_searchbar(&self) -> gtk::SearchBar {
        self.imp().playlist_searchbar.get()
    }

    pub fn playlist_searchentry(&self) -> gtk::SearchEntry {
        self.imp().playlist_searchentry.get()
    }

    pub fn begin_loading(&self) {
        self.imp().playlist_progress.set_fraction(0.0);
        self.imp().playlist_progress.set_visible(true);
    }

    pub fn end_loading(&self) {
        self.imp().playlist_progress.set_visible(false);
    }

    pub fn update_loading(&self, cur: u32, max: u32) {
        let step = cur as f64 / max as f64;
        self.imp().playlist_progress.set_fraction(step);
    }

    pub fn set_search(&self, search: bool) {
        self.imp().playlist_searchbar.set_search_mode(search);
    }
}
