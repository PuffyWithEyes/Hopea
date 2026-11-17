// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    cell::Cell,
    fmt::{self, Display, Formatter},
    rc::Rc,
};

use gtk::{
    gio,
    glib::{self, translate::IntoGlib},
    prelude::*,
};
use hopea_common::api::{
    self, ApiError, Me, Peaks, PendingCover, SearchOrder, SpeedLimits, Track, TrackPage, Transfer,
    UploadResponse,
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::de::DeserializeOwned;
use soup::prelude::*;

use crate::{config::VERSION, i18n::i18n};

// Upper bounds on response bodies, so a misbehaving server cannot make the
// player allocate without limit
const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
const MAX_COVER_BYTES: usize = 4 * 1024 * 1024;

pub const PAGE_SIZE: u32 = 50;

/// Cover art for an upload, as returned by [`LibraryClient::add_cover`].
/// Existing own tracks can also update artwork through `update_cover`.
#[derive(Clone, Debug, Default)]
pub struct UploadCover {
    /// Replaces any picture in the file
    pub cover: Option<String>,
    /// Used only if the file has no picture of its own
    pub fallback: Option<String>,
}

#[derive(Debug)]
pub enum ClientError {
    InvalidServer(String),
    Unauthorized,
    NotFound,
    Unavailable,
    Network(glib::Error),
    Server(String),
    InvalidResponse,
}

impl Display for ClientError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::InvalidServer(reason) => write!(f, "{reason}"),
            ClientError::Unauthorized => {
                write!(f, "{}", i18n("The access token was not accepted"))
            }
            ClientError::NotFound => {
                write!(f, "{}", i18n("The track no longer exists in the library"))
            }
            ClientError::Unavailable => write!(
                f,
                "{}",
                i18n("This song has no available sources right now")
            ),
            ClientError::Network(err) => {
                // Translators: `{}` is replaced by a technical error message
                write!(
                    f,
                    "{}",
                    i18n("Unable to reach the server: {}").replace("{}", err.message())
                )
            }
            ClientError::Server(message) => write!(f, "{message}"),
            ClientError::InvalidResponse => {
                write!(f, "{}", i18n("The server sent an invalid response"))
            }
        }
    }
}

impl From<glib::Error> for ClientError {
    fn from(err: glib::Error) -> Self {
        ClientError::Network(err)
    }
}

/// Scheme, host and port of the library, plus the path it is served under.
#[derive(Clone, Debug, PartialEq)]
pub struct Origin {
    scheme: String,
    host: String,
    port: i32,
    path: String,
}

