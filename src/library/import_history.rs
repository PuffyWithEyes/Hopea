// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Successful imports only. No credentials or signed service URLs are persisted.
use crate::i18n::i18n;
use gtk::{gio, glib, prelude::*};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use soup::prelude::*;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Imported {
    pub node_id: Option<i64>,
    #[serde(default)]
    pub node_sha256: Option<String>,
    pub cover_hash: Option<String>,
    pub metadata: bool,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
}

impl Imported {
    pub fn matches_audio(&self, hash: &str, title: &str, artist: Option<&str>) -> bool {
        self.node_sha256.as_deref().map_or_else(
            || self.title == title && self.artist.as_deref() == artist,
            |saved| saved == hash,
        )
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct History(pub BTreeMap<String, Imported>);

pub fn key(service: &str, id: &str) -> String {
    format!("{service}:{id}")
}
pub fn audio_path(directory: &Path, service: &str, id: &str) -> PathBuf {
    directory.join(format!(
        "{}.flac",
        hex::encode(Sha256::digest(key(service, id)))
    ))
}
fn history_path() -> PathBuf {
    crate::paths::data_dir().join("import-history.json")
}
impl History {
    pub fn load() -> Result<Self, String> {
        match std::fs::read(history_path()) {
            Ok(data) => serde_json::from_slice(&data)
                .map_err(|_| i18n("Import history is damaged; refusing to import duplicates.")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) => Err(i18n("Unable to read import history")),
        }
    }
    pub async fn save(&self) -> Result<(), String> {
        let snapshot = self.clone();
        gio::spawn_blocking(move || snapshot.save_sync())
            .await
            .map_err(|_| i18n("Unable to save import history"))?
    }

    fn save_sync(&self) -> Result<(), String> {
        let path = history_path();
        let result = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, serde_json::to_vec(self)?)?;
            std::fs::rename(tmp, path)
        })();
        result
            .map_err(|_| i18n("Unable to save import history; import stopped to avoid duplicates."))
    }
}

/// Images are fetched without account cookies, capped and decoded off the UI thread.
pub async fn fetch_cover(url: &str) -> Result<glib::Bytes, String> {
    if !gio::TlsBackend::default().supports_tls() {
        return Err(i18n("HTTPS support is missing. Install the GIO TLS module (glib-networking) and restart Hopea from the updated environment."));
    }
    let url = if url.starts_with("//") {
        format!("https:{url}")
    } else {
        url.into()
    };
    let uri =
        glib::Uri::parse(&url, glib::UriFlags::NONE).map_err(|_| i18n("Invalid cover address"))?;
    let host = uri.host().unwrap_or_default();
    let allowed = [
        "userapi.com",
        "vk.com",
        "vk.ru",
        "vkuserphoto.ru",
        "vkuserphoto.net",
        "vkcdn.ru",
        "yandex.net",
        "yandex.ru",
        "ytimg.com",
        "googleusercontent.com",
        "ggpht.com",
        "scdn.co",
        "spotifycdn.com",
    ]
    .iter()
    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
    if uri.scheme() != "https" || uri.userinfo().is_some() || !allowed {
        return Err(i18n("The service returned an unsupported cover address"));
    }
    let http = soup::Session::new();
    http.set_timeout(30);
    let message = soup::Message::new("GET", &url).map_err(|_| i18n("Invalid cover address"))?;
    message.set_flags(soup::MessageFlags::NO_REDIRECT);
    let stream = http
        .send_future(&message, glib::Priority::DEFAULT)
        .await
        .map_err(|_| i18n("Unable to download cover art"))?;
    if message.status() != soup::Status::Ok {
        return Err(i18n("Cover art is unavailable"));
    }
    let mut data = Vec::new();
    loop {
        let chunk = stream
            .read_bytes_future(65536, glib::Priority::DEFAULT)
            .await
            .map_err(|_| i18n("Unable to download cover art"))?;
        if chunk.is_empty() {
            break;
        }
        if data.len() + chunk.len() > 8 * 1024 * 1024 {
            return Err(i18n("Cover art is too large"));
        }
        data.extend_from_slice(&chunk);
    }
    gio::spawn_blocking(move || {
        // Reuse the node's PNG/JPEG dimension and allocation limits rather
        // than handing an untrusted image straight to a GTK decoder.
        hopea_node::ingest::reencode_cover(&data)
            .map(glib::Bytes::from_owned)
            .ok_or_else(|| i18n("Invalid or unsupported cover image"))
    })
    .await
    .map_err(|_| i18n("Unable to prepare cover image"))?
}

