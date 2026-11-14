// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rebuilds a FLAC file from STREAMINFO, an optional sanity-checked
//! SEEKTABLE, a fresh VORBIS_COMMENT block and the audio frames.
//!
//! Every other metadata block (PICTURE, APPLICATION, PADDING, CUESHEET,
//! reserved types) is dropped, as is a trailing ID3v1 tag.

use std::{fs::File, io::Write};

use super::{copy_range, malformed, read_at, Rebuilt, Rejection, Tags};

const STREAMINFO: u8 = 0;
const SEEKTABLE: u8 = 3;
const VORBIS_COMMENT: u8 = 4;
const INVALID: u8 = 127;

const STREAMINFO_LEN: usize = 34;
const SEEKPOINT_LEN: usize = 18;
const PLACEHOLDER: u64 = u64::MAX;

/// Upper bound on metadata blocks, to bound the time spent walking them
const MAX_BLOCKS: usize = 1024;

pub(crate) fn rebuild(
    input: &File,
    len: u64,
    output: &mut File,
    tags: &Tags,
) -> Result<Rebuilt, Rejection> {
    let mut pos = 4u64; // after "fLaC"
    let mut streaminfo: Option<Vec<u8>> = None;
    let mut seektable: Option<Vec<u8>> = None;

    for i in 0.. {
        if i == MAX_BLOCKS {
            return Err(malformed("too many FLAC metadata blocks"));
        }

        let header = read_at(input, pos, 4)?;
        let last = header[0] & 0x80 != 0;
        let kind = header[0] & 0x7F;
        let block_len = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        let body_pos = pos + 4;
        if body_pos + block_len as u64 > len {
            return Err(malformed(
                "a FLAC metadata block extends past the end of the file",
            ));
        }

        match kind {
            STREAMINFO if i == 0 => {
                if block_len != STREAMINFO_LEN {
                    return Err(malformed("invalid FLAC STREAMINFO block"));
                }
                streaminfo = Some(read_at(input, body_pos, block_len)?);
            }
            STREAMINFO => return Err(malformed("duplicate FLAC STREAMINFO block")),
            _ if i == 0 => return Err(malformed("FLAC STREAMINFO block is missing")),
            SEEKTABLE if seektable.is_none() => {
                let info = streaminfo.as_deref().unwrap();
                if let Some(table) = check_seektable(input, body_pos, block_len, len, info)? {
                    seektable = Some(table);
                }
            }
            INVALID => return Err(malformed("invalid FLAC metadata block type")),
            // Everything else is dropped
            _ => {}
        }

        pos = body_pos + block_len as u64;
        if last {
            break;
        }
    }

    let streaminfo = streaminfo.ok_or_else(|| malformed("FLAC STREAMINFO block is missing"))?;

    // The first frame must follow the metadata immediately
    let sync = read_at(input, pos, 2)?;
    if sync[0] != 0xFF || sync[1] & 0xFE != 0xF8 {
        return Err(malformed("no FLAC audio frame after the metadata"));
    }

    let mut audio_end = len;
    if len - pos > 128 && read_at(input, len - 128, 3)? == b"TAG" {
        audio_end -= 128;
    }

    let comment = vorbis_comment(tags);
    let mut head = Vec::new();
    head.extend_from_slice(b"fLaC");
    write_block(&mut head, STREAMINFO, false, &streaminfo);
    if let Some(ref table) = seektable {
        write_block(&mut head, SEEKTABLE, false, table);
    }
    write_block(&mut head, VORBIS_COMMENT, true, &comment);
    output.write_all(&head)?;

    copy_range(input, pos, audio_end - pos, output)?;

    Ok(Rebuilt {
        metadata_bytes: head.len() as u64,
        inert_bytes: 0,
    })
}

/// Keeps a seek table only if it is well formed and no larger than one
/// point per second of audio; otherwise it is silently dropped (seeking
/// still works without it).
fn check_seektable(
    input: &File,
    pos: u64,
    block_len: usize,
    file_len: u64,
    streaminfo: &[u8],
) -> Result<Option<Vec<u8>>, Rejection> {
    if block_len == 0 || block_len % SEEKPOINT_LEN != 0 {
        return Ok(None);
    }

    let sample_rate =
        (u32::from_be_bytes([0, streaminfo[10], streaminfo[11], streaminfo[12]]) >> 4) as u64;
    let total_samples = (u64::from(streaminfo[13] & 0x0F) << 32)
        | u64::from(u32::from_be_bytes([
            streaminfo[14],
            streaminfo[15],
            streaminfo[16],
            streaminfo[17],
        ]));
    let max_points = if sample_rate > 0 {
        total_samples / sample_rate + 16
    } else {
        16
    };
    let n_points = block_len / SEEKPOINT_LEN;
    if n_points as u64 > max_points {
        return Ok(None);
    }

    let table = read_at(input, pos, block_len)?;
    let mut prev: Option<(u64, u64)> = None;
    for point in table.chunks_exact(SEEKPOINT_LEN) {
        let sample = u64::from_be_bytes(point[0..8].try_into().unwrap());
        let offset = u64::from_be_bytes(point[8..16].try_into().unwrap());
        if sample == PLACEHOLDER {
            continue;
        }
        let ascending = prev.is_none_or(|(s, o)| sample > s && offset >= o);
        if !ascending || offset >= file_len || (total_samples > 0 && sample >= total_samples) {
            return Ok(None);
        }
        prev = Some((sample, offset));
    }

    Ok(Some(table))
}

fn write_block(out: &mut Vec<u8>, kind: u8, last: bool, body: &[u8]) {
    let len = (body.len() as u32).to_be_bytes();
    out.push(kind | if last { 0x80 } else { 0 });
    out.extend_from_slice(&len[1..4]);
    out.extend_from_slice(body);
}

fn vorbis_comment(tags: &Tags) -> Vec<u8> {
    fn push_str(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    let fields = tags.vorbis_fields();
    let mut out = Vec::new();
    push_str(&mut out, "anysik");
    out.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    for (key, value) in fields {
        push_str(&mut out, &format!("{key}={value}"));
    }
    out
}
