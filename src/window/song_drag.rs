// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Only the lifted song is an overlay. Neighbours translate their existing
//! render trees: no second list, estimated geometry or remeasured text.
//! Allocations stay stable until release, so hit-testing cannot chase the gap.
//! Row-icon approach: https://blogs.gnome.org/gtk/2017/04/23/drag-and-drop-in-lists/
use super::*;
#[cfg(test)]
use adw::prelude::AdwApplicationWindowExt;
use gtk::graphene;
use std::collections::HashMap;

struct Reorder {
    song: Song,
    layer: gtk::Fixed,
    lifted: gtk::Picture,
    touched: Vec<(QueueRow, gtk::Widget, gtk::Overflow)>,
    grab: f64,
    pointer: f64,
    start_y: f64,
    stride: f64,
    target: Option<(Song, bool)>,
    settling: Option<Instant>,
    target_frozen: bool,
    lifted_y: f64,
    last_tick: Instant,
    order: Rc<HashMap<String, u32>>,
    gap: Option<f64>,
    scroll: f64,
}

// Each neighbour crosses one stable midpoint. Nearest-centre hit testing
// changed slots halfway between rows, especially with unequal row heights.
fn insertion(from: u32, center: f64, rows: impl Iterator<Item = (u32, f64)>) -> u32 {
    let mut to = from;
    for (pos, midpoint) in rows {
        if pos < from && center <= midpoint {
            to = to.min(pos);
        } else if pos > from && center >= midpoint {
            to = to.max(pos);
        }
    }
    to
}

// Animate one gap, not every row independently. Offsets are monotonic in
// compacted row order, so their sum can never create a second empty slot.
fn gap_offset(pos: u32, from: u32, gap: f64, stride: f64) -> f64 {
    let rank = pos - u32::from(pos > from);
    let removed = if pos > from { stride } else { 0.0 };
    stride * (rank as f64 + 1.0 - gap).clamp(0.0, 1.0) - removed
}

fn card(row: &QueueRow) -> Option<gtk::Picture> {
    // Capture the actual allocated row, including inherited CSS, progress,
    // ellipsizing and its precise height. Rebuilding a QueueRow in GtkFixed
    // gave it different natural sizes and made text overlap its neighbours.
    let image = capture(row)?;
    let picture = gtk::Picture::for_paintable(&image);
    picture.set_content_fit(gtk::ContentFit::Fill);
    picture.set_can_shrink(true);
    picture.set_size_request(row.width(), row.height());
    if let Some(song) = row.song() {
        picture.set_widget_name(&song.uri());
    }
    Some(picture)
}

fn capture(row: &QueueRow) -> Option<gdk::Paintable> {
    let snapshot = gtk::Snapshot::new();
    let mut child = row.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        row.snapshot_child(&widget, &snapshot);
    }
    snapshot.to_paintable(Some(&graphene::Size::new(
        row.width() as f32,
        row.height() as f32,
    )))
}

impl Drop for Reorder {
    fn drop(&mut self) {
        for (row, parent, overflow) in &self.touched {
            row.set_drag_visual(0.0, false);
            parent.set_overflow(*overflow);
        }
    }
}

fn rows(widget: &gtk::Widget, found: &mut Vec<QueueRow>) {
    if let Some(row) = widget.downcast_ref::<QueueRow>() {
        found.push(row.clone());
        return;
    }
    let mut child = widget.first_child();
    while let Some(w) = child {
        child = w.next_sibling();
        rows(&w, found);
    }
}

