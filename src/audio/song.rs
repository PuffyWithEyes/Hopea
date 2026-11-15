// SPDX-FileCopyrightText: 2022  Emmanuele Bassi
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    cell::{Cell, RefCell},
    convert::TryInto,
    fmt::{self, Display, Formatter},
    path::PathBuf,
    time::Instant,
};

use glib::{
    ParamSpec, ParamSpecBoolean, ParamSpecDouble, ParamSpecObject, ParamSpecString, ParamSpecUInt,
    Value,
};
use gtk::{gdk, gio, glib, prelude::*, subclass::prelude::*};
use hopea_common::api::Track;
use itertools::Itertools;
use lofty::{
    prelude::{Accessor, TaggedFileExt},
    probe::Probe,
    tag::ItemKey,
};
use log::{debug, warn};
use once_cell::sync::Lazy;
use sha2::{Digest, Sha256};

use crate::{
    audio::cover_cache::{CoverArt, CoverCache},
    i18n::i18n,
};

#[derive(Debug, Clone)]
pub struct SongData {
    artist: Option<String>,
    title: Option<String>,
    album: Option<String>,
    cover_art: Option<CoverArt>,
    cover_uuid: Option<String>,
    uuid: Option<String>,
    // Same full-file SHA-256 as the sharing node, independent of filename,
    // display tags, stream URL and the UUID used for saved UI customizations.
    content_hash: Option<String>,
    duration: u64,
    duration_ms: Option<u64>,
    file: gio::File,
    // Set for songs streamed from the shared library
    remote_id: Option<i64>,
}

/// File I/O and tag parsing can run outside GTK's main thread.
struct PreparedSong {
    file: gio::File,
    path: PathBuf,
    tagged_file: lofty::file::TaggedFile,
    content_hash: Option<String>,
}

impl PreparedSong {
    fn read(uri: &str) -> Option<Self> {
        let file = gio::File::for_uri(uri);
        let path = match file.path() {
            Some(path) => path,
            None => {
                warn!("Not a local file: {}", uri);
                return None;
            }
        };

        let (mime_type, _) = gio::content_type_guess(Some(&path), None);
        if !mime_type.starts_with("audio") || mime_type.ends_with("x-mpegurl") {
            return None;
        }

        let file_probe = match Probe::open(&path) {
            Ok(p) => p,
            Err(e) => {
                warn!("Unable to open file {:?}: {}", path, e);
                return None;
            }
        };

        let tagged_file = match file_probe.guess_file_type() {
            Ok(t) => match t.read() {
                Ok(f) => f,
                Err(e) => {
                    warn!("Unable to read parsed file {:?}: {}", path, e);
                    return None;
                }
            },
            Err(e) => {
                warn!("Unable to parse file type {:?}: {}", path, e);
                return None;
            }
        };

        let content_hash = file_hash(&path);
        Some(Self {
            file,
            path,
            tagged_file,
            content_hash,
        })
    }
}

