// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Listener edits belong to durable data, never to the disposable lyrics or
//! artwork cache. Audio files and their content hashes remain unchanged.
use crate::audio::Song;
use gtk::{gio, glib, prelude::*};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Read, path::PathBuf};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Edits {
    pub cover: Option<String>,
    pub lyrics: Option<String>,
}

pub fn path(song: &Song) -> PathBuf {
    let key = if song.remote_id().is_some() {
        song.uuid().unwrap_or_else(|| song.uri())
    } else {
        song.uri()
    };
    crate::paths::data_dir().join("song-edits").join(format!(
        "{}.json",
        hex::encode(Sha256::digest(key.as_bytes()))
    ))
}

pub fn read(path: &std::path::Path) -> Result<Edits, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Edits::default()),
        Err(_) => return Err("Unable to read saved song edits".into()),
    };
    let mut bytes = Vec::new();
    file.take(6 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Unable to read saved song edits")?;
    if bytes.len() > 6 * 1024 * 1024 {
        return Err("Saved song edits are too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Saved song edits could not be read".into())
}

pub async fn save(path: PathBuf, edits: Edits) -> Result<(), String> {
    gio::spawn_blocking(move || {
        if let Some(text) = &edits.lyrics {
            hopea_common::lyrics::validate(text, None).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec(&edits).map_err(|_| "Unable to encode song edits")?;
        if bytes.len() > 6 * 1024 * 1024 {
            return Err("Song edits are too large".into());
        }
        std::fs::create_dir_all(path.parent().unwrap())
            .map_err(|_| "Unable to create song edits folder")?;
        gio::File::for_path(path)
            .replace_contents(
                &bytes,
                None,
                false,
                gio::FileCreateFlags::PRIVATE,
                gio::Cancellable::NONE,
            )
            .map(|_| ())
            .map_err(|_| "Unable to save song edits".into())
    })
    .await
    .map_err(|_| "Saving song edits was interrupted")?
}

pub fn apply_cover(song: &Song) {
    if let Ok(Edits {
        cover: Some(encoded),
        ..
    }) = read(&path(song))
    {
        song.set_cover_bytes(&glib::Bytes::from_owned(glib::base64_decode(&encoded)));
    }
}
