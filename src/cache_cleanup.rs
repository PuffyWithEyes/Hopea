// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Only regenerable cache categories. Never traverse the data directory or
//! follow links; original audio, imported artwork and playlists are excluded.
use crate::i18n::{i18n, i18n_f};
use adw::prelude::*;
use gtk::{gio, glib};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

#[derive(Clone, Copy)]
enum Category {
    Covers,
    Waveforms,
    Lyrics,
}
const CATEGORIES: [Category; 3] = [Category::Covers, Category::Waveforms, Category::Lyrics];
impl Category {
    fn folder(self) -> &'static str {
        match self {
            Self::Covers => "covers",
            Self::Waveforms => "waveforms",
            Self::Lyrics => "lyrics",
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Covers => "png",
            Self::Waveforms => "json",
            Self::Lyrics => "lrc",
        }
    }
    fn title(self) -> String {
        i18n(match self {
            Self::Covers => "Cover Images",
            Self::Waveforms => "Waveforms",
            Self::Lyrics => "Downloaded Lyrics",
        })
    }
}
#[derive(Default)]
struct ResultCount {
    bytes: u64,
    files: u64,
    failed: u64,
}
fn category_files(
    base: &Path,
    category: Category,
    keep: Option<&Path>,
    remove: bool,
) -> std::io::Result<ResultCount> {
    let mut result = ResultCount::default();
    let directory = base.join(category.folder());
    let meta = match std::fs::symlink_metadata(&directory) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Cache folder is not a regular directory",
        ));
    }
    for item in std::fs::read_dir(directory)? {
        let item = item?;
        let path = item.path();
        if Some(path.as_path()) == keep
            || path.extension().and_then(|s| s.to_str()) != Some(category.extension())
            || !item.file_type()?.is_file()
        {
            continue;
        }
        let size = item.metadata()?.len();
        if !remove || std::fs::remove_file(&path).is_ok() {
            result.bytes += size;
            result.files += 1;
        } else {
            result.failed += 1;
        }
    }
    Ok(result)
}

