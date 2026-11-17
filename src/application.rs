// SPDX-FileCopyrightText: 2022  Emmanuele Bassi
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use adw::prelude::AdwDialogExt;
use adw::subclass::prelude::*;
#[cfg(target_os = "linux")]
use ashpd::{desktop::background::Background, WindowIdentifier};
use async_channel::Receiver;
use glib::clone;
use gtk::{gio, glib, prelude::*};
use log::{debug, warn};

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "windows",
    target_os = "macos"
))]
use crate::tray::{Tray, TrayEvent, TrayState};
use crate::{
    audio::AudioPlayer,
    config::{APPLICATION_ICON, APPLICATION_ID, VERSION},
    i18n::i18n,
    library, utils,
    window::Window,
};

/// How often the VPN is checked, between the network change notices
const GUARD_INTERVAL: Duration = Duration::from_secs(2);
/// How long to wait before trying to join again after it failed
const JOIN_RETRY_DELAY: Duration = Duration::from_secs(60);

pub enum ApplicationAction {
    Present,
}

mod imp {
    use super::*;

    #[derive(Debug)]
    pub struct Application {
        pub player: Rc<AudioPlayer>,
        pub receiver: RefCell<Option<Receiver<ApplicationAction>>>,
        pub background_hold: RefCell<Option<gio::ApplicationHoldGuard>>,
        /// Keeps the app running while it shares music, with the window closed
        pub sharing_hold: RefCell<Option<gio::ApplicationHoldGuard>>,
        #[cfg(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly",
            target_os = "windows",
            target_os = "macos"
        ))]
        pub tray: RefCell<Option<Tray>>,
        pub settings: gio::Settings,
        /// Joining or leaving the network is under way
        pub switching: Cell<bool>,
        /// When joining last failed, to not retry it every moment
        pub join_failed_at: Cell<Option<Instant>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Application {
        const NAME: &'static str = "HopeaApplication";
        type Type = super::Application;
        type ParentType = adw::Application;

        fn new() -> Self {
            let (sender, r) = async_channel::unbounded();
            let receiver = RefCell::new(Some(r));

            Self {
                player: AudioPlayer::new(sender),
                receiver,
                background_hold: RefCell::default(),
                sharing_hold: RefCell::default(),
                #[cfg(any(
                    target_os = "linux",
                    target_os = "freebsd",
                    target_os = "openbsd",
                    target_os = "netbsd",
                    target_os = "dragonfly",
                    target_os = "windows",
                    target_os = "macos"
                ))]
                tray: RefCell::default(),
                settings: utils::settings_manager(),
                switching: Cell::default(),
                join_failed_at: Cell::default(),
            }
        }
    }

    impl ObjectImpl for Application {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();
            obj.setup_channel();
            obj.setup_gactions();
            obj.setup_settings();

            obj.set_accels_for_action("app.quit", &["<primary>q"]);

            obj.set_accels_for_action("queue.add-song", &["<primary>s"]);
            // Ctrl+A belongs to the focused text editor, including search.
            obj.set_accels_for_action("queue.add-folder", &["<primary><shift>o"]);
            obj.set_accels_for_action("queue.clear", &["<primary>L"]);
            obj.set_accels_for_action("queue.toggle", &["F9"]);
            obj.set_accels_for_action("queue.search", &["<primary>F"]);
            obj.set_accels_for_action("queue.shuffle", &["<primary>r"]);
            obj.set_accels_for_action("win.shared-library", &["<primary>u"]);

            obj.set_accels_for_action("win.seek-backwards", &["<primary>Left"]);
            obj.set_accels_for_action("win.seek-forward", &["<primary>Right"]);
            obj.set_accels_for_action("win.previous", &["<primary>b"]);
            obj.set_accels_for_action("win.next", &["<primary>n"]);
            obj.set_accels_for_action("win.play", &["<primary>p"]);
            obj.set_accels_for_action("win.copy", &["<primary>c"]);
        }
    }

    impl ApplicationImpl for Application {
        fn startup(&self) {
            self.parent_startup();

            if let Some(display) = gtk::gdk::Display::default() {
                gtk::IconTheme::for_display(&display).add_resource_path("/io/hopea/Hopea/icons");
            }
            gtk::Window::set_default_icon_name(APPLICATION_ICON);
            crate::accent::init();
            crate::gaming::init();
            #[cfg(any(
                target_os = "linux",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly",
                target_os = "windows",
                target_os = "macos"
            ))]
            self.obj().setup_tray();

            // Once the listener joined, the app joins the network as it
            // starts, unless they chose not to or this is a phone
            if self.settings.boolean("library-enabled") && library::joins_at_start(&self.settings) {
                library::set_wanted(true);
            }
            self.obj().watch_connection();
            if library::wanted() && library::allowed(&self.settings) {
                glib::spawn_future_local(async {
                    if let Err(err) = library::ensure_started().await {
                        warn!("Unable to start the shared library: {err}");
                    }
                });
            }
        }

        fn activate(&self) {
            debug!("Application::activate");

            self.obj().present_main_window();
        }

        fn shutdown(&self) {
            #[cfg(any(
                target_os = "linux",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly",
                target_os = "windows",
                target_os = "macos"
            ))]
            self.tray.take();
            library::shutdown();
            self.parent_shutdown();
        }

        fn open(&self, files: &[gio::File], _hint: &str) {
            debug!("Application::open");

            let application = self.obj();
            application.present_main_window();
            if let Some(window) = application.active_window() {
                window.downcast_ref::<Window>().unwrap().open_files(files);
            }
        }
    }

    impl GtkApplicationImpl for Application {}
    impl AdwApplicationImpl for Application {}
}

