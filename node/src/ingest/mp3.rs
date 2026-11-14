// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Strips every tag from an MP3 file (leading ID3v2, trailing ID3v1,
//! APEv2, Lyrics3v2 and appended ID3v2) and writes a fresh ID3v2.4 tag
//! containing only sanitized text frames.

use std::{fs::File, io::Write};

use hopea_common::AudioFormat;

use super::{copy_range, malformed, read_at, Rebuilt, Rejection, Tags};

/// Upper bound on stacked tags at either end of the file
const MAX_TAGS: usize = 8;

pub(crate) fn rebuild(
    input: &File,
    len: u64,
    output: &mut File,
    tags: &Tags,
) -> Result<Rebuilt, Rejection> {
    let mut start = 0u64;
    for _ in 0..MAX_TAGS {
        if start + 10 > len {
            break;
        }
        let header = read_at(input, start, 10)?;
        if &header[0..3] != b"ID3" {
            break;
        }
        start += 10 + id3v2_body_len(&header)?;
    }

    let mut end = len;
    for _ in 0..MAX_TAGS {
        let trimmed = trailing_tag_len(input, start, end)?;
        if trimmed == 0 {
            break;
        }
        end -= trimmed;
    }

    if start + 4 > end {
        return Err(malformed("the MP3 file contains no audio frames"));
    }
    let frame = read_at(input, start, 4)?;
    if AudioFormat::sniff(&frame) != Some(AudioFormat::Mp3) {
        return Err(malformed("no MP3 audio frame after the ID3 tag"));
    }

    let tag = id3v24_tag(tags);
    output.write_all(&tag)?;
    copy_range(input, start, end - start, output)?;

    Ok(Rebuilt {
        metadata_bytes: tag.len() as u64,
        inert_bytes: 0,
    })
}

/// Length of an ID3v2 tag after its 10 byte header, including the footer.
fn id3v2_body_len(header: &[u8]) -> Result<u64, Rejection> {
    if !(2..=4).contains(&header[3]) {
        return Err(malformed("unsupported ID3v2 version"));
    }
    let size = syncsafe(&header[6..10]).ok_or_else(|| malformed("invalid ID3v2 tag size"))?;
    let has_footer = header[5] & 0x10 != 0;
    Ok(size + if has_footer { 10 } else { 0 })
}

/// Size of a tag ending at `end`, or 0 if there is none.
fn trailing_tag_len(input: &File, start: u64, end: u64) -> Result<u64, Rejection> {
    let avail = end - start;

    if avail >= 128 && read_at(input, end - 128, 3)? == b"TAG" {
        return Ok(128);
    }

    if avail >= 32 {
        let footer = read_at(input, end - 32, 32)?;
        if &footer[0..8] == b"APETAGEX" {
            let size = u32::from_le_bytes(footer[12..16].try_into().unwrap()) as u64;
            let flags = u32::from_le_bytes(footer[20..24].try_into().unwrap());
            let has_header = flags & 0x8000_0000 != 0;
            let total = size + if has_header { 32 } else { 0 };
            if total < 32 || total > avail {
                return Err(malformed("invalid APE tag"));
            }
            return Ok(total);
        }
    }

    if avail >= 15 {
        let footer = read_at(input, end - 15, 15)?;
        if &footer[6..15] == b"LYRICS200" {
            let size = std::str::from_utf8(&footer[0..6])
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(|| malformed("invalid Lyrics3 tag"))?;
            let total = size + 15;
            if total > avail {
                return Err(malformed("invalid Lyrics3 tag"));
            }
            return Ok(total);
        }
    }

    // ID3v2.4 tag appended at the end, identified by its footer
    if avail >= 20 {
        let footer = read_at(input, end - 10, 10)?;
        if &footer[0..3] == b"3DI" {
            let size = syncsafe(&footer[6..10]).ok_or_else(|| malformed("invalid ID3v2 footer"))?;
            let total = size + 20;
            if total > avail {
                return Err(malformed("invalid ID3v2 footer"));
            }
            return Ok(total);
        }
    }

    Ok(0)
}

fn syncsafe(bytes: &[u8]) -> Option<u64> {
    bytes.iter().try_fold(0u64, |acc, &b| {
        (b & 0x80 == 0).then_some((acc << 7) | u64::from(b))
    })
}

fn to_syncsafe(n: u32) -> [u8; 4] {
    [
        ((n >> 21) & 0x7F) as u8,
        ((n >> 14) & 0x7F) as u8,
        ((n >> 7) & 0x7F) as u8,
        (n & 0x7F) as u8,
    ]
}

fn id3v24_tag(tags: &Tags) -> Vec<u8> {
    let mut frames = Vec::new();
    for (id, value) in tags.id3_frames() {
        // Text encoding 3 (UTF-8); UTF-8 never contains 0xFF, so no
        // unsynchronisation is needed
        let size = 1 + value.len() as u32;
        frames.extend_from_slice(id.as_bytes());
        frames.extend_from_slice(&to_syncsafe(size));
        frames.extend_from_slice(&[0, 0]);
        frames.push(3);
        frames.extend_from_slice(value.as_bytes());
    }

    let mut tag = Vec::with_capacity(10 + frames.len());
    tag.extend_from_slice(b"ID3\x04\x00\x00");
    tag.extend_from_slice(&to_syncsafe(frames.len() as u32));
    tag.extend_from_slice(&frames);
    tag
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syncsafe_roundtrip() {
        assert_eq!(syncsafe(&to_syncsafe(0x0FFF_FFFF)), Some(0x0FFF_FFFF));
        assert_eq!(syncsafe(&[0x00, 0x00, 0x02, 0x01]), Some(257));
        assert_eq!(syncsafe(&[0x80, 0, 0, 0]), None);
    }
}
