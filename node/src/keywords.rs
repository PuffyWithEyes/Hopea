// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! The words that make a node findable.
//!
//! Nothing on the network holds a list of everyone's tracks. Instead, each
//! node announces itself in the BitTorrent DHT under one key per word in the
//! artists, titles and albums it has, the same way torrents announce their
//! peers. Searching for "linkin park" looks up the keys for "linkin" and
//! "park", and asks the nodes found there. The DHT only ever stores
//! addresses; the track details stay on the nodes that have the files.

use std::collections::HashSet;

use hopea_common::AudioFormat;
use sha1::{Digest, Sha1};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

/// Words looked up in the DHT for one search; the longest are used, as
/// they tend to be the most specific
pub const MAX_QUERY_WORDS: usize = 3;
/// Words a node announces at most, to bound its DHT traffic
pub const MAX_ANNOUNCED_WORDS: usize = 300;
/// Keys a node announces at most, words and their beginnings together
pub const MAX_ANNOUNCED_KEYS: usize = 400;

const STOP_WORDS: &[&str] = &[
    "the", "and", "of", "an", "in", "on", "to", "for", "feat", "ft", "vs", "with", "de", "la",
    "le", "el", "из", "на", "по", "не",
];

/// Splits text into normalized words: lowercase, without diacritics (so
/// "Beyoncé" and "beyonce" are the same word), at least two characters.
pub fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    words_of_length(text, 2)
}

/// Like [`words`], but also single letters and digits: only used for titles
/// such as "A" or "7" that have no longer word at all.
fn short_words(text: &str) -> impl Iterator<Item = String> + '_ {
    words_of_length(text, 1)
}

/// All words of a text, normalized like [`words`], including single
/// letters and stop words: for matching text, not for DHT keys.
pub fn plain_words(text: &str) -> impl Iterator<Item = String> + '_ {
    normalize(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>()
        .into_iter()
}

fn normalize(text: &str) -> String {
    text.nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

fn words_of_length(text: &str, min_chars: usize) -> impl Iterator<Item = String> + '_ {
    normalize(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= min_chars && !STOP_WORDS.contains(w))
        .map(str::to_string)
        .collect::<Vec<_>>()
        .into_iter()
}

/// Removes audio file extensions from a query, since people search for
/// "a.mp3" when they mean the song "A". Song titles never keep them: a title
/// taken from a file name loses its extension.
pub fn strip_extensions(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| match term.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() && AudioFormat::from_extension(ext).is_some() => {
                stem
            }
            _ => term,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The DHT key a node announces under when it has `word`.
pub fn dht_key(word: &str) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(b"anysik/word/v1/");
    hasher.update(word.as_bytes());
    hasher.finalize().into()
}

/// Letters a word's beginning is also announced under, so that searches for
/// part of a word, or a word with a typo after its start, still find nodes
const PREFIX_CHARS: usize = 3;

/// The beginning of a longer word, announced along with it.
fn prefix(word: &str) -> Option<String> {
    (word.chars().count() > PREFIX_CHARS).then(|| word.chars().take(PREFIX_CHARS).collect())
}

/// The DHT keys, as words, a node announces for its tracks: each word and
/// the beginning of each longer word.
pub fn announced_keys<'a>(
    tracks: impl Iterator<Item = [Option<&'a str>; 4]> + Clone,
) -> Vec<String> {
    let mut seen = HashSet::new();
    announced_words(tracks)
        .into_iter()
        .flat_map(|w| {
            let p = prefix(&w);
            std::iter::once(w).chain(p)
        })
        .filter(|k| seen.insert(k.clone()))
        .take(MAX_ANNOUNCED_KEYS)
        .collect()
}

/// The DHT keys, as words, looked up for a query: its most specific words,
/// and their beginnings, which find the nodes that have words starting the
/// same way (partial words, and typos after the first letters).
pub fn query_keys(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    query_words(query)
        .into_iter()
        .flat_map(|w| {
            let p = prefix(&w);
            std::iter::once(w).chain(p)
        })
        .filter(|k| seen.insert(k.clone()))
        .collect()
}