impl Window {
    pub(super) fn setup_queue_reorder(&self) {
        let view = self.imp().playlist_view.get();
        let overlay = view.drag_overlay();
        let gesture = gtk::GestureDrag::new();
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let state: Rc<RefCell<Option<Reorder>>> = Rc::new(RefCell::new(None));
        let pending: Rc<RefCell<Option<(QueueRow, f64)>>> = Rc::new(RefCell::new(None));
        let dirty = Rc::new(Cell::new(true));
        if let Some(player) = self.player() {
            player.queue().model().connect_items_changed(clone!(
                #[weak]
                dirty,
                move |_, _, _, _| dirty.set(true)
            ));
        }
        gesture.connect_drag_begin(clone!(
            #[weak]
            overlay,
            #[strong]
            pending,
            move |gesture, x, y| {
                let mut picked = overlay.pick(x, y, gtk::PickFlags::DEFAULT);
                while let Some(widget) = picked {
                    if let Ok(row) = widget.clone().downcast::<QueueRow>() {
                        if !row.property::<bool>("selection-mode") {
                            pending.replace(Some((row, y)));
                        }
                        return;
                    }
                    if widget.is::<gtk::Button>()
                        || widget.is::<gtk::CheckButton>()
                        || widget.is::<gtk::MenuButton>()
                    {
                        break;
                    }
                    picked = widget.parent();
                }
                gesture.set_state(gtk::EventSequenceState::Denied);
            }
        ));
        gesture.connect_drag_update(clone!(
            #[weak]
            overlay,
            #[weak]
            view,
            #[strong]
            pending,
            #[strong]
            state,
            move |gesture, _, dy| {
                if let Some(drag) = state.borrow_mut().as_mut() {
                    drag.pointer = drag.start_y + dy;
                    return;
                }
                if dy.abs() < 8.0 {
                    return;
                }
                if !view.playlist_searchentry().text().is_empty() {
                    return;
                }
                let Some((row, start_y)) = pending.borrow_mut().take() else {
                    return;
                };
                let (Some(song), Some(bounds)) = (row.song(), row.compute_bounds(&overlay)) else {
                    return;
                };
                let mut neighbours = Vec::new();
                rows(view.queue_view().upcast_ref(), &mut neighbours);
                let mut bounds_list: Vec<_> = neighbours
                    .iter()
                    .filter(|r| r.is_mapped())
                    .filter_map(|r| r.compute_bounds(&overlay))
                    .collect();
                bounds_list.sort_by(|a, b| a.y().total_cmp(&b.y()));
                let spacing = bounds_list
                    .windows(2)
                    .map(|p| (p[1].y() - p[0].y() - p[0].height()) as f64)
                    .filter(|s| (0.0..=8.0).contains(s))
                    .min_by(f64::total_cmp)
                    .unwrap_or(2.0);
                let stride = row.height() as f64 + spacing;
                let layer = gtk::Fixed::new();
                layer.set_can_target(false);
                layer.set_overflow(gtk::Overflow::Hidden);
                layer.set_hexpand(true);
                layer.set_vexpand(true);
                let Some(lifted) = card(&row) else {
                    return;
                };
                lifted.add_css_class("lifted-song");
                lifted.set_size_request(bounds.width() as i32, row.height());
                layer.put(&lifted, bounds.x() as f64, bounds.y() as f64);
                overlay.add_overlay(&layer);
                // The transparent overlay contains exactly one card. GTK
                // continues drawing every other row, including viewport edges.
                view.queue_view().add_css_class("reordering");
                gesture.set_state(gtk::EventSequenceState::Claimed);
                state.replace(Some(Reorder {
                    song,
                    layer,
                    lifted,
                    touched: Vec::new(),
                    grab: start_y - bounds.y() as f64,
                    pointer: start_y + dy,
                    start_y,
                    stride,
                    target: None,
                    settling: None,
                    target_frozen: false,
                    lifted_y: bounds.y() as f64,
                    last_tick: Instant::now(),
                    order: Rc::new(HashMap::new()),
                    gap: None,
                    scroll: view.scroll().vadjustment().value(),
                }));
            }
        ));
        gesture.connect_drag_end(clone!(
            #[strong]
            pending,
            #[strong]
            state,
            move |_, _, dy| {
                pending.take();
                if let Some(drag) = state.borrow_mut().as_mut() {
                    drag.pointer = drag.start_y + dy;
                    drag.settling = Some(Instant::now());
                }
            }
        ));
        gesture.connect_cancel(clone!(
            #[weak]
            overlay,
            #[weak]
            view,
            #[strong]
            pending,
            #[strong]
            state,
            move |_, _| {
                pending.take();
                if let Some(drag) = state.take() {
                    overlay.remove_overlay(&drag.layer);
                }
                view.queue_view().remove_css_class("reordering");
            }
        ));
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(clone!(
            #[weak]
            overlay,
            #[weak]
            view,
            #[weak]
            gesture,
            #[strong]
            state,
            #[strong]
            pending,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, _| {
                if key != gdk::Key::Escape || state.borrow().is_none() {
                    return glib::Propagation::Proceed;
                }
                if let Some(drag) = state.take() {
                    overlay.remove_overlay(&drag.layer);
                }
                pending.take();
                view.queue_view().remove_css_class("reordering");
                gesture.set_state(gtk::EventSequenceState::Denied);
                glib::Propagation::Stop
            }
        ));
        overlay.add_controller(keys);
        overlay.add_controller(gesture);
        overlay.connect_unmap(clone!(
            #[weak]
            view,
            #[strong]
            state,
            move |overlay| {
                if let Some(drag) = state.take() {
                    overlay.remove_overlay(&drag.layer);
                }
                view.queue_view().remove_css_class("reordering");
            }
        ));
        overlay.add_tick_callback(clone!(
            #[weak(rename_to = win)]
            self,
            #[weak]
            view,
            #[strong]
            state,
            #[strong]
            dirty,
            #[upgrade_or]
            glib::ControlFlow::Break,
            move |overlay, _| {
                let mut slot = state.borrow_mut();
                let Some(drag) = slot.as_mut() else {
                    return glib::ControlFlow::Continue;
                };
                let elapsed = drag.last_tick.elapsed().as_secs_f64();
                let dt = elapsed.min(0.05);
                drag.last_tick = Instant::now();
                if drag.settling.is_none() {
                    let scroll = view.scroll();
                    let adjustment = scroll.vadjustment();
                    let height = overlay.height() as f64;
                    let speed = if drag.pointer < 48.0 {
                        -(48.0 - drag.pointer).min(48.0) * 9.0
                    } else if drag.pointer > height - 48.0 {
                        (drag.pointer - height + 48.0).min(48.0) * 9.0
                    } else {
                        0.0
                    };
                    adjustment.set_value((adjustment.value() + speed * dt).clamp(
                        adjustment.lower(),
                        (adjustment.upper() - adjustment.page_size()).max(adjustment.lower()),
                    ));
                }
                let scroll = view.scroll().vadjustment().value();
                let scrolled = (scroll - drag.scroll).abs() > 0.1;
                drag.scroll = scroll;
                let mut visible = Vec::new();
                rows(view.queue_view().upcast_ref(), &mut visible);
                let Some(player) = win.player() else {
                    return glib::ControlFlow::Continue;
                };
                let queue = player.queue();
                if dirty.replace(false) || drag.order.is_empty() {
                    drag.order = Rc::new(
                        (0..queue.n_songs())
                            .filter_map(|i| queue.song_at(i).map(|song| (song.uri(), i)))
                            .collect(),
                    );
                }
                let order = drag.order.clone();
                let index = |song: &Song| order.get(&song.uri()).copied();
                let Some(from) = index(&drag.song) else {
                    let drag = slot.take().unwrap();
                    overlay.remove_overlay(&drag.layer);
                    view.queue_view().remove_css_class("reordering");
                    return glib::ControlFlow::Continue;
                };
                let mut visible: Vec<_> = visible
                    .into_iter()
                    .filter_map(|row| {
                        let song = row.song()?;
                        let bounds = row.compute_bounds(overlay)?;
                        (bounds.width() > 0.0
                            && bounds.height() > 0.0
                            && row.is_mapped()
                            && bounds.y() + bounds.height() >= -drag.stride as f32
                            && bounds.y() < overlay.height() as f32 + drag.stride as f32)
                            .then_some((row, song, bounds))
                    })
                    .collect();
                visible.sort_by(|a, b| a.2.y().total_cmp(&b.2.y()));
                // Include the final pointer event even if release precedes
                // the next frame. Then freeze the target while settling.
                if !drag.target_frozen {
                    let center =
                        drag.pointer - drag.grab + drag.lifted.height_request() as f64 / 2.0;
                    let mut to = insertion(
                        from,
                        center,
                        visible.iter().filter_map(|(_, song, bounds)| {
                            index(song)
                                .map(|pos| (pos, bounds.y() as f64 + bounds.height() as f64 / 2.0))
                        }),
                    );
                    // A tall card must still reach a shorter first/last row
                    // without requiring the pointer to leave the viewport.
                    let top = drag.pointer - drag.grab;
                    for (_, song, bounds) in &visible {
                        if index(song) == Some(0) && top <= bounds.y() as f64 {
                            to = 0;
                        } else if index(song) == Some(queue.n_songs() - 1)
                            && top + drag.lifted.height_request() as f64
                                >= (bounds.y() + bounds.height()) as f64
                        {
                            to = queue.n_songs() - 1;
                        }
                    }
                    drag.target = queue.song_at(to).map(|song| (song, to > from));
                    if drag.settling.is_some() {
                        drag.target_frozen = true;
                        drag.settling = Some(Instant::now());
                    }
                }
                let to = drag
                    .target
                    .as_ref()
                    .and_then(|(song, _)| index(song))
                    .unwrap_or(from);
                let animations =
                    gtk::Settings::default().is_some_and(|s| s.is_gtk_enable_animations());
                let gap = drag.gap.get_or_insert(from as f64);
                // Never animate the gap through distant rows or chase a
                // scrolling viewport: only the crossed neighbour slides.
                if !animations
                    || scrolled
                    || (to as f64 - *gap).abs() > 1.0
                    || drag
                        .settling
                        .is_some_and(|t| t.elapsed().as_millis() >= 120)
                {
                    *gap = to as f64;
                } else {
                    *gap += (to as f64 - *gap) * (1.0 - (-24.0 * elapsed).exp());
                }
                let gap = *gap;
                let mut landing = drag.lifted_y;
                for (row, song, bounds) in &visible {
                    let Some(pos) = index(song) else {
                        continue;
                    };
                    if pos == to {
                        landing = bounds.y() as f64
                            + if to > from {
                                bounds.height() as f64 - drag.lifted.height() as f64
                            } else {
                                0.0
                            };
                    }
                    if !drag.touched.iter().any(|(r, _, _)| r == row) {
                        if let Some(parent) = row.parent() {
                            let overflow = parent.overflow();
                            parent.set_overflow(gtk::Overflow::Visible);
                            drag.touched.push((row.clone(), parent, overflow));
                        }
                    }
                    row.set_drag_visual(
                        gap_offset(pos, from, gap, drag.stride),
                        song.equals(&drag.song),
                    );
                }
                let target_y = if drag.settling.is_some() {
                    landing
                } else {
                    (drag.pointer - drag.grab)
                        .clamp(0.0, (overlay.height() - drag.lifted.height()).max(0) as f64)
                };
                if drag.settling.is_some() {
                    drag.lifted_y += (target_y - drag.lifted_y) * (1.0 - (-32.0 * dt).exp());
                } else {
                    drag.lifted_y = target_y;
                }
                drag.layer.move_(
                    &drag.lifted,
                    visible.first().map(|v| v.2.x()).unwrap_or(6.0) as f64,
                    drag.lifted_y,
                );
                if drag
                    .settling
                    .is_some_and(|start| start.elapsed().as_millis() >= 150)
                {
                    let drag = slot.take().unwrap();
                    overlay.remove_overlay(&drag.layer);
                    view.queue_view().remove_css_class("reordering");
                    let song = drag.song.clone();
                    let target = drag.target.clone();
                    drop(drag);
                    if let Some((target, after)) = target {
                        if queue.move_song(&song, &target, after) {
                            if !win.in_for_you() {
                                utils::store_playlist(&queue);
                            }
                        }
                    }
                }
                glib::ControlFlow::Continue
            }
        ));
    }
}