pub fn cover_hash(bytes: &glib::Bytes) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub async fn save_cover(audio: &Path, bytes: &glib::Bytes) -> Result<(), String> {
    let path = audio.with_extension("cover.jpg");
    let bytes = bytes.clone();
    gio::spawn_blocking(move || {
        let tmp = path.with_extension("tmp");
        std::fs::create_dir_all(path.parent().unwrap())
            .and_then(|_| std::fs::write(&tmp, bytes))
            .and_then(|_| std::fs::rename(tmp, path))
            .map_err(|_| i18n("Unable to save cover art"))
    })
    .await
    .map_err(|_| i18n("Unable to save cover art"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Opt-in real CDN check. URLs are supplied in a private local file and
    /// never included in assertion messages or logs.
    #[test]
    #[ignore = "requires explicitly supplied live cover URLs and network"]
    fn live_service_cover_download() {
        let path = std::env::var_os("HOPEA_TEST_COVER_URL_FILE").expect("cover URL file");
        let rows: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let context = glib::MainContext::new();
        assert!(
            gio::TlsBackend::default().supports_tls(),
            "GIO TLS backend is missing"
        );
        context
            .with_thread_default(|| {
                for row in rows.as_array().unwrap().iter().take(3) {
                    let bytes = context
                        .block_on(fetch_cover(row["url"].as_str().unwrap()))
                        .unwrap_or_else(|reason| panic!("Cover download failed: {}", reason));
                    assert!(!bytes.is_empty());
                }
            })
            .unwrap();
    }

    #[test]
    fn stable_service_keys_and_safe_paths() {
        assert_ne!(key("vk", "123"), key("youtube", "123"));
        let path = audio_path(Path::new("/tmp/import"), "vk", "../../secret");
        assert_eq!(path.parent(), Some(Path::new("/tmp/import")));
        let history = History(BTreeMap::from([(
            key("vk", "123"),
            Imported {
                node_id: Some(7),
                cover_hash: Some("hash".into()),
                ..Default::default()
            },
        )]));
        let restored: History =
            serde_json::from_slice(&serde_json::to_vec(&history).unwrap()).unwrap();
        assert_eq!(restored.0["vk:123"].node_id, Some(7));
    }

    #[test]
    fn reused_node_ids_do_not_skip_different_audio() {
        let record = Imported {
            node_id: Some(7),
            node_sha256: Some("original-file".into()),
            title: "Song".into(),
            ..Default::default()
        };
        assert!(record.matches_audio("original-file", "Renamed song", None));
        assert!(!record.matches_audio("replacement-file", "Song", None));
    }
}

/// Temporary, shared full-resolution artwork for one chooser/import job.
/// URLs exist only as in-memory keys; disk names contain their hashes.
#[derive(Default)]
pub struct CoverCache {
    folder: std::cell::RefCell<Option<PathBuf>>,
    entries:
        std::cell::RefCell<BTreeMap<String, std::rc::Rc<futures::lock::Mutex<Option<PathBuf>>>>>,
}
impl CoverCache {
    pub async fn get(&self, url: &str) -> Result<glib::Bytes, String> {
        self.get_with(url, fetch_cover(url)).await
    }

    async fn get_with(
        &self,
        url: &str,
        fetch: impl std::future::Future<Output = Result<glib::Bytes, String>>,
    ) -> Result<glib::Bytes, String> {
        let entry = self
            .entries
            .borrow_mut()
            .entry(url.into())
            .or_default()
            .clone();
        // The same album may be requested by several visible rows at once.
        let mut saved = entry.lock().await;
        if let Some(path) = saved.as_ref() {
            if let Ok(bytes) =
                crate::utils::read_file_limited(&gio::File::for_path(path), 8 * 1024 * 1024).await
            {
                return Ok(glib::Bytes::from_owned(bytes));
            }
        }
        let bytes = fetch.await?;
        let folder = {
            let mut folder = self.folder.borrow_mut();
            if folder.is_none() {
                *folder = Some(
                    glib::mkdtemp(std::env::temp_dir().join("hopea-import-art-XXXXXX"))
                        .ok_or_else(|| i18n("Unable to create the temporary cover cache"))?,
                );
            }
            folder.as_ref().unwrap().clone()
        };
        let path = folder.join(format!(
            "{}.jpg",
            hex::encode(Sha256::digest(url.as_bytes()))
        ));
        gio::File::for_path(&path)
            .replace_contents_future(bytes.clone(), None, false, gio::FileCreateFlags::PRIVATE)
            .await
            .map_err(|_| i18n("Unable to save the temporary cover cache"))?;
        *saved = Some(path);
        Ok(bytes)
    }
}
impl Drop for CoverCache {
    fn drop(&mut self) {
        if let Some(folder) = self.folder.take() {
            let _ = std::thread::spawn(move || {
                let _ = std::fs::remove_dir_all(folder);
            });
        }
    }
}

#[cfg(test)]
mod cover_cache_tests {
    use super::*;
    #[test]
    fn preview_and_import_share_one_download_even_when_requested_concurrently() {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let cache = CoverCache::default();
                    let requests = std::cell::Cell::new(0);
                    let fetch = || async {
                        requests.set(requests.get() + 1);
                        glib::timeout_future(std::time::Duration::from_millis(10)).await;
                        Ok(glib::Bytes::from_static(b"normalized cover bytes"))
                    };
                    let (a, b) = futures::join!(
                        cache.get_with("private-url", fetch()),
                        cache.get_with("private-url", fetch())
                    );
                    assert_eq!(a.unwrap(), b.unwrap());
                    assert_eq!(requests.get(), 1);
                    assert_eq!(
                        cache
                            .get_with("private-url", fetch())
                            .await
                            .unwrap()
                            .as_ref(),
                        b"normalized cover bytes"
                    );
                    assert_eq!(
                        requests.get(),
                        1,
                        "import must reuse the chooser's original bytes"
                    );
                    cache.get_with("changed-cover-url", fetch()).await.unwrap();
                    assert_eq!(requests.get(), 2, "a different cover must be fetched");
                })
            })
            .unwrap();
    }
}