/// The words of a search query that are looked up in the DHT.
pub fn query_words(query: &str) -> Vec<String> {
    let query = strip_extensions(query);
    let mut seen = HashSet::new();
    let mut found: Vec<String> = words(&query).filter(|w| seen.insert(w.clone())).collect();
    if found.is_empty() {
        // "A", "7": only nodes with such short titles announce these
        found = short_words(&query)
            .filter(|w| seen.insert(w.clone()))
            .collect();
    }
    found.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    found.truncate(MAX_QUERY_WORDS);
    found
}

/// The words to announce for a set of tracks, given as
/// `(artist, genre, title, album)` texts. Artist words come first, since that
/// is what people search for most, then genres (which recommendations search
/// for), then titles, then albums.
pub fn announced_words<'a>(
    tracks: impl Iterator<Item = [Option<&'a str>; 4]> + Clone,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut add = |word: String, out: &mut Vec<String>| {
        if out.len() < MAX_ANNOUNCED_WORDS && seen.insert(word.clone()) {
            out.push(word);
        }
    };
    for field in 0..4 {
        for texts in tracks.clone() {
            for word in texts[field].into_iter().flat_map(words) {
                add(word, &mut out);
            }
        }
    }
    // A track with no word of two letters or more, such as one called "A",
    // is announced under its single letters and digits instead
    for texts in tracks {
        if texts.iter().flatten().all(|t| words(t).next().is_none()) {
            for word in texts.iter().flatten().flat_map(|t| short_words(t)) {
                add(word, &mut out);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        let w: Vec<_> = words("Beyoncé – Crazy in Love (feat. JAY-Z)").collect();
        assert_eq!(w, ["beyonce", "crazy", "love", "jay"]);
        let w: Vec<_> = words("Кино — Звезда по имени Солнце").collect();
        assert_eq!(w, ["кино", "звезда", "имени", "солнце"]);
        assert_eq!(dht_key("linkin"), dht_key(&words("LINKIN").next().unwrap()));
    }

    #[test]
    fn picks_specific_query_words() {
        assert_eq!(query_words("linkin park numb"), ["linkin", "park", "numb"]);
        // With nothing longer, single letters are looked up; stop words never
        assert_eq!(query_words("a b the"), ["a", "b"]);
        assert_eq!(
            query_words("in the end linkin park linkin"),
            ["linkin", "park", "end"]
        );
    }

    #[test]
    fn beginnings_of_words_are_keys_too() {
        let tracks = vec![[Some("Linkin Park"), None, Some("Numb"), None]];
        let keys = announced_keys(tracks.iter().copied());
        assert_eq!(keys, ["linkin", "lin", "park", "par", "numb", "num"]);
        // A typo after the first letters still shares the beginning
        assert_eq!(query_keys("linkn"), ["linkn", "lin"]);
        assert_eq!(query_keys("lin"), ["lin"]);
    }

    #[test]
    fn finds_short_titles_and_file_names() {
        assert_eq!(strip_extensions("A.mp3"), "A");
        assert_eq!(
            strip_extensions("Night Drive.FLAC live"),
            "Night Drive live"
        );
        assert_eq!(strip_extensions("v1.2 .mp3"), "v1.2 .mp3");
        assert_eq!(query_words("A.mp3"), ["a"]);
        assert_eq!(query_words("A"), ["a"]);
        // Single letters only when there is nothing longer
        assert_eq!(query_words("a numb"), ["numb"]);

        let tracks = vec![
            [None, None, Some("A"), None],
            [Some("Muse"), None, Some("B"), None],
        ];
        let words = announced_words(tracks.iter().copied());
        assert_eq!(words, ["muse", "a"]);
    }

    #[test]
    fn announces_artists_first() {
        let tracks = vec![
            [
                Some("Linkin Park"),
                Some("Nu Metal"),
                Some("Numb"),
                Some("Meteora"),
            ],
            [
                Some("Daft Punk"),
                Some("House"),
                Some("One More Time"),
                None,
            ],
        ];
        let words = announced_words(tracks.iter().copied());
        assert_eq!(
            words,
            [
                "linkin", "park", "daft", "punk", "nu", "metal", "house", "numb", "one", "more",
                "time", "meteora"
            ]
        );
    }
}
