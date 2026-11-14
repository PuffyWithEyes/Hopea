// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

// The shared library is a peer-to-peer node built into the player (see
// `node.rs`, and `node/` in the source tree). The player talks to it over
// its HTTP API on loopback, and keeps the connection in two places:
//
// - the LibraryClient, which owns a libsoup session and is only used from
//   the main context
// - the origin and token, kept behind a Mutex, because GStreamer asks for
//   the Authorization header from its own threads when it sets up an HTTP
//   source for a remote song
//
// The token is only ever attached to requests for the node's own origin
// and API path; it is never sent to any other host, and redirects are not
// followed.

pub mod browser_import;
mod client;
pub mod collections;
pub mod duplicates;
pub mod import;
pub mod import_albums;
pub mod import_history;
mod node;
pub mod protection;
pub mod service_import;
pub mod shared_files;
pub mod spotify_audio;
pub mod thumbnails;

use std::{cell::RefCell, sync::Mutex};

use futures::future::{FutureExt, LocalBoxFuture, Shared};
use gio::prelude::*;
use gst::prelude::*;
use gtk::gio;
use hopea_common::api::SpeedLimits;
use log::debug;

pub use client::{ClientError, LibraryClient, UploadCover};
pub(crate) use node::set_gaming;

thread_local! {
    /// Whether the listener wants to be on the network in this session
    static WANTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static CLIENT: RefCell<Option<LibraryClient>> = const { RefCell::new(None) };
    static STARTING: RefCell<Option<Shared<LocalBoxFuture<'static, Result<(), String>>>>> =
        const { RefCell::new(None) };
}

/// Starts the node if needed and connects to it. Safe to call while a start
/// is already in progress: every caller waits for the same one.
pub async fn ensure_started() -> Result<LibraryClient, String> {
    if let Some(client) = client() {
        return Ok(client);
    }
    let starting = STARTING.with(|s| {
        s.borrow_mut()
            .get_or_insert_with(|| start().boxed_local().shared())
            .clone()
    });
    let result = starting.await;
    STARTING.with(|s| s.borrow_mut().take());
    result?;
    client().ok_or_else(|| "The library stopped".to_string())
}

async fn start() -> Result<(), String> {
    let settings = crate::utils::settings_manager();
    let ipv4_only = network_mode(&settings)
        .ok_or_else(|| crate::i18n::i18n("Connect to your VPN before sharing music"))?;
    let name = settings.string("library-name").trim().to_string();
    let peers = settings
        .strv("library-peers")
        .iter()
        .filter_map(|p| match p.parse() {
            Ok(addr) => Some(addr),
            Err(_) => {
                log::warn!("Ignoring library peer '{p}': not an address and port");
                None
            }
        })
        .collect();
    let kib = |key: &str| {
        Some(settings.uint(key))
            .filter(|&k| k > 0)
            .map(|k| k.saturating_mul(1024))
    };
    let speed = SpeedLimits {
        download: kib("library-download-limit"),
        upload: kib("library-upload-limit"),
    };
    let (url, token) =
        node::start((!name.is_empty()).then_some(name), peers, speed, ipv4_only).await?;
    // Routes can change while starting the node. Do not expose its client
    // until the current preference and the socket mode still agree.
    if network_mode(&settings) != Some(ipv4_only) {
        let _ = gio::spawn_blocking(node::stop).await;
        return Err(crate::i18n::i18n(
            "The connection changed. Please try joining again.",
        ));
    }
    let client = LibraryClient::new(&url, &token).map_err(|e| e.to_string())?;
    set_client(Some(client));
    Ok(())
}

/// Whether the listener wants to be on the network in this session: they
/// joined, or the app did as it started. A VPN going off doesn't change it,
/// so the app joins again when it comes back.
pub fn wanted() -> bool {
    WANTED.with(|w| w.get())
}

pub fn set_wanted(wanted: bool) {
    WANTED.with(|w| w.set(wanted));
    update_sharing();
}