/// Retire the old library entry only after the new audio and a verified backup
/// of the old bytes both exist. Never uses the user's delete/trash preference.
pub async fn replace_preserving_file(
    old: &super::duplicates::Candidate,
    new: &hopea_common::api::Track,
) -> Result<(), String> {
    let client = super::client().ok_or_else(|| i18n("Not connected"))?;
    replace_at(
        &client,
        old,
        new,
        crate::paths::data_dir().join("replaced-audio"),
    )
    .await
}

async fn replace_at(
    client: &super::LibraryClient,
    old: &super::duplicates::Candidate,
    new: &hopea_common::api::Track,
    folder: PathBuf,
) -> Result<(), String> {
    let id = old
        .id
        .ok_or_else(|| i18n("The existing track is unavailable"))?;
    if id == new.id {
        return Ok(());
    }

    let replacement = client.track(new.id).await.map_err(|e| e.to_string())?;
    if !replacement.local || replacement.sha256 != new.sha256 {
        return Err(i18n("The new version has not finished importing"));
    }
    let track = client.track(id).await.map_err(|e| e.to_string())?;
    if !old.matches(&track) {
        return Err(i18n("The existing track has changed since comparison"));
    }
    if !track.local {
        return Err(i18n("The existing file cannot be preserved yet"));
    }
    let directory = folder.clone();
    gio::spawn_blocking(move || std::fs::create_dir_all(directory))
        .await
        .map_err(|_| i18n("Unable to create the backup folder"))?
        .map_err(|_| i18n("Unable to create the backup folder"))?;
    let destination = folder.join(format!(
        "{}.{}",
        track.sha256,
        track.format.canonical_extension()
    ));
    let temporary = folder.join(format!("{}.part", glib::uuid_string_random()));
    let file = gio::File::for_path(&temporary);
    if let Err(error) = client.save_file(id, &file).await {
        let _ = file.delete_future(glib::Priority::DEFAULT).await;
        return Err(error.to_string());
    }
    let expected_hash = track.sha256.clone();
    let expected_size = track.size;
    let verify_path = temporary.clone();
    let result = gio::spawn_blocking(move || {
        use std::io::Read;
        let mut source =
            std::fs::File::open(&verify_path).map_err(|_| "Unable to read the backup")?;
        let mut digest = Sha256::new();
        let mut size = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let n = source
                .read(&mut buffer)
                .map_err(|_| "Unable to verify the backup")?;
            if n == 0 {
                break;
            }
            size += n as u64;
            digest.update(&buffer[..n]);
        }
        if size != expected_size || hex::encode(digest.finalize()) != expected_hash {
            return Err("The saved backup did not match the existing audio");
        }
        source.sync_all().map_err(|_| "Unable to save the backup")?;
        std::fs::rename(&verify_path, destination).map_err(|_| "Unable to keep the backup")
    })
    .await
    .map_err(|_| i18n("Backup verification was interrupted"))?;
    if let Err(reason) = result {
        let _ = file.delete_future(glib::Priority::DEFAULT).await;
        return Err(reason.into());
    }
    // Recheck the identity before removing anything after asynchronous I/O.
    let current = client.track(id).await.map_err(|e| e.to_string())?;
    if current.sha256 != track.sha256 {
        return Err(i18n("The existing track changed during import"));
    }
    if track.mine {
        client.delete(id).await.map_err(|e| e.to_string())
    } else {
        client
            .set_favorite(id, false)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod replacement_tests {
    use super::*;

    struct Server {
        folder: PathBuf,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        thread: Option<std::thread::JoinHandle<()>>,
        url: String,
        token: String,
    }
    impl Server {
        fn new() -> Self {
            let folder =
                glib::mkdtemp(std::env::temp_dir().join("hopea-replacement-test-XXXXXX")).unwrap();
            let data = folder.join("node");
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let (ready, receiver) = std::sync::mpsc::channel();
            let thread = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async {
                    let token = hopea_node::auth::generate_token();
                    let node =
                        hopea_node::Node::start(hopea_node::config::Config::isolated(data), &token)
                            .await
                            .unwrap();
                    ready
                        .send((format!("http://{}", node.api_addr), token))
                        .unwrap();
                    let _ = stopped.await;
                    node.shutdown().await;
                });
            });
            let (url, token) = receiver
                .recv_timeout(std::time::Duration::from_secs(20))
                .unwrap();
            Self {
                folder,
                stop: Some(stop),
                thread: Some(thread),
                url,
                token,
            }
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.stop.take().unwrap().send(());
            let _ = self.thread.take().unwrap().join();
            let _ = std::fs::remove_dir_all(&self.folder);
        }
    }

    #[test]
    fn replacement_preserves_verified_old_audio_and_failure_keeps_both_entries() {
        let server = Server::new();
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let client =
                        super::super::LibraryClient::new(&server.url, &server.token).unwrap();
                    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("node/tests/fixtures");
                    let mut tracks = Vec::new();
                    for name in ["tone.flac", "tone.mp3"] {
                        let path = root.join(name);
                        let track = client
                            .upload(
                                &gio::File::for_path(&path),
                                &format!("Artist - {name}"),
                                std::fs::metadata(&path).unwrap().len(),
                                &super::super::client::UploadCover::default(),
                                |_| {},
                            )
                            .await
                            .unwrap()
                            .track;
                        tracks.push(track);
                    }
                    let old = &tracks[0];
                    let new = &tracks[1];
                    assert_ne!(old.id, new.id);
                    let candidate = super::super::duplicates::Candidate {
                        id: Some(old.id),
                        sha256: Some(old.sha256.clone()),
                        title: old.title.clone(),
                        artist: old.artist.clone(),
                        duration_ms: Some(old.duration_ms),
                    };
                    let folder = server.folder.join("backups");
                    let blocked = server.folder.join("blocked");
                    std::fs::write(&blocked, b"not a directory").unwrap();
                    assert!(replace_at(&client, &candidate, new, blocked).await.is_err());
                    assert!(client.track(old.id).await.unwrap().local);
                    let mut unavailable = new.clone();
                    unavailable.id = i64::MAX;
                    assert!(
                        replace_at(&client, &candidate, &unavailable, folder.clone())
                            .await
                            .is_err()
                    );
                    assert!(client.track(old.id).await.unwrap().local);
                    let mut wrong = candidate.clone();
                    wrong.sha256 = Some("00".repeat(32));
                    assert!(replace_at(&client, &wrong, new, folder.clone())
                        .await
                        .is_err());
                    assert!(client.track(old.id).await.unwrap().local);
                    let original = std::fs::read(root.join("tone.flac")).unwrap();
                    replace_at(&client, &candidate, new, folder.clone())
                        .await
                        .unwrap();
                    assert!(matches!(
                        client.track(old.id).await,
                        Err(super::super::ClientError::NotFound)
                    ));
                    assert!(client.track(new.id).await.unwrap().local);
                    let backup = std::fs::read(folder.join(format!(
                        "{}.{}",
                        old.sha256,
                        old.format.canonical_extension()
                    )))
                    .unwrap();
                    assert_eq!(hex::encode(Sha256::digest(&backup)), old.sha256);
                    assert_eq!(backup.len() as u64, old.size);
                    assert_eq!(std::fs::read(root.join("tone.flac")).unwrap(), original);
                })
            })
            .unwrap();
    }
}
