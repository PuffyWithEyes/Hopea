// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rebuilds a RIFF WAVE file from just its `fmt ` and `data` chunks.
//!
//! LIST/INFO, `id3 `, `bext`, JUNK and any other chunk, as well as anything
//! after the RIFF chunk, is dropped. WAV files carry no tags afterwards; the
//! library keeps them in its database.

use std::{fs::File, io::Write};

use super::{copy_range, malformed, read_at, Rebuilt, Rejection};

const MAX_CHUNKS: usize = 1024;

const FORMAT_PCM: u16 = 0x0001;
const FORMAT_FLOAT: u16 = 0x0003;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

pub(crate) fn rebuild(input: &File, len: u64, output: &mut File) -> Result<Rebuilt, Rejection> {
    let header = read_at(input, 0, 12)?;
    let riff_len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as u64;
    let end = riff_len + 8;
    if end > len {
        return Err(malformed("the WAV file is truncated"));
    }

    let mut fmt: Option<Vec<u8>> = None;
    let mut data: Option<(u64, u64)> = None;
    let mut pos = 12u64;
    let mut n_chunks = 0;

    while pos + 8 <= end {
        n_chunks += 1;
        if n_chunks > MAX_CHUNKS {
            return Err(malformed("too many WAV chunks"));
        }

        let chunk = read_at(input, pos, 8)?;
        let id = &chunk[0..4];
        let size = u32::from_le_bytes(chunk[4..8].try_into().unwrap()) as u64;
        let body = pos + 8;
        if body + size > end {
            return Err(malformed("a WAV chunk extends past the end of the file"));
        }

        match id {
            b"fmt " => {
                if fmt.is_some() {
                    return Err(malformed("duplicate WAV format chunk"));
                }
                if ![16, 18, 40].contains(&size) {
                    return Err(malformed("invalid WAV format chunk"));
                }
                let body = read_at(input, body, size as usize)?;
                let tag = u16::from_le_bytes([body[0], body[1]]);
                if ![FORMAT_PCM, FORMAT_FLOAT, FORMAT_EXTENSIBLE].contains(&tag) {
                    return Err(Rejection::Unsupported(
                        "only PCM and floating point WAV files are accepted".into(),
                    ));
                }
                fmt = Some(body);
            }
            b"data" => {
                if data.is_some() {
                    return Err(malformed("duplicate WAV data chunk"));
                }
                data = Some((body, size));
            }
            // Everything else is dropped
            _ => {}
        }

        // Chunks are padded to an even size
        pos = body + size + (size & 1);
    }

    let fmt = fmt.ok_or_else(|| malformed("the WAV format chunk is missing"))?;
    let (data_pos, data_len) = data.ok_or_else(|| malformed("the WAV data chunk is missing"))?;
    let pad = data_len & 1;

    let riff_size = 4 + (8 + fmt.len() as u64) + (8 + data_len + pad);
    let riff_size = u32::try_from(riff_size).map_err(|_| malformed("the WAV file is too large"))?;

    let mut head = Vec::with_capacity(44);
    head.extend_from_slice(b"RIFF");
    head.extend_from_slice(&riff_size.to_le_bytes());
    head.extend_from_slice(b"WAVE");
    head.extend_from_slice(b"fmt ");
    head.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    head.extend_from_slice(&fmt);
    head.extend_from_slice(b"data");
    head.extend_from_slice(&(data_len as u32).to_le_bytes());
    output.write_all(&head)?;

    copy_range(input, data_pos, data_len, output)?;
    if pad == 1 {
        output.write_all(&[0])?;
    }

    Ok(Rebuilt {
        metadata_bytes: head.len() as u64 + pad,
        inert_bytes: 0,
    })
}
