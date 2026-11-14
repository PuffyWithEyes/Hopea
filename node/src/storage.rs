// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! On-disk layout of the data directory.
//!
//! ```text
//! <data>/catalog.db                  tracks this node knows about
//! <data>/identity.key                signing key (owner only)
//! <data>/media/<info hash>/track.flac shared and downloaded audio
//! <data>/covers/<info hash>.jpg      re-encoded cover art of own tracks
//! <data>/session/                    BitTorrent DHT state
//! <data>/tmp/                        uploads in progress
//! ```
//!
//! Stored names are derived only from info hashes and the verified format,
//! never from anything another person sent.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use hopea_common::AudioFormat;

use crate::{
    fsx,
    torrent_info::{self, is_info_hash},
};

#[derive(Clone, Debug)]
pub struct Storage {
    root: PathBuf,
}

fn create_dir(dir: &Path) -> io::Result<()> {
    fsx::create_private_dir(dir)
}

impl Storage {
    /// Creates the directory layout (owner-only permissions) and removes
    /// leftovers of interrupted uploads.
    pub fn init(root: &Path) -> io::Result<Self> {
        let storage = Self {
            root: root.to_path_buf(),
        };
        for dir in [
            root,
            &storage.media_root(),
            &storage.covers_dir(),
            &storage.session_dir(),
            &storage.tmp_dir(),
        ] {
            create_dir(dir)?;
        }
        for entry in fs::read_dir(storage.tmp_dir())? {
            let _ = fs::remove_file(entry?.path());
        }
        Ok(storage)
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("catalog.db")
    }

    pub fn identity_path(&self) -> PathBuf {
        self.root.join("identity.key")
    }

    fn media_root(&self) -> PathBuf {
        self.root.join("media")
    }

    fn covers_dir(&self) -> PathBuf {
        self.root.join("covers")
    }

    pub fn session_dir(&self) -> PathBuf {
        self.root.join("session")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// The folder a torrent is downloaded into and shared from.
    pub fn media_dir(&self, info_hash: &str) -> PathBuf {
        assert!(is_info_hash(info_hash));
        self.media_root().join(info_hash)
    }

    pub fn media_path(&self, info_hash: &str, format: AudioFormat) -> PathBuf {
        self.media_dir(info_hash)
            .join(torrent_info::file_name(format))
    }

    pub fn cover_path(&self, info_hash: &str) -> PathBuf {
        assert!(is_info_hash(info_hash));
        self.covers_dir().join(format!("{info_hash}.jpg"))
    }

    /// Moves a sanitized upload into place.
    pub fn store_track(
        &self,
        file: tempfile::TempPath,
        info_hash: &str,
        format: AudioFormat,
    ) -> io::Result<PathBuf> {
        let dest = self.media_path(info_hash, format);
        create_dir(dest.parent().unwrap())?;
        file.persist(&dest).map_err(|e| e.error)?;
        fsx::make_private(&dest)?;
        Ok(dest)
    }

    pub fn store_cover(&self, info_hash: &str, jpeg: &[u8]) -> io::Result<()> {
        let mut tmp = tempfile::NamedTempFile::new_in(self.tmp_dir())?;
        io::Write::write_all(&mut tmp, jpeg)?;
        tmp.persist(self.cover_path(info_hash))
            .map_err(|e| e.error)?;
        Ok(())
    }

    pub fn lyrics_path(&self, info_hash: &str) -> PathBuf {
        assert!(is_info_hash(info_hash));
        self.root.join("lyrics").join(format!("{info_hash}.json"))
    }

    /// Deletes a track's audio and cover, if present.
    pub fn remove(&self, info_hash: &str) {
        let _ = fs::remove_dir_all(self.media_dir(info_hash));
        let _ = fs::remove_file(self.cover_path(info_hash));
        let _ = fs::remove_file(self.lyrics_path(info_hash));
    }
}
