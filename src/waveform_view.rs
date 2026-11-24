// SPDX-FileCopyrightText: 2022  Emmanuele Bassi
// SPDX-License-Identifier: GPL-3.0-or-later

// Based on gnome-sound-recorder/src/waveform.js:
// - Copyright 2013 Meg Ford
// - Copyright 2022 Kavan Mevada
// Released under the terms of the LGPL 2.0 or later

#![allow(deprecated)]

use std::{
    cell::{Cell, RefCell},
    ops::DivAssign,
};

use adw::subclass::prelude::*;
use glib::clone;
use gtk::{gdk, glib, graphene, prelude::*};
use log::debug;

#[derive(Debug, PartialEq)]
pub struct PeakPair {
    pub left: f64,
    pub right: f64,
}

impl PeakPair {
    pub fn new(left: f64, right: f64) -> Self {
        Self { left, right }
    }
}

impl DivAssign<f64> for PeakPair {
    fn div_assign(&mut self, rhs: f64) {
        self.left /= rhs;
        self.right /= rhs;
    }
}

mod imp {
    use glib::{subclass::Signal, ParamSpec, ParamSpecDouble, Value};
    use once_cell::sync::Lazy;

    use super::*;

    #[derive(Debug, Default)]
    pub struct WaveformView {
        pub position: Cell<f64>,
        pub hover_position: Cell<Option<f64>>,
        // left and right channel peaks, normalised between 0 and 1
        pub peaks: RefCell<Option<Vec<PeakPair>>>,
        pub tick_id: RefCell<Option<gtk::TickCallbackId>>,
        pub first_frame_time: Cell<Option<i64>>,
        pub factor: Cell<Option<f64>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WaveformView {
        const NAME: &'static str = "HopeaWaveformView";
        type Type = super::WaveformView;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("waveformview");
            klass.set_accessible_role(gtk::AccessibleRole::Slider);
        }
    }

    impl ObjectImpl for WaveformView {
        fn properties() -> &'static [ParamSpec] {
            static PROPERTIES: Lazy<Vec<ParamSpec>> = Lazy::new(|| {
                vec![ParamSpecDouble::builder("position")
                    .minimum(0.0)
                    .maximum(1.0)
                    .default_value(0.0)
                    .build()]
            });

            PROPERTIES.as_ref()
        }

        fn set_property(&self, _id: usize, value: &Value, pspec: &ParamSpec) {
            match pspec.name() {
                "position" => self.position.replace(value.get::<f64>().unwrap()),
                _ => unimplemented!(),
            };
        }

        fn property(&self, _id: usize, pspec: &ParamSpec) -> Value {
            match pspec.name() {
                "position" => self.position.get().to_value(),
                _ => unimplemented!(),
            }
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: Lazy<Vec<Signal>> = Lazy::new(|| {
                vec![Signal::builder("position-changed")
                    .param_types([f64::static_type()])
                    .build()]
            });

            SIGNALS.as_ref()
        }

        fn constructed(&self) {
            self.parent_constructed();

            self.obj().set_focusable(true);

            self.obj().setup_gesture();

            self.obj()
                .upcast_ref::<gtk::Accessible>()
                .update_property(&[
                    (gtk::accessible::Property::ValueMin(0.0)),
                    (gtk::accessible::Property::ValueMax(1.0)),
                    (gtk::accessible::Property::ValueNow(0.0)),
                ]);
        }

