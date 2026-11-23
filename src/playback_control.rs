// SPDX-FileCopyrightText: 2022  Emmanuele Bassi
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::subclass::prelude::*;
use gtk::{gio, glib, prelude::*, CompositeTemplate};

use crate::{audio::RepeatMode, i18n::i18n, volume_control::VolumeControl};

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/io/hopea/Hopea/playback-control.ui")]
    pub struct PlaybackControl {
        // Template widgets
        #[template_child]
        pub start_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub center_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub end_box: TemplateChild<gtk::Box>,

        #[template_child]
        pub previous_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub play_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub next_button: TemplateChild<gtk::Button>,

        #[template_child]
        pub volume_control: TemplateChild<VolumeControl>,

        #[template_child]
        pub playlist_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub shuffle_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub favorite_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub repeat_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub menu_button: TemplateChild<gtk::MenuButton>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlaybackControl {
        const NAME: &'static str = "HopeaPlaybackControl";
        type Type = super::PlaybackControl;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);

            klass.set_layout_manager_type::<gtk::BinLayout>();
            klass.set_css_name("playbackcontrol");
            klass.set_accessible_role(gtk::AccessibleRole::Group);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            VolumeControl::static_type();
            obj.init_template();
        }
    }

    impl ObjectImpl for PlaybackControl {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            self.menu_button.set_primary(!cfg!(target_os = "android"));
            // Android opens My Music from the top header, next to Settings.
            self.playlist_button
                .set_visible(!cfg!(target_os = "android"));
        }
    }

    impl WidgetImpl for PlaybackControl {}
}

glib::wrapper! {
    pub struct PlaybackControl(ObjectSubclass<imp::PlaybackControl>)
        @extends gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible,
                    gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for PlaybackControl {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl PlaybackControl {
    pub fn set_audition(&self, active: bool) {
        // Keep the transport and volume; hide library/menu controls.
        if let Some(controls) = self.imp().center_box.last_child() {
            controls.set_visible(!active);
        }
    }
    pub fn new() -> Self {
        Self::default()
    }

    pub fn play_button(&self) -> gtk::Button {
        self.imp().play_button.get()
    }

    pub fn repeat_button(&self) -> gtk::Button {
        self.imp().repeat_button.get()
    }

    /// Shows whether the current song is in the library; `None`, when
    /// nothing plays, makes the star unavailable.
    pub fn set_favorite_state(&self, favorite: Option<bool>) {
        let button = &self.imp().favorite_button;
        button.set_sensitive(favorite.is_some());
        if favorite == Some(true) {
            button.add_css_class("suggested-action");
            button.set_icon_name("hopea-starred-symbolic");
            button.set_tooltip_text(Some(&i18n("Remove From Your Library")));
        } else {
            button.remove_css_class("suggested-action");
            button.set_icon_name("hopea-non-starred-symbolic");
            button.set_tooltip_text(Some(&i18n("Keep and Share")));
        }
    }

    pub fn volume_control(&self) -> VolumeControl {
        self.imp().volume_control.get()
    }

    pub fn set_repeat_mode(&self, repeat_mode: RepeatMode) {
        let repeat_button = self.imp().repeat_button.get();
        match repeat_mode {
            RepeatMode::Consecutive => {
                repeat_button.set_icon_name("hopea-media-playlist-consecutive-symbolic");
                repeat_button.set_tooltip_text(Some(&i18n("Do Not Repeat")));
            }
            RepeatMode::RepeatAll => {
                repeat_button.set_icon_name("hopea-media-playlist-repeat-symbolic");
                repeat_button.set_tooltip_text(Some(&i18n("Repeat All Songs")));
            }
            RepeatMode::RepeatOne => {
                repeat_button.set_icon_name("hopea-media-playlist-repeat-song-symbolic");
                repeat_button.set_tooltip_text(Some(&i18n("Repeat the Current Song")));
            }
        }
    }
}
