// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Personal collections never rewrite the original album tags. Public files
//! contain content hashes, not local paths, node credentials or signed URLs.
use gtk::{gio, glib};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub const MAX_FILE: usize = 8 * 1024 * 1024;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Hashed provider membership identity, excluded from shared manifests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_key: Option<String>,
    pub title: String,
    pub artist: Option<String>,
    pub sha256: Option<String>,
    pub node_id: Option<i64>,
    pub local_uri: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub cover: Option<String>,
    pub tracks: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicAlbum {
    hopea_album: u32,
    name: String,
    cover: Option<String>,
    tracks: Vec<PublicTrack>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicTrack {
    title: String,
    artist: Option<String>,
    sha256: String,
}

impl Album {
    pub fn new() -> Self {
        Self {
            id: glib::uuid_string_random().into(),
            name: String::new(),
            cover: None,
            tracks: Vec::new(),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.id.len() != 36 || !self.id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            return Err("Invalid album identifier".into());
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 200 {
            return Err("Enter an album name (up to 200 characters)".into());
        }
        if self.tracks.len() > 20000 {
            return Err("An album can contain at most 20,000 songs".into());
        }
        for t in &self.tracks {
            if t.source_key
                .as_ref()
                .is_some_and(|key| key.len() != 64 || !key.bytes().all(|c| c.is_ascii_hexdigit()))
            {
                return Err("Invalid imported track identity".into());
            }
            if t.title.len() > 2000
                || t.artist.as_ref().is_some_and(|s| s.len() > 2000)
                || t.sha256
                    .as_ref()
                    .is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err("Invalid album track".into());
            }
            if t.local_uri
                .as_ref()
                .is_some_and(|s| !s.starts_with("file://") || s.len() > 16384)
            {
                return Err("Invalid local song location".into());
            }
        }
        if self
            .cover
            .as_ref()
            .is_some_and(|c| c.len() > 4 * 1024 * 1024)
        {
            return Err("Album cover is too large".into());
        }
        Ok(())
    }
    pub fn export(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        if self.tracks.is_empty() {
            return Err("Add songs before sharing the album".into());
        }
        let tracks = self
            .tracks
            .iter()
            .map(|t| {
                Ok(PublicTrack {
                    title: t.title.clone(),
                    artist: t.artist.clone(),
                    sha256: t
                        .sha256
                        .clone()
                        .ok_or("Share every song before exporting the album")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        serde_json::to_vec_pretty(&PublicAlbum {
            hopea_album: 1,
            name: self.name.clone(),
            cover: self.cover.clone(),
            tracks,
        })
        .map_err(|_| "Unable to encode album".into())
    }
    pub fn import(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_FILE {
            return Err("Album file is too large".into());
        }
        let public: PublicAlbum =
            serde_json::from_slice(bytes).map_err(|_| "This is not a Hopea album file")?;
        if public.hopea_album != 1 {
            return Err("Unsupported album file version".into());
        }
        let mut album = Self::new();
        album.name = public.name;
        album.cover = public.cover;
        album.tracks = public
            .tracks
            .into_iter()
            .map(|t| Entry {
                title: t.title,
                artist: t.artist,
                sha256: Some(t.sha256.to_ascii_lowercase()),
                ..Default::default()
            })
            .collect();
        album.validate()?;
        if let Some(cover) = &album.cover {
            let bytes = glib::base64_decode(cover);
            album.cover = Some(normalize_cover(&bytes)?);
        }
        Ok(album)
    }
}

pub fn normalize_cover(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_FILE {
        return Err("Choose a PNG or JPEG smaller than 8 MB".into());
    }
    hopea_node::ingest::reencode_cover(bytes)
        .map(|bytes| glib::base64_encode(&bytes).to_string())
        .ok_or_else(|| "Choose a valid PNG or JPEG cover".into())
}
fn folder() -> PathBuf {
    crate::paths::data_dir().join("collections")
}
fn read_limited(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| "Unable to open album")?
        .take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Unable to read album")?;
    if bytes.len() > MAX_FILE {
        return Err("Album file is too large".into());
    }
    Ok(bytes)
}
pub async fn list() -> Result<Vec<Album>, String> {
    gio::spawn_blocking(|| {
        let dir = folder();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut albums = Vec::new();
        for file in std::fs::read_dir(dir).map_err(|_| "Unable to read albums")? {
            let path = file.map_err(|_| "Unable to read albums")?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let album: Album = serde_json::from_slice(&read_limited(&path)?)
                .map_err(|_| "Unable to read a saved album")?;
            album.validate()?;
            albums.push(album);
        }
        albums.sort_by_key(|a| a.name.to_lowercase());
        Ok(albums)
    })
    .await
    .map_err(|_| "Unable to read albums")?
}
pub async fn load(id: String) -> Result<Option<Album>, String> {
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err("Invalid album identifier".into());
    }
    gio::spawn_blocking(move || {
        let path = folder().join(format!("{id}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let album: Album = serde_json::from_slice(&read_limited(&path)?)
            .map_err(|_| "Unable to read a saved album")?;
        album.validate()?;
        Ok(Some(album))
    })
    .await
    .map_err(|_| "Unable to read album")?
}

pub async fn save(album: Album) -> Result<(), String> {
    gio::spawn_blocking(move || save_at(&folder(), &album))
        .await
        .map_err(|_| "Unable to save album")?
}
fn save_at(dir: &Path, album: &Album) -> Result<(), String> {
    album.validate()?;
    let bytes = serde_json::to_vec(album).map_err(|_| "Unable to encode album")?;
    if bytes.len() > MAX_FILE {
        return Err("Album file is too large".into());
    }
    std::fs::create_dir_all(dir).map_err(|_| "Unable to create album folder")?;
    let path = dir.join(format!("{}.json", album.id));
    let temp = dir.join(format!("{}.tmp", glib::uuid_string_random()));
    std::fs::write(&temp, bytes)
        .and_then(|_| std::fs::rename(&temp, &path))
        .map_err(|_| "Unable to save album")?;
    Ok(())
}
/// Removes only the collection manifest; audio and sharing are independent.
pub async fn remove(id: String) -> Result<(), String> {
    gio::spawn_blocking(move || remove_at(&folder(), &id))
        .await
        .map_err(|_| "Unable to delete album")?
}
fn remove_at(dir: &Path, id: &str) -> Result<(), String> {
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err("Invalid album identifier".into());
    }
    match std::fs::remove_file(dir.join(format!("{id}.json"))) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Unable to delete album".into()),
    }
}
pub async fn open(path: PathBuf) -> Result<Album, String> {
    gio::spawn_blocking(move || Album::import(&read_limited(&path)?))
        .await
        .map_err(|_| "Unable to open album")?
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_album_preserves_order_but_never_local_locations() {
        let mut album = Album::new();
        album.name = "My mix".into();
        album.tracks = vec![
            Entry {
                title: "Second".into(),
                sha256: Some("ab".repeat(32)),
                source_key: Some("ef".repeat(32)),
                node_id: Some(1234567),
                local_uri: Some("file:///private/music.flac".into()),
                ..Default::default()
            },
            Entry {
                title: "First".into(),
                sha256: Some("cd".repeat(32)),
                ..Default::default()
            },
        ];
        let bytes = album.export().unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(!text.contains("private"));
        assert!(!text.contains("1234567"));
        assert!(!text.contains("node_id"));
        assert!(!text.contains("source_key"));
        assert!(!text.contains(&"ef".repeat(32)));
        let restored = Album::import(&bytes).unwrap();
        assert_ne!(restored.id, album.id);
        assert_eq!(restored.tracks[0].title, "Second");
        assert!(restored
            .tracks
            .iter()
            .all(|t| t.node_id.is_none() && t.local_uri.is_none()));
        let dir = std::env::temp_dir().join(format!("hopea-album-{}", album.id));
        save_at(&dir, &album).unwrap();
        album.name = "Renamed".into();
        album.tracks.swap(0, 1);
        save_at(&dir, &album).unwrap();
        let saved: Album =
            serde_json::from_slice(&read_limited(&dir.join(format!("{}.json", album.id))).unwrap())
                .unwrap();
        assert_eq!(saved.name, "Renamed");
        assert_eq!(saved.tracks[0].title, "First");
        let other = dir.join("music.flac");
        std::fs::write(&other, b"audio must remain").unwrap();
        assert!(remove_at(&dir, "../../escape").is_err());
        remove_at(&dir, &album.id).unwrap();
        assert!(!dir.join(format!("{}.json", album.id)).exists());
        assert_eq!(std::fs::read(&other).unwrap(), b"audio must remain");
        remove_at(&dir, &album.id).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn rejects_foreign_paths_and_invalid_manifests() {
        assert!(Album::import(br#"{"hopea_album":1,"name":"x","cover":null,"tracks":[{"title":"x","artist":null,"sha256":"bad"}]}"#).is_err());
        assert!(Album::import(br#"{"hopea_album":1,"name":"x","cover":null,"local_uri":"file:///secret","tracks":[]}"#).is_err());
        let mut a = Album::new();
        a.name = "x".into();
        a.id = "../../escape".into();
        assert!(a.validate().is_err());
        a = Album::new();
        a.name = "x".into();
        a.tracks.push(Entry::default());
        assert!(a.export().is_err());
    }
}