glib::wrapper! {
    pub struct Application(ObjectSubclass<imp::Application>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl Default for Application {
    fn default() -> Self {
        glib::Object::builder::<Application>()
            .property("application-id", APPLICATION_ID)
            .property("flags", gio::ApplicationFlags::HANDLES_OPEN)
            .property("resource-base-path", "/io/hopea/Hopea")
            .build()
    }
}

impl Application {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn player(&self) -> Rc<AudioPlayer> {
        self.imp().player.clone()
    }

    fn setup_settings(&self) {
        self.imp().settings.connect_changed(
            Some("background-play"),
            clone!(
                #[weak(rename_to = this)]
                self,
                move |settings, _| {
                    let background_play = settings.boolean("background-play");
                    debug!("GSettings:background-play: {background_play}");
                    if background_play {
                        this.request_background();
                    } else {
                        debug!("Dropping background hold");
                        this.imp().background_hold.replace(None);
                    }
                }
            ),
        );

        let _dummy = self.imp().settings.boolean("background-play");
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "windows",
        target_os = "macos"
    )))]
    pub fn update_sharing(&self) {}

    /// Network sharing continues after closing the window on desktop systems.
    /// The player tray itself is independent of the sharing setting.
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "windows",
        target_os = "macos"
    ))]
    pub fn update_sharing(&self) {
        let imp = self.imp();
        let sharing = imp.settings.boolean("library-enabled") && library::wanted();
        if sharing && imp.sharing_hold.borrow().is_none() {
            imp.sharing_hold.replace(Some(self.hold()));
        } else if !sharing {
            imp.sharing_hold.take();
        }
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "windows",
        target_os = "macos"
    ))]
    fn setup_tray(&self) {
        // GTK tests must not publish temporary applications to the user's panel.
        #[cfg(test)]
        if std::env::var_os("HOPEA_TEST_ENABLE_TRAY").is_none() {
            return;
        }
        let (sender, receiver) = async_channel::unbounded();
        self.imp()
            .tray
            .replace(Tray::show(sender, TrayState::from_player(&self.player())));
        glib::spawn_future_local(clone!(
            #[weak(rename_to = app)]
            self,
            async move {
                while let Ok(event) = receiver.recv().await {
                    app.handle_tray_event(event);
                }
            }
        ));
        self.player().state().connect_notify_local(
            None,
            clone!(
                #[weak(rename_to = app)]
                self,
                move |_, property| {
                    if matches!(property.name(), "song" | "title" | "artist" | "playing") {
                        app.refresh_tray();
                    }
                }
            ),
        );
        self.player().queue().connect_notify_local(
            Some("n-songs"),
            clone!(
                #[weak(rename_to = app)]
                self,
                move |_, _| app.refresh_tray()
            ),
        );
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "windows",
        target_os = "macos"
    ))]
    fn refresh_tray(&self) {
        if let Some(tray) = self.imp().tray.borrow().as_ref() {
            tray.update(TrayState::from_player(&self.player()));
        }
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "windows",
        target_os = "macos"
    ))]
    pub(crate) fn handle_tray_event(&self, event: TrayEvent) {
        let player = self.player();
        match event {
            TrayEvent::Show => self.activate(),
            TrayEvent::Quit => self.quit(),
            TrayEvent::Previous => player.skip_previous(),
            TrayEvent::PlayPause => player.toggle_play(),
            TrayEvent::Next => player.skip_next(),
        }
    }

    /// Keeps an eye on what the network may be used through, to leave it
    /// the moment it may not and join again when it may (see
    /// [`Self::guard_connection`]).
    fn watch_connection(&self) {
        self.imp().settings.connect_changed(
            Some("library-only-protected"),
            clone!(
                #[weak(rename_to = app)]
                self,
                move |_, _| {
                    app.imp().join_failed_at.set(None);
                    app.guard_connection();
                }
            ),
        );
        glib::timeout_add_local(
            GUARD_INTERVAL,
            clone!(
                #[weak(rename_to = app)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    app.guard_connection();
                    glib::ControlFlow::Continue
                }
            ),
        );
        gio::NetworkMonitor::default().connect_network_changed(clone!(
            #[weak(rename_to = app)]
            self,
            move |_, _| app.guard_connection()
        ));
    }

    /// Leaves the network if it may no longer be used, as when the VPN the
    /// listener shares through goes off, and joins again once it may, if
    /// they want to be on it.
    fn guard_connection(&self) {
        let imp = self.imp();
        let settings = &imp.settings;
        if imp.switching.get() || !settings.boolean("library-enabled") {
            return;
        }
        let connected = library::client().is_some();
        let mode = library::network_mode(settings);
        let allowed = mode.is_some();
        if connected && (mode != Some(library::uses_ipv4_only()) || library::route_changed()) {
            imp.switching.set(true);
            glib::spawn_future_local(clone!(
                #[weak(rename_to = app)]
                self,
                async move {
                    library::disconnect().await;
                    app.imp().switching.set(false);
                    app.connection_changed(if allowed {
                        i18n("Reconnecting with the updated connection settings…")
                    } else {
                        i18n("The VPN is off. Sharing will resume when it reconnects.")
                    });
                    app.guard_connection();
                }
            ));
        } else if !connected && allowed && library::wanted() {
            if imp
                .join_failed_at
                .get()
                .is_some_and(|t| t.elapsed() < JOIN_RETRY_DELAY)
            {
                return;
            }
            imp.switching.set(true);
            let protected = settings.boolean("library-only-protected");
            glib::spawn_future_local(clone!(
                #[weak(rename_to = app)]
                self,
                async move {
                    let result = library::ensure_started().await;
                    app.imp().switching.set(false);
                    match result {
                        Ok(_) => app.connection_changed(if protected {
                            i18n("The VPN is on: joined the network")
                        } else {
                            i18n("Joined the network")
                        }),
                        Err(err) => {
                            warn!("Unable to join the network: {err}");
                            app.imp().join_failed_at.set(Some(Instant::now()));
                        }
                    }
                }
            ));
        }
    }

    /// Tells the window the app joined or left the network by itself.
    fn connection_changed(&self, message: String) {
        if let Some(window) = self.active_window().and_downcast::<Window>() {
            window.library_connection_changed(message);
        }
    }

    fn setup_channel(&self) {
        let receiver = self.imp().receiver.borrow_mut().take().unwrap();
        glib::MainContext::default().spawn_local(clone!(
            #[strong(rename_to = this)]
            self,
            async move {
                use futures::prelude::*;

                let mut receiver = std::pin::pin!(receiver);

                while let Some(action) = receiver.next().await {
                    this.process_action(action);
                }
            }
        ));
    }

    fn process_action(&self, action: ApplicationAction) -> glib::ControlFlow {
        match action {
            ApplicationAction::Present => self.present_main_window(),
            // _ => debug!("Received action {:?}", action),
        }

        glib::ControlFlow::Continue
    }

    fn present_main_window(&self) {
        let window = if let Some(window) = self.active_window() {
            window
        } else {
            let window = Window::new(self);
            window.upcast()
        };

        self.request_background();

        window.present();
    }

    fn setup_gactions(&self) {
        self.add_action_entries([
            gio::ActionEntry::builder("quit")
                .activate(|app: &Application, _, _| {
                    app.quit();
                })
                .build(),
            gio::ActionEntry::builder("about")
                .activate(|app: &Application, _, _| {
                    app.show_about();
                })
                .build(),
        ]);

        let background_play = self.imp().settings.boolean("background-play");
        self.add_action_entries([gio::ActionEntry::builder("background-play")
            .state(background_play.to_variant())
            .activate(|this: &Application, action, _| {
                let state = action.state().unwrap();
                let action_state: bool = state.get().unwrap();
                let background_play = !action_state;
                action.set_state(&background_play.to_variant());

                this.imp()
                    .settings
                    .set_boolean("background-play", background_play)
                    .expect("Unable to store background-play setting");
            })
            .build()]);
    }

    fn show_about(&self) {
        let window = self.active_window().unwrap();
        let dialog = adw::AboutDialog::builder()
            .application_icon(APPLICATION_ICON)
            .application_name("Hopea")
            .developer_name("Daniil Klimov")
            .version(VERSION)
            .website("https://github.com/PuffyWithEyes/Hopea")
            .issue_url("https://github.com/PuffyWithEyes/Hopea/issues")
            .developers(vec!["Daniil Klimov", "Emmanuele Bassi"])
            .copyright("© 2022 Emmanuele Bassi\n© 2026 Daniil Klimov")
            .license_type(gtk::License::Gpl30)
            // Translators: Replace "translator-credits" with your names, one name per line
            .translator_credits(i18n("translator-credits"))
            .build();

        dialog.present(Some(&window));
    }

    #[cfg(target_os = "linux")]
    async fn portal_request_background(&self) {
        if let Some(window) = self.active_window() {
            let root = window.native().unwrap();
            let identifier = WindowIdentifier::from_native(&root).await;
            let request = Background::request()
                .identifier(identifier)
                .reason(&*i18n("Hopea needs to run in the background to play music"));

            match request.send().await.and_then(
                |r: ashpd::desktop::Request<ashpd::desktop::background::Background>| r.response(),
            ) {
                Ok(response) => {
                    debug!("Background request successful: {:?}", response);
                    self.imp().background_hold.replace(Some(self.hold()));
                }
                Err(err) => {
                    warn!("Background request denied: {}", err);
                    self.imp()
                        .settings
                        .set_boolean("background-play", false)
                        .expect("Unable to set background-play settings key");
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn request_background(&self) {
        let background_play = self.imp().settings.boolean("background-play");
        if background_play {
            let ctx = glib::MainContext::default();
            ctx.spawn_local(clone!(
                #[weak(rename_to = app)]
                self,
                async move { app.portal_request_background().await }
            ));
        }
    }

    #[cfg(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    fn request_background(&self) {
        if self.imp().settings.boolean("background-play")
            && self.imp().background_hold.borrow().is_none()
        {
            self.imp().background_hold.replace(Some(self.hold()));
        }
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "windows",
        target_os = "macos"
    )))]
    fn request_background(&self) {}
}