impl SongData {
    pub fn artist(&self) -> Option<&str> {
        self.artist.as_deref()
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn album(&self) -> Option<&str> {
        self.album.as_deref()
    }

    pub fn uuid(&self) -> Option<&str> {
        self.uuid.as_deref()
    }

    pub fn cover_uuid(&self) -> Option<&str> {
        self.cover_uuid.as_deref()
    }

    pub fn duration(&self) -> u64 {
        self.duration
    }

    pub fn cover_texture(&self) -> Option<&gdk::Texture> {
        if let Some(cover) = &self.cover_art {
            return Some(cover.texture());
        }

        None
    }

    pub fn cover_palette(&self) -> Option<&Vec<gdk::RGBA>> {
        if let Some(cover) = &self.cover_art {
            return Some(cover.palette());
        }

        None
    }

    pub fn cover_cache(&self) -> Option<&PathBuf> {
        if let Some(cover) = &self.cover_art {
            return cover.cache();
        }

        None
    }

    pub fn from_uri(uri: &str) -> Self {
        PreparedSong::read(uri)
            .map(Self::from_prepared)
            .unwrap_or_default()
    }

    fn from_prepared(prepared: PreparedSong) -> Self {
        let now = Instant::now();
        let PreparedSong {
            file,
            path,
            tagged_file,
            content_hash,
        } = prepared;
        let uri = file.uri();

        let mut cover_cache = CoverCache::global().lock().unwrap();

        let mut artist = None;
        let mut title = None;
        let mut album = None;
        let mut cover_art = None;
        let mut cover_uuid = None;
        if let Some(tag) = tagged_file.primary_tag() {
            debug!("Found primary tag");
            artist = Some(tag.get_strings(ItemKey::TrackArtist).join(", "));
            title = tag.title().map(|s| s.to_string());
            album = tag.album().map(|s| s.to_string());
        } else {
            warn!("Unable to load primary tag for: {}", uri);
            for tag in tagged_file.tags() {
                debug!("Found tag: {:?}", tag.tag_type());
                artist = Some(tag.get_strings(ItemKey::TrackArtist).join(", "));
                title = tag.title().map(|s| s.to_string());
                album = tag.album().map(|s| s.to_string());
                if artist.is_some() && title.is_some() {
                    break;
                }
            }
        };

        // Cover art may live in a different tag, or next to completely
        // untagged audio. All constructors (including queue restoration)
        // go through this path.
        let empty_tag = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
        for tag in tagged_file.tags().iter().chain(std::iter::once(&empty_tag)) {
            if let Some((art, uuid)) = cover_cache.cover_art(&path, tag) {
                cover_art = Some(art);
                cover_uuid = Some(uuid);
                break;
            }
        }
        if cover_art.is_none() {
            debug!("No cover art in audio tags, per-track sidecar or album folder");
        }

        let uuid = match file.query_info(
            "standard::display-name",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        ) {
            Ok(info) => {
                let mut hasher = Sha256::new();

                hasher.update(info.display_name().as_str());

                if let Some(ref artist) = artist {
                    hasher.update(artist);
                }
                if let Some(ref title) = title {
                    hasher.update(title);
                }
                if let Some(ref album) = album {
                    hasher.update(album);
                }

                Some(hex::encode(hasher.finalize()))
            }
            _ => None,
        };

        let properties = lofty::prelude::AudioFile::properties(&tagged_file);
        let duration = properties.duration().as_secs();
        drop(cover_cache);

        debug!(
            "Song {:?} ('{:?}') loading time: {} ms",
            &uuid,
            &title,
            now.elapsed().as_millis()
        );

        SongData {
            artist,
            title,
            album,
            cover_art,
            cover_uuid,
            uuid,
            content_hash,
            duration,
            duration_ms: properties.duration().as_millis().try_into().ok(),
            file,
            remote_id: None,
        }
    }

    pub fn from_remote(track: &Track, audio_uri: &str, cover: Option<&glib::Bytes>) -> Self {
        // The server names content by hash, so this is stable and unique
        let uuid = format!("hopea-{}", track.sha256);

        // Named by the picture itself, so that the songs of an album share
        // one copy of it in memory
        let cover_uuid =
            cover.map(|bytes| format!("hopea-cover-{}", hex::encode(Sha256::digest(bytes))));
        let cover_art = cover.zip(cover_uuid.as_deref()).and_then(|(bytes, key)| {
            let mut cover_cache = CoverCache::global().lock().unwrap();
            cover_cache.cover_art_from_bytes(key, bytes)
        });
        let cover_uuid = cover_art.as_ref().and(cover_uuid);

        SongData {
            artist: track.artist.clone(),
            // A title that is really a file name is shown without its extension
            title: Some(hopea_common::AudioFormat::strip_extension(&track.title).to_string()),
            album: track.album.clone(),
            cover_art,
            cover_uuid,
            uuid: Some(uuid),
            content_hash: Some(track.sha256.clone()),
            duration: track.duration_ms / 1000,
            duration_ms: Some(track.duration_ms),
            file: gio::File::for_uri(audio_uri),
            remote_id: Some(track.id),
        }
    }

    pub fn uri(&self) -> String {
        self.file.uri().to_string()
    }

    pub fn file(&self) -> gio::File {
        self.file.clone()
    }
}

impl Default for SongData {
    fn default() -> Self {
        SongData {
            artist: Some("Invalid Artist".to_string()),
            title: Some("Invalid Title".to_string()),
            album: Some("Invalid Album".to_string()),
            cover_art: None,
            cover_uuid: None,
            uuid: None,
            content_hash: None,
            duration: 0,
            duration_ms: None,
            file: gio::File::for_path("/does-not-exist"),
            remote_id: None,
        }
    }
}

fn file_hash(path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let modified = metadata.modified().ok()?;
    let stamp = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    // Restoring a large playlist must not reread gigabytes of unchanged
    // audio on every launch. A cache miss is harmless; changes invalidate it.
    let mut key = Sha256::new();
    key.update(path.as_os_str().as_encoded_bytes());
    key.update(metadata.len().to_le_bytes());
    key.update(stamp.as_nanos().to_le_bytes());
    let directory = crate::paths::cache_dir().join("song-hashes");
    let cached = directory.join(hex::encode(key.finalize()));
    if let Ok(hash) = std::fs::read_to_string(&cached) {
        if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(hash);
        }
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let size = file.read(&mut buffer).ok()?;
        if size == 0 {
            break;
        }
        digest.update(&buffer[..size]);
    }
    let after = file.metadata().ok()?;
    if after.len() != metadata.len() || after.modified().ok()? != modified {
        return None;
    }
    let hash = hex::encode(digest.finalize());
    if std::fs::create_dir_all(directory).is_ok() {
        // A partial cache write is rejected by the length check above.
        let _ = std::fs::write(cached, &hash);
    }
    Some(hash)
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct Song {
        pub data: RefCell<SongData>,
        pub playing: Cell<bool>,
        pub selected: Cell<bool>,
        /// In the library: shared from here, or starred
        pub favorite: Cell<bool>,
        /// The library track this song is, if it is one
        pub library_id: Cell<Option<i64>>,
        /// Shared from this computer
        pub mine: Cell<bool>,
        /// How a song from the network is coming along, in words
        pub status: RefCell<String>,
        /// How much of a song from the network is here, from 0 to 1
        pub progress: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Song {
        const NAME: &'static str = "HopeaSong";
        type Type = super::Song;
    }

    impl ObjectImpl for Song {
        fn properties() -> &'static [ParamSpec] {
            static PROPERTIES: Lazy<Vec<ParamSpec>> = Lazy::new(|| {
                vec![
                    ParamSpecString::builder("uri").construct_only().build(),
                    ParamSpecString::builder("artist").read_only().build(),
                    ParamSpecString::builder("title").read_only().build(),
                    ParamSpecString::builder("album").read_only().build(),
                    ParamSpecUInt::builder("duration").read_only().build(),
                    ParamSpecObject::builder::<gdk::Texture>("cover")
                        .read_only()
                        .build(),
                    ParamSpecBoolean::builder("playing").build(),
                    ParamSpecBoolean::builder("selected").build(),
                    ParamSpecBoolean::builder("favorite").build(),
                    ParamSpecString::builder("status").build(),
                    ParamSpecDouble::builder("progress")
                        .minimum(0.0)
                        .maximum(1.0)
                        .build(),
                ]
            });
            PROPERTIES.as_ref()
        }

        fn set_property(&self, _id: usize, value: &Value, pspec: &ParamSpec) {
            match pspec.name() {
                "uri" => {
                    let obj = self.obj();
                    if let Ok(p) = value.get::<&str>() {
                        self.data.replace(SongData::from_uri(p));
                        obj.notify("artist");
                        obj.notify("title");
                        obj.notify("album");
                        obj.notify("duration");
                        obj.notify("cover");
                    }
                }
                "playing" => {
                    let p = value.get::<bool>().expect("Value must be a boolean");
                    self.playing.set(p);
                }
                "selected" => {
                    let p = value.get::<bool>().expect("Value must be a boolean");
                    self.selected.set(p);
                }
                "favorite" => {
                    let p = value.get::<bool>().expect("Value must be a boolean");
                    self.favorite.set(p);
                }
                "status" => {
                    let p = value.get::<Option<String>>().ok().flatten();
                    self.status.replace(p.unwrap_or_default());
                }
                "progress" => {
                    let p = value.get::<f64>().expect("Value must be a double");
                    self.progress.set(p);
                }
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &ParamSpec) -> Value {
            let obj = self.obj();
            match pspec.name() {
                "artist" => obj.artist().to_value(),
                "title" => obj.title().to_value(),
                "album" => obj.album().to_value(),
                "duration" => obj.duration().to_value(),
                "uri" => obj.uri().to_value(),
                "cover" => obj.cover_texture().to_value(),
                "playing" => self.playing.get().to_value(),
                "selected" => self.selected.get().to_value(),
                "favorite" => self.favorite.get().to_value(),
                "status" => self.status.borrow().to_value(),
                "progress" => self.progress.get().to_value(),
                _ => unimplemented!(),
            }
        }
    }
}

glib::wrapper! {
    pub struct Song(ObjectSubclass<imp::Song>);
}

impl Song {
    pub fn set_duration(&self, seconds: u64) {
        {
            let mut data = self.imp().data.borrow_mut();
            if data.duration != seconds || data.duration_ms.is_none() {
                data.duration_ms = seconds.checked_mul(1000);
            }
            data.duration = seconds;
        }
        self.notify("duration");
    }
    /// Artwork for collection rows, including local overrides, without reading
    /// audio tags or hashing a large file on GTK's main thread.
    pub async fn local_cover(uri: String) -> Option<gdk::Texture> {
        let data = gio::spawn_blocking(move || SongData::from_uri(&uri))
            .await
            .ok()?;
        let song = Self::empty();
        song.imp().data.replace(data);
        crate::song_edits::apply_cover(&song);
        song.cover_texture()
    }