#[cfg(test)]
impl Window {
    pub(super) async fn test_animated_reorder(&self) {
        self.switch_playback_queue(false);
        self.set_playlist_selection(false);
        self.imp().playlist_view.playlist_searchentry().set_text("");
        self.imp().player_stack.set_visible_child_name("player");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let player = self.player().unwrap();
        let queue = player.queue();
        let previous: Vec<_> = (0..queue.n_songs())
            .filter_map(|i| queue.song_at(i))
            .collect();
        queue.clear();
        let songs: Vec<_> = ["tone.wav", "tone.flac", "tone.mp3"]
            .iter()
            .map(|name| {
                Song::from_uri(
                    &gio::File::for_path(root.join("node/tests/fixtures").join(name)).uri(),
                )
                .unwrap()
            })
            .collect();
        queue.add_songs(&songs);
        queue.set_current_song(Some(songs[0].clone()));
        self.set_playlist_shuffled(true);
        assert!(queue.is_shuffled());
        let destination = queue.song_at(2).unwrap();
        self.set_default_size(1050, 800);
        self.imp().library_split.set_show_sidebar(false);
        self.imp().split_view.set_show_sidebar(true);
        self.imp().main_stack.set_visible_child_name("main-view");
        GtkWindowExt::present(self);
        self.imp()
            .playlist_view
            .queue_view()
            .scroll_to(0, gtk::ListScrollFlags::NONE, None);
        self.imp()
            .playlist_view
            .scroll()
            .vadjustment()
            .set_value(0.0);
        let wait = || glib::timeout_future(std::time::Duration::from_millis(300));
        glib::timeout_future_seconds(2).await;
        let view = self.imp().playlist_view.get();
        let overlay = view.drag_overlay();
        let mut visible = Vec::new();
        rows(view.queue_view().upcast_ref(), &mut visible);
        let source = visible
            .iter()
            .find(|r| r.song().is_some_and(|s| s.equals(&songs[0])))
            .unwrap();
        let target = visible
            .iter()
            .find(|r| r.song().is_some_and(|s| s.equals(&destination)))
            .unwrap();
        let a = source.compute_bounds(&overlay).unwrap();
        let b = target.compute_bounds(&overlay).unwrap();
        assert!(
            a.width() > 100.0 && b.y() > a.y(),
            "drag row bounds: {:?}, {:?}",
            a,
            b
        );
        let controllers = overlay.observe_controllers();
        let gesture = (0..controllers.n_items())
            .find_map(|i| {
                controllers
                    .item(i)
                    .and_then(|o| o.downcast::<gtk::GestureDrag>().ok())
            })
            .unwrap();
        let mut picked = overlay.pick(
            a.x() as f64 + 80.0,
            a.y() as f64 + 15.0,
            gtk::PickFlags::DEFAULT,
        );
        let mut chain = Vec::new();
        while let Some(w) = picked {
            chain.push(w.type_().name().to_string());
            picked = w.parent();
        }
        assert!(
            chain.iter().any(|name| name == "HopeaQueueRow"),
            "picked {:?}, bounds={:?} overlay={}x{}, mapped={} sensitive={} dialogs={}",
            chain,
            a,
            overlay.width(),
            overlay.height(),
            overlay.is_mapped(),
            overlay.is_sensitive(),
            self.visible_dialog().is_some()
        );
        gesture.emit_by_name::<()>(
            "drag-begin",
            &[&(a.x() as f64 + 80.0), &(a.y() as f64 + 15.0)],
        );
        let neighbour_song = queue.song_at(1).unwrap();
        let neighbour = visible
            .iter()
            .find(|r| r.song().is_some_and(|s| s.equals(&neighbour_song)))
            .unwrap();
        let n = neighbour.compute_bounds(&overlay).unwrap();
        let threshold = (n.y() + n.height() / 2.0 - a.y() - a.height() / 2.0) as f64;
        gesture.emit_by_name::<()>("drag-update", &[&0.0, &(threshold - 2.0)]);
        wait().await;
        let position = |song: &Song| {
            let row = visible
                .iter()
                .find(|r| r.song().is_some_and(|s| s.equals(song)))
                .unwrap();
            let rect = row.compute_bounds(&overlay).unwrap();
            graphene::Rect::new(
                rect.x(),
                rect.y() + row.drag_visual().0 as f32,
                rect.width(),
                rect.height(),
            )
        };
        assert!(
            (position(&neighbour_song).y() - n.y()).abs() < 1.0,
            "do not open a slot before crossing the neighbour's midpoint"
        );
        gesture.emit_by_name::<()>("drag-update", &[&0.0, &(threshold + 2.0)]);
        for _ in 0..30 {
            if (position(&neighbour_song).y() - a.y()).abs() < 1.0 {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
        }
        assert!((position(&neighbour_song).y() - a.y()).abs() < 1.0,
            "neighbour {:?}, source {:?}, original {:?}, last {:?}, pointer delta {}, parent height {}",
            position(&neighbour_song), a, n, b, threshold + 2.0, source.parent().unwrap().height());
        assert!(
            (position(&destination).y() - b.y()).abs() < 1.0,
            "moving past one neighbour must not skip the next track"
        );
        let dy = (b.y() + b.height() / 2.0 - a.y() - a.height() / 2.0) as f64 + 2.0;
        gesture.emit_by_name::<()>("drag-update", &[&0.0, &dy]);
        wait().await;
        assert!(
            view.queue_view().has_css_class("reordering"),
            "real rows must stay allocated while animated cards move"
        );
        assert_eq!(
            queue.song_at(0),
            Some(songs[0].clone()),
            "hover must not mutate the queue"
        );
        let layer = overlay
            .last_child()
            .unwrap()
            .downcast::<gtk::Fixed>()
            .unwrap();
        let mut cards = Vec::new();
        let mut child = layer.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Ok(picture) = widget.downcast::<gtk::Picture>() {
                cards.push(picture);
            }
        }
        assert_eq!(
            cards.len(),
            1,
            "only the grabbed row may be copied into an overlay"
        );
        let lifted = cards
            .iter()
            .find(|r| r.has_css_class("lifted-song"))
            .unwrap();
        assert_eq!(lifted.widget_name(), songs[0].uri());
        assert_eq!(lifted.height(), source.height());
        assert_eq!(lifted.width(), source.width());
        assert!(source.drag_visual().1);
        assert!(position(&neighbour_song).y() < b.y() - a.height());
        assert_eq!(source.height(), a.height() as i32);
        assert_eq!(source.width(), a.width() as i32);
        if let Some(path) = std::env::var_os("HOPEA_TEST_DRAG_SCREENSHOT") {
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gdk::RGBA::WHITE,
                &graphene::Rect::new(0.0, 0.0, overlay.width() as f32, overlay.height() as f32),
            );
            gtk::WidgetPaintable::new(Some(&overlay)).snapshot(
                &snapshot,
                overlay.width() as f64,
                overlay.height() as f64,
            );
            self.renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
        gesture.emit_by_name::<()>("drag-end", &[&0.0, &((b.y() - a.y()) as f64)]);
        for _ in 0..50 {
            if !view.queue_view().has_css_class("reordering") {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
        }
        assert!(!view.queue_view().has_css_class("reordering"));
        assert_eq!(queue.song_at(2), Some(songs[0].clone()));
        assert_eq!(queue.current_song(), Some(songs[0].clone()));
        assert!(
            queue.is_shuffled() && self.playlist_shuffled(),
            "dragging must keep Shuffle enabled"
        );
        // Exercise both edge rows, then Escape. Every real row must recover
        // its original rendering and the queue must remain unchanged on cancel.
        wait().await;
        let bounds_for = |song: &Song| {
            let mut current = Vec::new();
            rows(view.queue_view().upcast_ref(), &mut current);
            current
                .iter()
                .find(|r| r.song().is_some_and(|s| s.equals(song)))
                .unwrap()
                .compute_bounds(&overlay)
                .unwrap()
        };
        let last = bounds_for(&songs[0]);
        let first = bounds_for(&queue.song_at(0).unwrap());
        let up = (first.y() - last.y()) as f64 - 2.0;
        gesture.emit_by_name::<()>(
            "drag-begin",
            &[&(last.x() as f64 + 80.0), &(last.y() as f64 + 15.0)],
        );
        gesture.emit_by_name::<()>("drag-update", &[&0.0, &up]);
        wait().await;
        gesture.emit_by_name::<()>("drag-end", &[&0.0, &up]);
        for _ in 0..50 {
            if !view.queue_view().has_css_class("reordering") {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(queue.song_at(0), Some(songs[0].clone()));
        assert!(queue.is_shuffled());
        let before: Vec<_> = (0..queue.n_songs())
            .map(|i| queue.song_at(i).unwrap())
            .collect();
        let first = bounds_for(&songs[0]);
        gesture.emit_by_name::<()>(
            "drag-begin",
            &[&(first.x() as f64 + 80.0), &(first.y() as f64 + 15.0)],
        );
        gesture.emit_by_name::<()>("drag-update", &[&0.0, &100.0]);
        wait().await;
        let keys = (0..controllers.n_items())
            .find_map(|i| {
                controllers
                    .item(i)
                    .and_then(|o| o.downcast::<gtk::EventControllerKey>().ok())
            })
            .unwrap();
        let _: bool = keys.emit_by_name(
            "key-pressed",
            &[&gdk::Key::Escape, &0u32, &gdk::ModifierType::empty()],
        );
        wait().await;
        assert!(!view.queue_view().has_css_class("reordering"));
        assert_eq!(
            before,
            (0..queue.n_songs())
                .map(|i| queue.song_at(i).unwrap())
                .collect::<Vec<_>>()
        );
        let mut restored = Vec::new();
        rows(view.queue_view().upcast_ref(), &mut restored);
        assert!(restored.iter().all(|row| row.drag_visual() == (0.0, false)));

        // Real queues contain rows of different heights and recycle widgets
        // while scrolling. Test that case with animations enabled as well.
        let fixture_dir = crate::paths::data_dir().join("drag-fixtures");
        std::fs::create_dir_all(&fixture_dir).unwrap();
        let many: Vec<_> = (0..40)
            .map(|i| {
                let file = fixture_dir.join(format!("Artist - Song {i}.wav"));
                // These must be different recordings: identical copied files
                // are intentionally collapsed by the queue's duplicate guard.
                let mut audio = std::fs::read(root.join("node/tests/fixtures/tone.wav")).unwrap();
                *audio.last_mut().unwrap() ^= (i + 1) as u8;
                std::fs::write(&file, audio).unwrap();
                let song = Song::from_uri(&gio::File::for_path(file).uri()).unwrap();
                if i % 2 == 0 {
                    song.set_property("status", "");
                }
                song
            })
            .collect();
        self.set_playlist_shuffled(false);
        queue.clear();
        queue.add_songs(&many);
        assert_eq!(queue.n_songs(), 40);
        queue.set_current_song(Some(many[0].clone()));
        self.set_playlist_shuffled(true);
        self.set_playlist_visible(true);
        view.scroll().vadjustment().set_value(0.0);
        gtk::Settings::default()
            .unwrap()
            .set_gtk_enable_animations(true);
        glib::timeout_future_seconds(2).await;
        let mut visible = Vec::new();
        rows(view.queue_view().upcast_ref(), &mut visible);
        let source = visible
            .iter()
            .find(|r| r.song().is_some_and(|s| s.equals(&many[0])))
            .unwrap();
        let bounds = source.compute_bounds(&overlay).unwrap();
        assert!(
            bounds.y() >= 0.0 && bounds.y() < overlay.height() as f32,
            "second drag source is outside viewport: {:?}, height {}",
            bounds,
            overlay.height()
        );
        gesture.emit_by_name::<()>(
            "drag-begin",
            &[&(bounds.x() as f64 + 70.0), &(bounds.y() as f64 + 15.0)],
        );
        gesture.emit_by_name::<()>("drag-update", &[&0.0, &100.0]);
        wait().await;
        assert!(
            view.queue_view().has_css_class("reordering"),
            "second gesture must start: bounds {:?}, picked {:?}",
            bounds,
            overlay
                .pick(
                    bounds.x() as f64 + 70.0,
                    bounds.y() as f64 + 15.0,
                    gtk::PickFlags::DEFAULT
                )
                .map(|w| w.type_().name())
        );
        view.scroll().vadjustment().set_value(450.0);
        gesture.emit_by_name::<()>("drag-update", &[&0.0, &(overlay.height() as f64 - 20.0)]);
        glib::timeout_future_seconds(2).await;
        let layer = overlay
            .last_child()
            .unwrap()
            .downcast::<gtk::Fixed>()
            .unwrap();
        assert_eq!(layer.first_child(), layer.last_child());
        let mut current_rows = Vec::new();
        rows(view.queue_view().upcast_ref(), &mut current_rows);
        let mut positions = Vec::new();
        for row in &current_rows {
            let (offset, hidden) = row.drag_visual();
            if hidden || !row.is_mapped() {
                continue;
            }
            let rect = row.compute_bounds(&overlay).unwrap();
            let rect = graphene::Rect::new(
                rect.x(),
                rect.y() + offset as f32,
                rect.width(),
                rect.height(),
            );
            if rect.y() + rect.height() > 0.0 && rect.y() < overlay.height() as f32 {
                positions.push(rect);
            }
        }
        positions.sort_by(|a, b| a.y().total_cmp(&b.y()));
        assert!(positions.len() >= 3);
        for pair in positions.windows(2) {
            assert!(
                pair[0].y() + pair[0].height() <= pair[1].y() + 1.0,
                "animated neighbours must not overlap: {:?}",
                pair
            );
        }
        if let Some(path) = std::env::var_os("HOPEA_TEST_DRAG_LONG_SCREENSHOT") {
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gdk::RGBA::WHITE,
                &graphene::Rect::new(0.0, 0.0, overlay.width() as f32, overlay.height() as f32),
            );
            gtk::WidgetPaintable::new(Some(&overlay)).snapshot(
                &snapshot,
                overlay.width() as f64,
                overlay.height() as f64,
            );
            self.renderer()
                .unwrap()
                .render_texture(&snapshot.to_node().unwrap(), None)
                .save_to_png(path)
                .unwrap();
        }
        let gaps: Vec<_> = positions
            .windows(2)
            .map(|p| p[1].y() - p[0].y() - p[0].height())
            .collect();
        let extra_space: f32 = gaps.iter().map(|gap| (gap - 4.0).max(0.0)).sum();
        assert!(
            extra_space <= bounds.height() + 6.0,
            "real rows must have only one slot, not gaps between every track: {:?}",
            gaps
        );
        assert!(
            gaps.iter().filter(|&&gap| gap > 8.0).count() <= 1,
            "only rows next to the moving slot may separate"
        );
        gesture.emit_by_name::<()>("drag-end", &[&0.0, &(overlay.height() as f64 - 20.0)]);
        for _ in 0..50 {
            if !view.queue_view().has_css_class("reordering") {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(100)).await;
        }
        assert!(!view.queue_view().has_css_class("reordering"));
        assert!(queue.is_shuffled() && self.playlist_shuffled());
        assert_eq!(queue.current_song(), Some(many[0].clone()));
        gtk::Settings::default()
            .unwrap()
            .set_gtk_enable_animations(false);
        self.set_playlist_shuffled(false);
        queue.clear();
        queue.add_songs(&previous);
        self.imp().split_view.set_show_sidebar(false);
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::{gap_offset, insertion};

    #[test]
    fn uneven_rows_cross_one_midpoint_at_a_time_in_both_directions() {
        let rows = [(0, 36.0), (1, 92.0), (2, 150.0), (3, 218.0)];
        assert_eq!(insertion(0, 91.9, rows.iter().copied()), 0);
        assert_eq!(insertion(0, 92.1, rows.iter().copied()), 1);
        assert_eq!(insertion(0, 149.9, rows.iter().copied()), 1);
        assert_eq!(insertion(0, 150.1, rows.iter().copied()), 2);
        assert_eq!(insertion(3, 150.1, rows.iter().copied()), 3);
        assert_eq!(insertion(3, 149.9, rows.iter().copied()), 2);
        assert_eq!(insertion(3, 92.1, rows.iter().copied()), 2);
        assert_eq!(insertion(3, 91.9, rows.iter().copied()), 1);
    }

    #[test]
    fn animation_never_opens_more_than_one_slot_or_overlaps_uneven_rows() {
        let heights = [72.0, 40.0, 68.0, 52.0, 72.0];
        let mut tops = Vec::new();
        let mut y = 0.0;
        for height in heights {
            tops.push(y);
            y += height + 4.0;
        }
        for from in 0..heights.len() {
            let stride = heights[from] + 4.0;
            for step in 0..=400 {
                let gap = step as f64 / 100.0;
                let rects: Vec<_> = (0..heights.len())
                    .filter(|&i| i != from)
                    .map(|i| {
                        (
                            tops[i] + gap_offset(i as u32, from as u32, gap, stride),
                            heights[i],
                        )
                    })
                    .collect();
                let mut empty = 0.0;
                for pair in rects.windows(2) {
                    let space = pair[1].0 - pair[0].0 - pair[0].1 - 4.0;
                    assert!(space >= -0.001, "neighbouring rows must not overlap");
                    empty += space;
                }
                assert!(empty <= stride + 0.001, "only one slot may be open");
            }
        }
    }
}
