// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! A continuous, virtualized catalogue. Selection belongs to the catalogue,
//! not to the rows currently bound by GTK.
use super::*;
use library::{duplicates::Candidate, service_import::CatalogTrack};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DuplicateChoice {
    Existing,
    Incoming,
    Both,
}
impl DuplicateChoice {
    fn description(self) -> String {
        i18n(match self {
            Self::Existing => "Left selected — reuse the existing track",
            Self::Incoming => "Right selected — replace the library entry after downloading",
            Self::Both => "Both selected — keep the existing track and import the new version",
        })
    }
}

#[derive(Clone)]
pub(super) struct Decision {
    pub candidate: Candidate,
    pub choice: DuplicateChoice,
}
pub(super) struct Selection {
    pub tracks: Vec<CatalogTrack>,
    pub decisions: std::collections::HashMap<String, Decision>,
    pub collections: std::collections::HashSet<String>,
}
impl std::ops::Deref for Selection {
    type Target = [CatalogTrack];
    fn deref(&self) -> &Self::Target {
        &self.tracks
    }
}

struct Group {
    key: String,
    title: String,
    album: bool,
    indices: Vec<usize>,
    cover: Option<String>,
}

fn groups(tracks: &[CatalogTrack]) -> Vec<Group> {
    let mut result: std::collections::BTreeMap<String, Group> = Default::default();
    for (index, track) in tracks.iter().enumerate() {
        let mut add = |key: String, title: String, album, cover: Option<String>| {
            let group = result.entry(key.clone()).or_insert_with(|| Group {
                key,
                title,
                album,
                indices: Vec::new(),
                cover: None,
            });
            if group.indices.last() != Some(&index) {
                group.indices.push(index);
            }
            if group.cover.is_none() {
                group.cover = cover.or_else(|| track.cover.clone());
            }
        };
        for collection in &track.collections {
            if collection.kind != "library" && !collection.title.trim().is_empty() {
                add(
                    collection.key.clone(),
                    collection.title.clone(),
                    collection.kind == "album",
                    collection.cover.clone(),
                );
            }
        }
    }
    let mut result: Vec<_> = result.into_values().collect();
    for group in &mut result {
        let key = group.key.clone();
        group.indices.sort_by_key(|&index| {
            tracks[index]
                .collections
                .iter()
                .find(|collection| collection.key == key)
                .and_then(|collection| collection.position)
                .unwrap_or(index as u32)
        });
    }
    result.sort_by_key(|group| (!group.album, group.title.to_lowercase()));
    result
}

// Membership comes from service collections, never incidental album tags.
// A song can remain in several collections, but is downloaded only once.
fn ungrouped(tracks: &[CatalogTrack], groups: &[Group]) -> Vec<usize> {
    let mut grouped = vec![false; tracks.len()];
    for group in groups {
        for &index in &group.indices {
            grouped[index] = true;
        }
    }
    (0..tracks.len()).filter(|&i| !grouped[i]).collect()
}

