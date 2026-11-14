// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Preserve existing libraries and playlists across the product rename.
use gtk::glib;
use std::path::{Path, PathBuf};

fn compatible_dir(base: &Path) -> PathBuf {
    let legacy = base.join("amberol");
    if legacy.is_dir() {
        legacy
    } else {
        base.join("hopea")
    }
}

pub fn data_dir() -> PathBuf {
    compatible_dir(&glib::user_data_dir())
}

pub fn cache_dir() -> PathBuf {
    compatible_dir(&glib::user_cache_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_existing_library_in_place() {
        let base = std::env::temp_dir().join(format!("hopea-paths-{}", std::process::id()));
        std::fs::create_dir_all(base.join("amberol")).unwrap();
        assert_eq!(compatible_dir(&base), base.join("amberol"));
        assert_eq!(
            compatible_dir(&base.join("fresh")),
            base.join("fresh/hopea")
        );
        std::fs::remove_dir(base.join("amberol")).unwrap();
        std::fs::remove_dir(base).unwrap();
    }
}