        fn dispose(&self) {
            if let Some(tick_id) = self.tick_id.replace(None) {
                tick_id.remove();
            }
        }
    }

    impl WidgetImpl for WaveformView {
        fn focus(&self, direction: gtk::DirectionType) -> bool {
            debug!("WaveformView::focus({:?})", direction);
            let widget = self.obj();
            if !widget.is_focus() {
                widget.grab_focus();
                return true;
            }

            let pos = self.position.get();

            match direction {
                gtk::DirectionType::Left if pos == 0.0 => false,
                gtk::DirectionType::Left if pos > 0.0 => true,
                gtk::DirectionType::Right if pos < 1.0 => true,
                gtk::DirectionType::Right if pos == 1.0 => false,
                _ => false,
            }
        }

        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => {
                    // We ask for as many samples we can fit within a 256 pixels wide area
                    if let Some(ref peaks) = *self.peaks.borrow() {
                        let n_peaks = peaks.len() as i32;
                        let width = i32::min(n_peaks * 4, 256);

                        (width, width, -1, -1)
                    } else {
                        (256, 256, -1, -1)
                    }
                }
                gtk::Orientation::Vertical => (48, 48, -1, -1),
                _ => (0, 0, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let w = widget.width();
            let h = widget.height();
            if w == 0 || h == 0 {
                return;
            }

            // Our reference line
            let center_y = h as f32 / 2.0;

            // Grab the colors
            let hc = adw::StyleManager::default().is_high_contrast();

            let style_context = widget.style_context();
            let color = style_context.color();
            let empty_opacity = if hc { 0.4 } else { 0.2 };
            let hover_opacity = if hc { 0.7 } else { 0.45 };

            let empty_color = gdk::RGBA::new(
                color.red(),
                color.green(),
                color.blue(),
                color.alpha() * empty_opacity,
            );

            let is_rtl = widget.direction() == gtk::TextDirection::Rtl;
            let bar_size = 2;
            let space_size = 2;
            let block_size = bar_size + space_size;
            let available_width = w;

            if let Some(ref peaks) = *self.peaks.borrow() {
                let n_peaks = peaks.len() as i32;
                let waveform_width = w as f64;

                // We have two cursors:
                //
                // 1. the state position, updated by the player
                // 2. the hover position, updated by the motion controller
                //
                // The hover position may be behind the state position, if we are
                // scrubbing backwards; or after the state position, if we are
                // scrubbing forward.
                //
                // The area between the state position and the hover position is
                // meant to be shown as a dimmed cursor color; the area between
                // the start of the waveform and the state position is meant to be
                // shown as a full cursor color; and the area between the hover
                // position and the end of the waveform is meant to be shown as a
                // current foreground color.
                let position = if is_rtl {
                    1.0 - self.position.get()
                } else {
                    self.position.get()
                };
                let mut cursor_pos: [f64; 2] =
                    [position * waveform_width, position * waveform_width];
                if let Some(hover) = self.hover_position.get() {
                    if is_rtl {
                        if hover <= position {
                            cursor_pos[1] = hover * waveform_width;
                        } else {
                            cursor_pos[0] = hover * waveform_width;
                        }
                    } else if hover <= position {
                        cursor_pos[0] = hover * waveform_width;
                    } else {
                        cursor_pos[1] = hover * waveform_width;
                    }
                }

                // If the number of samples is too big to fit into the available
                // width, we average the samples that fit within a bar
                let pixels_per_sample = if available_width > n_peaks * block_size {
                    block_size as f64
                } else {
                    (available_width as f64 / 2.0) / n_peaks as f64
                };

                let mut current_pixel = 0.0;
                let mut samples_in_accum = 0;
                let mut accum = PeakPair::new(0.0, 0.0);
                let mut offset = if is_rtl { waveform_width } else { 0.0 };

                for (i, sample) in peaks.iter().enumerate() {
                    current_pixel += pixels_per_sample;
                    samples_in_accum += 1;
                    accum.left += sample.left;
                    accum.right += sample.right;
                    if current_pixel > bar_size as f64 || i == peaks.len() - 1 {
                        accum /= samples_in_accum as f64;

                        // Scale by half: left goes in the upper half of the
                        // available space, and right goes in the lower half
                        let mut left = accum.left / 2.0;
                        let mut right = accum.right / 2.0;

                        // We optionally apply the scaling factor computed
                        // during the animation
                        if let Some(factor) = self.factor.get() {
                            left *= factor.clamp(0.0, 1.0);
                            right *= factor.clamp(0.0, 1.0);
                        }

                        // The block rectangle, clamped to avoid overdrawing
                        let block = {
                            let x = if is_rtl {
                                offset as f32 - bar_size as f32
                            } else {
                                offset as f32
                            };
                            let y =
                                f32::clamp(center_y - right as f32 * h as f32, 0.0, h as f32 / 2.0);
                            let width: f32 = 2.0;
                            let height = f32::clamp(
                                right as f32 * h as f32 + left as f32 * h as f32,
                                2.0,
                                h as f32,
                            );

                            graphene::Rect::new(x, y, width, height)
                        };

                        if is_rtl {
                            if offset > cursor_pos[0] {
                                snapshot.append_color(&color, &block);
                            } else if offset > cursor_pos[1] {
                                let hover_color = gdk::RGBA::new(
                                    color.red(),
                                    color.green(),
                                    color.blue(),
                                    color.alpha() * hover_opacity,
                                );
                                snapshot.append_color(&hover_color, &block);
                            } else {
                                snapshot.append_color(&empty_color, &block);
                            }
                        } else if offset < cursor_pos[0] {
                            snapshot.append_color(&color, &block);
                        } else if offset < cursor_pos[1] {
                            let hover_color = gdk::RGBA::new(
                                color.red(),
                                color.green(),
                                color.blue(),
                                color.alpha() * hover_opacity,
                            );
                            snapshot.append_color(&hover_color, &block);
                        } else {
                            snapshot.append_color(&empty_color, &block);
                        }

                        accum.left = 0.0;
                        accum.right = 0.0;
                        samples_in_accum = 0;
                        current_pixel -= bar_size as f64;

                        if is_rtl {
                            offset -= block_size as f64;
                        } else {
                            offset += block_size as f64;
                        }
                    }
                }
            } else {
                let mut offset = space_size;
                while offset < w - space_size {
                    let x = offset as f32;
                    let y = center_y - 1.0;
                    let width = bar_size as f32;
                    let height: f32 = 2.0;
                    snapshot.append_color(&color, &graphene::Rect::new(x, y, width, height));

                    offset += block_size;
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct WaveformView(ObjectSubclass<imp::WaveformView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

fn ease_out_cubic(t: f64) -> f64 {
    let p = t - 1.0;
    p * p * p + 1.0
}

impl Default for WaveformView {
    fn default() -> Self {
        glib::Object::new()
    }
}

const ANIMATION_USECS: f64 = 250_000.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a GTK display and temporary XDG directories"]
    fn waveform_updates_do_not_restart_reveal() {
        gtk::init().unwrap();
        let view = WaveformView::new();
        view.settings().set_gtk_enable_animations(true);
        let peaks = vec![(0.25, 0.5), (0.5, 0.0)];
        view.set_peaks(Some(peaks.clone()));
        assert_eq!(view.imp().factor.get(), Some(0.0));
        assert_eq!(view.animate_frame(1_000_000), glib::ControlFlow::Continue);
        view.animate_frame(1_125_000);
        let factor = view.imp().factor.get();
        for _ in 0..100 {
            view.set_peaks(Some(peaks.clone()));
            view.set_position(0.5);
            assert_eq!(view.imp().factor.get(), factor);
            assert_eq!(view.imp().first_frame_time.get(), Some(1_000_000));
        }
        // Removing the callback here substitutes for GTK consuming Break.
        view.imp().tick_id.take().unwrap().remove();
        assert_eq!(view.animate_frame(1_250_000), glib::ControlFlow::Break);
        for _ in 0..100 {
            view.set_peaks(Some(peaks.clone()));
            assert!(view.imp().tick_id.borrow().is_none());
            assert!(view.imp().factor.get().is_none());
        }
        // Improved data for the current song also remains fully visible.
        view.set_peaks(Some(vec![(0.1, 0.2), (0.5, 0.5)]));
        assert!(view.imp().tick_id.borrow().is_none());
        assert!(view.imp().factor.get().is_none());
        // A different song can clear the drawing and reveal it once again.
        view.set_peaks(None);
        assert!(view.imp().peaks.borrow().is_none());
        view.set_peaks(Some(peaks));
        assert_eq!(view.imp().factor.get(), Some(0.0));
        view.animate_frame(2_000_000);
        view.imp().tick_id.take().unwrap().remove();
        assert_eq!(view.animate_frame(1_999_999), glib::ControlFlow::Break);
        assert!(view.imp().factor.get().is_none());
        // Silent/invalid channels must never introduce NaN drawing coordinates.
        let normalized = view.normalize_peaks(vec![(0.0, 1.0), (f64::NAN, 0.5)]);
        assert_eq!(
            normalized,
            vec![PeakPair::new(0.0, 1.0), PeakPair::new(0.0, 0.5)]
        );
        view.set_peaks(Some(Vec::new()));
        assert!(view.imp().peaks.borrow().is_none());
        view.settings().set_gtk_enable_animations(false);
        view.set_peaks(Some(vec![(1.0, 1.0)]));
        assert!(view.imp().tick_id.borrow().is_none());
        assert!(view.imp().factor.get().is_none());
    }
}

impl WaveformView {
    pub fn new() -> Self {
        Self::default()
    }

    fn setup_gesture(&self) {
        let drag_gesture = gtk::GestureDrag::new();
        drag_gesture.set_name(Some("waveform-drag"));
        drag_gesture.set_button(0);
        drag_gesture.connect_drag_begin(clone!(
            #[strong(rename_to = this)]
            self,
            move |gesture, start_x, _| {
                if !this.has_focus() {
                    this.grab_focus();
                }
                gesture.set_state(gtk::EventSequenceState::Claimed);
                this.seek_to_coord(start_x);
            }
        ));
        drag_gesture.connect_drag_update(clone!(
            #[strong(rename_to = this)]
            self,
            move |gesture, offset_x, _| {
                if !this.has_focus() {
                    this.grab_focus();
                }
                gesture.set_state(gtk::EventSequenceState::Claimed);
                this.seek_to_coord(gesture.start_point().unwrap().0 + offset_x);
            }
        ));
        self.add_controller(drag_gesture);

        let motion_gesture = gtk::EventControllerMotion::new();
        motion_gesture.set_name(Some("waveform-motion"));
        motion_gesture.connect_motion(clone!(
            #[strong(rename_to = this)]
            self,
            move |_, x, _| {
                let width = this.width() as f64;
                let position = x / width;
                this.imp().hover_position.replace(Some(position));
                this.queue_draw();
            }
        ));
        motion_gesture.connect_leave(clone!(
            #[strong(rename_to = this)]
            self,
            move |_| {
                this.imp().hover_position.replace(None);
                this.queue_draw();
            }
        ));
        self.add_controller(motion_gesture);

        let key_controller = gtk::EventControllerKey::new();
        key_controller.set_name(Some("waveform-key"));
        key_controller.connect_key_released(clone!(
            #[strong(rename_to = this)]
            self,
            move |_, keyval, _, _| {
                let delta = match keyval {
                    gdk::Key::Left => -0.05,
                    gdk::Key::Right => 0.05,
                    _ => return,
                };

                let position = this.imp().position.get() + delta;
                this.emit_by_name::<()>("position-changed", &[&position]);
            }
        ));
        self.add_controller(key_controller);
    }

    fn seek_to_coord(&self, pos: f64) {
        let width = self.width();
        let position = match self.direction() {
            gtk::TextDirection::Rtl => 1.0 - (pos / width as f64),
            _ => pos / width as f64,
        };
        debug!(
            "Seeking to coord {} (width: {}, position: {})",
            pos, width, position
        );
        self.emit_by_name::<()>("position-changed", &[&position]);
    }

    fn normalize_peaks(&self, peaks: Vec<(f64, f64)>) -> Vec<PeakPair> {
        let amplitude = |value: f64| {
            if value.is_finite() {
                value.max(0.0)
            } else {
                0.0
            }
        };
        let max_left = peaks.iter().map(|p| amplitude(p.0)).fold(0.0, f64::max);
        let max_right = peaks.iter().map(|p| amplitude(p.1)).fold(0.0, f64::max);

        let normalized: Vec<PeakPair> = peaks
            .iter()
            .map(|p| {
                PeakPair::new(
                    if max_left > 0.0 {
                        amplitude(p.0) / max_left
                    } else {
                        0.0
                    },
                    if max_right > 0.0 {
                        amplitude(p.1) / max_right
                    } else {
                        0.0
                    },
                )
            })
            .collect();

        debug!("Peaks: {}", normalized.len());
        normalized
    }

    pub fn set_peaks(&self, peaks: Option<Vec<(f64, f64)>>) {
        let peak_pairs = peaks
            .filter(|p| !p.is_empty())
            .map(|p| self.normalize_peaks(p));
        // Catalog/cache notifications may repeat the same waveform. In
        // particular, do not restart an in-progress reveal on those updates.
        if *self.imp().peaks.borrow() == peak_pairs {
            return;
        }
        let reveal = self.imp().peaks.borrow().is_none() && peak_pairs.is_some();
        if let Some(tick_id) = self.imp().tick_id.replace(None) {
            tick_id.remove();
        }
        self.imp().peaks.replace(peak_pairs);
        self.imp().factor.set(None);
        self.imp().first_frame_time.set(None);
        // A refreshed waveform replaces the drawing in place. Collapsing an
        // already available waveform makes background updates look like blinking.
        if !reveal || !self.settings().is_gtk_enable_animations() {
            self.queue_resize();
            self.queue_draw();
            return;
        }
        self.imp().factor.set(Some(0.0));

        let tick_id = self.add_tick_callback(clone!(
            #[strong(rename_to = this)]
            self,
            move |_, clock| this.animate_frame(clock.frame_time())
        ));

        self.imp().tick_id.replace(Some(tick_id));
        self.queue_resize();
    }

    fn animate_frame(&self, frame_time: i64) -> glib::ControlFlow {
        let first = self.imp().first_frame_time.get().unwrap_or(frame_time);
        self.imp().first_frame_time.set(Some(first));
        let elapsed = frame_time.saturating_sub(first);
        // Finish on elapsed time, not an easing value exceeding its range.
        // A discontinuous platform clock must not leave a permanent callback.
        if elapsed < 0 || elapsed as f64 >= ANIMATION_USECS {
            self.imp().factor.set(None);
            self.imp().first_frame_time.set(None);
            self.imp().tick_id.take();
            self.queue_draw();
            return glib::ControlFlow::Break;
        }
        self.imp()
            .factor
            .set(Some(ease_out_cubic(elapsed as f64 / ANIMATION_USECS)));
        self.queue_draw();
        glib::ControlFlow::Continue
    }

    pub fn set_position(&self, position: f64) {
        let pos = position.clamp(0.0, 1.0);
        self.imp().position.replace(pos);
        self.update_property(&[gtk::accessible::Property::ValueNow(pos)]);
        self.queue_draw();
    }
}
