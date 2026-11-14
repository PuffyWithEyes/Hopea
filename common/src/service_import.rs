// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Import catalogue protocol shared with the disposable Spotify worker.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Collection {
    pub key: String,
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub cover: Option<String>,
    /// Position within this collection, independent of library/like ordering.
    #[serde(default)]
    pub position: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CatalogTrack {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub cover: Option<String>,
    pub collections: Vec<Collection>,
}
