// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads tags from an upload and reduces them to a small set of sanitized
//! text fields.

use std::{
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};

use hopea_common::AudioFormat;
use lofty::{
    config::{ParseOptions, ParsingMode},
    picture::PictureType,
    prelude::{Accessor, ItemKey, TaggedFileExt},
    probe::Probe,
    tag::Tag,
};

/// Longest text value kept, in characters
const MAX_FIELD_CHARS: usize = 200;
/// Largest embedded picture considered for cover art
const MAX_PICTURE_BYTES: usize = 16 * 1024 * 1024;

/// A per-item allocation limit alone does not bound a file containing many
/// small tags. Count actual reads (including rereads after seeks), not sizes
/// declared in the container. Do not scan arbitrary audio for optional tags.
struct TagReader {
    file: File,
    remaining: usize,
    seeks: usize,
    deadline: Instant,
}
impl Read for TagReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 || Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "metadata read budget exceeded",
            ));
        }
        let count = buf.len().min(self.remaining);
        let read = self.file.read(&mut buf[..count])?;
        self.remaining -= read;
        Ok(read)
    }
}
impl Seek for TagReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.seeks == 0 || Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "metadata seek budget exceeded",
            ));
        }
        self.seeks -= 1;
        self.file.seek(position)
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
}

impl Tags {
    fn texts(&self) -> [(&'static str, &'static str, Option<String>); 7] {
        [
            ("TITLE", "TIT2", self.title.clone()),
            ("ARTIST", "TPE1", self.artist.clone()),
            ("ALBUM", "TALB", self.album.clone()),
            ("ALBUMARTIST", "TPE2", self.album_artist.clone()),
            (
                "TRACKNUMBER",
                "TRCK",
                self.track_number.map(|n| n.to_string()),
            ),
            ("DATE", "TDRC", self.year.map(|y| y.to_string())),
            ("GENRE", "TCON", self.genre.clone()),
        ]
    }

    pub(crate) fn vorbis_fields(&self) -> Vec<(&'static str, String)> {
        self.texts()
            .into_iter()
            .filter_map(|(key, _, value)| Some((key, value?)))
            .collect()
    }

    pub(crate) fn id3_frames(&self) -> Vec<(&'static str, String)> {
        self.texts()
            .into_iter()
            .filter_map(|(_, id, value)| Some((id, value?)))
            .collect()
    }
}

/// Reads tags and the raw front cover from `path`. Unreadable tags are not
/// an error: the decoder decides whether the file is acceptable.
pub(crate) fn read(path: &Path, format: AudioFormat) -> (Tags, Option<Vec<u8>>) {
    let options = ParseOptions::new()
        .read_properties(false)
        .read_cover_art(true)
        .parsing_mode(ParsingMode::BestAttempt);

    let tagged = File::open(path)
        .map(|file| {
            Probe::new(BufReader::new(TagReader {
                file,
                remaining: 32 * 1024 * 1024,
                seeks: 65536,
                deadline: Instant::now() + Duration::from_secs(8),
            }))
            .options(options)
        })
        .ok()
        .and_then(|p| p.guess_file_type().ok())
        .and_then(|p| p.read().ok());
    let Some(tagged) = tagged else {
        tracing::debug!("No readable tags in {format:?} upload");
        return (Tags::default(), None);
    };

    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return (Tags::default(), None);
    };

    let tags = Tags {
        title: tag.title().and_then(|s| sanitize(&s)),
        artist: tag.artist().and_then(|s| sanitize(&s)),
        album: tag.album().and_then(|s| sanitize(&s)),
        album_artist: tag.get_string(ItemKey::AlbumArtist).and_then(sanitize),
        track_number: tag.track().filter(|n| (1..=9999).contains(n)),
        year: tag
            .date()
            .map(|d| i32::from(d.year))
            .filter(|y| (1000..=9999).contains(y)),
        genre: tag.genre().and_then(|s| sanitize(&s)),
    };

    (tags, front_cover(tag))
}

fn front_cover(tag: &Tag) -> Option<Vec<u8>> {
    let pictures = tag.pictures();
    let picture = pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| {
            pictures
                .iter()
                .find(|p| matches!(p.pic_type(), PictureType::Other | PictureType::Undefined(_)))
        })?;
    (picture.data().len() <= MAX_PICTURE_BYTES).then(|| picture.data().to_vec())
}

/// Characters that are never kept in text: control characters, and the
/// bidirectional overrides that can make a title render differently from
/// what it contains.
fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')
        || c == '\u{FEFF}'
}

/// Removes unsafe characters, collapses whitespace and limits the length.
pub(crate) fn sanitize(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut count = 0;
    let mut space = false;
    for c in s.chars() {
        if count == MAX_FIELD_CHARS {
            break;
        }
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if is_unsafe_char(c) {
            continue;
        }
        if space {
            out.push(' ');
            count += 1;
            space = false;
            if count == MAX_FIELD_CHARS {
                break;
            }
        }
        out.push(c);
        count += 1;
    }
    (!out.is_empty()).then_some(out)
}

/// Like [`sanitize`], for short values echoed back in error messages.
pub(crate) fn sanitize_short(s: &str, max_chars: usize) -> String {
    sanitize(s)
        .map(|s| s.chars().take(max_chars).collect())
        .unwrap_or_default()
}

/// Turns an uploaded file's name (without extension) into a title.
pub(crate) fn fallback_title(stem: &str) -> String {
    let base = stem.rsplit(['/', '\\']).next().unwrap_or(stem);
    sanitize(&base.replace('_', " ")).unwrap_or_else(|| "Untitled".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizing() {
        assert_eq!(sanitize("  Hello\n\tWorld \0"), Some("Hello World".into()));
        assert_eq!(sanitize("evil\u{202E}3pm.exe"), Some("evil3pm.exe".into()));
        assert_eq!(sanitize("\u{1}\u{2}"), None);
        assert_eq!(sanitize(&"a".repeat(500)).unwrap().len(), MAX_FIELD_CHARS);
    }

    #[test]
    fn fallback_titles() {
        assert_eq!(fallback_title("../../etc/My_Song"), "My Song");
        assert_eq!(fallback_title(""), "Untitled");
    }

    #[test]
    fn seeking_cannot_reset_the_metadata_read_budget() {
        use std::io::Write;
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"12345678").unwrap();
        file.rewind().unwrap();
        let mut reader = TagReader {
            file,
            remaining: 8,
            seeks: 2,
            deadline: Instant::now() + Duration::from_secs(1),
        };
        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf).unwrap();
        reader.rewind().unwrap();
        reader.read_exact(&mut buf).unwrap();
        reader.rewind().unwrap();
        assert!(reader.read(&mut buf).is_err());
        assert!(reader.rewind().is_err());
    }
}
