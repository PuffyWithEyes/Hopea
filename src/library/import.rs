// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bringing a listener's music over from streaming services.
//!
//! This module reads lists of songs and playlists: titles and artists.
//! Already downloaded audio is imported through the library panel's usual
//! file validation and sharing path instead. Each listed song is looked
//! for on the network, and the ones
//! someone shares get a star, which downloads them and puts them in the
//! library. The others stay on a list kept in the user data dir and are
//! looked for again now and then, as more people share more music.
//!
//! The list is read from what the services let people export:
//! - any JSON with song objects, as Spotify's "Download your data"
//!   (`YourLibrary.json`, `Playlist1.json`) and Apple Music's privacy export
//!   have;
//! - CSV with title and artist columns, as Google Takeout gives for YouTube
//!   Music and Exportify, TuneMyMusic and Soundiiz give for any service;
//! - plain lines of "Artist - Title";
//! - and, for Yandex Music, a link to a public playlist, read from the page
//!   data its website loads.

use std::{collections::HashSet, path::PathBuf};

use gtk::{gio, glib, prelude::*};
use hopea_common::{
    api::{SearchOrder, Track},
    AudioFormat,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use soup::prelude::*;

use super::LibraryClient;
use crate::{config::VERSION, i18n::i18n};

/// Songs kept on the list at most
const MAX_WANTED: usize = 20_000;
/// The longest text read from a file or a page
pub const MAX_IMPORT_BYTES: usize = 64 * 1024 * 1024;
/// How long before a song not found is looked for again, in seconds
const RETRY_AFTER: i64 = 12 * 60 * 60;

/// A song the listener has elsewhere, to look for on the network.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Wanted {
    pub title: String,
    pub artist: Option<String>,
    /// When it was last looked for, as a Unix timestamp
    #[serde(default)]
    pub tried_at: i64,
}

impl Wanted {
    fn new(title: &str, artist: Option<&str>) -> Option<Self> {
        let clean = |s: &str| {
            let s: String = s
                .chars()
                .filter(|c| !c.is_control())
                .take(200)
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            (!s.is_empty()).then_some(s)
        };
        Some(Self {
            title: clean(title)?,
            artist: artist.and_then(clean),
            tried_at: 0,
        })
    }

    fn key(&self) -> (String, String) {
        (
            normalize(&self.title),
            normalize(self.artist.as_deref().unwrap_or("")),
        )
    }

    /// What is searched for on the network.
    pub fn query(&self) -> String {
        match &self.artist {
            Some(artist) => format!("{artist} {}", base_title(&self.title)),
            None => base_title(&self.title),
        }
    }

    /// Whether a track found on the network is this song.
    pub fn matches(&self, track: &Track) -> bool {
        let wanted = normalize(&base_title(&self.title));
        let found = normalize(&base_title(AudioFormat::strip_extension(&track.title)));
        let title_matches = !wanted.is_empty()
            && (wanted == found
                || (wanted.len() >= 4 && found.contains(&wanted))
                || (found.len() >= 4 && wanted.contains(&found)));
        if !title_matches {
            return false;
        }
        let Some(artist) = &self.artist else {
            return true;
        };
        // "Artist A, Artist B" and "Artist A feat. Artist B" both match on
        // their first artist
        let first = normalize(
            artist
                .split([',', '&', ';', '/'])
                .next()
                .unwrap_or(artist)
                .split(" feat")
                .next()
                .unwrap_or(artist),
        );
        [track.artist.as_deref(), track.album_artist.as_deref()]
            .iter()
            .flatten()
            .map(|a| normalize(a))
            .any(|a| !first.is_empty() && (a.contains(&first) || first.contains(&a)))
    }
}

/// Lowercase letters and digits only, single spaces between words.
fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A title without what versions add to it: "(Remastered 2011)", "- Live",
/// "[feat. Someone]".
fn base_title(title: &str) -> String {
    let mut base = title.to_string();
    for open in ['(', '['] {
        if let Some(i) = base.find(open) {
            if i > 0 {
                base.truncate(i);
            }
        }
    }
    if let Some(i) = base.find(" - ") {
        if i > 0 {
            base.truncate(i);
        }
    }
    base.trim().to_string()
}

