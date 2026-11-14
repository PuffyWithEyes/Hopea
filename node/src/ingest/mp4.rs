// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checks the box structure of an MPEG-4 file and neutralizes metadata in
//! place.
//!
//! Removing boxes would shift the sample data and invalidate the chunk
//! offset tables, so metadata boxes (`udta`, `meta`, `uuid`, `free`, ...)
//! are instead renamed to `free` and their contents overwritten with zeros.
//!
//! Independently of the decoder, every track must be a sound track: an
//! `.mp4` with a video, subtitle or hint track is rejected.

use std::fs::File;

use crate::fsx::write_all_at;

use super::{malformed, read_at, Rebuilt, Rejection};

/// Upper bound on boxes visited, to bound the time spent walking them
const MAX_BOXES: usize = 4096;
const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    kind: [u8; 4],
    start: u64,
    header_len: u64,
    size: u64,
}

impl BoxHeader {
    fn body(&self) -> u64 {
        self.start + self.header_len
    }

    fn end(&self) -> u64 {
        self.start + self.size
    }
}

struct Walker<'a> {
    file: &'a File,
    n_boxes: usize,
    inert_bytes: u64,
    sound_tracks: usize,
}

pub(crate) fn sanitize(file: &File) -> Result<Rebuilt, Rejection> {
    let len = file.metadata()?.len();
    let mut walker = Walker {
        file,
        n_boxes: 0,
        inert_bytes: 0,
        sound_tracks: 0,
    };

    let top = walker.children(0, len, true)?;
    if top.first().map(|b| &b.kind) != Some(b"ftyp") {
        return Err(malformed(
            "the MPEG-4 file does not start with a file type box",
        ));
    }

    let mut n_moov = 0;
    let mut n_mdat = 0;
    for b in &top[1..] {
        match &b.kind {
            b"moov" => {
                n_moov += 1;
                walker.moov(b)?;
            }
            b"mdat" => n_mdat += 1,
            b"free" | b"skip" | b"wide" | b"uuid" => walker.neutralize(b)?,
            b"moof" | b"mfra" | b"sidx" | b"styp" => {
                return Err(Rejection::Unsupported(
                    "fragmented MPEG-4 files are not accepted".into(),
                ))
            }
            kind => return Err(unexpected_box(kind)),
        }
    }

    if n_moov != 1 || n_mdat == 0 {
        return Err(malformed(
            "the MPEG-4 file is missing its movie or media data",
        ));
    }
    if walker.sound_tracks != 1 {
        return Err(Rejection::Unsupported(
            "only MPEG-4 files with a single audio track are accepted".into(),
        ));
    }

    Ok(Rebuilt {
        metadata_bytes: 0,
        inert_bytes: walker.inert_bytes,
    })
}

