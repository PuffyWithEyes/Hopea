// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Song lyrics, shown in place of the cover art, line by line in time with
//! the song when they have time stamps (the LRC format).
//!
//! They come from, in this order: a copy kept from an earlier lookup, an
//! `.lrc` file next to a local song, the lyrics in its tags, and, only when
//! the listener asks, lrclib.net, a free lyrics database, which then learns
//! the song's title, artist, album and length. Lyrics are text from others:
//! they are cleaned of control characters, limited in size, and only ever
//! shown as plain text.

use std::path::PathBuf;

use gtk::{gio, glib, prelude::*};
use serde::Deserialize;
use sha2::{Digest, Sha256};
#[cfg(not(target_os = "android"))]
use soup::prelude::*;

#[cfg(target_os = "android")]
#[path = "lyrics_https.rs"]
mod https;

use crate::{audio::Song, config::VERSION, i18n::i18n};

const MAX_BYTES: usize = hopea_common::lyrics::MAX_BYTES;
pub use hopea_common::lyrics::Lyrics;

pub fn parse(text: &str) -> Option<Lyrics> {
    let lyrics = hopea_common::lyrics::parse(text, None).ok()?;
    (!lyrics.lines.is_empty()).then_some(lyrics)
}
pub fn validate_for_song(song: &Song, text: &str) -> Result<(), String> {
    hopea_common::lyrics::validate(text, song.duration_ms()).map_err(|e| e.to_string())
}
/// Where lyrics found earlier for `song` are kept.
fn cache_path(song: &Song) -> PathBuf {
    let key = song.uuid().unwrap_or_else(|| song.uri());
    crate::paths::cache_dir().join("lyrics").join(format!(
        "{}.lrc",
        hex::encode(Sha256::digest(key.as_bytes()))
    ))
}

/// Lyrics already on this computer: kept from an earlier lookup, next to
/// the song, or in its tags.
pub async fn on_this_computer(song: &Song) -> Option<Lyrics> {
    let lyrics =
        hopea_common::lyrics::parse(&raw_on_this_computer(song).await?, song.duration_ms()).ok()?;
    (!lyrics.lines.is_empty()).then_some(lyrics)
}

/// Keep exact LRC timestamps when opening the editor.
pub async fn raw_on_this_computer(song: &Song) -> Option<String> {
    if let Some(text) = raw_local(song)
        .await
        .filter(|t| validate_for_song(song, t).is_ok())
    {
        return Some(text);
    }
    let client = crate::library::client()?;
    client
        .lyrics(song.library_id()?)
        .await
        .ok()
        .flatten()
        .filter(|t| validate_for_song(song, t).is_ok())
}

async fn raw_local(song: &Song) -> Option<String> {
    let cached = cache_path(song);
    let path = song.file().path();
    let edits = crate::song_edits::path(song);
    let text = gio::spawn_blocking(move || {
        let read = |p: &std::path::Path| {
            use std::io::Read;
            if !p.metadata().ok()?.is_file() {
                return None;
            }
            let mut text = String::new();
            std::fs::File::open(p)
                .ok()?
                .take(MAX_BYTES as u64 + 1)
                .read_to_string(&mut text)
                .ok()?;
            (text.len() <= MAX_BYTES).then_some(text)
        };
        if let Ok(crate::song_edits::Edits {
            lyrics: Some(text), ..
        }) = crate::song_edits::read(&edits)
        {
            return Some(text);
        }
        if let Some(text) = read(&cached) {
            return Some(text);
        }
        let path = path?;
        if let Some(text) = read(&path.with_extension("lrc")) {
            return Some(text);
        }
        use lofty::file::TaggedFileExt;
        let options = lofty::config::ParseOptions::new().read_cover_art(false);
        let tagged = lofty::probe::Probe::open(&path)
            .ok()?
            .options(options)
            .read()
            .ok()?;
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
        tag.get_string(lofty::tag::ItemKey::Lyrics)
            .filter(|t| t.len() <= MAX_BYTES)
            .map(str::to_string)
    })
    .await
    .ok()??;
    Some(text)
}

pub async fn publish(song: &Song, text: &str) -> Result<(), String> {
    validate_for_song(song, text)?;
    let Some(id) = song.library_id() else {
        return Ok(());
    };
    let client = crate::library::client().ok_or_else(|| i18n("Reconnect to share the lyrics"))?;
    client
        .publish_lyrics(id, text)
        .await
        .map_err(|e| e.to_string())
}

