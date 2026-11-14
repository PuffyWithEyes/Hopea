// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Signed descriptions of shared tracks.
//!
//! An entry is what a node returns when another node searches it: the
//! track's details and the hashes of its torrent `info` dictionary, signed
//! by the node that first shared it. Nodes that later download the track
//! keep the entry and return it unchanged, so the publisher shown for a
//! track is always the one who signed it.
//!
//! The info dictionary itself (most of a `.torrent`: the piece hashes, up to
//! about 20 KiB) is not in search answers, which keeps them small. A node
//! fetches it only to play the track, from a node that has it, and accepts
//! it only if both its SHA-1 (the info hash) and its SHA-256 match the
//! signed entry. Entries of the first version carried the dictionary inline;
//! they are still accepted.
//!
//! The signature covers the exact body bytes as sent. Receivers verify the
//! signature first and only then parse the body, and every field is checked
//! as strictly as an upload would be: everything in it ends up on screen or
//! in the BitTorrent session.

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, VerifyingKey};
use hopea_common::AudioFormat;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    config::Limits,
    identity::Identity,
    ingest::{sanitize, sanitize_short},
    torrent_info::{self, is_info_hash, InfoDict},
};

const VERSION: u32 = 2;
/// Largest accepted body; a first version body with its info dictionary is
/// at most about 40 KiB
const MAX_BODY_BYTES: usize = 64 * 1024;
/// Largest accepted info dictionary
pub const MAX_INFO_BYTES: usize = 48 * 1024;
pub const MAX_NAME_CHARS: usize = 32;

/// An entry as sent between nodes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireEntry {
    /// Base64 of the signed JSON body
    pub body: String,
    /// Base64 of the Ed25519 signature over the body bytes
    pub sig: String,
    /// Base64 of the info dictionary, when it is sent along; not covered by
    /// the signature, but by the hashes in the body
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<String>,
}

/// Details of a track, as claimed by its publisher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackMeta {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub duration_ms: u64,
    pub format: AudioFormat,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub size: u64,
    /// SHA-256 of the file
    pub sha256: String,
    /// SHA-256 of the audio packets only
    pub audio_sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    v: u32,
    publisher: String,
    name: Option<String>,
    created_at: i64,
    /// Version 2: SHA-1 of the info dictionary, the BitTorrent info hash
    #[serde(default, skip_serializing_if = "Option::is_none")]
    info_hash: Option<String>,
    /// Version 2: SHA-256 of the info dictionary
    #[serde(default, skip_serializing_if = "Option::is_none")]
    info_sha256: Option<String>,
    /// Version 1: base64 of the info dictionary itself
    #[serde(default, skip_serializing_if = "Option::is_none")]
    info: Option<String>,
    /// SHA-256 of the track's cover art, which nodes that have it send on
    /// request; set when the track has one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cover_sha256: Option<String>,
    track: TrackMeta,
}

