// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which files on this computer are songs it shares, by the node's track id.
//!
//! The node keeps its own copy of what is shared, but the playlist holds the
//! listener's original files. Knowing which original a shared track came from
//! lets the playlist show it as in the library, and lets the original be
//! moved or put in the trash when it leaves the library.

use std::{cell::RefCell, collections::HashMap, path::PathBuf};

use log::warn;

thread_local! {
    static FILES: RefCell<Option<HashMap<String, i64>>> = const { RefCell::new(None) };
}

fn path() -> PathBuf {
    crate::paths::data_dir().join("shared-files.json")
}

fn with<R>(f: impl FnOnce(&mut HashMap<String, i64>) -> R) -> R {
    FILES.with(|files| {
        let mut files = files.borrow_mut();
        let files = files.get_or_insert_with(|| {
            std::fs::read(path())
                .ok()
                .and_then(|data| serde_json::from_slice(&data).ok())
                .unwrap_or_default()
        });
        f(files)
    })
}

fn save(files: &HashMap<String, i64>) {
    let path = path();
    let result = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(files)?)?;
        std::fs::rename(&tmp, &path)
    })();
    if let Err(err) = result {
        warn!("Unable to remember which files are shared: {err}");
    }
}

/// The track a file was shared as, if it was.
pub fn id_for(uri: &str) -> Option<i64> {
    with(|files| files.get(uri).copied())
}

/// The file a track was shared from, if it came from this computer.
pub fn uri_for(id: i64) -> Option<String> {
    with(|files| {
        files
            .iter()
            .find(|(_, &track)| track == id)
            .map(|(uri, _)| uri.clone())
    })
}

pub fn remember(uri: &str, id: i64) {
    with(|files| {
        if files.insert(uri.to_string(), id) != Some(id) {
            save(files);
        }
    });
}

pub fn forget(id: i64) {
    with(|files| {
        let before = files.len();
        files.retain(|_, track| *track != id);
        if files.len() != before {
            save(files);
        }
    });
}