    pub fn set_cover_bytes(&self, bytes: &glib::Bytes) {
        let key = format!("hopea-import-cover-{}", hex::encode(Sha256::digest(bytes)));
        let art = CoverCache::global()
            .lock()
            .unwrap()
            .cover_art_from_bytes(&key, bytes);
        if let Some(art) = art {
            let mut data = self.imp().data.borrow_mut();
            data.cover_art = Some(art);
            data.cover_uuid = Some(key);
            drop(data);
            self.notify("cover");
        }
    }

    pub fn new(uri: &str) -> Self {
        let song = glib::Object::builder::<Self>().property("uri", uri).build();
        crate::artwork::watch_song(&song);
        crate::song_edits::apply_cover(&song);
        song
    }

    pub fn from_uri(uri: &str) -> Result<Song, &'static str> {
        let res = Song::new(uri);
        if res.equals(&Song::default()) {
            Err("Invalid song")
        } else {
            res.set_property("progress", 1.0_f64);
            res.set_property("status", crate::i18n::i18n("On this computer"));
            // A file shared from here is in the library
            if let Some(id) = crate::library::shared_files::id_for(uri) {
                res.set_library(Some(id), true, false);
            }
            Ok(res)
        }
    }

    /// Only the parsed file data crosses threads; GTK objects stay on the UI thread.
    pub async fn load_local(uri: String) -> Result<Song, &'static str> {
        let prepared = gio::spawn_blocking(move || PreparedSong::read(&uri))
            .await
            .map_err(|_| "Unable to read song")?
            .ok_or("Invalid song")?;
        let song = Self::empty();
        song.imp().data.replace(SongData::from_prepared(prepared));
        crate::artwork::watch_song(&song);
        crate::song_edits::apply_cover(&song);
        song.set_property("progress", 1.0_f64);
        song.set_property("status", i18n("On this computer"));
        if let Some(id) = crate::library::shared_files::id_for(&song.uri()) {
            song.set_library(Some(id), true, false);
        }
        Ok(song)
    }

    pub fn empty() -> Self {
        glib::Object::new()
    }

    /// An audition is ephemeral, never a queue entry or a published file.
    /// Use catalogue metadata without re-reading/hashing audio on the UI thread.
    pub fn from_import(track: &hopea_common::service_import::CatalogTrack, uri: &str) -> Self {
        let song = Self::empty();
        song.imp().data.replace(SongData {
            title: Some(track.title.clone()),
            artist: track.artist.clone(),
            album: track.album.clone(),
            duration: track.duration_ms.unwrap_or_default() / 1000,
            duration_ms: track.duration_ms,
            file: gio::File::for_uri(uri),
            uuid: Some(format!(
                "hopea-audition-{}",
                hex::encode(Sha256::digest(uri.as_bytes()))
            )),
            ..SongData::default()
        });
        crate::artwork::watch_song(&song);
        song
    }

    pub fn from_remote(track: &Track, audio_uri: &str, cover: Option<&glib::Bytes>) -> Self {
        let song = Self::empty();
        song.imp()
            .data
            .replace(SongData::from_remote(track, audio_uri, cover));
        song.imp().favorite.set(track.favorite);
        song.imp().library_id.set(Some(track.id));
        song.imp().mine.set(track.mine);
        song.set_property("progress", if track.local { 1.0_f64 } else { 0.0_f64 });
        song.set_property(
            "status",
            if track.local {
                i18n("On this computer")
            } else {
                i18n("Available on the network")
            },
        );
        crate::artwork::watch_song(&song);
        crate::song_edits::apply_cover(&song);
        song
    }

    /// How much of a song from the network is here, from 0 to 1.
    pub fn progress(&self) -> f64 {
        self.imp().progress.get()
    }

    /// The library track this song is, whether it was shared from a file
    /// here or comes from the library.
    pub fn library_id(&self) -> Option<i64> {
        self.imp().library_id.get()
    }

    /// Whether the song is shared from this computer.
    pub fn is_mine(&self) -> bool {
        self.imp().mine.get()
    }

    /// Puts the song in the library or takes it out.
    pub fn set_library(&self, id: Option<i64>, mine: bool, favorite: bool) {
        self.imp().library_id.set(id);
        self.imp().mine.set(mine);
        self.set_property("favorite", favorite);
    }

    /// The shared library track id, for songs streamed from the library
    pub fn remote_id(&self) -> Option<i64> {
        self.imp().data.borrow().remote_id
    }

    pub fn equals(&self, other: &Self) -> bool {
        self.uri() == other.uri()
            // Sharing rebuilds the container to remove unsafe metadata. Its
            // file hash can differ from the original, but both refer to the
            // same entry in this player's local node database.
            || self.library_id().zip(other.library_id()).is_some_and(|(a, b)| a == b)
            || self
                .content_hash()
                .zip(other.content_hash())
                .is_some_and(|(a, b)| a == b)
    }

    pub fn content_hash(&self) -> Option<String> {
        self.imp().data.borrow().content_hash.clone()
    }

    pub fn uri(&self) -> String {
        self.imp().data.borrow().uri()
    }

    /// The artist, if the song says.
    pub fn known_artist(&self) -> Option<String> {
        self.imp().data.borrow().artist().map(str::to_string)
    }

    pub fn artist(&self) -> String {
        match self.imp().data.borrow().artist() {
            Some(artist) => artist.to_string(),
            None => i18n("Unknown artist"),
        }
    }

    pub fn title(&self) -> String {
        match self.imp().data.borrow().title() {
            Some(title) => title.to_string(),
            None => i18n("Unknown title"),
        }
    }

    pub fn album(&self) -> String {
        match self.imp().data.borrow().album() {
            Some(album) => album.to_string(),
            None => i18n("Unknown album"),
        }
    }

    pub fn has_cover_art(&self) -> bool {
        self.imp().data.borrow().cover_texture().is_some()
    }

    pub fn cover_texture(&self) -> Option<gdk::Texture> {
        self.imp()
            .data
            .borrow()
            .cover_texture()
            .cloned()
            .or_else(crate::artwork::texture)
    }

    pub fn cover_color(&self) -> Option<gdk::RGBA> {
        self.imp().data.borrow().cover_palette().map(|p| p[0])
    }

    pub fn cover_palette(&self) -> Option<Vec<gdk::RGBA>> {
        self.imp().data.borrow().cover_palette().cloned()
    }

    pub fn cover_uuid(&self) -> Option<String> {
        self.imp().data.borrow().cover_uuid().map(|s| s.to_string())
    }

    pub fn cover_cache(&self) -> Option<PathBuf> {
        let path = self.imp().data.borrow().cover_cache().cloned()?;
        // Cache cleanup does not discard the texture held by a queued song.
        // Recreate its MPRIS image when it is needed again.
        if !path.is_file() {
            if let Some(texture) = self.cover_texture() {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if texture.save_to_png(&path).is_err() {
                    return None;
                }
            }
        }
        Some(path)
    }

    pub fn duration(&self) -> u64 {
        self.imp().data.borrow().duration()
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.imp().data.borrow().duration_ms
    }

    pub fn playing(&self) -> bool {
        self.imp().playing.get()
    }

    pub fn set_playing(&self, playing: bool) {
        let was_playing = self.imp().playing.replace(playing);
        if was_playing != playing {
            self.notify("playing");
        }
    }

    pub fn selected(&self) -> bool {
        self.imp().selected.get()
    }

    pub fn set_selected(&self, selected: bool) {
        let was_selected = self.imp().selected.replace(selected);
        if was_selected != selected {
            self.notify("selected");
        }
    }

    pub fn uuid(&self) -> Option<String> {
        self.imp().data.borrow().uuid().map(|s| s.to_string())
    }

    pub fn search_key(&self) -> String {
        format!("{} {} {}", self.artist(), self.album(), self.title())
    }

    pub fn file(&self) -> gio::File {
        self.imp().data.borrow().file()
    }
}

impl Default for Song {
    fn default() -> Self {
        Self::empty()
    }
}

impl Display for Song {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "Song {{ uuid: {}, uri: {}, artist: '{}', title: '{}' }}",
            self.uuid().unwrap(),
            self.uri(),
            self.artist(),
            self.title()
        )
    }
}