fn matches(track: &CatalogTrack, query: &str) -> bool {
    query.is_empty()
        || format!(
            "{} {} {} {}",
            track.title,
            track.artist.as_deref().unwrap_or(""),
            track.album.as_deref().unwrap_or(""),
            track
                .collections
                .iter()
                .map(|c| c.title.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        )
        .to_lowercase()
        .contains(query)
}

pub(super) struct Picker {
    dialog: adw::Dialog,
    tracks: Vec<CatalogTrack>,
    groups: Vec<Group>,
    ungrouped: Vec<usize>,
    duplicates: Vec<Option<Candidate>>,
    imported: Vec<bool>,
    selected: RefCell<Vec<bool>>,
    group_selected: RefCell<Vec<bool>>,
    decisions: RefCell<std::collections::HashMap<usize, DuplicateChoice>>,
    pending: RefCell<std::collections::HashMap<usize, u64>>,
    resolution_ids: RefCell<std::collections::HashMap<usize, u64>>,
    next_resolution: Cell<u64>,
    resolution_toasts: RefCell<std::collections::HashMap<usize, glib::WeakRef<adw::Toast>>>,
    previous_selection: RefCell<std::collections::HashMap<usize, bool>>,
    toasts: adw::ToastOverlay,
    scope: Cell<Option<usize>>,
    duplicates_only: Cell<bool>,
    hidden_duplicates: RefCell<std::collections::HashMap<Option<usize>, bool>>,
    hide_duplicates: gtk::ToggleButton,
    review: gtk::Button,
    visible: RefCell<Vec<usize>>,
    syncing: Cell<bool>,
    model: gio::ListStore,
    list: gtk::ListView,
    search: gtk::SearchEntry,
    heading: gtk::Label,
    back: gtk::Button,
    empty: gtk::Stack,
    empty_message: gtk::Label,
    select_all: gtk::CheckButton,
    count: gtk::Label,
    import: gtk::Button,
    shelf: gtk::Box,
    artwork: Rc<super::import_preview::Artwork>,
    group_checks: RefCell<Vec<gtk::CheckButton>>,
    group_counts: RefCell<Vec<gtk::Label>>,
    group_warnings: RefCell<Vec<Option<gtk::Label>>>,
    group_cards: RefCell<Vec<gtk::Box>>,
    row_checks: RefCell<Vec<(usize, glib::WeakRef<gtk::CheckButton>)>>,
    preview: Rc<super::import_preview::Preview>,
    parent: Option<glib::WeakRef<Window>>,
    auditioning: Cell<bool>,
    closing_for_audition: Cell<bool>,
    resume_pending: Cell<bool>,
    result_sender: async_channel::Sender<bool>,
}

impl Picker {
    pub(super) async fn choose(
        parent: Option<&Window>,
        tracks: Vec<CatalogTrack>,
        duplicates: Vec<Option<Candidate>>,
        imported: Vec<bool>,
        covers: bool,
        warning: Option<&str>,
        preview: Rc<super::import_preview::Preview>,
    ) -> Option<Selection> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_margin_start(18);
        content.set_margin_end(18);
        let title = adw::WindowTitle::new(&i18n("Choose Music to Import"), "");
        let header = adw::HeaderBar::builder()
            .title_widget(&title)
            .show_end_title_buttons(false)
            .build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&crate::dialogs::scroll(&content)));
        toolbar.set_content(Some(&toasts));
        let dialog = adw::Dialog::builder()
            .title(i18n("Choose Music to Import"))
            .child(&toolbar)
            .content_width(880)
            .content_height(820)
            .build();
        dialog.set_widget_name("import-picker");
        if let Some(warning) = warning {
            content.append(&super::import_warnings::banner(warning, &dialog));
        }
        let groups = groups(&tracks);
        let ungrouped = ungrouped(&tracks, &groups);
        let albums = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        albums.set_homogeneous(true);
        albums.set_margin_top(4);
        albums.set_margin_bottom(8);
        let album_scroll = gtk::ScrolledWindow::builder()
            .name("import-albums")
            .child(&albums)
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .overlay_scrolling(false)
            .min_content_height(136)
            .propagate_natural_height(true)
            .valign(gtk::Align::Start)
            .build();
        let shelf = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let shelf_heading = gtk::Label::builder()
            .label(i18n("Albums and Playlists"))
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        shelf_heading.set_tooltip_text(Some(&i18n(
            "Open an album to view its tracks. Use its checkbox to select or deselect all songs.",
        )));
        shelf.append(&shelf_heading);
        let selection_hint = gtk::Label::builder()
            .label(i18n(
                "Select albums separately. Open an album to browse and listen to its songs.",
            ))
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .xalign(0.0)
            .max_width_chars(80)
            .css_classes(["dim-label", "caption"])
            .build();
        shelf.append(&selection_hint);
        shelf.append(&album_scroll);
        shelf.set_visible(!groups.is_empty());
        content.append(&shelf);
        let search = gtk::SearchEntry::builder()
            .name("import-search")
            .placeholder_text(i18n("Search songs, artists or albums"))
            .hexpand(true)
            .build();
        content.append(&search);
        let navigation = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let back = gtk::Button::builder()
            .name("import-all-songs")
            .label(i18n("Other Songs"))
            .tooltip_text(i18n("Return to songs outside albums and playlists"))
            .visible(false)
            .build();
        let heading = gtk::Label::builder()
            .label(i18n("Other Songs"))
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading"])
            .build();
        let review = gtk::Button::builder()
            .name("import-review-duplicates")
            .build();
        let hide_duplicates = gtk::ToggleButton::builder()
            .name("import-hide-duplicates")
            .label(i18n("Hide Duplicates"))
            .tooltip_text(i18n(
                "Hide possible duplicates in this list without changing your selection",
            ))
            .build();
        navigation.prepend(&back);
        navigation.append(&heading);
        content.append(&navigation);
        let filters = crate::dialogs::actions();
        filters.append(&hide_duplicates);
        filters.append(&review);
        content.append(&filters);
        let select_all = gtk::CheckButton::builder()
            .name("import-select-all")
            .label(i18n("Select all tracks"))
            .build();
        select_all.set_hexpand(true);
        filters.prepend(&select_all);
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let list = gtk::ListView::builder()
            .name("import-track-list")
            .model(&gtk::NoSelection::new(Some(model.clone())))
            .css_classes(["background"])
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .name("import-track-scroll")
            .min_content_height(128)
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .overlay_scrolling(false)
            .vexpand(true)
            .build();
        let empty = gtk::Stack::builder().vexpand(true).build();
        empty.add_named(&scroll, Some("songs"));
        let empty_message = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build();
        empty.add_named(&empty_message, Some("empty"));
        content.append(&empty);
        let preview_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let stop = gtk::Button::builder()
            .icon_name("hopea-media-playback-stop-symbolic")
            .tooltip_text(i18n("Stop Preview"))
            .build();
        preview.label.set_hexpand(true);
        preview.label.set_max_width_chars(45);
        preview_bar.append(&stop);
        preview_bar.append(&preview.label);
        preview_bar.set_visible(false);
        preview.label.connect_label_notify(clone!(
            #[weak]
            preview_bar,
            move |label| preview_bar.set_visible(!label.text().is_empty())
        ));
        content.append(&preview_bar);
        let footer = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .min_children_per_line(1)
            .max_children_per_line(2)
            .row_spacing(8)
            .column_spacing(8)
            .css_classes(["action-wrap"])
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(18)
            .margin_end(18)
            .build();
        let count = gtk::Label::builder()
            .name("import-selected-count")
            .xalign(0.0)
            .hexpand(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build();
        let cancel = gtk::Button::with_label(&i18n("Cancel"));
        cancel.set_widget_name("import-cancel-selection");
        let import = gtk::Button::builder()
            .name("import-confirm")
            .label(i18n("Import Selected"))
            .css_classes(["suggested-action"])
            .build();
        footer.append(&count);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.append(&cancel);
        buttons.append(&import);
        footer.append(&buttons);
        toolbar.add_bottom_bar(&footer);
        if !covers {
            content.append(
                &gtk::Label::builder()
                    .label(i18n("Cover downloads are off"))
                    .xalign(0.0)
                    .css_classes(["dim-label"])
                    .build(),
            );
        }
        let mut selected: Vec<_> = duplicates.iter().map(Option::is_none).collect();
        // Albums are opt-in; selecting loose songs never selects a collection.
        let group_selected = vec![false; groups.len()];
        for group in &groups {
            for &i in &group.indices {
                selected[i] = false;
            }
        }
        let (sender, receiver) = async_channel::bounded(1);
        let picker = Rc::new(Self {
            dialog,
            tracks,
            groups,
            ungrouped,
            duplicates,
            imported,
            selected: RefCell::new(selected),
            group_selected: RefCell::new(group_selected),
            decisions: RefCell::new(Default::default()),
            pending: RefCell::new(Default::default()),
            resolution_ids: RefCell::new(Default::default()),
            next_resolution: Cell::new(0),
            resolution_toasts: RefCell::new(Default::default()),
            previous_selection: RefCell::new(Default::default()),
            toasts,
            scope: Cell::new(None),
            duplicates_only: Cell::new(false),
            hidden_duplicates: RefCell::new(Default::default()),
            hide_duplicates,
            review,
            visible: RefCell::new(Vec::new()),
            syncing: Cell::new(false),
            model,
            list,
            search,
            heading,
            back,
            empty,
            empty_message,
            select_all,
            count,
            import,
            shelf,
            artwork: preview.artwork.clone(),
            group_checks: RefCell::new(Vec::new()),
            group_counts: RefCell::new(Vec::new()),
            group_warnings: RefCell::new(Vec::new()),
            group_cards: RefCell::new(Vec::new()),
            row_checks: RefCell::new(Vec::new()),
            preview,
            parent: parent.map(|w| w.downgrade()),
            auditioning: Cell::new(false),
            closing_for_audition: Cell::new(false),
            resume_pending: Cell::new(false),
            result_sender: sender.clone(),
        });
        let weak = Rc::downgrade(&picker);
        picker.preview.on_play(move |song, window| {
            let Some(picker) = weak.upgrade() else {
                return;
            };
            picker.auditioning.set(true);
            picker.closing_for_audition.set(true);
            picker.dialog.close();
            let weak = Rc::downgrade(&picker);
            window.start_import_audition(
                song,
                Box::new(move |resume| {
                    if let Some(picker) = weak.upgrade() {
                        if resume {
                            picker.resume_after_audition();
                        } else {
                            let _ = picker.result_sender.try_send(false);
                        }
                    }
                }),
            );
        });
        picker.populate_groups(&albums);
        picker.setup_list();
        picker.refilter();
        let weak = Rc::downgrade(&picker);
        picker.search.connect_search_changed(move |_| {
            if let Some(picker) = weak.upgrade() {
                picker.refilter();
            }
        });
        let weak = Rc::downgrade(&picker);
        picker.select_all.connect_toggled(move |check| {
            let Some(picker) = weak.upgrade() else {
                return;
            };
            if picker.syncing.get() {
                return;
            }
            if let Some(group) = picker.scope.get() {
                picker.set_group_selected(group, check.is_active());
            } else {
                for &index in picker.visible.borrow().iter() {
                    if picker.scope.get().is_some() {
                        return;
                    }
                    picker.selected.borrow_mut()[index] = check.is_active();
                }
            }
            picker.update_selection();
        });
        let weak = Rc::downgrade(&picker);
        picker.back.connect_clicked(move |_| {
            if let Some(picker) = weak.upgrade() {
                let scope = if picker.duplicates_only.get() {
                    picker.scope.get()
                } else {
                    None
                };
                picker.open_scope(scope, false);
            }
        });
        let weak = Rc::downgrade(&picker);
        picker.review.connect_clicked(move |_| {
            if let Some(picker) = weak.upgrade() {
                picker.open_scope(picker.scope.get(), true);
            }
        });
        let weak = Rc::downgrade(&picker);
        picker.hide_duplicates.connect_toggled(move |button| {
            let Some(picker) = weak.upgrade() else {
                return;
            };
            if picker.syncing.get() {
                return;
            }
            picker
                .hidden_duplicates
                .borrow_mut()
                .insert(picker.scope.get(), button.is_active());
            picker.refilter();
        });
        let weak = Rc::downgrade(&picker);
        stop.connect_clicked(move |_| {
            if let Some(picker) = weak.upgrade() {
                picker.preview.stop();
            }
        });
        let weak = Rc::downgrade(&picker);
        picker.dialog.connect_closed(move |_| {
            let Some(picker) = weak.upgrade() else {
                return;
            };
            if picker.closing_for_audition.replace(false) {
                if picker.resume_pending.replace(false) {
                    let weak = Rc::downgrade(&picker);
                    glib::idle_add_local_once(move || {
                        if let Some(picker) = weak.upgrade() {
                            picker.resume_after_audition();
                        }
                    });
                }
            } else if !picker.auditioning.get() {
                let _ = picker.result_sender.try_send(false);
            }
        });
        cancel.connect_clicked(clone!(
            #[weak(rename_to = dialog)]
            picker.dialog,
            move |_| {
                dialog.close();
            }
        ));
        picker.import.connect_clicked(clone!(
            #[weak(rename_to = dialog)]
            picker.dialog,
            #[strong]
            sender,
            move |_| {
                let _ = sender.try_send(true);
                dialog.close();
            }
        ));
        // A preview and the import share one adapter stream. Do not start a
        // second request until preparation of the preview has completed.
        let weak = Rc::downgrade(&picker);
        let monitor = glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
            let Some(picker) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            picker.import.set_sensitive(
                !picker.preview.busy.get() && picker.selected.borrow().iter().any(|v| *v),
            );
            glib::ControlFlow::Continue
        });
        AdwDialogExt::present(&picker.dialog, parent);
        let accepted = receiver.recv().await.unwrap_or(false);
        monitor.remove();
        picker.preview.close();
        if !accepted {
            return None;
        }
        while picker.preview.busy.get() {
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        let selected = picker.selected.borrow();
        let result = Some(Selection {
            collections: picker
                .groups
                .iter()
                .zip(picker.group_selected.borrow().iter())
                .filter(|(_, active)| **active)
                .map(|(group, _)| group.key.clone())
                .collect(),
            tracks: picker
                .tracks
                .iter()
                .enumerate()
                .filter(|(i, _)| selected[*i])
                .map(|(_, t)| t.clone())
                .collect(),
            decisions: picker
                .decisions
                .borrow()
                .iter()
                .filter(|(i, _)| selected[**i])
                .filter_map(|(&i, &choice)| {
                    picker.duplicates[i].clone().map(|candidate| {
                        (picker.tracks[i].key.clone(), Decision { candidate, choice })
                    })
                })
                .collect(),
        });
        result
    }

    fn resume_after_audition(&self) {
        if self.closing_for_audition.get() {
            self.resume_pending.set(true);
            return;
        }
        self.auditioning.set(false);
        self.preview.label.set_text("");
        if let Some(parent) = self.parent.as_ref().and_then(|w| w.upgrade()) {
            // Re-present the same widgets: selection, decisions, query and
            // scroll adjustments remain intact. No catalogue rebuild here.
            AdwDialogExt::present(&self.dialog, Some(&parent));
        } else {
            let _ = self.result_sender.try_send(false);
        }
    }

    fn set_group_selected(&self, index: usize, active: bool) {
        self.group_selected.borrow_mut()[index] = active;
        let mut selected = self.selected.borrow_mut();
        for group in &self.groups {
            for &i in &group.indices {
                selected[i] = false;
            }
        }
        for (group, &active) in self.groups.iter().zip(self.group_selected.borrow().iter()) {
            if active {
                for &i in &group.indices {
                    selected[i] = true;
                }
            }
        }
    }

    fn unresolved(&self, index: usize) -> bool {
        self.duplicates[index].is_some() && !self.decisions.borrow().contains_key(&index)
    }

    fn choose_duplicate(self: &Rc<Self>, index: usize, choice: DuplicateChoice) {
        if self.decisions.borrow().contains_key(&index) {
            return;
        }
        self.previous_selection
            .borrow_mut()
            .insert(index, self.selected.borrow()[index]);
        self.decisions.borrow_mut().insert(index, choice);
        // A version decision never opts an album into downloading. Only
        // its explicit checkbox selects the collection; loose songs are independent.
        if self.scope.get().is_none() {
            self.selected.borrow_mut()[index] = true;
        }
        let id = self.next_resolution.get().wrapping_add(1);
        self.next_resolution.set(id);
        self.resolution_ids.borrow_mut().insert(index, id);
        self.pending.borrow_mut().insert(index, id);
        self.preview.stop();
        self.refilter();
        let toast = adw::Toast::builder()
            .title(i18n_f(
                "Conflict resolved: {}",
                &[&self.tracks[index].title],
            ))
            .button_label(i18n("Undo"))
            .use_markup(false)
            .timeout(8)
            .build();
        let weak = Rc::downgrade(self);
        toast.connect_button_clicked(move |_| {
            if let Some(picker) = weak.upgrade() {
                picker.undo_duplicate(index, id);
            }
        });
        self.resolution_toasts
            .borrow_mut()
            .insert(index, toast.downgrade());
        self.toasts.add_toast(toast);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_secs(1), move || {
            if let Some(picker) = weak.upgrade() {
                let current = picker.pending.borrow().get(&index).copied();
                if current == Some(id) {
                    picker.pending.borrow_mut().remove(&index);
                    picker.refilter();
                }
            }
        });
    }

    fn undo_duplicate(&self, index: usize, id: u64) {
        if self.resolution_ids.borrow().get(&index).copied() != Some(id) {
            return;
        }
        let toast = self
            .resolution_toasts
            .borrow_mut()
            .remove(&index)
            .and_then(|t| t.upgrade());
        if let Some(toast) = toast {
            toast.dismiss();
        }
        self.resolution_ids.borrow_mut().remove(&index);
        self.pending.borrow_mut().remove(&index);
        self.decisions.borrow_mut().remove(&index);
        let previous = self
            .previous_selection
            .borrow_mut()
            .remove(&index)
            .unwrap_or(false);
        if self.ungrouped.contains(&index) {
            self.selected.borrow_mut()[index] = previous;
        }
        self.refilter();
    }

    fn resolved_card(self: &Rc<Self>, index: usize, choice: DuplicateChoice) -> gtk::Box {
        let card = gtk::Box::builder()
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .css_classes(["card", "import-comparison"])
            .name("import-conflict-resolved")
            .build();
        let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
        text.set_hexpand(true);
        text.append(
            &gtk::Label::builder()
                .label(i18n("Conflict resolved"))
                .xalign(0.0)
                .css_classes(["heading", "success"])
                .build(),
        );
        text.append(
            &gtk::Label::builder()
                .label(choice.description())
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .css_classes(["dim-label"])
                .build(),
        );
        card.append(&text);
        let undo = gtk::Button::builder()
            .label(i18n("Undo"))
            .name("import-undo-conflict")
            .valign(gtk::Align::Center)
            .build();
        let id = self.resolution_ids.borrow()[&index];
        let weak = Rc::downgrade(self);
        undo.connect_clicked(move |_| {
            if let Some(picker) = weak.upgrade() {
                picker.undo_duplicate(index, id);
            }
        });
        card.append(&undo);
        card
    }

    fn open_scope(&self, scope: Option<usize>, duplicates: bool) {
        self.scope.set(scope);
        self.duplicates_only.set(duplicates);
        self.search.set_text("");
        self.refilter();
        if self.model.n_items() > 0 {
            self.list.scroll_to(0, gtk::ListScrollFlags::NONE, None);
        }
    }

    fn refilter(&self) {
        self.shelf.set_visible(
            self.scope.get().is_none() && !self.duplicates_only.get() && !self.groups.is_empty(),
        );
        let query = self.search.text().trim().to_lowercase();
        let matching: Vec<_> = self.tracks.iter().map(|t| matches(t, &query)).collect();
        for (group, card) in self.groups.iter().zip(self.group_cards.borrow().iter()) {
            card.set_visible(
                self.scope.get().is_some() || group.indices.iter().any(|&i| matching[i]),
            );
        }
        let candidates: Vec<_> = self
            .scope
            .get()
            .map(|index| self.groups[index].indices.clone())
            .unwrap_or_else(|| self.ungrouped.clone());
        let duplicate_count = candidates.iter().filter(|&&i| self.unresolved(i)).count();
        let hidden = self
            .hidden_duplicates
            .borrow()
            .get(&self.scope.get())
            .copied()
            .unwrap_or(false);
        self.syncing.set(true);
        self.hide_duplicates.set_active(hidden);
        self.hide_duplicates.set_sensitive(duplicate_count > 0);
        self.hide_duplicates
            .set_visible(!self.duplicates_only.get());
        self.review.set_label(&i18n_f(
            "Compare Duplicates ({})",
            &[&duplicate_count.to_string()],
        ));
        self.review
            .set_visible(duplicate_count > 0 && !self.duplicates_only.get());
        self.syncing.set(false);
        let indices: Vec<_> = candidates
            .into_iter()
            .filter(|&index| {
                (if self.duplicates_only.get() {
                    self.unresolved(index) || self.pending.borrow().contains_key(&index)
                } else {
                    (self.scope.get().is_some() || !self.decisions.borrow().contains_key(&index))
                        && (!hidden || !self.unresolved(index))
                }) && matching[index]
            })
            .collect();
        self.visible.replace(indices.clone());
        self.model.splice(
            0,
            self.model.n_items(),
            &indices
                .iter()
                .map(|&index| glib::BoxedAnyObject::new(index))
                .collect::<Vec<_>>(),
        );
        self.empty
            .set_visible_child_name(if indices.is_empty() { "empty" } else { "songs" });
        self.empty_message.set_text(&if self.duplicates_only.get() && duplicate_count == 0 {
            i18n("All conflicts in this list are resolved. Your choices are saved.")
        } else if hidden && !self.duplicates_only.get() {
            i18n("No songs match this filter. Turn off Hide Duplicates to see possible matches.")
        } else if query.is_empty()
            && self.scope.get().is_none() && !self.duplicates_only.get() && !self.groups.is_empty() {
            i18n("All songs are in albums or playlists. Open a collection above to choose its tracks.")
        } else {
            i18n("No matching songs")
        });
        let title = self
            .scope
            .get()
            .map(|i| self.groups[i].title.clone())
            .unwrap_or_else(|| i18n("Other Songs"));
        self.heading.set_text(&if self.duplicates_only.get() {
            i18n_f("Duplicates in {}", &[&title])
        } else {
            title
        });
        self.back.set_label(&if self.duplicates_only.get() {
            i18n("Back to Songs")
        } else {
            i18n("Back to Albums")
        });
        self.back
            .set_tooltip_text(Some(&i18n("Return to the previous list")));
        self.back
            .set_visible(self.scope.get().is_some() || self.duplicates_only.get());
        self.select_all.set_visible(!self.duplicates_only.get());
        self.select_all
            .set_label(Some(&if self.scope.get().is_some() {
                i18n("Import Entire Album")
            } else if query.is_empty() {
                i18n("Select all tracks")
            } else {
                i18n("Select all matches")
            }));
        self.update_selection();
    }

    fn update_selection(&self) {
        self.syncing.set(true);
        let selected = self.selected.borrow();
        let count = selected.iter().filter(|v| **v).count();
        self.count.set_text(&i18n_f(
            "{} of {} selected",
            &[&count.to_string(), &self.tracks.len().to_string()],
        ));
        self.import
            .set_sensitive(count > 0 && !self.preview.busy.get());
        let visible = self.visible.borrow();
        let n = visible.iter().filter(|&&i| selected[i]).count();
        if let Some(group) = self.scope.get() {
            self.select_all.set_sensitive(true);
            self.select_all
                .set_active(self.group_selected.borrow()[group]);
            self.select_all.set_inconsistent(false);
        } else {
            self.select_all.set_sensitive(!visible.is_empty());
            self.select_all
                .set_active(!visible.is_empty() && n == visible.len());
            self.select_all.set_inconsistent(n > 0 && n < visible.len());
        }
        for (index, group) in self.groups.iter().enumerate() {
            let active = self.group_selected.borrow()[index];
            let n = if active { group.indices.len() } else { 0 };
            let check = &self.group_checks.borrow()[index];
            check.set_active(active);
            check.set_inconsistent(false);
            self.group_counts.borrow()[index].set_text(&i18n_f(
                if active {
                    "All {} songs selected"
                } else {
                    "{} songs · Not selected"
                },
                &[&group.indices.len().to_string()],
            ));
            let card = &self.group_cards.borrow()[index];
            if let Some(label) = self.group_warnings.borrow()[index].as_ref() {
                let unresolved = group
                    .indices
                    .iter()
                    .filter(|&&i| self.unresolved(i))
                    .count();
                label.set_label(&i18n_f(
                    "{} possible duplicates",
                    &[&unresolved.to_string()],
                ));
                label.set_visible(unresolved > 0);
            }
            if n > 0 {
                card.add_css_class("import-collection-selected");
            } else {
                card.remove_css_class("import-collection-selected");
            }
        }
        self.row_checks.borrow_mut().retain(|(index, weak)| {
            if let Some(check) = weak.upgrade() {
                check.set_active(selected[*index]);
                true
            } else {
                false
            }
        });
        self.syncing.set(false);
    }

    fn populate_groups(self: &Rc<Self>, albums: &gtk::Box) {
        for (index, group) in self.groups.iter().enumerate() {
            let card = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(6)
                .width_request(260)
                .valign(gtk::Align::Fill)
                .css_classes(["card", "import-collection"])
                .build();
            let check = gtk::CheckButton::builder()
                .name("import-collection-selection")
                .valign(gtk::Align::Center)
                .build();
            check.set_tooltip_text(Some(&i18n_f("Select all songs in {}", &[&group.title])));
            card.append(&check);
            let body = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            body.append(&self.artwork.image(group.cover.as_deref(), 72));
            let labels = gtk::Box::new(gtk::Orientation::Vertical, 4);
            labels.set_valign(gtk::Align::Center);
            labels.append(
                &gtk::Label::builder()
                    .label(&group.title)
                    .xalign(0.0)
                    .width_chars(14)
                    .max_width_chars(18)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["heading"])
                    .build(),
            );
            labels.append(
                &gtk::Label::builder()
                    .label(if group.album {
                        i18n("Album")
                    } else {
                        i18n("Playlist")
                    })
                    .xalign(0.0)
                    .css_classes(["dim-label"])
                    .build(),
            );
            let count = gtk::Label::builder()
                .label(i18n_f(
                    "All {} songs selected",
                    &[&group.indices.len().to_string()],
                ))
                .xalign(0.0)
                .css_classes(["caption", "dim-label"])
                .build();
            labels.append(&count);
            let duplicates = group
                .indices
                .iter()
                .filter(|&&i| self.unresolved(i))
                .count();
            let warning = if duplicates > 0 {
                let warning = gtk::Label::builder()
                    .name("import-collection-duplicates")
                    .label(i18n_f("{} possible duplicates — review before importing", &[&duplicates.to_string()]))
                    .xalign(0.0).max_width_chars(24)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .tooltip_text(i18n("Some songs may already be in your library. Open the album to compare them, or check it to import the whole album."))
                    .css_classes(["caption", "import-conflict"]).build();
                labels.append(&warning);
                Some(warning)
            } else {
                None
            };
            self.group_warnings.borrow_mut().push(warning);
            body.append(&labels);
            body.append(&gtk::Image::from_icon_name("hopea-go-next-symbolic"));
            let open = gtk::Button::builder()
                .name("import-open-collection")
                .child(&body)
                .tooltip_text(&group.title)
                .css_classes(["flat"])
                .hexpand(true)
                .build();
            // Keep the collection name available independently of tooltip text.
            open.set_widget_name("import-open-collection");
            card.append(&open);
            albums.append(&card);
            let weak = Rc::downgrade(self);
            check.connect_toggled(move |check| {
                let Some(picker) = weak.upgrade() else {
                    return;
                };
                if picker.syncing.get() {
                    return;
                }
                picker.set_group_selected(index, check.is_active());
                picker.update_selection();
            });
            let weak = Rc::downgrade(self);
            open.connect_clicked(move |_| {
                if let Some(picker) = weak.upgrade() {
                    picker.open_scope(Some(index), false);
                }
            });
            self.group_checks.borrow_mut().push(check);
            self.group_counts.borrow_mut().push(count);
            self.group_cards.borrow_mut().push(card);
        }
        // ScrolledWindow otherwise gets allocated less than its styled cards
        // need when a warning and the song list compete for vertical space.
        // Use the measured minimum (including larger fonts), not a fixed cap.
        if let Some(scroll) = albums
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_downcast::<gtk::ScrolledWindow>()
        {
            let size_shelf = |scroll: &gtk::ScrolledWindow, albums: &gtk::Box| {
                let (minimum, natural, _, _) = albums.measure(gtk::Orientation::Vertical, -1);
                let content_height = minimum.max(natural).max(80);
                let (bar_min, bar_natural, _, _) =
                    scroll.hscrollbar().measure(gtk::Orientation::Vertical, -1);
                // With vertical scrolling disabled GTK can ignore the content
                // minimum. Request the outer height explicitly, including the
                // non-overlay horizontal scrollbar, so its viewport never clips.
                scroll.set_min_content_height(content_height);
                scroll.set_height_request(content_height + bar_min.max(bar_natural));
            };
            size_shelf(&scroll, albums);
            let albums = albums.clone();
            scroll.add_tick_callback(move |scroll, _| {
                size_shelf(scroll, &albums);
                glib::ControlFlow::Break
            });
        }
    }

    fn comparison(self: &Rc<Self>, index: usize) -> gtk::Box {
        let resolved = self.decisions.borrow().get(&index).copied();
        if let Some(choice) = resolved {
            return self.resolved_card(index, choice);
        }
        let track = &self.tracks[index];
        let existing = self.duplicates[index].as_ref().unwrap();
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(6)
            .margin_bottom(6)
            .css_classes(["card", "import-comparison"])
            .build();
        card.set_widget_name("import-comparison");
        card.append(
            &gtk::Label::builder()
                .label(i18n("Possible duplicate — listen to both versions"))
                .css_classes(["import-conflict"])
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .xalign(0.0)
                .build(),
        );
        let pair = crate::dialogs::actions();
        pair.set_max_children_per_line(2);
        pair.set_homogeneous(true);
        let parent = self.parent.as_ref().and_then(|w| w.upgrade());
        for (left, title, artist, duration, choice, button_title) in [
            (
                true,
                existing.title.as_str(),
                existing.artist.as_deref(),
                existing.duration_ms,
                DuplicateChoice::Existing,
                i18n("Keep Existing"),
            ),
            (
                false,
                track.title.as_str(),
                track.artist.as_deref(),
                track.duration_ms,
                DuplicateChoice::Incoming,
                i18n("Import This Version"),
            ),
        ] {
            let side = gtk::Box::new(gtk::Orientation::Vertical, 6);
            side.set_widget_name(if left {
                "import-existing-track"
            } else {
                "import-incoming-track"
            });
            side.append(
                &gtk::Label::builder()
                    .label(if left {
                        i18n("In Your Library")
                    } else {
                        i18n("Incoming Track")
                    })
                    .xalign(0.0)
                    .css_classes(["dim-label"])
                    .build(),
            );
            if left {
                let art = self.artwork.image(None, 64);
                art.set_widget_name("import-existing-art");
                let placeholder = art.child().unwrap();
                let cover = gtk::Picture::builder()
                    .can_shrink(true)
                    .content_fit(gtk::ContentFit::Cover)
                    .build();
                if let Some(id) = existing.id {
                    let weak = cover.downgrade();
                    let placeholder = placeholder.downgrade();
                    glib::spawn_future_local(async move {
                        if let Some(texture) = library::thumbnails::load(id).await {
                            if let Some(cover) = weak.upgrade() {
                                cover.set_paintable(Some(&texture));
                                if let Some(placeholder) = placeholder.upgrade() {
                                    placeholder.set_opacity(0.0);
                                }
                            }
                        }
                    });
                }
                art.add_overlay(&cover);
                side.append(&art);
            } else {
                let art = self.artwork.image(track.cover.as_deref(), 64);
                art.set_widget_name("import-incoming-art");
                side.append(&art);
            }
            if (left && existing.id.is_none()) || (!left && self.preview.unavailable(&track.key)) {
                side.add_css_class("import-unavailable");
            }
            let song_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let metadata = gtk::Box::new(gtk::Orientation::Vertical, 4);
            metadata.set_hexpand(true);
            metadata.append(
                &gtk::Label::builder()
                    .label(title)
                    .use_markup(false)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .xalign(0.0)
                    .css_classes(["heading"])
                    .build(),
            );
            metadata.append(
                &gtk::Label::builder()
                    .label(artist.unwrap_or(""))
                    .use_markup(false)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .xalign(0.0)
                    .build(),
            );
            if let Some(ms) = duration {
                metadata.append(
                    &gtk::Label::builder()
                        .label(format!("{}:{:02}", ms / 60000, ms / 1000 % 60))
                        .xalign(0.0)
                        .css_classes(["dim-label"])
                        .build(),
                );
            }
            let mut audio = track.clone();
            if left {
                audio.title = existing.title.clone();
                audio.artist = existing.artist.clone();
            }
            let listen = self.preview.button(
                audio,
                if left { existing.id } else { None },
                parent.as_ref(),
            );
            listen.set_widget_name(if left {
                "import-play-existing"
            } else {
                "import-play-incoming"
            });
            listen.set_child(Some(
                &adw::ButtonContent::builder()
                    .icon_name("hopea-media-playback-start-symbolic")
                    .label(i18n("Play"))
                    .build(),
            ));
            listen.set_sensitive(!left || existing.id.is_some());
            song_row.append(&metadata);
            song_row.append(&listen);
            side.append(&song_row);
            let choose = gtk::Button::with_label(&button_title);
            choose.set_widget_name(if left {
                "import-choose-left"
            } else {
                "import-choose-right"
            });
            choose.set_sensitive(!left || existing.id.is_some());
            if left && existing.id.is_none() {
                side.append(
                    &gtk::Label::builder()
                        .label(i18n("Only an import record remains; audio is unavailable."))
                        .wrap(true)
                        .wrap_mode(gtk::pango::WrapMode::WordChar)
                        .xalign(0.0)
                        .css_classes(["import-unavailable"])
                        .build(),
                );
            }
            let weak = Rc::downgrade(self);
            choose.connect_clicked(clone!(move |_| {
                if let Some(picker) = weak.upgrade() {
                    picker.choose_duplicate(index, choice);
                }
            }));
            side.append(&choose);
            pair.append(&side);
        }
        card.append(&pair);
        let both = gtk::Button::with_label(&i18n("Keep Both"));
        both.set_widget_name("import-choose-both");
        let weak = Rc::downgrade(self);
        both.connect_clicked(clone!(move |_| {
            if let Some(picker) = weak.upgrade() {
                picker.choose_duplicate(index, DuplicateChoice::Both);
            }
        }));
        card.append(&both);
        card.append(&gtk::Label::builder().label(i18n("Replacement happens after a successful import. The left audio file is kept on disk."))
            .wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).xalign(0.0).css_classes(["caption", "dim-label"]).build());
        card
    }

    fn setup_list(self: &Rc<Self>) {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            item.downcast_ref::<gtk::ListItem>()
                .unwrap()
                .set_activatable(false);
        });
        let weak = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let Some(picker) = weak.upgrade() else {
                return;
            };
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let object = item
                .item()
                .unwrap()
                .downcast::<glib::BoxedAnyObject>()
                .unwrap();
            let index = *object.borrow::<usize>();
            let track = &picker.tracks[index];
            if picker.duplicates_only.get() {
                item.set_child(Some(&picker.comparison(index)));
                return;
            }
            let row = adw::ActionRow::builder()
                .title(&track.title)
                .use_markup(false)
                .title_lines(1)
                .subtitle_lines(1)
                .subtitle(format!(
                    "{} · {}{}",
                    track.artist.as_deref().unwrap_or(""),
                    track.album.as_deref().unwrap_or(""),
                    if picker.imported[index] {
                        format!(" · {}", i18n("Previously imported"))
                    } else {
                        String::new()
                    }
                ))
                .build();
            let check = gtk::CheckButton::builder()
                .name("import-track-selection")
                .active(picker.selected.borrow()[index])
                .valign(gtk::Align::Center)
                .build();
            picker
                .row_checks
                .borrow_mut()
                .push((index, check.downgrade()));
            check.set_visible(picker.scope.get().is_none());
            row.add_prefix(&check);
            let cover = track
                .cover
                .as_deref()
                .or_else(|| track.collections.iter().find_map(|c| c.cover.as_deref()));
            row.add_prefix(&picker.artwork.image(cover, 48));
            if picker.scope.get().is_none() {
                row.set_activatable_widget(Some(&check));
            }
            let parent = picker.parent.as_ref().and_then(|w| w.upgrade());
            row.add_suffix(&picker.preview.button(track.clone(), None, parent.as_ref()));
            if let Some(existing) = picker.duplicates[index]
                .as_ref()
                .filter(|_| picker.unresolved(index))
            {
                row.add_css_class("import-conflict");
                row.set_subtitle(&format!(
                    "{} · {} — {}",
                    i18n("Possible duplicate"),
                    existing.artist.as_deref().unwrap_or(""),
                    existing.title
                ));
                if let Some(id) = existing.id {
                    let mut candidate = track.clone();
                    candidate.title = existing.title.clone();
                    candidate.artist = existing.artist.clone();
                    let listen = picker.preview.button(candidate, Some(id), parent.as_ref());
                    listen.set_icon_name("hopea-audio-speakers-symbolic");
                    listen.set_tooltip_text(Some(&i18n("Listen to the existing copy")));
                    row.add_suffix(&listen);
                }
            }
            if picker.preview.unavailable(&track.key) {
                row.add_css_class("import-unavailable");
                row.set_subtitle(&i18n(
                    "Audio unavailable — the service did not provide a download",
                ));
            }
            let weak = Rc::downgrade(&picker);
            check.connect_toggled(move |check| {
                let Some(picker) = weak.upgrade() else {
                    return;
                };
                if picker.syncing.get() {
                    return;
                }
                picker.selected.borrow_mut()[index] = check.is_active();
                picker.update_selection();
            });
            let wrapper = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .build();
            wrapper.append(&row);
            item.set_child(Some(&wrapper));
        });
        let weak = Rc::downgrade(self);
        factory.connect_unbind(move |_, item| {
            item.downcast_ref::<gtk::ListItem>()
                .unwrap()
                .set_child(gtk::Widget::NONE);
            if let Some(picker) = weak.upgrade() {
                picker
                    .row_checks
                    .borrow_mut()
                    .retain(|(_, check)| check.upgrade().is_some());
            }
        });
        self.list.set_factory(Some(&factory));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(album: &str, artist: &str, collections: serde_json::Value) -> CatalogTrack {
        serde_json::from_value(serde_json::json!({
            "key":"track", "title":"Song", "artist":artist, "album":album,
            "album_artist":null, "track_number":null, "cover":null, "collections":collections,
        }))
        .unwrap()
    }

    #[test]
    fn only_service_memberships_remove_songs_from_other_songs() {
        let tracks = vec![
            track(
                "Hybrid Theory",
                "Linkin Park",
                serde_json::json!([
                    {"key":"library", "kind":"library", "title":"Likes"}
                ]),
            ),
            track(
                "Hybrid Theory",
                "Linkin Park",
                serde_json::json!([
                    {"key":"a", "kind":"album", "title":"Album A"},
                    {"key":"library", "kind":"library", "title":"Likes"}
                ]),
            ),
            track(
                "",
                "Artist",
                serde_json::json!([
                    {"key":"p", "kind":"collection", "title":"My mix"}
                ]),
            ),
            track("", "Artist", serde_json::json!([])),
        ];
        let collections = groups(&tracks);
        assert_eq!(collections.len(), 2);
        assert_eq!(ungrouped(&tracks, &collections), vec![0, 3]);
        assert_eq!(collections[0].indices, vec![1]);
        assert_eq!(collections[1].indices, vec![2]);
        assert!(ungrouped(&tracks[1..3], &groups(&tracks[1..3])).is_empty());
    }

    #[test]
    fn album_and_playlist_selection_share_track_membership_without_duplicate_cards() {
        let memberships = serde_json::json!([
            {"key":"a", "kind":"album", "title":"Album A", "cover":"https://example.invalid/cover"},
            {"key":"p", "kind":"playlist", "title":"Playlist B"},
        ]);
        let tracks = vec![
            track("Album A", "Artist", memberships.clone()),
            track("Album A", "Artist", memberships),
        ];
        let albums = groups(&tracks);
        assert_eq!(albums.len(), 2);
        assert!(albums[0].album);
        assert_eq!(albums[0].indices, vec![0, 1]);
        assert_eq!(albums[1].indices, vec![0, 1]);
        assert!(albums[0].cover.is_some());
    }
}

