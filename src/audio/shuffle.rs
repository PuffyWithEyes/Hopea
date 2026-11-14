// SPDX-FileCopyrightText: 2022  Emmanuele Bassi
// SPDX-License-Identifier: GPL-3.0-or-later

use std::cell::RefCell;

use glib::clone;
use gtk::{gio, glib, prelude::*, subclass::prelude::*};
use rand::prelude::*;

mod imp {
    use glib::{ParamSpec, ParamSpecObject, Value};
    use once_cell::sync::Lazy;

    use super::*;

    #[derive(Debug, Default)]
    pub struct ShuffleListModel {
        pub model: RefCell<Option<gio::ListModel>>,
        pub shuffle: RefCell<Option<Vec<u32>>>,
        pub changed: RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ShuffleListModel {
        const NAME: &'static str = "ShuffleListModel";
        type Type = super::ShuffleListModel;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for ShuffleListModel {
        fn dispose(&self) {
            if let (Some(model), Some(handler)) = (self.model.take(), self.changed.take()) {
                model.disconnect(handler);
            }
        }
        fn properties() -> &'static [ParamSpec] {
            static PROPERTIES: Lazy<Vec<ParamSpec>> = Lazy::new(|| {
                vec![ParamSpecObject::builder::<gio::ListModel>("model")
                    .explicit_notify()
                    .build()]
            });

            PROPERTIES.as_ref()
        }

        fn property(&self, _id: usize, pspec: &ParamSpec) -> Value {
            match pspec.name() {
                "model" => self.model.borrow().to_value(),
                _ => unimplemented!(),
            }
        }

        fn set_property(&self, _id: usize, value: &Value, pspec: &ParamSpec) {
            match pspec.name() {
                "model" => self
                    .obj()
                    .set_model(value.get::<gio::ListModel>().ok().as_ref()),
                _ => unimplemented!(),
            };
        }
    }

    impl ListModelImpl for ShuffleListModel {
        fn item_type(&self) -> glib::Type {
            if let Some(ref model) = *self.model.borrow() {
                return model.item_type();
            }

            glib::Object::static_type()
        }

        fn n_items(&self) -> u32 {
            if let Some(ref model) = *self.model.borrow() {
                return model.n_items();
            }

            0
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            if let Some(ref model) = *self.model.borrow() {
                if let Some(ref shuffle) = *self.shuffle.borrow() {
                    return shuffle
                        .get(position as usize)
                        .and_then(|pos| model.item(*pos));
                }
                return model.item(position);
            }

            None
        }
    }
}

glib::wrapper! {
    pub struct ShuffleListModel(ObjectSubclass<imp::ShuffleListModel>)
        @implements gio::ListModel;
}

impl Default for ShuffleListModel {
    fn default() -> Self {
        Self::new(gio::ListModel::NONE)
    }
}

impl ShuffleListModel {
    pub fn new(model: Option<&impl IsA<gio::ListModel>>) -> Self {
        glib::Object::builder::<Self>()
            .property("model", model.map(|m| m.as_ref()))
            .build()
    }

    pub fn model(&self) -> Option<gio::ListModel> {
        self.imp().model.borrow().as_ref().cloned()
    }

    pub fn set_model(&self, model: Option<&gio::ListModel>) {
        let old_count = self.n_items();
        if let (Some(old), Some(handler)) = (self.imp().model.take(), self.imp().changed.take()) {
            old.disconnect(handler);
        }
        self.imp().shuffle.replace(None);
        if let Some(model) = model {
            self.imp().model.replace(Some(model.clone()));
            let handler = model.connect_items_changed(clone!(
                #[weak(rename_to = this)]
                self,
                move |_, position, removed, added| {
                    this.source_changed(position, removed, added);
                }
            ));
            self.imp().changed.replace(Some(handler));
        }
        self.items_changed(0, old_count, self.n_items());
        self.notify("model");
    }

    fn source_changed(&self, position: u32, removed: u32, added: u32) {
        let change = {
            let mut state = self.imp().shuffle.borrow_mut();
            if let Some(order) = state.as_mut() {
                let old_count = order.len() as u32;
                let end = position + removed;
                order.retain(|&p| p < position || p >= end);
                for p in order.iter_mut() {
                    if *p >= end {
                        *p = *p - removed + added;
                    }
                }
                // New imports join the end without reshuffling existing songs.
                order.extend(position..position + added);
                if removed == 0 {
                    (old_count, 0, added)
                } else {
                    (0, old_count, order.len() as u32)
                }
            } else {
                (position, removed, added)
            }
        };
        // No RefCell borrow may survive a synchronous model notification.
        self.items_changed(change.0, change.1, change.2);
    }

    /// `anchor` is an index in the backing store, not the displayed permutation.
    pub fn reshuffle(&self, anchor: Option<u32>) {
        let count = self.n_items();
        let positions = shuffled_positions(count, anchor, &mut rand::rng());
        self.imp().shuffle.replace(Some(positions));
        if count > 0 {
            self.items_changed(0, count, count);
        }
    }