/// A verified entry.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Hex Ed25519 public key of the publisher
    pub publisher: String,
    /// Name the publisher chose to show, if any; not verified in any way
    pub name: Option<String>,
    pub created_at: i64,
    pub info_hash: String,
    pub info_sha256: String,
    /// The info dictionary, once known and checked against the hashes
    pub info_bytes: Option<Vec<u8>>,
    /// SHA-256 of the cover art, if the track has one
    pub cover_sha256: Option<String>,
    pub track: TrackMeta,
    pub body: Vec<u8>,
    pub sig: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum EntryError {
    #[error("malformed entry")]
    Malformed,
    #[error("bad signature")]
    BadSignature,
    #[error("invalid entry: {0}")]
    Invalid(&'static str),
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

impl Entry {
    /// Creates and signs an entry for a track this node shares.
    pub fn create(
        identity: &Identity,
        name: Option<&str>,
        created_at: i64,
        info_bytes: Vec<u8>,
        track: TrackMeta,
        cover_sha256: Option<String>,
    ) -> Self {
        let info_hash = torrent_info::info_hash(&info_bytes);
        let info_sha256 = sha256_hex(&info_bytes);
        let body = Body {
            v: VERSION,
            publisher: identity.public_hex(),
            name: name.map(str::to_string),
            created_at,
            info_hash: Some(info_hash.clone()),
            info_sha256: Some(info_sha256.clone()),
            info: None,
            cover_sha256: cover_sha256.clone(),
            track,
        };
        let body_bytes = serde_json::to_vec(&body).expect("entries always serialize");
        let sig = identity.sign(&body_bytes).to_vec();
        Self {
            publisher: body.publisher,
            name: body.name,
            created_at,
            info_hash,
            info_sha256,
            info_bytes: Some(info_bytes),
            cover_sha256,
            track: body.track,
            body: body_bytes,
            sig,
        }
    }

    /// The entry as sent to other nodes, with its info dictionary if asked
    /// for and known.
    pub fn to_wire(&self, with_info: bool) -> WireEntry {
        WireEntry {
            body: STANDARD.encode(&self.body),
            sig: STANDARD.encode(&self.sig),
            info: self
                .info_bytes
                .as_ref()
                .filter(|_| with_info)
                .map(|i| STANDARD.encode(i)),
        }
    }

    /// Restores an entry from the database, where it was stored after
    /// being verified, along with its info dictionary if known.
    pub fn from_stored(
        body: Vec<u8>,
        sig: Vec<u8>,
        info: Option<Vec<u8>>,
        limits: &Limits,
        now: i64,
    ) -> Result<Self, EntryError> {
        let mut entry = Self::verify_bytes(body, sig, limits, now)?;
        if let Some(info) = info.filter(|i| !i.is_empty()) {
            entry.attach_info(info, limits)?;
        }
        Ok(entry)
    }

    /// Verifies an entry received from another node, and the info
    /// dictionary sent along with it, if any.
    pub fn verify(wire: &WireEntry, limits: &Limits, now: i64) -> Result<Self, EntryError> {
        if wire.body.len() > MAX_BODY_BYTES * 4 / 3 + 4 || wire.sig.len() > 128 {
            return Err(EntryError::Malformed);
        }
        let body = STANDARD
            .decode(&wire.body)
            .map_err(|_| EntryError::Malformed)?;
        let sig = STANDARD
            .decode(&wire.sig)
            .map_err(|_| EntryError::Malformed)?;
        let mut entry = Self::verify_bytes(body, sig, limits, now)?;
        if let Some(info) = &wire.info {
            if entry.info_bytes.is_none() {
                entry.attach_info(decode_info(info)?, limits)?;
            }
        }
        Ok(entry)
    }

    /// Accepts an info dictionary for this entry if it is the one the
    /// signed hashes name, and fits the track.
    pub fn attach_info(&mut self, info_bytes: Vec<u8>, limits: &Limits) -> Result<(), EntryError> {
        if torrent_info::info_hash(&info_bytes) != self.info_hash
            || sha256_hex(&info_bytes) != self.info_sha256
        {
            return Err(EntryError::Invalid("the torrent does not match its hashes"));
        }
        let info = InfoDict::parse(&info_bytes, limits.max_upload_bytes)
            .ok_or(EntryError::Invalid("torrent"))?;
        if info.format() != Some(self.track.format) || info.length != self.track.size {
            return Err(EntryError::Invalid("torrent does not match the track"));
        }
        self.info_bytes = Some(info_bytes);
        Ok(())
    }

    fn verify_bytes(
        body_bytes: Vec<u8>,
        sig: Vec<u8>,
        limits: &Limits,
        now: i64,
    ) -> Result<Self, EntryError> {
        if body_bytes.len() > MAX_BODY_BYTES {
            return Err(EntryError::Malformed);
        }

        // The publisher key is needed to check the signature, so it is
        // pulled out before the rest of the body is trusted
        #[derive(Deserialize)]
        struct PublisherOnly {
            publisher: String,
        }
        let publisher = serde_json::from_slice::<PublisherOnly>(&body_bytes)
            .map_err(|_| EntryError::Malformed)?
            .publisher;
        let key_bytes: [u8; 32] = hex::decode(&publisher)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or(EntryError::Malformed)?;
        let key = VerifyingKey::from_bytes(&key_bytes).map_err(|_| EntryError::Malformed)?;
        let signature = Signature::from_slice(&sig).map_err(|_| EntryError::Malformed)?;
        key.verify_strict(&body_bytes, &signature)
            .map_err(|_| EntryError::BadSignature)?;

        let body: Body = serde_json::from_slice(&body_bytes).map_err(|_| EntryError::Malformed)?;
        // Hex must be canonical too, or the same key would have two spellings
        if body.publisher != hex::encode(key_bytes) {
            return Err(EntryError::Malformed);
        }
        if let Some(ref name) = body.name {
            if sanitize_short(name, MAX_NAME_CHARS) != *name || name.is_empty() {
                return Err(EntryError::Invalid("name"));
            }
        }
        // 2020-01-01 to a day from now, allowing for clocks that are off
        if !(1_577_836_800..=now + 86_400).contains(&body.created_at) {
            return Err(EntryError::Invalid("date"));
        }
        check_track(&body.track, limits)?;
        if body.cover_sha256.as_deref().is_some_and(|c| !is_sha256(c)) {
            return Err(EntryError::Invalid("cover"));
        }

        let mut entry = Self {
            publisher: body.publisher,
            name: body.name,
            created_at: body.created_at,
            info_hash: String::new(),
            info_sha256: String::new(),
            info_bytes: None,
            cover_sha256: body.cover_sha256,
            track: body.track,
            body: body_bytes,
            sig,
        };
        match (body.v, body.info, body.info_hash, body.info_sha256) {
            (1, Some(info), None, None) => {
                let info_bytes = decode_info(&info)?;
                entry.info_hash = torrent_info::info_hash(&info_bytes);
                entry.info_sha256 = sha256_hex(&info_bytes);
                entry.attach_info(info_bytes, limits)?;
            }
            (2, None, Some(info_hash), Some(info_sha256)) => {
                if !is_info_hash(&info_hash) || !is_sha256(&info_sha256) {
                    return Err(EntryError::Invalid("hashes"));
                }
                entry.info_hash = info_hash;
                entry.info_sha256 = info_sha256;
            }
            _ => return Err(EntryError::Invalid("unknown version")),
        }
        Ok(entry)
    }
}

fn decode_info(info: &str) -> Result<Vec<u8>, EntryError> {
    if info.len() > MAX_INFO_BYTES * 4 / 3 + 4 {
        return Err(EntryError::Malformed);
    }
    STANDARD.decode(info).map_err(|_| EntryError::Malformed)
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The codec names a format may carry, as reported by validation.
fn codec_allowed(format: AudioFormat, codec: &str) -> bool {
    match format {
        AudioFormat::Flac => codec == "FLAC",
        AudioFormat::Mp3 => codec == "MP3",
        AudioFormat::Mp4 => codec == "AAC" || codec == "ALAC",
        AudioFormat::Ogg => codec == "Vorbis",
        AudioFormat::Wav => codec == "PCM",
    }
}

fn check_track(t: &TrackMeta, limits: &Limits) -> Result<(), EntryError> {
    let clean = |s: &str| sanitize(s).as_deref() == Some(s);
    let clean_opt = |s: &Option<String>| s.as_deref().is_none_or(clean);

    if !clean(&t.title)
        || !clean_opt(&t.artist)
        || !clean_opt(&t.album)
        || !clean_opt(&t.album_artist)
        || !clean_opt(&t.genre)
    {
        return Err(EntryError::Invalid("text"));
    }
    if t.track_number.is_some_and(|n| !(1..=9999).contains(&n))
        || t.year.is_some_and(|y| !(1000..=9999).contains(&y))
    {
        return Err(EntryError::Invalid("numbers"));
    }
    if t.size == 0 || t.size > limits.max_upload_bytes {
        return Err(EntryError::Invalid("size"));
    }
    if !codec_allowed(t.format, &t.codec)
        || !limits.sample_rates.contains(&t.sample_rate)
        || !(1..=limits.max_channels).contains(&t.channels)
        || t.duration_ms == 0
        || t.duration_ms > limits.max_duration.as_millis() as u64
    {
        return Err(EntryError::Invalid("audio parameters"));
    }
    if !is_sha256(&t.sha256) || !is_sha256(&t.audio_sha256) {
        return Err(EntryError::Invalid("hashes"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn meta() -> TrackMeta {
        TrackMeta {
            title: "Numb".into(),
            artist: Some("Linkin Park".into()),
            album: None,
            album_artist: None,
            track_number: Some(13),
            year: Some(2003),
            genre: None,
            duration_ms: 185_000,
            format: AudioFormat::Flac,
            codec: "FLAC".into(),
            sample_rate: 44_100,
            channels: 2,
            size: 40_000,
            sha256: "a".repeat(64),
            audio_sha256: "b".repeat(64),
        }
    }

    fn info() -> Vec<u8> {
        InfoDict {
            name: "track.flac".into(),
            length: 40_000,
            piece_length: 16 * 1024,
            pieces: vec![1; 60],
        }
        .encode()
    }

    #[test]
    fn signs_and_verifies() {
        let id = Identity::generate();
        let entry = Entry::create(&id, Some("alice"), NOW, info(), meta(), None);

        // Search answers leave the info dictionary out
        let wire = entry.to_wire(false);
        assert!(wire.info.is_none());
        assert!(wire.body.len() < 2000, "{} bytes", wire.body.len());
        let mut back = Entry::verify(&wire, &Limits::default(), NOW).unwrap();
        assert_eq!(back.publisher, id.public_hex());
        assert_eq!(back.name.as_deref(), Some("alice"));
        assert_eq!(back.track, meta());
        assert_eq!(back.info_hash, entry.info_hash);
        assert!(back.info_bytes.is_none());

        // It is fetched later and must match the signed hashes
        let mut other = info();
        other[20] ^= 1;
        assert!(back.attach_info(other, &Limits::default()).is_err());
        back.attach_info(info(), &Limits::default()).unwrap();

        // Or it comes along
        let with = Entry::verify(&entry.to_wire(true), &Limits::default(), NOW).unwrap();
        assert_eq!(with.info_bytes, Some(info()));
    }

    #[test]
    fn rejects_tampering() {
        let id = Identity::generate();
        let entry = Entry::create(&id, None, NOW, info(), meta(), None);
        let limits = Limits::default();

        // Someone else's key on the same body
        let mut body: serde_json::Value = serde_json::from_slice(&entry.body).unwrap();
        body["publisher"] = Identity::generate().public_hex().into();
        let forged = WireEntry {
            body: STANDARD.encode(serde_json::to_vec(&body).unwrap()),
            sig: entry.to_wire(false).sig,
            info: None,
        };
        assert!(matches!(
            Entry::verify(&forged, &limits, NOW),
            Err(EntryError::BadSignature)
        ));

        // A changed title
        let changed = String::from_utf8(entry.body.clone())
            .unwrap()
            .replace("Numb", "Faint");
        let wire = WireEntry {
            body: STANDARD.encode(changed),
            sig: entry.to_wire(false).sig,
            info: None,
        };
        assert!(matches!(
            Entry::verify(&wire, &limits, NOW),
            Err(EntryError::BadSignature)
        ));

        // A different info dictionary sent along
        let mut wire = entry.to_wire(false);
        let mut other = info();
        other[20] ^= 1;
        wire.info = Some(STANDARD.encode(other));
        assert!(Entry::verify(&wire, &limits, NOW).is_err());
    }

    #[test]
    fn rejects_bad_contents_even_when_signed() {
        let id = Identity::generate();
        let limits = Limits::default();
        let check = |track: TrackMeta, info: Vec<u8>| {
            let entry = Entry::create_unchecked(&id, track, info);
            Entry::verify(&entry, &limits, NOW).is_err()
        };

        let mut t = meta();
        t.title = "bad\u{202E}title".into();
        assert!(check(t, info()));

        // The torrent must fit the track
        let mut t = meta();
        t.size = 1;
        assert!(check(t, info()));

        let mut t = meta();
        t.format = AudioFormat::Mp3;
        assert!(check(t, info()));

        let mut t = meta();
        t.codec = "Opus".into();
        assert!(check(t, info()));

        assert!(check(meta(), b"d4:infoe".to_vec()));
    }

    #[test]
    fn accepts_the_first_version() {
        let id = Identity::generate();
        let body = serde_json::json!({
            "v": 1,
            "publisher": id.public_hex(),
            "name": null,
            "created_at": NOW,
            "info": STANDARD.encode(info()),
            "track": meta(),
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let wire = WireEntry {
            sig: STANDARD.encode(id.sign(&bytes)),
            body: STANDARD.encode(bytes),
            info: None,
        };
        let entry = Entry::verify(&wire, &Limits::default(), NOW).unwrap();
        assert_eq!(entry.info_bytes, Some(info()));
        assert_eq!(entry.info_hash, torrent_info::info_hash(&info()));
    }

    impl Entry {
        /// Signs a body without the checks `create` relies on, sending the
        /// info dictionary along.
        fn create_unchecked(id: &Identity, track: TrackMeta, info: Vec<u8>) -> WireEntry {
            let body = Body {
                v: VERSION,
                publisher: id.public_hex(),
                name: None,
                created_at: NOW,
                info_hash: Some(torrent_info::info_hash(&info)),
                info_sha256: Some(sha256_hex(&info)),
                info: None,
                cover_sha256: None,
                track,
            };
            let bytes = serde_json::to_vec(&body).unwrap();
            WireEntry {
                sig: STANDARD.encode(id.sign(&bytes)),
                body: STANDARD.encode(bytes),
                info: Some(STANDARD.encode(info)),
            }
        }
    }
}