#[cfg(test)]
impl Picker {
    pub(super) async fn test_duplicate_scopes(parent: &Window) {
        fn descendants(w: &gtk::Widget) -> Vec<gtk::Widget> {
            let mut found = vec![w.clone()];
            let mut child = w.first_child();
            while let Some(w) = child {
                child = w.next_sibling();
                found.extend(descendants(&w));
            }
            found
        }
        let tracks: Vec<CatalogTrack> = (0..6)
            .map(|i| {
                serde_json::from_value(serde_json::json!({
            "key":i.to_string(), "title":format!("Song {i}"), "artist":"Artist",
            "album":null, "album_artist":null, "track_number":null, "cover":null,
            "collections": if i < 2 { serde_json::json!([]) } else { serde_json::json!([
                {"key":format!("album{}",i/2), "kind":"album", "title":format!("Album {}",i/2)}
            ]) }
        })).unwrap()
            })
            .collect();
        let duplicates = (0..6)
            .map(|i| {
                (i % 2 == 0).then(|| Candidate {
                    id: Some(i as i64 + 1),
                    sha256: None,
                    title: format!("Song {i}"),
                    artist: Some("Artist".into()),
                    duration_ms: None,
                })
            })
            .collect();
        gtk::prelude::WidgetExt::activate_action(parent, "win.library-settings", None).unwrap();
        glib::timeout_future(std::time::Duration::from_millis(250)).await;
        let settings_dialog = parent.visible_dialog().unwrap();
        assert_eq!(settings_dialog.widget_name(), "settings-form");
        let startup = descendants(settings_dialog.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<adw::SwitchRow>().ok())
            .find(|r| r.title() == "Join the Network at Startup")
            .unwrap();
        let original = startup.is_active();
        startup.set_active(!original);
        let win = parent.clone();
        let task = glib::spawn_future_local(async move {
            Picker::choose(
                Some(&win),
                tracks,
                duplicates,
                vec![false; 6],
                true,
                None,
                super::import_preview::Preview::new(None, None),
            )
            .await
        });
        for _ in 0..50 {
            if parent
                .visible_dialog()
                .is_some_and(|d| d.widget_name() == "import-picker")
            {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
        }
        let dialog = parent.visible_dialog().unwrap();
        let checks: Vec<_> = descendants(dialog.upcast_ref())
            .into_iter()
            .filter(|w| w.widget_name() == "import-collection-selection")
            .filter_map(|w| w.downcast::<gtk::CheckButton>().ok())
            .collect();
        assert_eq!(checks.len(), 2, "every album must have a checkbox");
        assert!(checks
            .iter()
            .all(|check| check.is_visible() && !check.is_active()));
        assert_eq!(
            descendants(dialog.upcast_ref())
                .iter()
                .filter(|w| w.widget_name() == "import-collection-duplicates")
                .count(),
            2,
            "unchecked duplicate albums must explain why"
        );
        let named = |name: &str| {
            descendants(dialog.upcast_ref())
                .into_iter()
                .find(|w| w.widget_name() == name)
                .unwrap()
        };
        parent.set_default_size(660, 640);
        glib::timeout_future(std::time::Duration::from_millis(300)).await;
        let shelf = named("import-albums")
            .downcast::<gtk::ScrolledWindow>()
            .unwrap();
        let viewport = shelf.child().unwrap();
        for _ in 0..60 {
            if viewport.is_mapped() && viewport.height() > 0 {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            viewport.is_mapped() && viewport.height() > 0,
            "album shelf must finish mapping before measuring it"
        );
        for card in descendants(shelf.upcast_ref())
            .iter()
            .filter(|w| w.has_css_class("import-collection"))
        {
            let rect = card.compute_bounds(&viewport).unwrap();
            assert!(
                rect.y() >= 0.0 && rect.y() + rect.height() <= viewport.height() as f32,
                "album cards with conflict labels must fit above the scrollbar: {rect:?} / {}",
                viewport.height()
            );
        }
        if let Some(path) = std::env::var_os("HOPEA_TEST_ALBUM_SHELF_SCREENSHOT") {
            let canvas = dialog.child().unwrap();
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gdk::RGBA::parse("#242424").unwrap(),
                &gtk::graphene::Rect::new(0.0, 0.0, canvas.width() as f32, canvas.height() as f32),
            );
            let mut child = canvas.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                canvas.snapshot_child(&widget, &snapshot);
            }
            parent
                .renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
        parent.set_default_size(960, 800);
        glib::timeout_future(std::time::Duration::from_millis(300)).await;
        let list = named("import-track-list")
            .downcast::<gtk::ListView>()
            .unwrap();
        let hide = named("import-hide-duplicates")
            .downcast::<gtk::ToggleButton>()
            .unwrap();
        let review = named("import-review-duplicates")
            .downcast::<gtk::Button>()
            .unwrap();
        let back = named("import-all-songs").downcast::<gtk::Button>().unwrap();
        let count = named("import-selected-count")
            .downcast::<gtk::Label>()
            .unwrap();
        let open = |title: &str| {
            descendants(dialog.upcast_ref())
                .into_iter()
                .filter(|w| w.widget_name() == "import-open-collection")
                .filter_map(|w| w.downcast::<gtk::Button>().ok())
                .find(|b| b.tooltip_text().as_deref() == Some(title))
                .unwrap()
                .emit_clicked()
        };
        assert_eq!(list.model().unwrap().n_items(), 2);
        assert_eq!(review.label().as_deref(), Some("Compare Duplicates (1)"));
        hide.set_active(true);
        assert_eq!(list.model().unwrap().n_items(), 1);
        assert_eq!(count.text(), "1 of 6 selected");
        open("Album 1");
        assert!(!hide.is_active());
        assert_eq!(list.model().unwrap().n_items(), 2);
        hide.set_active(true);
        assert_eq!(list.model().unwrap().n_items(), 1);
        review.emit_clicked();
        assert_eq!(list.model().unwrap().n_items(), 1);
        let item = list
            .model()
            .unwrap()
            .item(0)
            .unwrap()
            .downcast::<glib::BoxedAnyObject>()
            .unwrap();
        assert_eq!(
            *item.borrow::<usize>(),
            2,
            "review stays within the current album"
        );
        for _ in 0..50 {
            if descendants(list.upcast_ref())
                .iter()
                .any(|w| w.widget_name() == "import-existing-track")
            {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
        }
        crate::dialogs::test_fit(parent, &dialog).await;
        assert!(back.is_visible() && back.is_sensitive());
        assert!(descendants(list.upcast_ref())
            .iter()
            .any(|w| w.widget_name() == "import-existing-track"));
        assert!(descendants(list.upcast_ref())
            .iter()
            .any(|w| w.widget_name() == "import-incoming-track"));
        glib::timeout_future(std::time::Duration::from_millis(100)).await;
        for (side, art, play) in [
            (
                "import-existing-track",
                "import-existing-art",
                "import-play-existing",
            ),
            (
                "import-incoming-track",
                "import-incoming-art",
                "import-play-incoming",
            ),
        ] {
            let side = named(side);
            let art = named(art);
            let rect = art.compute_bounds(&side).unwrap();
            assert!(
                (rect.x() + rect.width() / 2.0 - side.width() as f32 / 2.0).abs() <= 1.0,
                "both comparison covers must be centered"
            );
            let play = named(play).downcast::<gtk::Button>().unwrap();
            assert!(play.is_sensitive());
            assert_eq!(
                play.child()
                    .unwrap()
                    .downcast::<adw::ButtonContent>()
                    .unwrap()
                    .label(),
                "Play"
            );
        }
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("node/tests/fixtures/tone.wav");
        let uri = gio::File::for_path(fixture).uri();
        library::shared_files::remember(&uri, 3);
        let queue_count = parent.player().unwrap().queue().n_songs();
        named("import-play-existing")
            .downcast::<gtk::Button>()
            .unwrap()
            .emit_clicked();
        for _ in 0..60 {
            if parent.is_import_audition() && parent.visible_dialog().is_none() {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        assert!(parent.is_import_audition());
        parent.test_import_audition_layout().await;
        assert!(
            parent.visible_dialog().is_none(),
            "the chooser must release the main player"
        );
        assert_eq!(parent.player().unwrap().queue().n_songs(), queue_count);
        assert_eq!(
            parent
                .player()
                .unwrap()
                .state()
                .current_song()
                .unwrap()
                .title(),
            "Song 2"
        );
        assert!(descendants(parent.upcast_ref())
            .iter()
            .any(|w| w.widget_name() == "import-preview-back" && w.is_mapped()));
        gtk::prelude::WidgetExt::activate_action(parent, "win.return-to-import", None).unwrap();
        for _ in 0..60 {
            if parent.visible_dialog().is_some_and(|d| d == dialog) {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        assert!(!parent.is_import_audition());
        assert_eq!(parent.visible_dialog().as_ref(), Some(&dialog));
        assert_eq!(
            startup.is_active(),
            !original,
            "audition must preserve unsaved settings"
        );
        assert_eq!(list.model().unwrap().n_items(), 1);
        assert_eq!(count.text(), "1 of 6 selected");
        library::shared_files::forget(3);
        for name in ["import-choose-right", "import-choose-both"] {
            named(name)
                .downcast::<gtk::Button>()
                .unwrap()
                .emit_clicked();
            assert_eq!(
                count.text(),
                "1 of 6 selected",
                "resolving a conflict must not select an album"
            );
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
            assert!(named("import-conflict-resolved").is_visible());
            named("import-undo-conflict")
                .downcast::<gtk::Button>()
                .unwrap()
                .emit_clicked();
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
            assert_eq!(
                count.text(),
                "1 of 6 selected",
                "Undo restores the previous album selection"
            );
            assert!(named("import-choose-left").is_visible());
        }
        if let Some(path) = std::env::var_os("HOPEA_TEST_COMPARISON_SCREENSHOT") {
            glib::timeout_future_seconds(2).await;
            let canvas = dialog.child().unwrap();
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gdk::RGBA::parse("#242424").unwrap(),
                &gtk::graphene::Rect::new(0.0, 0.0, canvas.width() as f32, canvas.height() as f32),
            );
            let mut child = canvas.first_child();
            while let Some(w) = child {
                child = w.next_sibling();
                canvas.snapshot_child(&w, &snapshot);
            }
            parent
                .renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
        named("import-choose-left")
            .downcast::<gtk::Button>()
            .unwrap()
            .emit_clicked();
        glib::timeout_future(std::time::Duration::from_millis(1200)).await;
        assert_eq!(
            list.model().unwrap().n_items(),
            0,
            "resolved conflicts leave the review after one second"
        );
        assert_eq!(
            count.text(),
            "1 of 6 selected",
            "removing a resolved row must not opt the album into importing"
        );
        back.emit_clicked();
        assert!(hide.is_active());
        hide.set_active(false);
        assert_eq!(
            list.model().unwrap().n_items(),
            2,
            "album browsing retains every song after resolving conflicts"
        );
        assert!(!review.is_visible());
        hide.set_active(true);
        back.emit_clicked();
        assert!(hide.is_active(), "general list keeps its own filter");
        glib::timeout_future(std::time::Duration::from_millis(200)).await;
        let cards: Vec<_> = descendants(shelf.upcast_ref())
            .into_iter()
            .filter(|w| w.has_css_class("import-collection"))
            .collect();
        assert_eq!(cards.len(), 2);
        assert_eq!(
            cards[0].height(),
            cards[1].height(),
            "cards with and without conflicts must have equal height"
        );
        assert_eq!(
            cards[0].width(),
            cards[1].width(),
            "album cards must have equal width"
        );
        open("Album 2");
        assert!(!hide.is_active(), "another album has an independent filter");
        assert_eq!(list.model().unwrap().n_items(), 2);
        back.emit_clicked();
        open("Album 1");
        assert!(hide.is_active());
        assert_eq!(list.model().unwrap().n_items(), 2);
        checks[0].set_active(true); // Only the album checkbox opts it in.
        assert_eq!(count.text(), "3 of 6 selected");
        named("import-confirm")
            .downcast::<gtk::Button>()
            .unwrap()
            .emit_clicked();
        let selection = task.await.unwrap().unwrap();
        assert_eq!(
            selection.iter().map(|t| t.key.as_str()).collect::<Vec<_>>(),
            vec!["1", "2", "3"]
        );
        assert!(selection.decisions["2"].choice == DuplicateChoice::Existing);
        glib::timeout_future(std::time::Duration::from_millis(300)).await;
        let save = descendants(settings_dialog.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.label().as_deref() == Some("Save"))
            .unwrap();
        save.emit_clicked();
        glib::timeout_future(std::time::Duration::from_millis(200)).await;
        let settings = crate::utils::settings_manager();
        assert_eq!(
            library::joins_at_start(&settings),
            !original,
            "settings must still save after audition"
        );
        settings
            .set_string("library-join-at-start", if original { "yes" } else { "no" })
            .unwrap();
    }
}