fn default_port(scheme: &str) -> i32 {
    if scheme == "https" {
        443
    } else {
        80
    }
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

impl Origin {
    /// Parses a server address typed by the user. Plain HTTP is only
    /// accepted for loopback addresses: tokens and music must not travel
    /// over the network unencrypted.
    pub fn parse(input: &str) -> Result<Self, ClientError> {
        let input = input.trim().trim_end_matches('/');
        if input.is_empty() {
            return Err(ClientError::InvalidServer(i18n("Enter the server address")));
        }
        let with_scheme = if input.contains("://") {
            input.to_string()
        } else {
            format!("https://{input}")
        };

        let invalid = || ClientError::InvalidServer(i18n("The server address is not valid"));
        let uri = glib::Uri::parse(&with_scheme, glib::UriFlags::NONE).map_err(|_| invalid())?;
        if uri.userinfo().is_some() || uri.query().is_some() || uri.fragment().is_some() {
            return Err(invalid());
        }

        let scheme = uri.scheme().to_ascii_lowercase();
        let host = uri.host().ok_or_else(invalid)?.to_ascii_lowercase();
        match scheme.as_str() {
            "https" => {}
            "http" if is_loopback(&host) => {}
            "http" => {
                return Err(ClientError::InvalidServer(i18n(
                    "The server must use a secure (https://) connection",
                )))
            }
            _ => return Err(invalid()),
        }

        let port = match uri.port() {
            -1 => default_port(&scheme),
            p => p,
        };

        Ok(Self {
            path: uri.path().trim_end_matches('/').to_string(),
            scheme,
            host,
            port,
        })
    }

    pub fn base_url(&self) -> String {
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == default_port(&self.scheme) {
            format!("{}://{host}{}", self.scheme, self.path)
        } else {
            format!("{}://{host}:{}{}", self.scheme, self.port, self.path)
        }
    }

    /// Whether `uri` is a request to this library's API.
    pub fn covers(&self, uri: &str) -> bool {
        let Ok(other) = Origin::parse(uri) else {
            return false;
        };
        let api_root = format!("{}{}/", self.path, api::API_PREFIX);
        other.scheme == self.scheme
            && other.host == self.host
            && other.port == self.port
            && other.path.starts_with(&api_root)
            && !other.path.contains("/../")
    }
}

struct Inner {
    session: soup::Session,
    origin: Origin,
    token: String,
}

/// A connection to a shared library server.
#[derive(Clone)]
pub struct LibraryClient {
    inner: Rc<Inner>,
}

impl LibraryClient {
    pub fn new(server: &str, token: &str) -> Result<Self, ClientError> {
        let origin = Origin::parse(server)?;
        let token = token.trim();
        if token.is_empty() || !token.chars().all(|c| c.is_ascii_graphic()) {
            return Err(ClientError::InvalidServer(i18n(
                "The access token is not valid",
            )));
        }

        let session = soup::Session::new();
        session.set_user_agent(&format!("Hopea/{VERSION}"));
        session.set_timeout(30);

        Ok(Self {
            inner: Rc::new(Inner {
                session,
                origin,
                token: token.to_string(),
            }),
        })
    }

    pub fn origin(&self) -> &Origin {
        &self.inner.origin
    }

    pub(super) fn authorization(&self) -> String {
        format!("Bearer {}", self.inner.token)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.inner.origin.base_url())
    }

    pub fn audio_uri(&self, id: i64) -> String {
        self.url(&api::audio_path(id))
    }

    fn message(&self, method: &str, path_and_query: &str) -> Result<soup::Message, ClientError> {
        let msg = soup::Message::new(method, &self.url(path_and_query))
            .map_err(|_| ClientError::InvalidResponse)?;
        // Never follow a redirect: the token must not leave the library
        msg.set_flags(soup::MessageFlags::NO_REDIRECT);
        let headers = msg.request_headers().unwrap();
        headers.append("Authorization", &self.authorization());
        headers.append("Accept", "application/json");
        Ok(msg)
    }

    async fn send(&self, msg: &soup::Message, limit: usize) -> Result<Vec<u8>, ClientError> {
        let stream = self
            .inner
            .session
            .send_future(msg, glib::Priority::DEFAULT)
            .await?;
        let body = read_limited(&stream, limit).await?;

        let status = msg.status();
        match status.into_glib() as u32 {
            200..=299 => Ok(body),
            401 => Err(ClientError::Unauthorized),
            404 => Err(ClientError::NotFound),
            _ => match serde_json::from_slice::<ApiError>(&body) {
                Ok(err) if err.error == "track_unavailable" => Err(ClientError::Unavailable),
                Ok(err) => Err(ClientError::Server(err.message)),
                Err(_) => Err(ClientError::InvalidResponse),
            },
        }
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let msg = self.message("GET", path)?;
        let body = self.send(&msg, MAX_JSON_BYTES).await?;
        serde_json::from_slice(&body).map_err(|_| ClientError::InvalidResponse)
    }

    pub async fn me(&self) -> Result<Me, ClientError> {
        self.get_json(&api::me_path()).await
    }

    /// Searches the tracks the node knows about; with `network`, other
    /// nodes are searched first, which takes a few seconds.
    pub async fn search(
        &self,
        query: &str,
        offset: u32,
        network: bool,
        order: SearchOrder,
    ) -> Result<TrackPage, ClientError> {
        let q = utf8_percent_encode(query, NON_ALPHANUMERIC);
        self.get_json(&format!(
            "{}?q={q}&limit={PAGE_SIZE}&offset={offset}&network={network}&order={}",
            api::tracks_path(),
            order.as_str()
        ))
        .await
    }

    pub async fn available_tracks(
        &self,
        query: &str,
        ids: &[i64],
    ) -> Result<Vec<i64>, ClientError> {
        let mut available = Vec::new();
        for (index, chunk) in ids.chunks(200).enumerate() {
            let ids = chunk
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let refresh = if index == 0 {
                format!("&q={}", utf8_percent_encode(query, NON_ALPHANUMERIC))
            } else {
                String::new()
            };
            let page: Vec<i64> = self
                .get_json(&format!(
                    "{}/available?ids={ids}{refresh}",
                    api::tracks_path()
                ))
                .await?;
            available.extend(page);
        }
        Ok(available)
    }

    pub async fn local_tracks(&self, offset: u32) -> Result<TrackPage, ClientError> {
        self.get_json(&format!(
            "{}?local=true&limit={PAGE_SIZE}&offset={offset}",
            api::tracks_path()
        ))
        .await
    }

    pub async fn available_recommendations(&self, ids: &[i64]) -> Result<Vec<i64>, ClientError> {
        let mut result = Vec::new();
        for chunk in ids.chunks(200) {
            let ids = chunk
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let available: Vec<i64> = self
                .get_json(&format!(
                    "{}/available?ids={ids}&playback=true",
                    api::tracks_path()
                ))
                .await?;
            result.extend(available);
        }
        Ok(result)
    }

    pub async fn discover(
        &self,
        query: &str,
        offset: u32,
        network: bool,
        order: SearchOrder,
    ) -> Result<TrackPage, ClientError> {
        let q = utf8_percent_encode(query, NON_ALPHANUMERIC);
        self.get_json(&format!(
            "{}?q={q}&limit={PAGE_SIZE}&offset={offset}&network={network}&discovery=true&order={}",
            api::tracks_path(),
            order.as_str()
        ))
        .await
    }

    /// One bounded peer search, with bounded snapshots of the node's local
    /// result database while replies arrive. Snapshots never contact peers.
    /// Returning false from `update` cancels a superseded UI search.
    pub async fn discover_live(
        &self,
        query: &str,
        order: SearchOrder,
        mut update: impl FnMut(Option<&TrackPage>) -> bool,
    ) -> Option<Result<TrackPage, ClientError>> {
        use futures::future::{select, Either};
        let search = Box::pin(self.discover(query, 0, true, order));
        let snapshots = Box::pin(async {
            for _ in 0..60 {
                if !update(None) {
                    return;
                }
                if let Ok(page) = self.discover(query, 0, false, order).await {
                    if !update(Some(&page)) {
                        return;
                    }
                }
                glib::timeout_future(std::time::Duration::from_millis(400)).await;
            }
            // The normal request timeout remains authoritative. Reaching the
            // snapshot budget must neither retry nor start a new peer search.
            futures::future::pending::<()>().await;
        });
        match select(search, snapshots).await {
            Either::Left((result, _)) => Some(result),
            Either::Right(_) => None,
        }
    }

    /// How a track's file is coming along, and how it is shared from here.
    pub async fn transfer(&self, id: i64) -> Result<Transfer, ClientError> {
        self.get_json(&api::transfer_path(id)).await
    }

    /// Starts downloading a track that plays next.
    pub async fn prefetch(&self, id: i64) -> Result<(), ClientError> {
        let msg = self.message("POST", &api::prefetch_path(id))?;
        self.send(&msg, MAX_JSON_BYTES).await.map(|_| ())
    }

    pub async fn track(&self, id: i64) -> Result<Track, ClientError> {
        self.get_json(&api::track_path(id)).await
    }

    /// The songs of an album, in order; with `network`, other nodes are
    /// searched for it first.
    pub async fn album(
        &self,
        album: &str,
        artist: Option<&str>,
        network: bool,
    ) -> Result<TrackPage, ClientError> {
        let mut query = format!(
            "{}?album={}&network={network}",
            api::album_path(),
            utf8_percent_encode(album, NON_ALPHANUMERIC)
        );
        if let Some(artist) = artist {
            query.push_str(&format!(
                "&artist={}",
                utf8_percent_encode(artist, NON_ALPHANUMERIC)
            ));
        }
        self.get_json(&query).await
    }

    /// Saves a song's file, which the node only sends once it has it in full
    /// and verified, to `dest`.
    pub async fn save_file(&self, id: i64, dest: &gio::File) -> Result<(), ClientError> {
        let msg = self.message("GET", &api::file_path(id))?;
        let stream = self
            .inner
            .session
            .send_future(&msg, glib::Priority::DEFAULT)
            .await?;
        if msg.status() != soup::Status::Ok {
            let body = read_limited(&stream, MAX_JSON_BYTES).await?;
            return Err(match serde_json::from_slice::<ApiError>(&body) {
                Ok(err) => ClientError::Server(err.message),
                Err(_) => ClientError::InvalidResponse,
            });
        }
        let out = dest
            .replace_future(
                None,
                false,
                gio::FileCreateFlags::REPLACE_DESTINATION,
                glib::Priority::DEFAULT,
            )
            .await?;
        out.splice_future(
            &stream,
            gio::OutputStreamSpliceFlags::CLOSE_SOURCE | gio::OutputStreamSpliceFlags::CLOSE_TARGET,
            glib::Priority::DEFAULT,
        )
        .await?;
        Ok(())
    }

    /// Sends a picture for the uploads that follow, and returns the name
    /// they give it (see [`UploadCover`]).
    pub async fn add_cover(&self, image: glib::Bytes) -> Result<String, ClientError> {
        let msg = self.message("POST", &api::covers_path())?;
        msg.set_request_body_from_bytes(Some("application/octet-stream"), Some(&image));
        let body = self.send(&msg, MAX_JSON_BYTES).await?;
        serde_json::from_slice::<PendingCover>(&body)
            .map(|p| p.cover)
            .map_err(|_| ClientError::InvalidResponse)
    }

    /// Updates only artwork; the stored audio and track ID are preserved.
    pub async fn update_cover(&self, id: i64, cover: &str) -> Result<Track, ClientError> {
        let msg = self.message("PUT", &api::cover_path(id))?;
        let body = serde_json::to_vec(&PendingCover {
            cover: cover.into(),
        })
        .map_err(|_| ClientError::InvalidResponse)?;
        msg.set_request_body_from_bytes(
            Some("application/json"),
            Some(&glib::Bytes::from_owned(body)),
        );
        let body = self.send(&msg, MAX_JSON_BYTES).await?;
        serde_json::from_slice(&body).map_err(|_| ClientError::InvalidResponse)
    }

    pub async fn metadata_variants(
        &self,
        id: i64,
        refresh: bool,
    ) -> Result<Vec<api::MetadataVariant>, ClientError> {
        self.get_json(&format!(
            "{}/variants?refresh={refresh}",
            api::track_path(id)
        ))
        .await
    }
    pub async fn metadata_data(&self, id: i64, hash: &str) -> Result<glib::Bytes, ClientError> {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ClientError::InvalidResponse);
        }
        let msg = self.message("GET", &format!("{}/variants/{hash}", api::track_path(id)))?;
        Ok(glib::Bytes::from_owned(self.send(&msg, 1024 * 1024).await?))
    }
    pub async fn select_metadata(&self, id: i64, hash: &str) -> Result<(), ClientError> {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ClientError::InvalidResponse);
        }
        let msg = self.message("PUT", &format!("{}/variants/{hash}", api::track_path(id)))?;
        self.send(&msg, MAX_JSON_BYTES).await.map(|_| ())
    }
    pub async fn lyrics(&self, id: i64) -> Result<Option<String>, ClientError> {
        self.get_json(&format!("{}/lyrics", api::track_path(id)))
            .await
    }

    pub async fn publish_lyrics(&self, id: i64, text: &str) -> Result<(), ClientError> {
        if text.len() > 256 * 1024 {
            return Err(ClientError::InvalidResponse);
        }
        let msg = self.message("PUT", &format!("{}/lyrics", api::track_path(id)))?;
        msg.set_request_body_from_bytes(
            Some("text/plain; charset=utf-8"),
            Some(&glib::Bytes::from(text.as_bytes())),
        );
        self.send(&msg, MAX_JSON_BYTES).await.map(|_| ())
    }

    /// Changes the speed limits; the node makes them fair and returns the
    /// limits in effect.
    pub async fn set_limits(&self, limits: SpeedLimits) -> Result<SpeedLimits, ClientError> {
        let msg = self.message("PUT", &api::limits_path())?;
        let body = serde_json::to_vec(&limits).map_err(|_| ClientError::InvalidResponse)?;
        msg.set_request_body_from_bytes(
            Some("application/json"),
            Some(&glib::Bytes::from_owned(body)),
        );
        let body = self.send(&msg, MAX_JSON_BYTES).await?;
        serde_json::from_slice(&body).map_err(|_| ClientError::InvalidResponse)
    }

    pub async fn favorites(&self, offset: u32) -> Result<TrackPage, ClientError> {
        self.get_json(&format!(
            "{}?limit={PAGE_SIZE}&offset={offset}",
            api::favorites_path()
        ))
        .await
    }

    /// The "For you" feed; `refresh` searches the network for new songs.
    pub async fn recommendations(&self, refresh: bool) -> Result<TrackPage, ClientError> {
        self.get_json(&format!(
            "{}?refresh={refresh}",
            api::recommendations_path()
        ))
        .await
    }

    pub async fn set_favorite(&self, id: i64, favorite: bool) -> Result<Track, ClientError> {
        let method = if favorite { "PUT" } else { "DELETE" };
        let msg = self.message(method, &api::favorite_path(id))?;
        let body = self.send(&msg, MAX_JSON_BYTES).await?;
        serde_json::from_slice(&body).map_err(|_| ClientError::InvalidResponse)
    }

    pub async fn peaks(&self, id: i64) -> Result<Peaks, ClientError> {
        self.get_json(&api::peaks_path(id)).await
    }

    pub async fn cover(&self, id: i64) -> Result<glib::Bytes, ClientError> {
        let msg = self.message("GET", &api::cover_path(id))?;
        let body = self.send(&msg, MAX_COVER_BYTES).await?;
        Ok(glib::Bytes::from_owned(body))
    }

    pub async fn delete(&self, id: i64) -> Result<(), ClientError> {
        let msg = self.message("DELETE", &api::track_path(id))?;
        self.send(&msg, MAX_JSON_BYTES).await.map(|_| ())
    }

    /// Uploads `file`; `progress` is called with the number of bytes sent.
    pub async fn upload(
        &self,
        file: &gio::File,
        name: &str,
        size: u64,
        cover: &UploadCover,
        progress: impl Fn(u64) + 'static,
    ) -> Result<UploadResponse, ClientError> {
        let stream = file.read_future(glib::Priority::DEFAULT).await?;
        let msg = self.message("POST", &api::tracks_path())?;
        let headers = msg.request_headers().unwrap();
        headers.append(
            api::FILENAME_HEADER,
            &utf8_percent_encode(name, NON_ALPHANUMERIC).to_string(),
        );
        if let Some(cover) = cover.cover.as_deref() {
            headers.append(api::COVER_HEADER, cover);
        }
        if let Some(cover) = cover.fallback.as_deref() {
            headers.append(api::FALLBACK_COVER_HEADER, cover);
        }
        msg.set_request_body(
            Some("application/octet-stream"),
            Some(&stream),
            size as isize,
        );

        let sent = Rc::new(Cell::new(0u64));
        msg.connect_wrote_body_data(move |_, n| {
            sent.set(sent.get() + u64::from(n));
            progress(sent.get());
        });

        let body = self.send(&msg, MAX_JSON_BYTES).await?;
        serde_json::from_slice(&body).map_err(|_| ClientError::InvalidResponse)
    }
}

