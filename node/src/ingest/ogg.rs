// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Replaces the Vorbis comment header of an Ogg Vorbis file.
//!
//! Writing a fresh `VorbisComments` tag rewrites the whole comment packet,
//! dropping every existing field, including embedded pictures
//! (`METADATA_BLOCK_PICTURE`). Lofty takes care of re-paging the stream.
//! Extra logical streams or junk pages are left for the decoder and the
//! byte accounting to catch.

use std::path::Path;

use lofty::{config::WriteOptions, ogg::tag::VorbisComments, tag::TagExt};

use super::{malformed, Rebuilt, Rejection, Tags};

/// Comment packet framing (vendor, field count, lengths, framing bit)
const COMMENT_OVERHEAD: u64 = 64;

pub(crate) fn retag(path: &Path, tags: &Tags) -> Result<Rebuilt, Rejection> {
    let mut comments = VorbisComments::new();
    comments.set_vendor("anysik".into());
    let mut written = COMMENT_OVERHEAD;
    for (key, value) in tags.vorbis_fields() {
        written += (key.len() + 1 + value.len() + 4) as u64;
        comments.push(key.to_string(), value);
    }

    comments
        .save_to_path(path, WriteOptions::new().preferred_padding(0))
        .map_err(|e| malformed(format!("unable to rewrite the Ogg comment header ({e})")))?;

    Ok(Rebuilt {
        metadata_bytes: written,
        inert_bytes: 0,
    })
}
