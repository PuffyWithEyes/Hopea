// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Selected service collections become durable, editable Hopea albums. Only
//! normalized artwork and track identities are saved, never service URLs.
use super::{
    collections::{self, Album, Entry},
    import_history::{CoverCache, Imported},
    service_import::CatalogTrack,
};
use gtk::{gio, prelude::*};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

pub struct ImportedAlbums {
    albums: Vec<Album>,
    memberships: HashMap<String, Vec<(usize, usize)>>,
    covers: HashMap<usize, String>,
}

fn identity(service: &str, key: &str) -> String {
    hex::encode(Sha256::digest(format!("{service}:{key}").as_bytes()))
}

impl ImportedAlbums {
    fn plan(
        service: &str,
        tracks: &[CatalogTrack],
        selected: &HashSet<String>,
        existing: Vec<Album>,
    ) -> Self {
        let mut existing: HashMap<_, _> = existing.into_iter().map(|a| (a.id.clone(), a)).collect();
        let mut groups: BTreeMap<
            String,
            Vec<(
                usize,
                &CatalogTrack,
                &hopea_common::service_import::Collection,
            )>,
        > = BTreeMap::new();
        for (index, track) in tracks.iter().enumerate() {
            for collection in &track.collections {
                if collection.kind != "library" && selected.contains(&collection.key) {
                    groups
                        .entry(collection.key.clone())
                        .or_default()
                        .push((index, track, collection));
                }
            }
        }
        let mut result = Self {
            albums: Vec::new(),
            memberships: HashMap::new(),
            covers: HashMap::new(),
        };
        for (key, mut members) in groups {
            members.sort_by_key(|(index, _, c)| c.position.unwrap_or(*index as u32));
            let hash = identity(service, &format!("collection:{key}"));
            let id = format!(
                "{}-{}-{}-{}-{}",
                &hash[..8],
                &hash[8..12],
                &hash[12..16],
                &hash[16..20],
                &hash[20..32]
            );
            let mut album = existing.remove(&id).unwrap_or_else(|| {
                let mut album = Album::new();
                album.id = id;
                album.name = members[0].2.title.clone();
                album
            });
            let album_index = result.albums.len();
            if album.cover.is_none() {
                if let Some(url) = members
                    .iter()
                    .find_map(|(_, _, c)| c.cover.clone())
                    .or_else(|| members.iter().find_map(|(_, t, _)| t.cover.clone()))
                {
                    result.covers.insert(album_index, url);
                }
            }
            for (_, track, _) in members {
                let source = identity(service, &track.key);
                let entry_index = album
                    .tracks
                    .iter()
                    .position(|entry| entry.source_key.as_ref() == Some(&source))
                    .unwrap_or_else(|| {
                        album.tracks.push(Entry {
                            source_key: Some(source),
                            title: track.title.clone(),
                            artist: track.artist.clone(),
                            ..Default::default()
                        });
                        album.tracks.len() - 1
                    });
                result
                    .memberships
                    .entry(track.key.clone())
                    .or_default()
                    .push((album_index, entry_index));
            }
            result.albums.push(album);
        }
        result
    }

    pub async fn begin(
        service: &str,
        tracks: &[CatalogTrack],
        selected: &HashSet<String>,
    ) -> Result<Self, String> {
        let existing = if selected.is_empty() {
            Vec::new()
        } else {
            collections::list().await?
        };
        let result = Self::plan(service, tracks, selected, existing);
        // Save the collection before downloads start, so cancel/restart leaves
        // an album with identifiable pending songs instead of loose files.
        for album in &result.albums {
            collections::save(album.clone()).await?;
        }
        Ok(result)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.memberships.contains_key(key)
    }
    pub fn len(&self) -> usize {
        self.albums.len()
    }

