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
/// Total keys a node announces, including overflow routing groups
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
    let legacy: Vec<_> = announced_words(tracks.clone())
        .into_iter()
        .flat_map(|w| {
            let p = prefix(&w);
            std::iter::once(w).chain(p)
        })
        .filter(|k| seen.insert(k.clone()))
        .take(MAX_ANNOUNCED_KEYS)
        .collect();
    // Scan every field, not just the first 300 words. A fixed bitmap bounds
    // memory even for huge catalogues. Group by the normalized three-character
    // prefix so partial words and the existing typo tolerance use the same route.
    let exact: HashSet<_> = legacy.iter().map(String::as_str).collect();
    let mut groups = [false; 256];
    let mut overflow = false;
    for fields in tracks {
        for text in fields.into_iter().flatten() {
            for word in short_words(text) {
                let route = routing_prefix(&word);
                groups[group_number(&route) as usize] = true;
                if !exact.contains(route.as_str()) {
                    overflow = true;
                }
            }
        }
    }
    if !overflow {
        return legacy;
    }
    // Reserve space for every occupied group before filling the remaining
    // budget with precise legacy keys. The relay registration format and its
    // 400-key validation limit stay unchanged.
    let coverage = groups
        .iter()
        .enumerate()
        .filter_map(|(n, present)| present.then(|| group_key(n as u8)));
    bounded_routing_keys(coverage.chain(legacy), MAX_ANNOUNCED_KEYS)
}

// Alphabetic namespace + two hex digits stays a normalized word in existing
// relay registrations. This is an address-routing hint, never a track match.
const GROUP_PREFIX: &str = "hopeacatalogroutev1";

fn routing_prefix(word: &str) -> String {
    word.chars().take(PREFIX_CHARS).collect()
}

fn group_number(prefix: &str) -> u8 {
    dht_key(prefix)[0]
}

pub fn routing_group(word: &str) -> String {
    group_key(group_number(&routing_prefix(word)))
}

fn group_key(number: u8) -> String {
    format!("{GROUP_PREFIX}{number:02x}")
}

pub fn is_routing_group(key: &str) -> bool {
    key.strip_prefix(GROUP_PREFIX).is_some_and(|suffix| {
        suffix.len() == 2
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Preserve all groups when several relay clients exceed the exact-key budget.
/// At most 256 groups exist, leaving room for precise legacy announcements.
pub fn bounded_routing_keys(keys: impl IntoIterator<Item = String>, limit: usize) -> Vec<String> {
    let mut groups = std::collections::BTreeSet::new();
    let mut exact = Vec::new();
    let mut seen = HashSet::new();
    for key in keys {
        if is_routing_group(&key) {
            groups.insert(key);
        } else if exact.len() < limit && seen.insert(key.clone()) {
            exact.push(key);
        }
    }
    groups.into_iter().chain(exact).take(limit).collect()
}

/// Exact/prefix routes remain compatible with old nodes. Overflow groups find
/// large catalogues whose precise term was outside their announcement budget.
pub fn query_routing_keys(query: &str) -> Vec<String> {
    let mut keys = Vec::new();
    // Start coverage routes before precise lookups can exhaust the shared
    // deadline. At most three groups leave concurrent slots for legacy keys.
    for word in query_words(query) {
        let key = group_key(group_number(&routing_prefix(&word)));
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    for key in query_keys(query) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
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
    fn every_title_in_a_large_catalogue_has_a_route_without_exceeding_the_budget() {
        let titles: Vec<_> = (0..2500)
            .map(|n| format!("{:x} raretitle{n}", n * 7919))
            .collect();
        let artists: Vec<_> = (0..2500).map(|n| format!("artist{n}")).collect();
        let tracks: Vec<_> = artists
            .iter()
            .zip(&titles)
            .map(|(artist, title)| [Some(artist.as_str()), None, Some(title.as_str()), None])
            .collect();
        let advertised: HashSet<_> = announced_keys(tracks.iter().copied()).into_iter().collect();
        assert!(advertised.len() <= MAX_ANNOUNCED_KEYS);
        assert!(advertised.iter().any(|k| is_routing_group(k)));
        for title in &titles {
            for word in words(title) {
                assert!(
                    query_routing_keys(&word)
                        .iter()
                        .any(|k| advertised.contains(k)),
                    "unreachable title term: {word}"
                );
            }
        }
        // Reversing the catalogue cannot remove any overflow groups.
        let reversed = announced_keys(tracks.iter().rev().copied());
        let groups = |keys: Vec<String>| {
            keys.into_iter()
                .filter(|k| is_routing_group(k))
                .collect::<HashSet<_>>()
        };
        assert_eq!(groups(advertised.into_iter().collect()), groups(reversed));
        assert!(query_routing_keys("three separate terms").len() <= 3 * MAX_QUERY_WORDS);
    }

    #[test]
    fn routing_groups_keep_normalization_and_short_title_coverage() {
        let tracks = [[Some("Muse"), None, Some("B"), None]];
        let keys = announced_keys(tracks.iter().copied());
        assert!(query_routing_keys("B.mp3").iter().any(|k| keys.contains(k)));
        assert_eq!(
            routing_group(&words("Beyoncé").next().unwrap()),
            routing_group("beyonce")
        );
        assert_eq!(routing_group("linkin"), routing_group("linkn"));
        for key in keys.iter().filter(|k| is_routing_group(k)) {
            assert!(key.len() <= 64);
            assert_eq!(plain_words(key).collect::<Vec<_>>(), [key.clone()]);
        }
    }

    #[test]
    fn relay_budget_keeps_all_groups_ahead_of_many_precise_terms() {
        let exact = (0..5000).map(|i| format!("term{i}"));
        let groups: Vec<_> = (0..=255).map(group_key).collect();
        let keys = bounded_routing_keys(exact.chain(groups.iter().cloned()), 1000);
        assert_eq!(keys.len(), 1000);
        assert!(groups.iter().all(|group| keys.contains(group)));
    }

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
