// SPDX-FileCopyrightText: 2022  Emmanuele Bassi
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{cell::Cell, collections::HashSet};

use gtk::{gio, glib, prelude::*, subclass::prelude::*};

use crate::audio::{CoverCache, RepeatMode, ShuffleListModel, Song};

mod imp {
    use glib::{ParamSpec, ParamSpecEnum, ParamSpecObject, ParamSpecUInt, Value};
    use once_cell::sync::Lazy;

    use super::*;

    #[derive(Debug)]
    pub struct Queue {
        pub model: ShuffleListModel,
        pub store: gio::ListStore,
        pub repeat_mode: Cell<RepeatMode>,
        pub current_pos: Cell<Option<u32>>,
        pub shuffled: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Queue {
        const NAME: &'static str = "HopeaQueue";
        type Type = super::Queue;

        fn new() -> Self {
            let store = gio::ListStore::new::<Song>();
            let model = ShuffleListModel::new(Some(&store));

            Self {
                store,
                model,
                repeat_mode: Cell::new(RepeatMode::default()),
                current_pos: Cell::new(None),
                shuffled: Cell::new(false),
            }
        }
    }

    impl ObjectImpl for Queue {
        fn properties() -> &'static [ParamSpec] {
            static PROPERTIES: Lazy<Vec<ParamSpec>> = Lazy::new(|| {
                vec![
                    ParamSpecObject::builder::<Song>("current")
                        .read_only()
                        .build(),
                    ParamSpecEnum::builder::<RepeatMode>("repeat-mode")
                        .read_only()
                        .build(),
                    ParamSpecUInt::builder("n-songs").read_only().build(),
                ]
            });

            PROPERTIES.as_ref()
        }

