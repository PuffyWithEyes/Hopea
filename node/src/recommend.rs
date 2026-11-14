// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! The "For you" feed.
//!
//! Taste is read from what this node knows: favorites weigh most, then how
//! often tracks were played, then the tracks shared from here. The genres
//! and artists that come out on top are searched for on the network, like
//! any search, and the tracks found that the listener has neither played
//! nor added to favorites are ranked by how well they match.
//!
//! Everything happens on this node; nothing about the listener's taste is
//! sent anywhere except the search words themselves.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use hopea_common::api::Track;

use crate::{keywords, p2p, AppState};

/// Genres and artists searched for at most per refresh
const SEARCHED_GENRES: usize = 3;
const SEARCHED_ARTISTS: usize = 2;
/// The network is searched again at most this often unless asked to
const REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
const CANDIDATES: u32 = 1000;

/// Splits a genre field such as "Rock; Alternative/Nu Metal" into
/// normalized genres.
fn genres(field: &str) -> Vec<String> {
    field
        .split([';', '/', ',', '|'])
        .map(|g| keywords::words(g).collect::<Vec<_>>().join(" "))
        .filter(|g| !g.is_empty())
        .collect()
}

fn artist_key(artist: &str) -> String {
    keywords::words(artist).collect::<Vec<_>>().join(" ")
}

#[derive(Default)]
struct Taste {
    genres: HashMap<String, f64>,
    artists: HashMap<String, f64>,
}

impl Taste {
    fn top(map: &HashMap<String, f64>, n: usize) -> Vec<String> {
        let mut v: Vec<_> = map.iter().collect();
        v.sort_by(|a, b| b.1.total_cmp(a.1).then_with(|| a.0.cmp(b.0)));
        v.into_iter().take(n).map(|(k, _)| k.clone()).collect()
    }

    fn score(&self, track: &Track) -> f64 {
        let genre = track
            .genre
            .as_deref()
            .map(genres)
            .unwrap_or_default()
            .iter()
            .filter_map(|g| self.genres.get(g))
            .sum::<f64>();
        let artist = track
            .artist
            .as_deref()
            .or(track.album_artist.as_deref())
            .and_then(|a| self.artists.get(&artist_key(a)))
            .copied()
            .unwrap_or(0.0);
        genre + 0.6 * artist
    }
}

async fn taste(state: &AppState) -> Taste {
    let db = state.db.clone();
    let rows = tokio::task::spawn_blocking(move || db.taste())
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let mut taste = Taste::default();
    for (genre, artist, weight) in rows {
        for g in genre.as_deref().map(genres).unwrap_or_default() {
            *taste.genres.entry(g).or_default() += weight;
        }
        if let Some(a) = artist.as_deref().map(artist_key).filter(|a| !a.is_empty()) {
            *taste.artists.entry(a).or_default() += weight;
        }
    }
    taste
}

/// Verify remembered sources without another DHT lookup. Concurrent callers
/// share a short cooldown, so opening the feed cannot flood known peers.
pub async fn refresh_sources(state: &AppState) {
    if state.torrents.gaming() {
        return;
    }
    let mut checked = state.recommendation_check.lock().await;
    if checked.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
        return;
    }
    let taste = taste(state).await;
    let queries: Vec<_> = Taste::top(&taste.genres, SEARCHED_GENRES)
        .into_iter()
        .chain(Taste::top(&taste.artists, SEARCHED_ARTISTS))
        .collect();
    futures_util::future::join_all(queries.iter().map(|q| p2p::refresh_sources(state, q))).await;
    *checked = Some(Instant::now());
}

/// The feed: up to `limit` tracks, best match first. Searches the network
/// for fresh candidates when `refresh` is set or the last search is old.
/// Returns the tracks and the number of nodes that answered.
pub async fn feed(state: &AppState, refresh: bool, limit: usize) -> (Vec<Track>, u32) {
    let taste = taste(state).await;
    if taste.genres.is_empty() && taste.artists.is_empty() {
        return (Vec::new(), 0);
    }

    let due = {
        let last = state
            .last_recommendation_search
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        refresh || last.is_none_or(|t| t.elapsed() > REFRESH_INTERVAL)
    };
    let mut answered = 0;
    if due && !state.torrents.gaming() {
        let queries: Vec<String> = Taste::top(&taste.genres, SEARCHED_GENRES)
            .into_iter()
            .chain(Taste::top(&taste.artists, SEARCHED_ARTISTS))
            .collect();
        let searches = queries.iter().map(|q| p2p::search(state, q));
        answered = futures_util::future::join_all(searches)
            .await
            .into_iter()
            .map(|r| r.nodes_answered)
            .max()
            .unwrap_or(0);
        *state
            .last_recommendation_search
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    } else {
        refresh_sources(state).await;
    }

    let db = state.db.clone();
    let candidates = tokio::task::spawn_blocking(move || db.recommendation_candidates(CANDIDATES))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();

    let mut scored: Vec<(f64, Track)> = candidates
        .into_iter()
        .map(|row| (taste.score(&row.track), row.track))
        .filter(|(score, _)| *score > 0.0)
        .collect();
    scored.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| b.1.uploaded_at.cmp(&a.1.uploaded_at))
    });

    // The same song shared by several people shows up once
    let mut seen = HashSet::new();
    let tracks = scored
        .into_iter()
        .map(|(_, t)| t)
        .filter(|t| {
            seen.insert((
                t.title.to_lowercase(),
                t.artist.as_deref().map(str::to_lowercase),
            ))
        })
        .take(limit)
        .collect();
    (tracks, answered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_genres() {
        assert_eq!(
            genres("Rock; Alternative/Nu-Metal"),
            ["rock", "alternative", "nu metal"]
        );
        assert_eq!(genres("Hip-Hop"), ["hip hop"]);
        assert!(genres(" ; ").is_empty());
    }

    #[test]
    fn scores_by_genre_then_artist() {
        let mut taste = Taste::default();
        taste.genres.insert("nu metal".into(), 5.0);
        taste.artists.insert("linkin park".into(), 5.0);

        let track = |genre: Option<&str>, artist: Option<&str>| Track {
            id: 1,
            title: "x".into(),
            artist: artist.map(Into::into),
            album: None,
            album_artist: None,
            track_number: None,
            year: None,
            genre: genre.map(Into::into),
            duration_ms: 1,
            format: hopea_common::AudioFormat::Mp3,
            codec: "MP3".into(),
            sample_rate: 44100,
            channels: 2,
            size: 1,
            sha256: String::new(),
            has_cover: false,
            uploader: String::new(),
            uploaded_at: 0,
            mine: false,
            local: false,
            favorite: false,
            sources: 0,
        };
        assert_eq!(
            taste.score(&track(Some("Nu Metal"), Some("Linkin Park"))),
            8.0
        );
        assert_eq!(taste.score(&track(Some("Nu Metal"), None)), 5.0);
        assert_eq!(taste.score(&track(Some("Jazz"), Some("LINKIN PARK"))), 3.0);
        assert_eq!(taste.score(&track(None, None)), 0.0);
        assert_eq!(Taste::top(&taste.genres, 3), ["nu metal"]);
    }
}