impl Walker<'_> {
    fn children(
        &mut self,
        start: u64,
        end: u64,
        top_level: bool,
    ) -> Result<Vec<BoxHeader>, Rejection> {
        let mut boxes = Vec::new();
        let mut pos = start;
        while pos < end {
            self.n_boxes += 1;
            if self.n_boxes > MAX_BOXES {
                return Err(malformed("too many MPEG-4 boxes"));
            }
            let b = self.header(pos, end, top_level)?;
            pos = b.end();
            boxes.push(b);
        }
        Ok(boxes)
    }

    fn header(&self, pos: u64, end: u64, top_level: bool) -> Result<BoxHeader, Rejection> {
        if pos + 8 > end {
            return Err(malformed("truncated MPEG-4 box"));
        }
        let h = read_at(self.file, pos, 8)?;
        let size32 = u32::from_be_bytes(h[0..4].try_into().unwrap()) as u64;
        let kind: [u8; 4] = h[4..8].try_into().unwrap();

        let (header_len, size) = match size32 {
            // Extends to the end of the file; only valid for the last box
            0 if top_level => (8, end - pos),
            0 => return Err(malformed("invalid MPEG-4 box size")),
            1 => {
                if pos + 16 > end {
                    return Err(malformed("truncated MPEG-4 box"));
                }
                let large = read_at(self.file, pos + 8, 8)?;
                (16, u64::from_be_bytes(large.try_into().unwrap()))
            }
            n => (8, n),
        };

        if size < header_len || pos.checked_add(size).is_none_or(|e| e > end) {
            return Err(malformed("an MPEG-4 box extends past its parent"));
        }

        Ok(BoxHeader {
            kind,
            start: pos,
            header_len,
            size,
        })
    }

    fn moov(&mut self, moov: &BoxHeader) -> Result<(), Rejection> {
        for b in self.children(moov.body(), moov.end(), false)? {
            match &b.kind {
                b"mvhd" | b"iods" => {}
                b"trak" => self.trak(&b, 2)?,
                b"udta" | b"meta" | b"free" | b"skip" | b"uuid" => self.neutralize(&b)?,
                b"mvex" => {
                    return Err(Rejection::Unsupported(
                        "fragmented MPEG-4 files are not accepted".into(),
                    ))
                }
                kind => return Err(unexpected_box(kind)),
            }
        }
        Ok(())
    }

    fn trak(&mut self, trak: &BoxHeader, depth: usize) -> Result<(), Rejection> {
        let mut handler = None;
        for b in self.children(trak.body(), trak.end(), false)? {
            match &b.kind {
                b"tkhd" | b"edts" | b"tref" => {}
                b"mdia" => handler = self.mdia(&b, depth + 1)?,
                b"udta" | b"meta" | b"free" | b"skip" | b"uuid" => self.neutralize(&b)?,
                kind => return Err(unexpected_box(kind)),
            }
        }

        match handler {
            Some(h) if &h == b"soun" => {
                self.sound_tracks += 1;
                Ok(())
            }
            Some(_) => Err(Rejection::Unsupported(
                "the file contains a video or other non-audio track".into(),
            )),
            None => Err(malformed("an MPEG-4 track has no handler")),
        }
    }

    fn mdia(&mut self, mdia: &BoxHeader, depth: usize) -> Result<Option<[u8; 4]>, Rejection> {
        if depth > MAX_DEPTH {
            return Err(malformed("MPEG-4 boxes are nested too deeply"));
        }
        let mut handler = None;
        for b in self.children(mdia.body(), mdia.end(), false)? {
            match &b.kind {
                b"hdlr" => {
                    if handler.is_some() {
                        return Err(malformed("duplicate MPEG-4 handler"));
                    }
                    // version/flags (4), pre_defined (4), handler_type (4)
                    if b.size < b.header_len + 12 {
                        return Err(malformed("invalid MPEG-4 handler"));
                    }
                    let h = read_at(self.file, b.body() + 8, 4)?;
                    handler = Some(h.try_into().unwrap());
                }
                b"mdhd" | b"minf" | b"elng" => {}
                b"udta" | b"meta" | b"free" | b"skip" | b"uuid" => self.neutralize(&b)?,
                kind => return Err(unexpected_box(kind)),
            }
        }
        Ok(handler)
    }

    /// Turns a box into a zero-filled `free` box of the same size.
    fn neutralize(&mut self, b: &BoxHeader) -> Result<(), Rejection> {
        write_all_at(self.file, b"free", b.start + 4)?;
        let zeros = vec![0u8; 64 * 1024];
        let mut pos = b.body();
        while pos < b.end() {
            let n = (b.end() - pos).min(zeros.len() as u64) as usize;
            write_all_at(self.file, &zeros[..n], pos)?;
            pos += n as u64;
        }
        self.inert_bytes += b.size;
        Ok(())
    }
}

fn unexpected_box(kind: &[u8; 4]) -> Rejection {
    let printable: String = kind
        .iter()
        .map(|&c| {
            if c.is_ascii_alphanumeric() {
                c as char
            } else {
                '?'
            }
        })
        .collect();
    Rejection::Unsupported(format!("unexpected MPEG-4 box “{printable}”"))
}
