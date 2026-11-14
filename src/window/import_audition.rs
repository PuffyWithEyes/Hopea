// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(super) struct State {
    pub(super) lyrics: Option<String>,
    playback: crate::audio::PreviewCheckpoint,
    parents: Vec<crate::dialogs::Suspended>,
    playlist: bool,
    library: bool,
    main_page: String,
    player_page: String,
    cover_page: String,
    return_to_picker: Box<dyn Fn(bool)>,
}

impl Window {
    #[cfg(test)]
    pub async fn test_import_audition_layout(&self) {
        let imp = self.imp();
        for _ in 0..100 {
            if imp.audition_bar.is_mapped() {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(25)).await;
        }
        assert!(self.is_import_audition());
        assert!(!imp.split_view.shows_sidebar());
        assert!(!imp.library_split.shows_sidebar());
        assert!(!imp.library_sheet.is_open());
        assert!(!imp.player_header.is_visible());
        assert!(!imp.mode_switch_narrow.is_visible());
        assert_eq!(
            imp.player_stack.visible_child_name().as_deref(),
            Some("player")
        );
        assert!(imp.audition_bar.is_mapped());
        if let Some(path) = std::env::var_os("HOPEA_TEST_AUDITION_SCREENSHOT") {
            self.queue_draw();
            glib::timeout_future(std::time::Duration::from_secs(1)).await;
            let canvas = imp.main_stack.get();
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gdk::RGBA::parse("#242424").unwrap(),
                &gtk::graphene::Rect::new(0.0, 0.0, canvas.width() as f32, canvas.height() as f32),
            );
            let page = canvas.visible_child().unwrap();
            assert!(page.is_mapped() && page.width() > 0);
            canvas.snapshot_child(&page, &snapshot);
            self.renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
    }

    pub fn is_import_audition(&self) -> bool {
        self.imp().audition.borrow().is_some()
    }

    pub fn start_import_audition(&self, song: Song, return_to_picker: Box<dyn Fn(bool)>) {
        let Some(player) = self.player() else {
            return;
        };
        let imp = self.imp();
        if !self.is_import_audition() {
            imp.audition.replace(Some(State {
                lyrics: None,
                playback: player.preview_checkpoint(),
                parents: crate::dialogs::suspend_forms(self),
                playlist: self.playlist_visible(),
                library: imp.library_split.shows_sidebar() || imp.library_sheet.is_open(),
                main_page: imp
                    .main_stack
                    .visible_child_name()
                    .unwrap_or_default()
                    .to_string(),
                player_page: imp
                    .player_stack
                    .visible_child_name()
                    .unwrap_or_default()
                    .to_string(),
                cover_page: imp
                    .cover_stack
                    .visible_child_name()
                    .unwrap_or_default()
                    .to_string(),
                return_to_picker,
            }));
        }
        // A one-song preview sequence keeps Next/EOS from entering the queue.
        player.preview_album(vec![song], 0);
        imp.main_stack.set_visible_child_name("main-view");
        imp.player_stack.set_visible_child_name("player");
        imp.cover_stack.set_visible_child_name("cover");
        self.enforce_import_audition();
        GtkWindowExt::present(self);
    }

    pub(super) fn enforce_import_audition(&self) {
        if !self.is_import_audition() {
            return;
        }
        let imp = self.imp();
        self.set_playlist_visible(false);
        imp.library_split.set_show_sidebar(false);
        imp.library_sheet.set_open(false);
        imp.library_sheet.set_property("reveal-bottom-bar", false);
        imp.player_header.set_visible(false);
        imp.mode_switch_narrow.set_visible(false);
        imp.audition_bar.set_visible(true);
        imp.playback_control.set_audition(true);
    }

    pub fn finish_import_audition(&self, resume_picker: bool) {
        let Some(saved) = self.imp().audition.take() else {
            return;
        };
        let imp = self.imp();
        if let Some(player) = self.player() {
            player.restore_preview(saved.playback);
        }
        imp.audition_bar.set_visible(false);
        imp.player_header.set_visible(true);
        imp.playback_control.set_audition(false);
        let narrow = self.library_in_sheet();
        imp.mode_switch_narrow.set_visible(narrow);
        imp.library_sheet.set_property("reveal-bottom-bar", narrow);
        self.set_playlist_visible(saved.playlist);
        if narrow {
            imp.library_sheet.set_open(saved.library);
        } else {
            imp.library_split.set_show_sidebar(saved.library);
        }
        imp.main_stack.set_visible_child_name(&saved.main_page);
        imp.player_stack.set_visible_child_name(&saved.player_page);
        imp.cover_stack.set_visible_child_name(&saved.cover_page);
        let parents = saved.parents;
        let return_to_picker = saved.return_to_picker;
        if resume_picker {
            glib::spawn_future_local(async move {
                for parent in parents {
                    parent.resume().await;
                }
                (return_to_picker)(true);
            });
        } else {
            for parent in parents {
                parent.cancel();
            }
            (return_to_picker)(false);
        }
    }
}