/// Migrate saved edits and old Search Online results when joining. Read only
/// metadata/sidecars on workers; never hash entire audio files on the UI thread.
pub async fn publish_saved(client: crate::library::LibraryClient) {
    let mut offset = 0;
    while offset < 20_000 && crate::library::client().is_some() {
        let Ok(page) = client.local_tracks(offset).await else {
            break;
        };
        if page.tracks.is_empty() {
            break;
        }
        offset += page.tracks.len() as u32;
        for track in page.tracks.into_iter().filter(|t| t.mine) {
            let uri = crate::library::shared_files::uri_for(track.id);
            publish_file(&client, track, uri).await;
        }
    }
}

pub async fn publish_file(
    client: &crate::library::LibraryClient,
    track: hopea_common::api::Track,
    uri: Option<String>,
) {
    if !track.mine {
        return;
    }
    let id = track.id;
    let text = gio::spawn_blocking(move || {
        let read = |path: PathBuf| -> Option<String> {
            use std::io::Read;
            let mut text = String::new();
            std::fs::File::open(path)
                .ok()?
                .take(MAX_BYTES as u64 + 1)
                .read_to_string(&mut text)
                .ok()?;
            (text.len() <= MAX_BYTES).then_some(text)
        };
        let remote_key = format!("hopea-{}", track.sha256);
        for key in uri
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(remote_key.as_str()))
        {
            let path = crate::paths::data_dir().join("song-edits").join(format!(
                "{}.json",
                hex::encode(Sha256::digest(key.as_bytes()))
            ));
            if let Ok(crate::song_edits::Edits {
                lyrics: Some(text), ..
            }) = crate::song_edits::read(&path)
            {
                return Some(text);
            }
        }
        let mut keys = vec![remote_key];
        let file = uri.as_ref().map(|uri| gio::File::for_uri(uri));
        if let Some(file) = &file {
            if let Ok(info) = file.query_info(
                "standard::display-name",
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            ) {
                let mut hash = Sha256::new();
                hash.update(info.display_name().as_bytes());
                if let Some(artist) = &track.artist {
                    hash.update(artist);
                }
                hash.update(&track.title);
                if let Some(album) = &track.album {
                    hash.update(album);
                }
                keys.push(hex::encode(hash.finalize()));
            }
        }
        for key in keys {
            if let Some(text) = read(crate::paths::cache_dir().join("lyrics").join(format!(
                "{}.lrc",
                hex::encode(Sha256::digest(key.as_bytes()))
            ))) {
                return Some(text);
            }
        }
        let path = file?.path()?;
        if let Some(text) = read(path.with_extension("lrc")) {
            return Some(text);
        }
        use lofty::file::TaggedFileExt;
        let tagged = lofty::probe::Probe::open(path)
            .ok()?
            .options(lofty::config::ParseOptions::new().read_cover_art(false))
            .read()
            .ok()?;
        tagged
            .primary_tag()
            .or_else(|| tagged.first_tag())?
            .get_string(lofty::tag::ItemKey::Lyrics)
            .filter(|t| t.len() <= MAX_BYTES)
            .map(str::to_string)
    })
    .await
    .ok()
    .flatten();
    if let Some(text) = text {
        if let Err(error) = client.publish_lyrics(id, &text).await {
            log::debug!("Could not publish saved lyrics: {error}");
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Found {
    duration: Option<f64>,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

/// Looks the song up on lrclib.net, and keeps what it finds for next time.
/// `Ok(None)` if it has no lyrics for it.
pub async fn look_up_online(song: &Song) -> Result<Option<Lyrics>, String> {
    let encode = |s: &str| {
        percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
    };
    let mut url = format!(
        "https://lrclib.net/api/search?track_name={}",
        encode(&song.title())
    );
    if let Some(artist) = song.known_artist() {
        url.push_str(&format!("&artist_name={}", encode(&artist)));
    }

    let Some(body) = fetch_online_body(url).await? else {
        return Ok(None);
    };
    let found: Vec<Found> = serde_json::from_slice(&body)
        .map_err(|_| i18n("The lyrics service sent something unexpected"))?;

    // The closest in length, as there may be several versions of a song
    let length = song.duration() as f64;
    let best = found
        .into_iter()
        .filter(|f| f.synced_lyrics.is_some() || f.plain_lyrics.is_some())
        .min_by(|a, b| {
            let off = |f: &Found| f.duration.map_or(f64::MAX, |d| (d - length).abs());
            let synced = |f: &Found| f.synced_lyrics.is_none();
            synced(a).cmp(&synced(b)).then(off(a).total_cmp(&off(b)))
        });
    let Some(text) = best.and_then(|f| f.synced_lyrics.or(f.plain_lyrics)) else {
        return Ok(None);
    };
    if text.len() > MAX_BYTES {
        return Err(i18n("Lyrics must be smaller than 256 KB"));
    }
    validate_for_song(song, &text)?;
    let Some(lyrics) = parse(&text) else {
        return Ok(None);
    };
    let cached = cache_path(song);
    let published = text.clone();
    let _ = gio::spawn_blocking(move || {
        if let Some(dir) = cached.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(cached, text)
    })
    .await;
    if let Err(error) = publish(song, &published).await {
        log::warn!("Lyrics saved locally; publication failed: {error}");
    }
    Ok(Some(lyrics))
}

#[cfg(not(target_os = "android"))]
async fn fetch_online_body(url: String) -> Result<Option<Vec<u8>>, String> {
    let session = soup::Session::new();
    session.set_user_agent(&format!("Hopea/{VERSION}"));
    session.set_timeout(20);
    let msg = soup::Message::new("GET", &url).map_err(|e| e.to_string())?;
    let stream = session
        .send_future(&msg, glib::Priority::DEFAULT)
        .await
        .map_err(|e| e.message().to_string())?;
    if msg.status() == soup::Status::NotFound {
        return Ok(None);
    }
    if msg.status() != soup::Status::Ok {
        return Err(i18n("The lyrics service didn’t answer properly"));
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
        if body.len() + chunk.len() > 4 * MAX_BYTES {
            return Err(i18n("The lyrics service sent too much"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

#[cfg(target_os = "android")]
async fn fetch_online_body(url: String) -> Result<Option<Vec<u8>>, String> {
    // GTK's main context is not a Tokio runtime. Keep DNS, TLS and the complete
    // bounded response on a worker; no GTK objects cross the thread boundary.
    gio::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        runtime
            .block_on(https::fetch(
                &url,
                &format!("Hopea/{VERSION}"),
                4 * MAX_BYTES,
            ))
            .map_err(|error| match error {
                https::FetchError::Transport(error) => error.to_string(),
                https::FetchError::UnexpectedStatus => {
                    i18n("The lyrics service didn’t answer properly")
                }
                https::FetchError::TooLarge => i18n("The lyrics service sent too much"),
            })
    })
    .await
    .map_err(|_| i18n("The lyrics service didn’t answer properly"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_timestamps_and_repeated_tags_are_bounded() {
        assert_eq!(
            hopea_common::lyrics::timestamp("18446744073709551615:59.999"),
            None
        );
        assert_eq!(hopea_common::lyrics::timestamp("307445734561826:00"), None);
        assert_eq!(hopea_common::lyrics::timestamp("01:02.123"), Some(62_123));
        let text = format!(
            "{}Chorus\n[00:02]Later",
            "[00:01]".repeat(hopea_common::lyrics::MAX_LINES + 1)
        );
        assert!(parse(&text).is_none());
        assert!(parse(&"a".repeat(MAX_BYTES + 1)).is_none());
    }

    #[test]
    fn synced_lyrics() {
        let lyrics = parse(
            "[ar:Someone]\n[ti:Song]\n[00:12.50]First line\n[00:15.00][01:02.3]Chorus\n\n[00:20.123]Third",
        )
        .unwrap();
        assert!(lyrics.synced());
        let times: Vec<_> = lyrics.lines.iter().map(|l| l.time_ms.unwrap()).collect();
        assert_eq!(times, [12_500, 15_000, 20_123, 62_300]);
        assert_eq!(lyrics.lines[3].text, "Chorus");
        assert_eq!(lyrics.line_at(0), None);
        assert_eq!(lyrics.line_at(16_000), Some(1));
        assert_eq!(lyrics.line_at(99_000), Some(3));
    }

    #[test]
    fn plain_lyrics() {
        assert!(parse("One\u{202E}\nTwo\u{7}").is_none());
        let lyrics = parse("\n\nOne\n\nTwo\n\n").unwrap();
        assert!(!lyrics.synced());
        let texts: Vec<_> = lyrics.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["One", "", "Two"]);
        assert_eq!(parse("\n \n"), None);
    }
}