    pub async fn update(
        &mut self,
        key: &str,
        record: &Imported,
        audio: &Path,
        artwork: Option<&CoverCache>,
    ) -> Result<(), String> {
        let Some(memberships) = self.memberships.get(key) else {
            return Ok(());
        };
        let local_uri = record
            .node_id
            .and_then(super::shared_files::uri_for)
            .filter(|uri| gio::File::for_uri(uri).query_exists(gio::Cancellable::NONE))
            .or_else(|| {
                (record.node_id.is_some() && audio.is_file())
                    .then(|| gio::File::for_path(audio).uri().to_string())
            });
        for &(album_index, entry_index) in memberships {
            let original = &self.albums[album_index];
            let source = &original.tracks[entry_index].source_key;
            // Respect edits/deletions made while the background import runs.
            let Some(mut album) = collections::load(original.id.clone()).await? else {
                continue;
            };
            let Some(entry) = album.tracks.iter_mut().find(|e| &e.source_key == source) else {
                continue;
            };
            entry.node_id = record.node_id;
            entry.sha256 = record.node_sha256.clone();
            entry.local_uri = local_uri.clone();
            if let (Some(url), Some(artwork)) = (self.covers.remove(&album_index), artwork) {
                if let Ok(bytes) = artwork.get(&url).await {
                    if album.cover.is_none() {
                        album.cover = collections::normalize_cover(&bytes).ok();
                    }
                }
            }
            collections::save(album.clone()).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track(key: &str, position: u32) -> CatalogTrack {
        serde_json::from_value(
            serde_json::json!({"key": key, "title": key, "collections": [
                {"key":"album:a", "kind":"album", "title":"Album A", "position":position},
                {"key":"album:b", "kind":"album", "title":"Not Selected", "position":position}
            ]}),
        )
        .unwrap()
    }
    #[test]
    fn albums_preserve_service_order_selection_and_user_edits_on_repeat() {
        let selected = HashSet::from(["album:a".to_string()]);
        let tracks = vec![track("second", 1), track("first", 0)];
        let mut result = ImportedAlbums::plan("vk", &tracks, &selected, vec![]);
        assert_eq!(result.len(), 1);
        let album = &mut result.albums[0];
        assert_eq!(
            album
                .tracks
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        album.name = "My custom name".into();
        album.tracks.swap(0, 1);
        let id = album.id.clone();
        let again = ImportedAlbums::plan("vk", &tracks, &selected, result.albums);
        assert_eq!(again.albums[0].id, id);
        assert_eq!(again.albums[0].name, "My custom name");
        assert_eq!(again.albums[0].tracks.len(), 2);
        assert_eq!(again.albums[0].tracks[0].title, "second");
        assert_ne!(
            ImportedAlbums::plan("yandex", &tracks, &selected, vec![]).albums[0].id,
            id
        );
        assert_eq!(
            ImportedAlbums::plan("vk", &tracks, &HashSet::new(), vec![]).len(),
            0
        );
    }
}
#[cfg(test)]
impl ImportedAlbums {
    pub(crate) async fn test_persistence() {
        let tracks: Vec<CatalogTrack> = [1, 0].iter().map(|position| {
            serde_json::from_value(serde_json::json!({
                "key": format!("track:{position}"), "title": format!("Song {position}"),
                "collections": [{"key":"test:collection", "kind":"album", "title":"Imported test album", "position":position}]
            })).unwrap()
        }).collect();
        let selected = HashSet::from(["test:collection".into()]);
        let mut job = Self::begin("test", &tracks, &selected).await.unwrap();
        let id = job.albums[0].id.clone();
        let mut album = collections::load(id.clone()).await.unwrap().unwrap();
        assert_eq!(album.tracks[0].title, "Song 0");
        assert!(album.tracks.iter().all(|entry| entry.node_id.is_none()));
        album.name = "Renamed during download".into();
        album.tracks.swap(0, 1);
        collections::save(album).await.unwrap();
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("node/tests/fixtures/tone.wav");
        let record = Imported {
            node_id: Some(424242),
            node_sha256: Some("ab".repeat(32)),
            ..Default::default()
        };
        job.update("track:0", &record, &path, None).await.unwrap();
        let album = collections::load(id.clone()).await.unwrap().unwrap();
        assert_eq!(album.name, "Renamed during download");
        assert_eq!(album.tracks[0].title, "Song 1");
        assert_eq!(album.tracks[1].node_id, Some(424242));
        assert!(album.tracks[1].local_uri.is_some());
        let retry = Self::begin("test", &tracks, &selected).await.unwrap();
        assert_eq!(
            retry.albums[0].tracks.len(),
            2,
            "retry must not duplicate album members"
        );
        assert_eq!(retry.albums[0].id, id);
        collections::remove(id.clone()).await.unwrap();
        job.update("track:1", &record, &path, None).await.unwrap();
        assert!(
            collections::load(id).await.unwrap().is_none(),
            "background job must not recreate a deleted album"
        );
    }
}
