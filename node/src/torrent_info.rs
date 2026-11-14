// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! The BitTorrent `info` dictionary of a shared track.
//!
//! Every torrent on the network holds exactly one audio file named
//! `track.<ext>`. The dictionary is built and parsed here rather than by the
//! BitTorrent library, so that only one strict, canonical shape is accepted:
//! the four keys of a single file torrent, in order, and nothing else. A
//! received dictionary is re-encoded and must come out byte for byte the
//! same, which rules out extra keys, odd integer spellings and file lists
//! with paths in them.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use hopea_common::AudioFormat;
use sha1::{Digest, Sha1};

const MIN_PIECE_LENGTH: u32 = 16 * 1024;
const MAX_PIECE_LENGTH: u32 = 4 * 1024 * 1024;
/// Piece hashes are kept to at most this many, so that an info dictionary
/// stays around 20 KiB even for the largest accepted file
const TARGET_MAX_PIECES: u64 = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InfoDict {
    pub name: String,
    pub length: u64,
    pub piece_length: u32,
    /// Concatenated SHA-1 hashes of the pieces
    pub pieces: Vec<u8>,
}

/// The name of the single file of a torrent holding `format` audio.
pub fn file_name(format: AudioFormat) -> String {
    format!("track.{}", format.canonical_extension())
}

fn piece_length_for(length: u64) -> u32 {
    let wanted = length.div_ceil(TARGET_MAX_PIECES).next_power_of_two();
    wanted.clamp(u64::from(MIN_PIECE_LENGTH), u64::from(MAX_PIECE_LENGTH)) as u32
}

fn piece_count(length: u64, piece_length: u32) -> u64 {
    length.div_ceil(u64::from(piece_length))
}

impl InfoDict {
    /// Hashes the file at `path` into an info dictionary for `format`.
    pub fn for_file(path: &Path, format: AudioFormat) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let length = file.metadata()?.len();
        if length == 0 || length > u64::from(MAX_PIECE_LENGTH) * TARGET_MAX_PIECES * 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file exceeds the torrent piece budget",
            ));
        }
        let piece_length = piece_length_for(length);
        let mut pieces = Vec::with_capacity(piece_count(length, piece_length) as usize * 20);
        let mut buf = vec![0; piece_length as usize];
        let mut read = 0u64;
        loop {
            let mut filled = 0;
            while filled < buf.len() {
                let n = file.read(&mut buf[filled..])?;
                if n == 0 {
                    break;
                }
                filled += n;
                read += n as u64;
                if read > length {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "file grew while hashing",
                    ));
                }
            }
            if filled == 0 {
                break;
            }
            pieces.extend_from_slice(&Sha1::digest(&buf[..filled]));
            if filled < buf.len() {
                break;
            }
        }
        if read != length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file shrank while hashing",
            ));
        }
        Ok(Self {
            name: file_name(format),
            length,
            piece_length,
            pieces,
        })
    }

    /// The canonical bencoding: keys sorted, as BitTorrent requires.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pieces.len() + 96);
        out.extend_from_slice(b"d6:lengthi");
        out.extend_from_slice(self.length.to_string().as_bytes());
        out.extend_from_slice(b"e4:name");
        out.extend_from_slice(format!("{}:", self.name.len()).as_bytes());
        out.extend_from_slice(self.name.as_bytes());
        out.extend_from_slice(b"12:piece lengthi");
        out.extend_from_slice(self.piece_length.to_string().as_bytes());
        out.extend_from_slice(b"e6:pieces");
        out.extend_from_slice(format!("{}:", self.pieces.len()).as_bytes());
        out.extend_from_slice(&self.pieces);
        out.push(b'e');
        out
    }

    /// Parses a dictionary received from another node. Returns `None` for
    /// anything that isn't exactly what [`InfoDict::encode`] would produce
    /// for a valid track.
    pub fn parse(bytes: &[u8], max_length: u64) -> Option<Self> {
        let mut p = Parser { rest: bytes };
        p.expect(b"d6:lengthi")?;
        let length = p.integer()?;
        p.expect(b"4:name")?;
        let name = String::from_utf8(p.string(32)?.to_vec()).ok()?;
        p.expect(b"12:piece lengthi")?;
        let piece_length = u32::try_from(p.integer()?).ok()?;
        p.expect(b"6:pieces")?;
        let pieces = p.string(TARGET_MAX_PIECES as usize * 2 * 20)?.to_vec();
        p.expect(b"e")?;
        if !p.rest.is_empty() {
            return None;
        }

        let info = Self {
            name,
            length,
            piece_length,
            pieces,
        };
        let valid = (1..=max_length).contains(&length)
            && piece_length == piece_length_for(length)
            && info.pieces.len() as u64 == piece_count(length, piece_length) * 20
            && info.format().is_some()
            && info.encode() == bytes;
        valid.then_some(info)
    }

    /// The audio format the file name claims.
    pub fn format(&self) -> Option<AudioFormat> {
        AudioFormat::ALL
            .into_iter()
            .find(|f| file_name(*f) == self.name)
    }
}