pub async fn show(parent: &impl IsA<gtk::Widget>, active_cover: Option<PathBuf>) {
    let dialog = adw::AlertDialog::builder()
        .heading(i18n("Clear Cache"))
        .body(i18n(
            "Choose what to clear. These files are recreated when needed.",
        ))
        .close_response("close")
        .build();
    dialog.add_response("close", &i18n("Close"));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let group = adw::PreferencesGroup::new();
    content.append(&group);
    let rows: Rc<Vec<_>> = Rc::new(
        CATEGORIES
            .iter()
            .map(|category| {
                let row = adw::SwitchRow::builder()
                    .title(category.title())
                    .subtitle(i18n("Calculating size…"))
                    .build();
                group.add(&row);
                row
            })
            .collect(),
    );
    let status = gtk::Label::builder().wrap(true).xalign(0.0).build();
    content.append(&status);
    dialog.set_extra_child(Some(&content));
    let clear = gtk::Button::with_label(&i18n("Clear Selected"));
    clear.add_css_class("destructive-action");
    clear.set_sensitive(false);
    content.append(&clear);
    for row in rows.iter() {
        row.connect_active_notify(glib::clone!(
            #[weak]
            clear,
            #[weak]
            rows,
            move |_| clear.set_sensitive(rows.iter().any(|r| r.is_active()))
        ));
    }
    let base = crate::paths::cache_dir();
    let measure_base = base.clone();
    let keep = active_cover.clone();
    let counts = gio::spawn_blocking(move || {
        CATEGORIES.map(|cat| category_files(&measure_base, cat, keep.as_deref(), false))
    })
    .await;
    if let Ok(counts) = counts {
        for (row, count) in rows.iter().zip(counts) {
            match count {
                Ok(count) => row.set_subtitle(&i18n_f(
                    "{} · {} files",
                    &[&glib::format_size(count.bytes), &count.files.to_string()],
                )),
                Err(_) => {
                    row.set_subtitle(&i18n("Unable to read this cache"));
                    row.set_sensitive(false);
                }
            }
        }
    }
    clear.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        #[strong]
        rows,
        #[weak]
        status,
        move |button| {
            let selected: Vec<_> = CATEGORIES
                .iter()
                .copied()
                .zip(rows.iter())
                .filter_map(|(cat, row)| row.is_active().then_some(cat))
                .collect();
            let base = base.clone();
            let keep = active_cover.clone();
            button.set_sensitive(false);
            for row in rows.iter() {
                row.set_sensitive(false);
            }
            dialog.set_can_close(false);
            status.set_text(&i18n("Clearing cache…"));
            glib::spawn_future_local(glib::clone!(
                #[strong]
                dialog,
                #[strong]
                rows,
                #[strong]
                status,
                #[weak]
                button,
                async move {
                    let result = gio::spawn_blocking(move || {
                        let mut sum = ResultCount::default();
                        for cat in selected {
                            match category_files(&base, cat, keep.as_deref(), true) {
                                Ok(c) => {
                                    sum.bytes += c.bytes;
                                    sum.files += c.files;
                                    sum.failed += c.failed;
                                }
                                Err(_) => sum.failed += 1,
                            }
                        }
                        let counts = CATEGORIES
                            .map(|cat| category_files(&base, cat, keep.as_deref(), false));
                        (sum, counts)
                    })
                    .await;
                    match result {
                        Ok((sum, counts)) => {
                            status.set_text(&if sum.failed == 0 {
                                i18n_f("Freed {}", &[&glib::format_size(sum.bytes)])
                            } else {
                                i18n_f(
                                    "Freed {}. Some files could not be removed.",
                                    &[&glib::format_size(sum.bytes)],
                                )
                            });
                            status.set_css_classes(&[if sum.failed == 0 {
                                "success"
                            } else {
                                "warning"
                            }]);
                            for (row, count) in rows.iter().zip(counts) {
                                row.set_active(false);
                                row.set_sensitive(count.is_ok());
                                if let Ok(c) = count {
                                    row.set_subtitle(&i18n_f(
                                        "{} · {} files",
                                        &[&glib::format_size(c.bytes), &c.files.to_string()],
                                    ));
                                }
                            }
                        }
                        Err(_) => {
                            status.set_text(&i18n("Unable to clear cache"));
                            status.add_css_class("error");
                            for row in rows.iter() {
                                row.set_sensitive(true);
                            }
                        }
                    }
                    button.set_sensitive(rows.iter().any(|r| r.is_active()));
                    dialog.set_can_close(true);
                }
            ));
        }
    ));
    AdwDialogExt::present(&dialog, Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_is_category_scoped_and_preserves_music_and_active_art() {
        let base = std::env::temp_dir().join(format!("hopea-cache-{}", glib::uuid_string_random()));
        for folder in ["covers", "waveforms", "lyrics", "playlists", "imports"] {
            std::fs::create_dir_all(base.join(folder)).unwrap();
        }
        for (file, bytes) in [
            ("covers/old.png", b"old".as_slice()),
            ("covers/active.png", b"active"),
            ("covers/song.flac", b"music"),
            ("waveforms/test.json", b"peaks"),
            ("lyrics/test.lrc", b"lyrics"),
            ("playlists/current.pls", b"playlist"),
            ("imports/cover.jpg", b"original"),
        ] {
            std::fs::write(base.join(file), bytes).unwrap();
        }
        let active = base.join("covers/active.png");
        let count = category_files(&base, Category::Covers, Some(&active), true).unwrap();
        assert_eq!(count.files, 1);
        assert_eq!(count.bytes, 3);
        for path in [
            "covers/active.png",
            "covers/song.flac",
            "waveforms/test.json",
            "lyrics/test.lrc",
            "playlists/current.pls",
            "imports/cover.jpg",
        ] {
            assert!(base.join(path).exists());
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                base.join("imports/cover.jpg"),
                base.join("covers/link.png"),
            )
            .unwrap();
            assert_eq!(
                category_files(&base, Category::Covers, Some(&active), true)
                    .unwrap()
                    .files,
                0
            );
            assert!(base.join("imports/cover.jpg").exists());
        }
        std::fs::remove_dir_all(base).unwrap();
    }
}