    /// Adjust the current permutation without switching shuffle off or
    /// changing the underlying (unshuffled) playlist.
    pub fn move_shuffled(&self, from: u32, to: u32) -> bool {
        let count = self.n_items();
        if from >= count || to >= count || from == to {
            return false;
        }
        {
            let mut state = self.imp().shuffle.borrow_mut();
            let Some(order) = state.as_mut() else {
                return false;
            };
            let item = order.remove(from as usize);
            order.insert(to as usize, item);
        }
        let first = from.min(to);
        let changed = from.max(to) - first + 1;
        self.items_changed(first, changed, changed);
        true
    }

    pub fn unshuffle(&self) {
        let count = self.n_items();
        self.imp().shuffle.replace(None);
        if count > 0 {
            self.items_changed(0, count, count);
        }
    }
}

fn shuffled_positions(count: u32, anchor: Option<u32>, rng: &mut impl Rng) -> Vec<u32> {
    let anchor = anchor.filter(|&pos| pos < count);
    let mut order: Vec<_> = (0..count).filter(|pos| Some(*pos) != anchor).collect();
    order.shuffle(rng);
    if let Some(anchor) = anchor {
        order.insert(0, anchor);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::StdRng, SeedableRng};

    #[test]
    fn current_song_is_first_and_both_sides_can_be_next() {
        for anchor in [0, 5, 9] {
            let mut next = std::collections::HashSet::new();
            for seed in 0..128 {
                let order = shuffled_positions(10, Some(anchor), &mut StdRng::seed_from_u64(seed));
                assert_eq!(order[0], anchor);
                next.insert(order[1]);
                let mut sorted = order.clone();
                sorted.sort_unstable();
                assert_eq!(sorted, (0..10).collect::<Vec<_>>());
            }
            assert_eq!(
                next.len(),
                9,
                "songs above and below the anchor must participate"
            );
        }
        assert!(shuffled_positions(0, Some(0), &mut StdRng::seed_from_u64(1)).is_empty());
        assert_eq!(
            shuffled_positions(1, Some(0), &mut StdRng::seed_from_u64(1)),
            [0]
        );
        let mut firsts = std::collections::HashSet::new();
        for seed in 0..128 {
            firsts.insert(shuffled_positions(10, None, &mut StdRng::seed_from_u64(seed))[0]);
        }
        assert_eq!(firsts.len(), 10, "without playback no first song is pinned");
    }

    fn names(model: &impl IsA<gio::ListModel>) -> Vec<String> {
        (0..model.as_ref().n_items())
            .map(|i| {
                model
                    .as_ref()
                    .item(i)
                    .unwrap()
                    .downcast::<gio::SimpleAction>()
                    .unwrap()
                    .name()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn source_edits_preserve_the_permutation_and_valid_notifications() {
        let store = gio::ListStore::new::<gio::SimpleAction>();
        for n in 0..10 {
            store.append(&gio::SimpleAction::new(&format!("song{n}"), None));
        }
        let model = ShuffleListModel::new(Some(&store));
        model.reshuffle(Some(5));
        let previous_count = std::rc::Rc::new(std::cell::Cell::new(10));
        let observed = previous_count.clone();
        model.connect_items_changed(move |model, pos, removed, added| {
            assert!(pos + removed <= observed.get());
            assert_eq!(model.n_items(), observed.get() - removed + added);
            observed.set(model.n_items());
            let items = names(model);
            let unique: std::collections::HashSet<_> = items.iter().collect();
            assert_eq!(items.len(), unique.len());
        });
        let before = names(&model);
        store.insert(2, &gio::SimpleAction::new("imported", None));
        let mut expected = before.clone();
        expected.push("imported".into());
        assert_eq!(names(&model), expected);
        store.remove(0);
        expected.retain(|s| s != "song0");
        assert_eq!(names(&model), expected);
        model.unshuffle();
        assert_eq!(names(&model), names(&store));
        model.reshuffle(Some(0));
        store.remove_all();
        assert_eq!(model.n_items(), 0);
        store.append(&gio::SimpleAction::new("new", None));
        assert_eq!(names(&model), ["new"]);
    }

    #[test]
    fn replacing_source_disconnects_old_notifications() {
        let old = gio::ListStore::new::<gio::SimpleAction>();
        let new = gio::ListStore::new::<gio::SimpleAction>();
        let model = ShuffleListModel::new(Some(&old));
        let signals = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = signals.clone();
        model.connect_items_changed(move |_, _, _, _| seen.set(seen.get() + 1));
        model.set_model(Some(new.upcast_ref()));
        let before = signals.get();
        old.append(&gio::SimpleAction::new("old", None));
        assert_eq!(signals.get(), before);
        new.append(&gio::SimpleAction::new("new", None));
        assert_eq!(names(&model), ["new"]);
    }
}