        fn property(&self, _id: usize, pspec: &ParamSpec) -> Value {
            match pspec.name() {
                "current" => self.obj().current_song().to_value(),
                "repeat-mode" => self.repeat_mode.get().to_value(),
                "n-songs" => self.store.n_items().to_value(),
                _ => unimplemented!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn songs() -> Vec<Song> {
        (0..8)
            .map(|n| {
                let track = serde_json::from_value(serde_json::json!({
                    "id": n, "title": format!("Song {n}"), "artist": null, "album": null,
                    "album_artist": null, "track_number": null, "year": null, "genre": null,
                    "duration_ms": 1000, "format": "wav", "codec": "pcm", "sample_rate": 44100,
                    "channels": 1, "size": 100, "sha256": format!("{n:064x}"),
                    "has_cover": false, "uploader": "fixture", "uploaded_at": 0
                }))
                .unwrap();
                // Queue tests need metadata only, not user artwork settings or
                // saved edits. Never initialize the user's settings backend.
                let song = Song::empty();
                song.imp()
                    .data
                    .replace(crate::audio::song::SongData::from_remote(
                        &track,
                        &format!("http://127.0.0.1:1/{n}"),
                        None,
                    ));
                song
            })
            .collect()
    }

    #[test]
    fn shuffle_updates_the_current_index_before_notifying_the_list() {
        for anchor in [0, 3, 7] {
            let queue = Queue::default();
            let songs = songs();
            queue.add_songs(&songs);
            let current = songs[anchor].clone();
            queue.set_current_song(Some(current.clone()));
            let weak = queue.downgrade();
            let expected = current.clone();
            queue.model().connect_items_changed(move |_, _, _, _| {
                assert_eq!(
                    weak.upgrade().unwrap().current_song(),
                    Some(expected.clone()),
                    "observers must never see a transient different current song"
                );
            });
            queue.set_shuffled(true);
            assert_eq!(queue.current_song_index(), Some(0));
            assert_eq!(queue.song_at(0), Some(current.clone()));
            assert_eq!(queue.current_song(), Some(current.clone()));
            assert!(!queue.is_last_song());
            queue.set_shuffled(false);
            assert_eq!(queue.current_song_index(), Some(anchor as u32));
            assert_eq!(queue.current_song(), Some(current));
            assert_eq!(
                (0..8)
                    .map(|i| queue.song_at(i).unwrap())
                    .collect::<Vec<_>>(),
                songs
            );
        }
    }

    #[test]
    fn editing_a_shuffled_queue_keeps_the_same_current_song() {
        let queue = Queue::default();
        let songs = songs();
        queue.add_songs(&songs);
        let current = songs[5].clone();
        queue.set_current_song(Some(current.clone()));
        queue.set_shuffled(true);
        assert!(queue.move_song(&current, &songs[0], true));
        queue.remove_song(&songs[0]);
        assert!(queue.is_shuffled());
        assert_eq!(queue.current_song(), Some(current.clone()));
        assert!(queue.add_song(&songs[0]));
        assert_eq!(queue.current_song(), Some(current));
        assert_eq!(queue.n_songs(), 8);
        let unique: HashSet<_> = (0..8).map(|i| queue.song_at(i).unwrap().uri()).collect();
        assert_eq!(unique.len(), 8);
        queue.clear();
        queue.add_song(&songs[0]);
        assert_eq!(queue.song_at(0), Some(songs[0].clone()));
        assert_eq!(queue.current_song(), None);
    }
}

glib::wrapper! {
    pub struct Queue(ObjectSubclass<imp::Queue>);
}

impl Default for Queue {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Queue {
    pub fn n_songs(&self) -> u32 {
        self.imp().model.n_items()
    }

    pub fn is_empty(&self) -> bool {
        self.imp().model.n_items() == 0
    }

    pub fn model(&self) -> &gio::ListModel {
        self.imp().model.as_ref()
    }

    pub fn song_at(&self, pos: u32) -> Option<Song> {
        if let Some(song) = self.imp().model.item(pos) {
            return Some(song.downcast::<Song>().unwrap());
        }

        None
    }

    pub fn current_song(&self) -> Option<Song> {
        if let Some(pos) = self.imp().current_pos.get() {
            return self.song_at(pos);
        }

        None
    }

    pub fn set_current_song(&self, song: Option<Song>) {
        if let Some(song) = song {
            for i in 0..self.n_songs() {
                let s = self.song_at(i).unwrap();
                if song.equals(&s) {
                    self.imp().current_pos.replace(Some(i));
                    self.notify("current");
                    return;
                }
            }
        } else {
            self.imp().current_pos.replace(None);
            self.notify("current");
        }
    }

    pub fn current_song_index(&self) -> Option<u32> {
        self.imp().current_pos.get()
    }

    pub fn add_song(&self, song: &Song) -> bool {
        if !song.equals(&Song::default()) && !self.contains(song) {
            // Add song to the backing store
            self.imp().store.append(song);
            self.notify("n-songs");
            true
        } else {
            false
        }
    }

    pub fn add_songs(&self, songs: &[impl IsA<glib::Object>]) {
        let mut uris = HashSet::new();
        let mut hashes = HashSet::new();
        let mut library_ids = HashSet::new();
        for pos in 0..self.imp().store.n_items() {
            let song = self
                .imp()
                .store
                .item(pos)
                .unwrap()
                .downcast::<Song>()
                .unwrap();
            uris.insert(song.uri());
            hashes.extend(song.content_hash());
            library_ids.extend(song.library_id());
        }
        let mut added = Vec::new();
        for object in songs {
            let Some(song) = object.as_ref().downcast_ref::<Song>() else {
                continue;
            };
            let hash = song.content_hash();
            if song.equals(&Song::default())
                || uris.contains(&song.uri())
                || hash.as_ref().is_some_and(|h| hashes.contains(h))
                || song
                    .library_id()
                    .is_some_and(|id| library_ids.contains(&id))
            {
                continue;
            }
            uris.insert(song.uri());
            hashes.extend(hash);
            library_ids.extend(song.library_id());
            added.push(song.clone());
        }
        if !added.is_empty() {
            self.imp()
                .store
                .splice(self.imp().store.n_items(), 0, &added);
            self.notify("n-songs");
        }
    }

    pub fn remove_song(&self, song: &Song) {
        let visible =
            (0..self.n_songs()).find(|&i| self.song_at(i).is_some_and(|s| s.equals(song)));
        let current = self.current_song_index();
        let n_songs = self.n_songs();
        for pos in 0..n_songs {
            let s = self
                .imp()
                .store
                .item(pos)
                .unwrap()
                .downcast::<Song>()
                .unwrap();
            if s.equals(song) {
                self.imp()
                    .current_pos
                    .set(current.and_then(|current| match visible {
                        Some(removed) if removed == current => None,
                        Some(removed) if removed < current => Some(current - 1),
                        _ => Some(current),
                    }));
                self.imp().store.remove(pos);
                break;
            }
        }

        if n_songs != self.n_songs() {
            self.notify("n-songs");
            self.notify("current");
        }

        if self.is_empty() {
            self.imp().current_pos.replace(None);
        }
    }

    /// Insert before/after the indicated row without reloading the current audio.
    pub fn move_song(&self, song: &Song, target: &Song, after: bool) -> bool {
        let current = self.current_song();
        let mut songs: Vec<_> = (0..self.n_songs())
            .filter_map(|i| self.song_at(i))
            .collect();
        let Some(from) = songs.iter().position(|s| s.equals(song)) else {
            return false;
        };
        let Some(target_index) = songs.iter().position(|s| s.equals(target)) else {
            return false;
        };
        if from == target_index {
            return false;
        }
        let insertion = target_index + usize::from(after);
        let to = insertion - usize::from(from < insertion);
        if from == to {
            return false;
        }
        let moving = songs.remove(from);
        songs.insert(to, moving);
        let previous_pos = self.imp().current_pos.get();
        self.imp().current_pos.set(current.and_then(|current| {
            songs
                .iter()
                .position(|s| s.equals(&current))
                .map(|index| index as u32)
        }));
        if self.is_shuffled() {
            if !self.imp().model.move_shuffled(from as u32, to as u32) {
                self.imp().current_pos.set(previous_pos);
                return false;
            }
        } else {
            // Keep unaffected row widgets alive instead of replacing the list.
            let start = from.min(to);
            let end = from.max(to) + 1;
            self.imp()
                .store
                .splice(start as u32, (end - start) as u32, &songs[start..end]);
        }
        true
    }

    pub fn clear(&self) {
        let mut cover_cache = CoverCache::global().lock().unwrap();
        cover_cache.clear();

        self.imp().current_pos.replace(None);
        self.imp().store.remove_all();
        self.notify("n-songs");
    }

    pub fn skip_song(&self, pos: u32) -> Option<Song> {
        self.imp().current_pos.replace(Some(pos));
        self.notify("current");
        self.song_at(pos)
    }

    pub fn previous_song(&self) -> Option<Song> {
        if let Some(current_pos) = self.imp().current_pos.get() {
            if current_pos > 0 {
                let prev = current_pos - 1;
                self.imp().current_pos.replace(Some(prev));
                self.notify("current");
                return self.song_at(current_pos - 1);
            }
        }

        None
    }

    pub fn next_song(&self) -> Option<Song> {
        let store = &self.imp().model;

        let n_songs = store.n_items();
        if n_songs == 0 {
            return None;
        }

        let repeat_mode = self.imp().repeat_mode.get();
        if let Some(current) = self.current_song_index() {
            let next: Option<u32> = match repeat_mode {
                RepeatMode::Consecutive if current < n_songs - 1 => Some(current + 1),
                RepeatMode::Consecutive if current == n_songs - 1 => None,
                RepeatMode::RepeatOne => Some(current),
                RepeatMode::RepeatAll if current < n_songs - 1 => Some(current + 1),
                RepeatMode::RepeatAll if current == n_songs - 1 => Some(0),
                _ => None,
            };

            if let Some(next) = next {
                self.imp().current_pos.replace(Some(next));
                self.notify("current");
                self.song_at(next)
            } else {
                self.imp().current_pos.replace(None);
                self.notify("current");
                None
            }
        } else {
            // Return the first song
            self.imp().current_pos.replace(Some(0));
            self.notify("current");
            self.song_at(0)
        }
    }

    pub fn repeat_mode(&self) -> RepeatMode {
        self.imp().repeat_mode.get()
    }

    pub fn set_repeat_mode(&self, repeat_mode: RepeatMode) {
        let old_mode = self.imp().repeat_mode.replace(repeat_mode);
        if old_mode != repeat_mode {
            self.notify("repeat-mode");
        }
    }

    pub fn is_first_song(&self) -> bool {
        if let Some(current_pos) = self.imp().current_pos.get() {
            return current_pos == 0;
        }

        false
    }

    pub fn is_last_song(&self) -> bool {
        let n_items = self.imp().model.n_items();

        if let Some(current_pos) = self.imp().current_pos.get() {
            if n_items > 0 {
                return current_pos == n_items - 1;
            }
        }

        false
    }

    pub fn is_shuffled(&self) -> bool {
        self.imp().shuffled.get()
    }

    pub fn set_shuffled(&self, shuffled: bool) {
        if shuffled != self.imp().shuffled.replace(shuffled) {
            let current = self.current_song();
            if shuffled {
                let current_pos = self.imp().current_pos.get();
                self.imp().current_pos.set(current.as_ref().map(|_| 0));
                self.imp().model.reshuffle(current_pos);
            } else {
                let source_pos = current.and_then(|song| {
                    (0..self.imp().store.n_items()).find(|&pos| {
                        self.imp()
                            .store
                            .item(pos)
                            .and_downcast::<Song>()
                            .is_some_and(|s| s.equals(&song))
                    })
                });
                self.imp().current_pos.set(source_pos);
                self.imp().model.unshuffle();
            }
            // Update transport availability without reloading audio or seeking.
            self.notify("current");
        }
    }

    pub fn select_song_at(&self, index: u32) {
        if let Some(song) = self.imp().model.item(index) {
            let song = song.downcast_ref::<Song>().unwrap();
            let is_selected = !song.selected();
            song.set_selected(is_selected);
        }
    }

    pub fn unselect_all_songs(&self) {
        for i in 0..self.imp().store.n_items() {
            let song = self.imp().store.item(i).unwrap();
            song.downcast_ref::<Song>().unwrap().set_selected(false);
        }
    }

    pub fn n_selected_songs(&self) -> u32 {
        let mut count = 0;
        for i in 0..self.imp().store.n_items() {
            let song = self.imp().store.item(i).unwrap();
            if song.downcast_ref::<Song>().unwrap().selected() {
                count += 1;
            }
        }

        count
    }

    pub fn contains(&self, s: &Song) -> bool {
        for i in 0..self.imp().store.n_items() {
            let song = self.imp().store.item(i).unwrap();
            if song.downcast_ref::<Song>().unwrap().equals(s) {
                return true;
            }
        }

        false
    }
}
