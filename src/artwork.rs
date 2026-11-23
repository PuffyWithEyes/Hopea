// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Global presentation for missing artwork. A fallback is never treated as
//! the song's actual cover or published as its artwork.
use crate::{audio::Song, i18n::i18n};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, io::Read, rc::Rc};

struct Placeholder {
    widget: glib::WeakRef<gtk::Image>,
    previous: Option<gdk::Paintable>,
}
thread_local! {
    static SETTINGS: gio::Settings = {
        let settings=crate::utils::settings_manager();
        for key in ["missing-cover", "missing-cover-image"] {
            settings.connect_changed(Some(key),|_,_| refresh());
        }
        settings
    };
    static TEXTURE: RefCell<Option<(String,Option<gdk::Texture>)>> = const {RefCell::new(None)};
    static SONGS: RefCell<Vec<glib::WeakRef<Song>>> = const {RefCell::new(Vec::new())};
    static IMAGES: RefCell<Vec<Placeholder>> = const {RefCell::new(Vec::new())};
}
pub fn texture() -> Option<gdk::Texture> {
    let (mode, key) = SETTINGS.with(|s| {
        (
            s.string("missing-cover").to_string(),
            s.string("missing-cover-image").to_string(),
        )
    });
    let cache_key = format!("{mode}:{key}");
    TEXTURE.with(|slot| {
        if let Some((previous, texture)) = slot.borrow().as_ref() {
            if *previous == cache_key {
                return texture.clone();
            }
        }
        let bytes = match mode.as_str() {
            "logo" => Some(glib::Bytes::from_static(include_bytes!(
                "../data/icons/cat-kz/256.png"
            ))),
            "custom" if key.len() == 64 && key.bytes().all(|c| c.is_ascii_hexdigit()) => {
                crate::utils::read_cover_file(
                    &crate::paths::data_dir()
                        .join("artwork")
                        .join(format!("{key}.jpg")),
                )
            }
            _ => None,
        };
        let texture = bytes.and_then(|b| {
            if mode == "logo" {
                // Embedded application asset: keep its original transparency.
                gdk::Texture::from_bytes(&b).ok()
            } else {
                crate::utils::load_cover_texture(&b).map(|p| gdk::Texture::for_pixbuf(&p))
            }
        });
        slot.replace(Some((cache_key, texture.clone())));
        texture
    })
}
pub fn watch_song(song: &Song) {
    SETTINGS.with(|_| {});
    SONGS.with(|songs| {
        let mut songs = songs.borrow_mut();
        if songs.len() % 100 == 0 {
            songs.retain(|s| s.upgrade().is_some());
        }
        songs.push(song.downgrade());
    });
}
pub fn placeholder(image: &gtk::Image) {
    let paintable = texture().map(|t| t.upcast::<gdk::Paintable>());
    if let Some(p) = &paintable {
        image.set_paintable(Some(p));
    } else {
        image.set_icon_name(Some("hopea-folder-music-symbolic"));
    }
    IMAGES.with(|images| {
        let mut images = images.borrow_mut();
        images.retain(|p| p.widget.upgrade().is_some());
        images.push(Placeholder {
            widget: image.downgrade(),
            previous: paintable,
        });
    });
}
fn refresh() {
    TEXTURE.with(|t| t.take());
    // Snapshot weak references before notifications: observers may construct
    // or recycle rows, registering additional songs/images in this callback.
    let songs = SONGS.with(|s| s.borrow().clone());
    for song in songs.into_iter().filter_map(|s| s.upgrade()) {
        if !song.has_cover_art() {
            song.notify("cover");
        }
    }
    let texture = texture().map(|t| t.upcast::<gdk::Paintable>());
    IMAGES.with(|images| {
        images.borrow_mut().retain_mut(|entry| {
            let Some(image) = entry.widget.upgrade() else {
                return false;
            };
            if image.paintable() != entry.previous {
                return false;
            }
            if let Some(p) = &texture {
                image.set_paintable(Some(p));
            } else {
                image.set_icon_name(Some("hopea-folder-music-symbolic"));
            }
            entry.previous = texture.clone();
            true
        })
    });
}
/// Read and normalize an image before GTK or the network sees it.
pub async fn choose(parent: Option<&gtk::Window>) -> Result<Option<glib::Bytes>, String> {
    let filter = gtk::FileFilter::new();
    filter.add_mime_type("image/png");
    filter.add_mime_type("image/jpeg");
    filter.set_name(Some(&i18n("PNG and JPEG images")));
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let chooser = gtk::FileDialog::builder()
        .title(i18n("Choose Cover Image"))
        .filters(&filters)
        .build();
    let file = match chooser.open_future(parent).await {
        Ok(f) => f,
        Err(e) if e.matches(gtk::DialogError::Dismissed) => return Ok(None),
        Err(_) => return Err(i18n("Unable to open the image chooser")),
    };
    let path = file
        .path()
        .ok_or_else(|| i18n("Choose an image on this device"))?;
    let bytes = gio::spawn_blocking(move || {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > 8 * 1024 * 1024 {
            return None;
        }
        hopea_node::ingest::reencode_cover(&bytes)
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(|| i18n("Choose a valid PNG or JPEG smaller than 8 MB"))?;
    Ok(Some(glib::Bytes::from_owned(bytes)))
}

pub struct Settings {
    pub group: adw::PreferencesGroup,
    mode: adw::ComboRow,
    popular: adw::SwitchRow,
    key: Rc<RefCell<String>>,
}
impl Settings {
    pub fn new(parent: &impl IsA<gtk::Widget>) -> Self {
        let settings = crate::utils::settings_manager();
        let names = [i18n("Music Note"), i18n("Hopea Logo"), i18n("Custom Image")];
        let mode = adw::ComboRow::builder()
            .title(i18n("Songs Without Cover Art"))
            .model(&gtk::StringList::new(
                &names.iter().map(String::as_str).collect::<Vec<_>>(),
            ))
            .selected(match settings.string("missing-cover").as_str() {
                "logo" => 1,
                "custom" => 2,
                _ => 0,
            })
            .build();
        let key = Rc::new(RefCell::new(
            settings.string("missing-cover-image").to_string(),
        ));
        let choose = adw::ButtonRow::builder()
            .title(i18n("Choose Custom Image…"))
            .start_icon_name("hopea-document-open-symbolic")
            .build();
        choose.set_visible(mode.selected() == 2);
        mode.connect_selected_notify(glib::clone!(
            #[weak]
            choose,
            move |mode| choose.set_visible(mode.selected() == 2)
        ));
        let parent = parent.as_ref().downgrade();
        choose.connect_activated(glib::clone!(
            #[strong]
            key,
            move |row| {
                let Some(parent) = parent.upgrade() else {
                    return;
                };
                glib::spawn_future_local(glib::clone!(
                    #[strong]
                    key,
                    #[strong]
                    row,
                    async move {
                        let window = parent.root().and_downcast::<gtk::Window>();
                        match self::choose(window.as_ref()).await {
                            Ok(Some(bytes)) => {
                                let hash = hex::encode(Sha256::digest(&bytes));
                                let saved_hash = hash.clone();
                                let result = gio::spawn_blocking(move || {
                                    let dir = crate::paths::data_dir().join("artwork");
                                    std::fs::create_dir_all(&dir)?;
                                    std::fs::write(
                                        dir.join(format!("{saved_hash}.jpg")),
                                        bytes.as_ref(),
                                    )
                                })
                                .await;
                                if matches!(result, Ok(Ok(()))) {
                                    key.replace(hash);
                                    row.set_title(&i18n("Custom Image Selected"));
                                } else {
                                    row.set_title(&i18n("Unable to Save Image — Try Again"));
                                }
                            }
                            Ok(None) => {}
                            Err(message) => {
                                let error = adw::AlertDialog::builder()
                                    .heading(i18n("Image Could Not Be Read"))
                                    .body(&message)
                                    .build();
                                error.add_response("close", &i18n("Close"));
                                AdwDialogExt::present(&error, Some(&row));
                            }
                        }
                    }
                ));
            }
        ));
        let group = adw::PreferencesGroup::builder()
            .title(i18n("Appearance"))
            .build();
        group.add(&mode);
        group.add(&choose);
        let popular = adw::SwitchRow::builder()
            .title(i18n("Choose the Most Shared Covers and Lyrics"))
            .subtitle(i18n("Use the variants shared by the most peers. When disabled, Hopea offers a choice after adding a song."))
            .active(settings.boolean("popular-metadata"))
            .build();
        popular.set_widget_name("popular-metadata");
        group.add(&popular);
        Self {
            group,
            mode,
            key,
            popular,
        }
    }
    pub fn apply(&self) -> Result<(), String> {
        let mode = match self.mode.selected() {
            1 => "logo",
            2 => "custom",
            _ => "note",
        };
        if mode == "custom" && self.key.borrow().is_empty() {
            return Err(i18n("Choose a custom image first"));
        }
        let settings = crate::utils::settings_manager();
        settings
            .set_boolean("popular-metadata", self.popular.is_active())
            .map_err(|_| i18n("Unable to save image preference"))?;
        settings
            .set_string("missing-cover-image", &self.key.borrow())
            .map_err(|_| i18n("Unable to save image preference"))?;
        settings
            .set_string("missing-cover", mode)
            .map_err(|_| i18n("Unable to save image preference"))?;
        Ok(())
    }
}