async fn read_limited(stream: &gio::InputStream, limit: usize) -> Result<Vec<u8>, ClientError> {
    let mut body = Vec::new();
    loop {
        let chunk = stream
            .read_bytes_future(64 * 1024, glib::Priority::DEFAULT)
            .await?;
        if chunk.is_empty() {
            break;
        }
        if body.len() + chunk.len() > limit {
            return Err(ClientError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_yields_local_results_before_network_finishes() {
        use std::{
            io::{BufRead, Write},
            sync::{
                atomic::{AtomicUsize, Ordering},
                Arc,
            },
            time::{Duration, Instant},
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let snapshots = Arc::new(AtomicUsize::new(0));
        let searches = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (observed, network, done) = (snapshots.clone(), searches.clone(), finished.clone());
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut handlers = Vec::new();
            while Instant::now() < deadline && !done.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                let (observed, network) = (observed.clone(), network.clone());
                handlers.push(std::thread::spawn(move || {
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                    let mut request = String::new();
                    reader.read_line(&mut request).unwrap();
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap() == 0 || header == "\r\n" { break; }
                    }
                    assert!(request.contains("limit=50&offset=0&"));
                    let total = if request.contains("network=true") {
                        network.fetch_add(1, Ordering::SeqCst);
                        // The final network response cannot complete until two
                        // separate local snapshots have become available.
                        while observed.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        3
                    } else {
                        assert!(request.contains("network=false"));
                        observed.fetch_add(1, Ordering::SeqCst) + 1
                    };
                    let body = format!("{{\"tracks\":[],\"total\":{total}}}");
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                }));
            }
            for handler in handlers {
                handler.join().unwrap();
            }
        });
        let context = glib::MainContext::new();
        let pages = context
            .with_thread_default(|| {
                context.block_on(async {
                    let client =
                        LibraryClient::new(&format!("http://{address}"), "fixture").unwrap();
                    let mut pages = Vec::new();
                    let final_page = client
                        .discover_live("fixture", SearchOrder::BestMatch, |page| {
                            if let Some(page) = page {
                                pages.push(page.total);
                            }
                            true
                        })
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(final_page.total, 3);
                    pages
                })
            })
            .unwrap();
        finished.store(true, Ordering::SeqCst);
        server.join().unwrap();
        assert_eq!(
            pages.first(),
            Some(&1),
            "first result must arrive before the final peer reply"
        );
        assert_eq!(
            searches.load(Ordering::SeqCst),
            1,
            "snapshots must not repeat network discovery"
        );
        assert!(snapshots.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn server_addresses() {
        let o = Origin::parse("music.example.org").unwrap();
        assert_eq!(o.base_url(), "https://music.example.org");
        let o = Origin::parse("https://Music.Example.org:8443/lib/").unwrap();
        assert_eq!(o.base_url(), "https://music.example.org:8443/lib");
        assert!(Origin::parse("http://127.0.0.1:8740").is_ok());
        assert!(Origin::parse("http://localhost:8740").is_ok());

        assert!(Origin::parse("http://music.example.org").is_err());
        assert!(Origin::parse("ftp://music.example.org").is_err());
        assert!(Origin::parse("https://user:pw@music.example.org").is_err());
        assert!(Origin::parse("https://music.example.org/?x=1").is_err());
        assert!(Origin::parse("").is_err());
    }

    #[test]
    fn token_only_goes_to_the_library() {
        let o = Origin::parse("https://music.example.org/lib").unwrap();
        assert!(o.covers("https://music.example.org/lib/api/v1/tracks/1/audio"));
        assert!(o.covers("https://music.example.org:443/lib/api/v1/tracks/1/audio"));
        assert!(!o.covers("http://music.example.org/lib/api/v1/tracks/1/audio"));
        assert!(!o.covers("https://music.example.org.evil.com/lib/api/v1/tracks/1/audio"));
        assert!(!o.covers("https://music.example.org:444/lib/api/v1/tracks/1/audio"));
        assert!(!o.covers("https://music.example.org/other/api/v1/tracks/1/audio"));
        assert!(!o.covers("https://music.example.org/lib/api/v1/../../x"));
        assert!(!o.covers("https://evil.com/?https://music.example.org/lib/api/v1/"));
    }
}