fn first_text(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    object.iter().find_map(|(key, value)| {
        let key = key.to_lowercase().replace(['_', ' '], "");
        if !keys.contains(&key.as_str()) {
            return None;
        }
        match value {
            Value::String(s) => Some(s.clone()),
            // ["A", "B"] or [{"name": "A"}, …]
            Value::Array(items) => {
                let names: Vec<String> = items
                    .iter()
                    .filter_map(|item| match item {
                        Value::String(s) => Some(s.clone()),
                        Value::Object(o) => first_text(o, &["name", "title"]),
                        _ => None,
                    })
                    .collect();
                (!names.is_empty()).then(|| names.join(", "))
            }
            Value::Object(o) => first_text(o, &["name", "title"]),
            _ => None,
        }
    })
}

/// Keys of the lists of songs in playlists and albums
const CONTAINER_KEYS: &[&str] = &[
    "tracks",
    "items",
    "songs",
    "volumes",
    "entries",
    "playlists",
];

const TITLE_KEYS: &[&str] = &[
    "trackname",
    "track",
    "title",
    "songtitle",
    "song",
    "songname",
];
const ARTIST_KEYS: &[&str] = &[
    "artistname",
    "artist",
    "artists",
    "artistnames",
    "albumartist",
    "creator",
    "performer",
];

/// Songs in any JSON: every object with a title and, if it has one, an
/// artist, however deep it is.
pub fn from_json(text: &str) -> Vec<Wanted> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut stack = vec![&value];
    while let Some(value) = stack.pop() {
        match value {
            Value::Object(object) => {
                // A playlist or an album has a title too, but holds a list
                // of songs: those are what counts
                let holds_songs = object.iter().any(|(key, value)| {
                    let key = key.to_lowercase();
                    value.is_array() && CONTAINER_KEYS.contains(&key.as_str())
                });
                match first_text(object, TITLE_KEYS) {
                    Some(title) if !holds_songs => {
                        let artist = first_text(object, ARTIST_KEYS);
                        found.extend(Wanted::new(&title, artist.as_deref()));
                    }
                    _ => stack.extend(object.values()),
                }
            }
            Value::Array(items) => stack.extend(items.iter()),
            _ => {}
        }
    }
    found.reverse();
    found
}

/// Splits a CSV line, with quoted fields.
fn csv_fields(line: &str, separator: char) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == separator && !quoted => fields.push(std::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    fields.push(field);
    fields
}

/// Songs in a CSV export with a header row naming the title and artist
/// columns, whatever service made it.
pub fn from_csv(text: &str) -> Vec<Wanted> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let header = header.trim_start_matches('\u{FEFF}');
    let separator = [',', ';', '\t']
        .iter()
        .copied()
        .max_by_key(|s| header.matches(*s).count())
        .unwrap_or(',');
    let columns: Vec<String> = csv_fields(header, separator)
        .iter()
        .map(|c| c.to_lowercase().replace(['_', ' ', '(', ')'], ""))
        .collect();
    let find = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| columns.iter().position(|c| c == k))
            .or_else(|| {
                keys.iter()
                    .find_map(|k| columns.iter().position(|c| c.starts_with(k)))
            })
    };
    let Some(title) = find(&["trackname", "songtitle", "title", "track", "song", "name"]) else {
        return Vec::new();
    };
    let artist = find(&[
        "artistnames",
        "artistname1",
        "artistname",
        "artist",
        "artists",
        "creator",
    ]);
    lines
        .filter_map(|line| {
            let fields = csv_fields(line, separator);
            let artist = artist.and_then(|i| fields.get(i)).map(String::as_str);
            Wanted::new(fields.get(title)?, artist)
        })
        .collect()
}

/// Songs in plain lines of "Artist - Title" (or just "Title").
pub fn from_lines(text: &str) -> Vec<Wanted> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim().trim_start_matches(|c: char| {
                c.is_ascii_digit() || matches!(c, '.' | ')' | '-' | '•' | '*') || c.is_whitespace()
            });
            for dash in [" — ", " – ", " - "] {
                if let Some((artist, title)) = line.split_once(dash) {
                    return Wanted::new(title, Some(artist));
                }
            }
            Wanted::new(line, None)
        })
        .collect()
}

/// Songs in an exported file, whatever its kind.
pub fn from_text(text: &str) -> Vec<Wanted> {
    let trimmed = text.trim_start_matches('\u{FEFF}').trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let found = from_json(trimmed);
        if !found.is_empty() {
            return found;
        }
    }
    let first = trimmed.lines().next().unwrap_or("");
    if first.contains(',') || first.contains(';') || first.contains('\t') {
        let found = from_csv(trimmed);
        if !found.is_empty() {
            return found;
        }
    }
    from_lines(trimmed)
}

