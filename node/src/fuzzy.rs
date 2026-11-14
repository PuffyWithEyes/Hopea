// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Forgiving search: parts of words and typos.
//!
//! Every word of a query must match some word of the track's title, artist,
//! album or genre, but a match can be:
//!
//! - the whole word ("numb"), best
//! - its beginning ("link" for "linkin")
//! - a part of it, from three letters on ("inki" for "linkin")
//! - the word with typos: one in words of three to five letters, two up to
//!   eight, three beyond ("prk", "linkn", "lnikin"); its beginning, the same
//!   way, from four letters on
//!
//! Tracks are ranked by how well their words match, with title and artist
//! counting more than album and genre.

use crate::keywords;

/// Words of a query considered at most
const MAX_QUERY_WORDS: usize = 16;

/// How much a match in each field counts
const TITLE: f64 = 1.0;
const ARTIST: f64 = 0.95;
const ALBUM: f64 = 0.8;
const GENRE: f64 = 0.6;

pub struct Query {
    words: Vec<String>,
}

/// The searchable text of a track.
pub struct Fields<'a> {
    pub title: &'a str,
    pub artist: Option<&'a str>,
    pub album: Option<&'a str>,
    pub album_artist: Option<&'a str>,
    pub genre: Option<&'a str>,
}

impl Query {
    /// `None` for a query without any word.
    pub fn new(text: &str) -> Option<Self> {
        let text = keywords::strip_extensions(text);
        let mut words: Vec<String> = keywords::plain_words(&text).collect();
        words.dedup();
        words.truncate(MAX_QUERY_WORDS);
        (!words.is_empty()).then_some(Self { words })
    }

    pub(crate) fn cache_key(&self) -> String {
        self.words.join("\0")
    }

    /// How well a track matches, from 0 to 1, or `None` if some word of
    /// the query matches nothing.
    pub fn score(&self, fields: &Fields<'_>) -> Option<f64> {
        let texts = [
            (Some(fields.title), TITLE),
            (fields.artist, ARTIST),
            (fields.album_artist, ARTIST),
            (fields.album, ALBUM),
            (fields.genre, GENRE),
        ];
        let candidates: Vec<(String, f64)> = texts
            .iter()
            .filter_map(|(text, weight)| Some((text.as_ref()?, *weight)))
            .flat_map(|(text, weight)| keywords::plain_words(text).map(move |w| (w, weight)))
            .collect();

        let mut total = 0.0;
        for query_word in &self.words {
            let best = candidates
                .iter()
                .map(|(word, weight)| similarity(query_word, word) * weight)
                .fold(0.0, f64::max);
            if best <= 0.0 {
                return None;
            }
            total += best;
        }
        // Of two titles the query matches as well, the one it covers more
        // of is closer: "numb" is closer to "Numb" than to "Numb Encore"
        let title: Vec<String> = keywords::plain_words(fields.title).collect();
        let covered = title
            .iter()
            .filter(|word| self.words.iter().any(|q| similarity(q, word) > 0.0))
            .count();
        let coverage = covered as f64 / title.len().max(1) as f64;
        Some(total / self.words.len() as f64 * (0.9 + 0.1 * coverage))
    }
}

/// Typos tolerated in a word of `len` characters.
fn allowed_typos(len: usize) -> usize {
    match len {
        0..=2 => 0,
        3..=5 => 1,
        6..=8 => 2,
        _ => 3,
    }
}

/// How well a query word matches a word, from 0 (not at all) to 1.
fn similarity(query: &str, word: &str) -> f64 {
    if query == word {
        return 1.0;
    }
    if word.starts_with(query) {
        return 0.9;
    }
    let query_len = query.chars().count();
    if query_len >= 3 && word.contains(query) {
        return 0.75;
    }

    let allowed = allowed_typos(query_len);
    if allowed == 0 {
        return 0.0;
    }
    let word_len = word.chars().count();
    // The whole word with typos, or its beginning with typos (a part of a
    // word, typed wrong)
    let whole =
        (word_len.abs_diff(query_len) <= allowed).then(|| strsim::damerau_levenshtein(query, word));
    // Beginnings of words are only compared from four letters on: a
    // three-letter start with a typo matches far too much
    let start = (word_len > query_len && query_len >= 4).then(|| {
        let prefix: String = word.chars().take(query_len).collect();
        strsim::damerau_levenshtein(query, &prefix)
    });
    let typos = match (whole, start) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => return 0.0,
    };
    if typos == 0 || typos > allowed {
        return 0.0;
    }
    0.7 - 0.1 * (typos - 1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track<'a>(title: &'a str, artist: Option<&'a str>) -> Fields<'a> {
        Fields {
            title,
            artist,
            album: None,
            album_artist: None,
            genre: None,
        }
    }

    fn score(query: &str, fields: &Fields<'_>) -> Option<f64> {
        Query::new(query).unwrap().score(fields)
    }

    #[test]
    fn whole_words_parts_and_typos() {
        let numb = track("Numb", Some("Linkin Park"));
        for query in [
            "numb",
            "Linkin Park",
            "link",   // beginning
            "inki",   // part
            "linkn",  // typo
            "lnikin", // swapped letters
            "linkin prk numb",
            "LINKIN",
            "numb.mp3", // a file name
        ] {
            assert!(score(query, &numb).is_some(), "{query}");
        }
        for query in ["metallica", "numb faint", "lnk", "xyz park"] {
            assert!(score(query, &numb).is_none(), "{query}");
        }
    }

    #[test]
    fn closer_matches_rank_higher() {
        let one = track("One", Some("Metallica"));
        let exact = score("metallica", &one).unwrap();
        let prefix = score("metal", &one).unwrap();
        let typo = score("metalica", &one).unwrap();
        let two_typos = score("mtalica", &one).unwrap();
        assert!(exact > prefix && prefix > typo && typo > two_typos);

        // A match in the title counts more than one in the genre
        let in_title = track("Rock Song", None);
        let in_genre = Fields {
            genre: Some("Rock"),
            ..track("Song", None)
        };
        assert!(score("rock", &in_title).unwrap() > score("rock", &in_genre).unwrap());
    }

    #[test]
    fn short_words_must_be_exact_or_beginnings() {
        let a = track("A", None);
        assert!(score("a", &a).is_some());
        assert!(score("b", &a).is_none());
        let abc = track("Abc", None);
        assert!(score("ab", &abc).is_some());
        assert!(score("ac", &abc).is_none());
    }

    #[test]
    fn other_alphabets() {
        let kino = track("Звезда по имени Солнце", Some("Кино"));
        assert!(score("звезда", &kino).is_some());
        assert!(score("звзда", &kino).is_some());
        assert!(score("солнц", &kino).is_some());
        assert!(score("кино звезда", &kino).is_some());
    }
}
