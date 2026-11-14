// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) struct Review {
    pub widget: gtk::Notebook,
    warnings: gtk::ListBox,
    covers: gtk::Box,
}
impl Review {
    pub fn new() -> Self {
        let widget = gtk::Notebook::new();
        widget.set_visible(false);
        let warnings = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        let covers = gtk::Box::new(gtk::Orientation::Vertical, 12);
        widget.append_page(
            &gtk::ScrolledWindow::builder()
                .child(&warnings)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .min_content_height(180)
                .max_content_height(340)
                .propagate_natural_height(true)
                .build(),
            Some(&gtk::Label::new(Some(&i18n("Duplicates")))),
        );
        widget.append_page(
            &gtk::ScrolledWindow::builder()
                .child(&covers)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .min_content_height(180)
                .max_content_height(340)
                .propagate_natural_height(true)
                .build(),
            Some(&gtk::Label::new(Some(&i18n("Choose Cover Art")))),
        );
        Self {
            widget,
            warnings,
            covers,
        }
    }
    pub fn duplicate(&self, track: &Track) {
        self.widget.set_visible(true);
        let row = adw::ActionRow::builder().title(&track.title).subtitle(i18n("Identical audio was found after downloading. The existing library track was reused."))
            .use_markup(false).build();
        row.add_prefix(&gtk::Image::from_icon_name("hopea-dialog-warning-symbolic"));
        self.warnings.append(&row);
    }
    pub async fn cover_choice(
        &self,
        track: &Track,
        incoming: &glib::Bytes,
        audio: &std::path::Path,
        parent: Option<&Window>,
    ) -> Result<bool, String> {
        let client = library::client().ok_or_else(|| i18n("Not connected"))?;
        let existing = if track.has_cover {
            Some(client.cover(track.id).await.map_err(|e| e.to_string())?)
        } else {
            None
        };
        if existing.as_ref().is_some_and(|bytes| {
            library::import_history::cover_hash(bytes)
                == library::import_history::cover_hash(incoming)
        }) {
            return Ok(false);
        }
        self.widget.set_visible(true);
        let row = gtk::Box::new(gtk::Orientation::Vertical, 8);
        row.append(
            &gtk::Label::builder()
                .label(&track.title)
                .xalign(0.0)
                .wrap(true)
                .build(),
        );
        let choices = crate::dialogs::actions();
        choices.set_max_children_per_line(2);
        for bytes in [existing.as_ref(), Some(incoming)] {
            let picture = gtk::Picture::builder()
                .width_request(160)
                .height_request(160)
                .can_shrink(true)
                .build();
            if let Some(bytes) = bytes {
                if let Ok(texture) = gdk::Texture::from_bytes(bytes) {
                    picture.set_paintable(Some(&texture));
                }
            }
            choices.append(&picture);
        }
        row.append(&choices);
        let buttons = crate::dialogs::actions();
        let keep = gtk::Button::with_label(&i18n("Keep Existing Cover"));
        let replace = gtk::Button::with_label(&i18n("Use New Cover"));
        let result = gtk::Label::builder().wrap(true).xalign(0.0).build();
        buttons.append(&keep);
        buttons.append(&replace);
        row.append(&buttons);
        row.append(&result);
        let weak = buttons.downgrade();
        let kept = result.clone();
        keep.connect_clicked(move |_| {
            if let Some(buttons) = weak.upgrade() {
                buttons.set_sensitive(false);
            }
            kept.set_text(&i18n("Existing cover kept"));
        });
        let id = track.id;
        let incoming = incoming.clone();
        let audio = audio.to_owned();
        let parent = parent.map(|w| w.downgrade());
        let weak = buttons.downgrade();
        replace.connect_clicked(move |_| {
            let Some(buttons) = weak.upgrade() else {
                return;
            };
            buttons.set_sensitive(false);
            let (client, bytes, audio, parent, result, buttons) = (
                client.clone(),
                incoming.clone(),
                audio.clone(),
                parent.clone(),
                result.clone(),
                buttons.clone(),
            );
            glib::spawn_future_local(async move {
                let changed: Result<(), String> = async {
                    let name = client
                        .add_cover(bytes.clone())
                        .await
                        .map_err(|e| e.to_string())?;
                    client
                        .update_cover(id, &name)
                        .await
                        .map_err(|e| e.to_string())?;
                    library::import_history::save_cover(&audio, &bytes).await?;
                    if let Some(uri) = library::shared_files::uri_for(id) {
                        if let Some(path) = gio::File::for_uri(&uri).path() {
                            library::import_history::save_cover(&path, &bytes).await?;
                        }
                    }
                    if let Some(window) = parent.and_then(|w| w.upgrade()) {
                        window.refresh_import_cover(id, &bytes);
                    }
                    Ok(())
                }
                .await;
                match changed {
                    Ok(()) => result.set_text(&i18n("Cover updated; audio was kept")),
                    Err(error) => {
                        result.set_text(&error);
                        buttons.set_sensitive(true);
                    }
                }
            });
        });
        self.covers.append(&row);
        Ok(true)
    }
}