/// The info hash, as lowercase hex.
pub fn info_hash(info_bytes: &[u8]) -> String {
    hex::encode(Sha1::digest(info_bytes))
}

pub fn is_info_hash(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A complete `.torrent` file for an info dictionary. There are no trackers:
/// peers are found through the DHT and through the nodes that returned the
/// track in a search.
pub fn torrent_file(info_bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(info_bytes.len() + 8);
    out.extend_from_slice(b"d4:info");
    out.extend_from_slice(info_bytes);
    out.push(b'e');
    out
}

struct Parser<'a> {
    rest: &'a [u8],
}

impl<'a> Parser<'a> {
    fn expect(&mut self, literal: &[u8]) -> Option<()> {
        self.rest = self.rest.strip_prefix(literal)?;
        Some(())
    }

    /// A non-negative integer up to `e`, without leading zeros.
    fn integer(&mut self) -> Option<u64> {
        let end = self.rest.iter().position(|&b| b == b'e')?;
        let digits = &self.rest[..end];
        if digits.is_empty()
            || digits.len() > 19
            || !digits.iter().all(u8::is_ascii_digit)
            || (digits.len() > 1 && digits[0] == b'0')
        {
            return None;
        }
        let value = std::str::from_utf8(digits).ok()?.parse().ok()?;
        self.rest = &self.rest[end + 1..];
        Some(value)
    }

    /// A length-prefixed byte string of at most `max` bytes.
    fn string(&mut self, max: usize) -> Option<&'a [u8]> {
        let colon = self.rest.iter().take(8).position(|&b| b == b':')?;
        let digits = &self.rest[..colon];
        if digits.is_empty()
            || !digits.iter().all(u8::is_ascii_digit)
            || (digits.len() > 1 && digits[0] == b'0')
        {
            return None;
        }
        let len: usize = std::str::from_utf8(digits).ok()?.parse().ok()?;
        let body = self.rest.get(colon + 1..colon + 1 + len)?;
        if len > max {
            return None;
        }
        self.rest = &self.rest[colon + 1 + len..];
        Some(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> InfoDict {
        InfoDict {
            name: "track.flac".into(),
            length: 40_000,
            piece_length: 16 * 1024,
            pieces: vec![7; 3 * 20],
        }
    }

    #[test]
    fn round_trips() {
        let info = sample();
        let bytes = info.encode();
        assert!(bytes.starts_with(b"d6:lengthi40000e4:name10:track.flac12:piece lengthi16384e"));
        assert_eq!(InfoDict::parse(&bytes, u64::MAX), Some(info));
        assert_eq!(info_hash(&bytes).len(), 40);
    }

    #[test]
    fn rejects_anything_else() {
        let good = sample().encode();
        let parse = |b: &[u8]| InfoDict::parse(b, u64::MAX);

        let mut trailing = good.clone();
        trailing.push(b'x');
        assert_eq!(parse(&trailing), None);

        // A path in the name, or a name that isn't one of ours
        for name in ["../track.flac", "track.exe", "track.FLAC", "song.flac"] {
            let info = InfoDict {
                name: name.into(),
                ..sample()
            };
            assert_eq!(parse(&info.encode()), None, "{name}");
        }

        // Wrong number of piece hashes, odd piece length
        let short = InfoDict {
            pieces: vec![7; 40],
            ..sample()
        };
        assert_eq!(parse(&short.encode()), None);
        let odd = InfoDict {
            piece_length: 20_000,
            pieces: vec![7; 40],
            ..sample()
        };
        assert_eq!(parse(&odd.encode()), None);

        // Leading zeros and a multi-file torrent
        let zero = String::from_utf8_lossy(&good).replace("i40000e", "i040000e");
        assert_eq!(parse(zero.as_bytes()), None);
        assert_eq!(
            parse(b"d5:filesld6:lengthi1e4:pathl1:aeee4:name1:a12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaae"),
            None
        );

        // Too large for this node's limit
        assert_eq!(InfoDict::parse(&good, 1000), None);
    }

    #[test]
    fn piece_lengths() {
        assert_eq!(piece_length_for(1), MIN_PIECE_LENGTH);
        assert_eq!(piece_length_for(5 * 1024 * 1024), MIN_PIECE_LENGTH);
        assert_eq!(piece_length_for(40 * 1024 * 1024), 64 * 1024);
        assert_eq!(piece_length_for(512 * 1024 * 1024), 512 * 1024);
        assert_eq!(piece_length_for(u64::MAX / 2), MAX_PIECE_LENGTH);
    }
}