/// Songs of a public Yandex Music playlist or album, from its link, read
/// the way the website reads them.
pub async fn from_yandex(link: &str) -> Result<Vec<Wanted>, String> {
    let url = glib::Uri::parse(link.trim(), glib::UriFlags::NONE)
        .map_err(|_| i18n("This isn’t a link"))?;
    let host = url.host().unwrap_or_default().to_lowercase();
    if !host.starts_with("music.yandex.") {
        return Err(i18n("This isn’t a Yandex Music link"));
    }
    let base = format!("https://{host}");
    let parts: Vec<String> = url
        .path()
        .split('/')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    let encode = |s: &str| {
        percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
    };
    let data_url = match parts.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["users", owner, "playlists", kind, ..] => format!(
            "{base}/handlers/playlist.jsx?owner={}&kinds={}&light=false",
            encode(owner),
            encode(kind)
        ),
        ["album", album, ..] => format!("{base}/handlers/album.jsx?album={}", encode(album)),
        _ => {
            return Err(i18n(
                "Use the link to a playlist (…/users/…/playlists/…) or an album (…/album/…)",
            ))
        }
    };
    let text = fetch_text(&data_url).await?;
    let found = from_json(&text);
    if found.is_empty() {
        return Err(i18n(
            "No songs came from this link. The playlist may be private: make it public in Yandex Music, or the site asked to prove you aren’t a robot; try again later.",
        ));
    }
    Ok(found)
}

