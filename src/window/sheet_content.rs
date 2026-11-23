// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! A sheet requests the available height without imposing that as its minimum.
//! A height-request on the panel would prevent Android from shrinking the
//! window when the keyboard opens, and would clip controls in landscape.

use gtk::{glib, prelude::*, subclass::prelude::*};
use std::cell::{Cell, RefCell};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SheetContent {
        pub child: RefCell<Option<gtk::Widget>>,
        pub available_height: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SheetContent {
        const NAME: &'static str = "HopeaSheetContent";
        type Type = super::SheetContent;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SheetContent {
        fn dispose(&self) {
            if let Some(child) = self.child.borrow_mut().take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for SheetContent {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.child.borrow().clone() else {
                return (0, 0, -1, -1);
            };
            let (minimum, natural, min_baseline, nat_baseline) =
                child.measure(orientation, for_size);
            if orientation == gtk::Orientation::Vertical && self.available_height.get() > 0 {
                (minimum, minimum.max(self.available_height.get()), -1, -1)
            } else {
                (minimum, natural, min_baseline, nat_baseline)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.child.borrow().as_ref() {
                child.allocate(width, height, baseline, None);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(child) = self.child.borrow().as_ref() {
                self.obj().snapshot_child(child, snapshot);
            }
        }
    }
}

glib::wrapper! {
    pub struct SheetContent(ObjectSubclass<imp::SheetContent>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for SheetContent {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl SheetContent {
    pub fn set_child(&self, child: Option<&gtk::Widget>) {
        let imp = self.imp();
        if imp.child.borrow().as_ref() == child {
            return;
        }
        if let Some(old) = imp.child.borrow_mut().take() {
            old.unparent();
        }
        if let Some(child) = child {
            child.set_parent(self);
        }
        imp.child.replace(child.cloned());
        self.queue_resize();
    }

    pub fn set_available_height(&self, height: i32) {
        let height = height.max(0);
        if self.imp().available_height.replace(height) != height {
            self.queue_resize();
        }
    }
}
