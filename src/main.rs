// SPDX-FileCopyrightText: 2022  Emmanuele Bassi
// SPDX-License-Identifier: GPL-3.0-or-later

// Release builds launched from Explorer must not open an extra console window.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod accent;
#[cfg(target_os = "android")]
mod android;
mod application;
mod artwork;
mod audio;
mod cache_cleanup;
mod config;
mod cover_picture;
mod device;
mod dialogs;
mod document_file;
mod drag_overlay;
mod gaming;
mod i18n;
mod language;
mod library;
mod library_panel;
mod lyrics;
mod marquee;
mod paths;
mod platform;
mod playback_control;
mod playlist_view;
mod queue_row;
mod search;
mod song_cover;
mod song_details;
mod song_edits;
mod sort;
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "windows",
    target_os = "macos"
))]
mod tray;
mod utils;
mod volume_control;
mod waveform_view;
mod window;

use std::env;

use config::{APPLICATION_ICON, APPLICATION_ID, GETTEXT_PACKAGE, PKGDATADIR, PROFILE};
use gettextrs::{bind_textdomain_codeset, bindtextdomain, setlocale, textdomain, LocaleCategory};
use gtk::{gio, glib, prelude::*};
use log::{debug, error, LevelFilter};

use self::application::Application;

/// `relative`, from the folder above the program's (`bin`), if it exists.
fn beside_program(relative: &str) -> Option<std::path::PathBuf> {
    let exe = env::current_exe().ok()?;
    let path = exe.parent()?.parent()?.join(relative);
    path.exists().then_some(path)
}

fn main() -> glib::ExitCode {
    platform::init();
    // Read the small bootstrap preference before GTK, GSettings or workers
    // start: changing the process locale while audio threads run is unsafe.
    let language = language::preference();
    if !language.is_empty() {
        env::set_var("LANGUAGE", &language);
    }
    let mut builder = pretty_env_logger::formatted_builder();
    let level = if APPLICATION_ID.ends_with("Devel") {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    // The shared library node built into the app logs under its own name
    builder.filter(Some("hopea"), level);
    builder.filter(Some("hopea_node"), level);
    builder.init();

    // Set up gettext translations
    debug!("Setting up locale data");
    // SAFETY: nothing else runs yet: no other thread can read the locale
    // while it changes
    unsafe {
        setlocale(LocaleCategory::LcAll, "");
        if !language.is_empty() {
            setlocale(LocaleCategory::LcMessages, "C.UTF-8");
        }
    }

    // A relocatable install, such as the Windows bundle, carries its
    // translations next to the program rather than where it was built for
    let locale_dir = language::catalog_dir();
    bindtextdomain(GETTEXT_PACKAGE, locale_dir).expect("Unable to bind the text domain");
    bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8")
        .expect("Unable to set the text domain encoding");
    textdomain(GETTEXT_PACKAGE).expect("Unable to switch to the text domain");

    debug!("Setting up pulseaudio environment");
    env::set_var("PULSE_PROP_application.icon_name", APPLICATION_ICON);
    env::set_var("PULSE_PROP_application.metadata().name", "Hopea");
    env::set_var("PULSE_PROP_media.role", "music");

    debug!("Loading resources");
    let resources = match env::var("MESON_DEVENV") {
        Err(_) => gio::Resource::load(PKGDATADIR.to_owned() + "/hopea.gresource")
            .or_else(|err| match beside_program("share/hopea/hopea.gresource") {
                Some(path) => gio::Resource::load(path),
                None => Err(err),
            })
            .expect("Unable to find hopea.gresource"),
        Ok(_) => match env::current_exe() {
            Ok(path) => {
                let mut resource_path = path;
                resource_path.pop();
                resource_path.push("hopea.gresource");
                gio::Resource::load(&resource_path)
                    .expect("Unable to find hopea.gresource in devenv")
            }
            Err(err) => {
                error!("Unable to find the current path: {}", err);
                return glib::ExitCode::FAILURE;
            }
        },
    };
    gio::resources_register(&resources);

    debug!("Setting up application (profile: {})", &PROFILE);
    glib::set_application_name("Hopea");
    glib::set_program_name(Some("hopea"));

    gst::init().expect("Failed to initialize gstreamer");

    let ctx = glib::MainContext::default();
    let _guard = ctx.acquire().unwrap();

    Application::new().run()
}