/// Whether the network may be used now: always, unless the listener shares
/// only through a VPN (`library-only-protected`) and none is on.
pub fn allowed(settings: &gtk::gio::Settings) -> bool {
    network_mode(settings).is_some()
}

/// None blocks sharing; true restricts every peer protocol to IPv4.
pub fn network_mode(settings: &gio::Settings) -> Option<bool> {
    if settings.boolean("library-only-protected") {
        protection::check().sharing_ipv4_only()
    } else {
        Some(false)
    }
}

pub fn uses_ipv4_only() -> bool {
    node::ipv4_only()
}

/// Leaves the network for now: disconnects right away, then stops the node
/// off the main thread.
pub async fn disconnect() {
    set_client(None);
    let _ = gtk::gio::spawn_blocking(node::stop).await;
}

/// Whether the app joins the network as it starts: as the listener chose,
/// or by default on computers and not on phones and tablets, where it would
/// cost battery and mobile data.
pub fn joins_at_start(settings: &gtk::gio::Settings) -> bool {
    match settings.string("library-join-at-start").as_str() {
        "yes" => true,
        "no" => false,
        _ => !crate::device::is_handheld(),
    }
}

/// Disconnects and stops the node, waiting for it to finish.
pub fn shutdown() {
    set_client(None);
    node::stop();
}

/// Stops the node and waits for it; may be called from any thread.
pub fn stop_node() {
    node::stop();
}

struct Auth {
    origin: client::Origin,
    header: String,
}

static AUTH: Mutex<Option<Auth>> = Mutex::new(None);

/// The connected library, if any.
pub fn client() -> Option<LibraryClient> {
    CLIENT.with(|c| c.borrow().clone())
}

pub fn set_client(client: Option<LibraryClient>) {
    let auth = client.as_ref().map(|c| Auth {
        origin: c.origin().clone(),
        header: c.authorization(),
    });
    *AUTH.lock().unwrap_or_else(|e| e.into_inner()) = auth;
    CLIENT.with(|c| c.replace(client));
    if let Some(client) = self::client() {
        gtk::glib::spawn_future_local(crate::lyrics::publish_saved(client));
    }
    update_sharing();
}

/// Lets the app keep running with its window closed, and show its tray
/// icon, for as long as the listener wants to be on the network (see
/// `Application::update_sharing`).
fn update_sharing() {
    gtk::glib::idle_add_local_once(|| {
        if let Some(app) = gtk::gio::Application::default()
            .and_then(|a| a.downcast::<crate::application::Application>().ok())
        {
            app.update_sharing();
        }
    });
}

/// The Authorization header value for `uri`, if it points at the API of the
/// connected library.
fn authorization_for(uri: &str) -> Option<String> {
    let auth = AUTH.lock().unwrap_or_else(|e| e.into_inner());
    let auth = auth.as_ref()?;
    auth.origin.covers(uri).then(|| auth.header.clone())
}

/// Called from playbin's `source-setup`: lets an HTTP source fetch songs
/// from the library by adding the access token, and only for the library.
pub fn configure_source(source: &gst::Element) {
    if source.find_property("extra-headers").is_none() || source.find_property("location").is_none()
    {
        return;
    }

    let Some(location) = source.property::<Option<String>>("location") else {
        return;
    };
    let Some(header) = authorization_for(&location) else {
        return;
    };

    debug!("Authorizing stream from the shared library");
    let headers = gst::Structure::builder("extra-headers")
        .field("Authorization", header)
        .build();
    source.set_property("extra-headers", headers);
    if source.find_property("automatic-redirect").is_some() {
        source.set_property("automatic-redirect", false);
    }
    if source.find_property("ssl-strict").is_some() {
        source.set_property("ssl-strict", true);
    }
    // The node may have to find the track on the network, or have a relay
    // fetch it, before the first byte: wait longer than the default 15 s
    if source.find_property("timeout").is_some() {
        source.set_property("timeout", 150u32);
    }
}