async fn fetch_text(url: &str) -> Result<String, String> {
    let session = soup::Session::new();
    session.set_user_agent(&format!("Mozilla/5.0 Hopea/{VERSION}"));
    session.set_timeout(30);
    let msg = soup::Message::new("GET", url).map_err(|e| e.to_string())?;
    let stream = session
        .send_future(&msg, glib::Priority::DEFAULT)
        .await
        .map_err(|e| e.message().to_string())?;
    if msg.status() != soup::Status::Ok {
        return Err(i18n("The music service didn’t answer properly"));
    }
    let mut body = Vec::new();
    loop {
        let chunk = stream
            .read_bytes_future(64 * 1024, glib::Priority::DEFAULT)
            .await
            .map_err(|e| e.message().to_string())?;
        if chunk.is_empty() {
            break;
        }
        if body.len() + chunk.len() > MAX_IMPORT_BYTES {
            return Err(i18n("The music service sent too much"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn list_path() -> PathBuf {
    crate::paths::data_dir().join("wanted.json")
}

/// The songs still looked for.
pub fn wanted() -> Vec<Wanted> {
    std::fs::read(list_path())
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

fn save(list: &[Wanted]) {
    let path = list_path();
    let result = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(list)?)?;
        std::fs::rename(&tmp, &path)
    })();
    if let Err(err) = result {
        log::warn!("Unable to keep the list of songs to look for: {err}");
    }
}

/// Adds songs to the list, once each. Returns how many were new.
pub fn add(songs: Vec<Wanted>) -> usize {
    let mut list = wanted();
    let mut seen: HashSet<(String, String)> = list.iter().map(Wanted::key).collect();
    let before = list.len();
    for song in songs {
        if list.len() >= MAX_WANTED {
            break;
        }
        if seen.insert(song.key()) {
            list.push(song);
        }
    }
    let added = list.len() - before;
    save(&list);
    added
}

/// The songs due to be looked for: never looked for, or not for a while.
pub fn due(now: i64) -> Vec<Wanted> {
    wanted()
        .into_iter()
        .filter(|w| now.saturating_sub(w.tried_at) >= RETRY_AFTER)
        .collect()
}

/// Takes the songs found off the list, and marks the others as looked for.
pub fn record(results: &[(Wanted, bool)], now: i64) {
    if results.is_empty() {
        return;
    }
    let found: HashSet<(String, String)> = results
        .iter()
        .filter(|(_, found)| *found)
        .map(|(w, _)| w.key())
        .collect();
    let tried: HashSet<(String, String)> = results.iter().map(|(w, _)| w.key()).collect();
    let mut list = wanted();
    list.retain(|w| !found.contains(&w.key()));
    for w in &mut list {
        if tried.contains(&w.key()) {
            w.tried_at = now;
        }
    }
    save(&list);
}

/// Looks a song up on the network, and returns the track that is it.
pub async fn look_for(client: &LibraryClient, song: &Wanted) -> Option<Track> {
    let page = client
        .search(&song.query(), 0, true, SearchOrder::BestMatch)
        .await
        .ok()?;
    page.tracks.into_iter().find(|t| song.matches(t))
}

/// Reads an exported file.
pub async fn read_file(file: &gio::File) -> Result<Vec<Wanted>, String> {
    let stream = file
        .read_future(glib::Priority::DEFAULT)
        .await
        .map_err(|e| e.message().to_string())?;
    let mut bytes = Vec::new();
    loop {
        let chunk = stream
            .read_bytes_future(64 * 1024, glib::Priority::DEFAULT)
            .await
            .map_err(|e| e.message().to_string())?;
        if chunk.is_empty() {
            break;
        }
        if chunk.len() > MAX_IMPORT_BYTES.saturating_sub(bytes.len()) {
            return Err(i18n("The file is too large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(from_text(&String::from_utf8_lossy(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(found: &[Wanted]) -> Vec<(&str, Option<&str>)> {
        found
            .iter()
            .map(|w| (w.title.as_str(), w.artist.as_deref()))
            .collect()
    }

    #[test]
    fn spotify_library_and_playlists() {
        let library = r#"{"tracks":[{"artist":"Linkin Park","album":"Meteora","track":"Numb","uri":"spotify:track:x"}],"albums":[]}"#;
        assert_eq!(pairs(&from_text(library)), [("Numb", Some("Linkin Park"))]);
        let playlists = r#"{"playlists":[{"name":"Road","items":[{"track":{"trackName":"Faint","artistName":"Linkin Park","albumName":"Meteora"}}]}]}"#;
        assert_eq!(
            pairs(&from_text(playlists)),
            [("Faint", Some("Linkin Park"))]
        );
    }

    #[test]
    fn yandex_playlist_data() {
        let data = r#"{"playlist":{"title":"Мне нравится","tracks":[{"title":"Кукушка","artists":[{"name":"Кино"}]}]}}"#;
        assert_eq!(pairs(&from_text(data)), [("Кукушка", Some("Кино"))]);
    }

    #[test]
    fn csv_exports() {
        let takeout = "Song Title,Album Title,Artist Name 1\n\"Numb, Live\",Meteora,Linkin Park\n";
        assert_eq!(
            pairs(&from_text(takeout)),
            [("Numb, Live", Some("Linkin Park"))]
        );
        let exportify = "\u{FEFF}Track URI;Track Name;Artist Name(s)\nx;Faint;Linkin Park\n";
        assert_eq!(
            pairs(&from_text(exportify)),
            [("Faint", Some("Linkin Park"))]
        );
    }

    #[test]
    fn plain_lists() {
        let list = "1. Кино - Кукушка\nЗемфира — Хочешь?\nJust A Title\n";
        assert_eq!(
            pairs(&from_text(list)),
            [
                ("Кукушка", Some("Кино")),
                ("Хочешь?", Some("Земфира")),
                ("Just A Title", None)
            ]
        );
    }

    #[test]
    fn matching_found_tracks() {
        let wanted = Wanted::new("Numb (Remastered 2023)", Some("Linkin Park, Jay-Z")).unwrap();
        assert_eq!(wanted.query(), "Linkin Park, Jay-Z Numb");
        let track = |title: &str, artist: &str| Track {
            id: 1,
            title: title.into(),
            artist: Some(artist.into()),
            album: None,
            album_artist: None,
            track_number: None,
            year: None,
            genre: None,
            duration_ms: 1,
            format: AudioFormat::Mp3,
            codec: "mp3".into(),
            sample_rate: 44_100,
            channels: 2,
            size: 1,
            sha256: String::new(),
            has_cover: false,
            uploader: String::new(),
            uploaded_at: 0,
            mine: false,
            local: false,
            favorite: false,
            sources: 1,
        };
        assert!(wanted.matches(&track("Numb.mp3", "Linkin Park")));
        assert!(!wanted.matches(&track("Numb", "Someone Else")));
        assert!(!wanted.matches(&track("Faint", "Linkin Park")));
    }
}
