// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Conservative metadata suggestions. Only the node's audio hash proves equality.
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

pub fn normalize(value: &str) -> String {
    value
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn versions(title: &str) -> Vec<&str> {
    const MARKERS: &[&str] = &[
        "live",
        "remix",
        "remastered",
        "remaster",
        "acoustic",
        "instrumental",
        "demo",
        "cover",
        "karaoke",
        "sped",
        "slowed",
        "ремикс",
        "концерт",
        "кавер",
    ];
    let mut result: Vec<_> = title
        .split_whitespace()
        .filter(|s| MARKERS.contains(s))
        .collect();
    result.sort_unstable();
    result.dedup();
    result
}

pub fn similar(
    title: &str,
    artist: Option<&str>,
    duration: Option<u64>,
    other_title: &str,
    other_artist: Option<&str>,
    other_duration: Option<u64>,
) -> bool {
    let (title, other_title) = (normalize(title), normalize(other_title));
    let (artist, other_artist) = (
        normalize(artist.unwrap_or("")),
        normalize(other_artist.unwrap_or("")),
    );
    if title.is_empty()
        || artist.is_empty()
        || other_artist.is_empty()
        || versions(&title) != versions(&other_title)
    {
        return false;
    }
    if let (Some(a), Some(b)) = (duration, other_duration) {
        if a.abs_diff(b) > 2500.max(a.min(b) / 50) {
            return false;
        }
    }
    let title_ok = title == other_title
        || (title.chars().count().min(other_title.chars().count()) >= 6
            && strsim::normalized_damerau_levenshtein(&title, &other_title) >= 0.90);
    let artist_ok = artist == other_artist
        || (artist.chars().count().min(other_artist.chars().count()) >= 6
            && strsim::normalized_damerau_levenshtein(&artist, &other_artist) >= 0.85);
    title_ok && artist_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn case_punctuation_and_one_artist_typo_are_candidates() {
        assert!(similar(
            "Pushing Me Away",
            Some("Linkin Park"),
            Some(191000),
            "PUSHING ME AWAY",
            Some("LINKINK PARK"),
            Some(192000)
        ));
        assert!(similar(
            "Déjà-vu",
            Some("Beyoncé"),
            None,
            "DEJA VU",
            Some("BEYONCE"),
            None
        ));
    }
    #[test]
    fn different_versions_artists_and_lengths_are_not_auto_deselected() {
        assert!(!similar(
            "Numb",
            Some("Linkin Park"),
            None,
            "Numb (Live)",
            Some("Linkin Park"),
            None
        ));
        assert!(!similar(
            "Hello",
            Some("Adele"),
            None,
            "Hello",
            Some("Lionel Richie"),
            None
        ));
        assert!(!similar(
            "Song",
            Some("Artist"),
            Some(120000),
            "Song",
            Some("Artist"),
            Some(180000)
        ));
        assert!(!similar("Song", None, None, "Song", None, None));
        assert!(!similar(
            "One",
            Some("Artist"),
            None,
            "On",
            Some("Artist"),
            None
        ));
    }
}

#[derive(Clone)]
pub struct Candidate {
    pub id: Option<i64>,
    pub sha256: Option<String>,
    pub title: String,
    pub artist: Option<String>,
    pub duration_ms: Option<u64>,
}

impl Candidate {
    pub fn matches(&self, track: &hopea_common::api::Track) -> bool {
        self.sha256.as_deref().map_or_else(
            || self.title == track.title && self.artist == track.artist,
            |hash| hash == track.sha256,
        )
    }
}

pub struct Index {
    items: Vec<Candidate>,
    buckets: std::collections::HashMap<String, Vec<usize>>,
}
impl Index {
    pub fn new(items: Vec<Candidate>) -> Self {
        let mut buckets: std::collections::HashMap<String, Vec<usize>> = Default::default();
        for (index, item) in items.iter().enumerate() {
            let key: String = normalize(&item.title).chars().take(4).collect();
            buckets.entry(key).or_default().push(index);
        }
        Self { items, buckets }
    }
    pub fn find(&self, track: &super::service_import::CatalogTrack) -> Option<Candidate> {
        let key: String = normalize(&track.title).chars().take(4).collect();
        self.buckets
            .get(&key)?
            .iter()
            .map(|&i| &self.items[i])
            .filter(|item| {
                similar(
                    &track.title,
                    track.artist.as_deref(),
                    track.duration_ms,
                    &item.title,
                    item.artist.as_deref(),
                    item.duration_ms,
                )
            })
            .max_by_key(|item| (item.id.is_some(), item.duration_ms.is_some()))
            .cloned()
    }
}
