// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::{i18n::i18n, window::Window};
use adw::prelude::*;
use gtk::glib;
use std::{
    cell::Cell,
    rc::{Rc, Weak},
};

pub fn actions() -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .min_children_per_line(1)
        .max_children_per_line(3)
        .row_spacing(8)
        .column_spacing(8)
        .css_classes(["action-wrap"])
        .build()
}

pub fn scroll(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(child)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build()
}

struct Form {
    dialog: adw::Dialog,
    parent: Option<glib::WeakRef<Window>>,
    suspended: Cell<bool>,
    closing: Cell<bool>,
    closed: (async_channel::Sender<()>, async_channel::Receiver<()>),
    result: async_channel::Sender<bool>,
}
thread_local! {
    static FORMS: std::cell::RefCell<Vec<Weak<Form>>> = const { std::cell::RefCell::new(Vec::new()) };
}
pub struct Suspended(Rc<Form>);
impl Suspended {
    pub async fn resume(self) {
        if self.0.closing.get() {
            let _ = self.0.closed.1.recv().await;
        }
        if let Some(parent) = self.0.parent.as_ref().and_then(|w| w.upgrade()) {
            self.0.suspended.set(false);
            AdwDialogExt::present(&self.0.dialog, Some(&parent));
        } else {
            self.cancel();
        }
    }
    pub fn cancel(self) {
        let _ = self.0.result.try_send(false);
    }
}

pub fn suspend_forms(window: &Window) -> Vec<Suspended> {
    FORMS.with(|forms| {
        let mut forms = forms.borrow_mut();
        forms.retain(|f| f.strong_count() > 0);
        forms
            .iter()
            .filter_map(Weak::upgrade)
            .filter_map(|form| {
                if form.suspended.get()
                    || form.parent.as_ref().and_then(|w| w.upgrade()).as_ref() != Some(window)
                {
                    return None;
                }
                form.suspended.set(true);
                form.closing.set(true);
                let _ = form.closed.1.try_recv();
                form.dialog.close();
                Some(Suspended(form))
            })
            .collect()
    })
}

/// Scrollable form with fixed actions. Suspending it for an audition preserves
/// both unsaved widget values and the awaiting caller; it is not a Cancel.
pub async fn confirm_form(
    parent: Option<&Window>,
    title: &str,
    child: &impl IsA<gtk::Widget>,
    focus: Option<&gtk::Widget>,
) -> bool {
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(560)
        .content_height(680)
        .build();
    dialog.set_widget_name("settings-form");
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(
        &adw::HeaderBar::builder()
            .title_widget(&adw::WindowTitle::new(title, ""))
            .show_end_title_buttons(false)
            .build(),
    );
    child.set_margin_start(18);
    child.set_margin_end(18);
    child.set_margin_top(12);
    child.set_margin_bottom(18);
    toolbar.set_content(Some(&scroll(child)));
    // These two permanent actions share the entire footer equally. A FlowBox
    // reserves columns as the window grows, leaving an empty slot on the right.
    let buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .homogeneous(true)
        .build();
    buttons.set_margin_start(18);
    buttons.set_margin_end(18);
    buttons.set_margin_top(12);
    buttons.set_margin_bottom(12);
    let cancel = gtk::Button::with_label(&i18n("Cancel"));
    cancel.set_hexpand(true);
    let save = gtk::Button::builder()
        .label(i18n("Save"))
        .hexpand(true)
        .css_classes(["suggested-action"])
        .build();
    buttons.append(&cancel);
    buttons.append(&save);
    toolbar.add_bottom_bar(&buttons);
    dialog.set_child(Some(&toolbar));
    dialog.set_default_widget(Some(&save));
    if let Some(focus) = focus {
        dialog.set_focus(Some(focus));
    }
    let (sender, receiver) = async_channel::bounded(1);
    let form = Rc::new(Form {
        dialog,
        parent: parent.map(|w| w.downgrade()),
        suspended: Cell::new(false),
        closing: Cell::new(false),
        closed: async_channel::bounded(1),
        result: sender,
    });
    FORMS.with(|forms| forms.borrow_mut().push(Rc::downgrade(&form)));
    let weak = Rc::downgrade(&form);
    form.dialog.connect_closed(move |_| {
        if let Some(form) = weak.upgrade() {
            if form.closing.replace(false) {
                let _ = form.closed.0.try_send(());
            } else if !form.suspended.get() {
                let _ = form.result.try_send(false);
            }
        }
    });
    for (button, accepted) in [(cancel, false), (save, true)] {
        let weak = Rc::downgrade(&form);
        button.connect_clicked(move |_| {
            if let Some(form) = weak.upgrade() {
                let _ = form.result.try_send(accepted);
                form.dialog.close();
            }
        });
    }
    AdwDialogExt::present(&form.dialog, parent);
    receiver.recv().await.unwrap_or(false)
}

#[cfg(test)]
pub async fn test_fit(parent: &Window, dialog: &adw::Dialog) {
    let (requested_width, requested_height) = parent.default_size();
    let saved = (
        if requested_width > 0 {
            requested_width
        } else {
            parent.width().max(360)
        },
        if requested_height > 0 {
            requested_height
        } else {
            parent.height().max(640)
        },
    );
    let child = dialog.child().unwrap();
    for (width, height) in [(360, 640), (390, 780), (480, 720)] {
        parent.set_default_size(width, height);
        for _ in 0..100 {
            glib::timeout_future(std::time::Duration::from_millis(25)).await;
            if parent.width() == width && child.is_mapped() && child.width() > 0 {
                break;
            }
        }
        glib::timeout_future(std::time::Duration::from_millis(120)).await;
        let min_width = child.measure(gtk::Orientation::Horizontal, -1).0;
        assert!(
            min_width <= width,
            "{} needs {min_width}px at {width}px",
            dialog.title()
        );
        assert!(
            parent.width() <= width,
            "{} stretches the window to {} instead of {width}",
            dialog.title(),
            parent.width()
        );
        assert!(
            dialog.width() <= parent.width() && dialog.height() <= parent.height(),
            "{} exceeds its window",
            dialog.title()
        );
        if let Some(directory) = std::env::var_os("HOPEA_TEST_MOBILE_SCREENSHOTS") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            let path = directory.join(format!(
                "{}-{width}.png",
                dialog.title().replace(['/', '\\'], "_")
            ));
            glib::timeout_future(std::time::Duration::from_millis(200)).await;
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(
                &gtk::gdk::RGBA::parse("#242424").unwrap(),
                &gtk::graphene::Rect::new(0.0, 0.0, child.width() as f32, child.height() as f32),
            );
            parent.queue_draw();
            glib::timeout_future(std::time::Duration::from_millis(300)).await;
            let mut widget = child.first_child();
            while let Some(current) = widget {
                widget = current.next_sibling();
                child.snapshot_child(&current, &snapshot);
            }
            if let Some(node) = snapshot.to_node() {
                parent
                    .renderer()
                    .unwrap()
                    .render_texture(&node, None)
                    .save_to_png(path)
                    .unwrap();
            }
        }
    }
    parent.set_default_size(saved.0, saved.1);
    glib::timeout_future(std::time::Duration::from_millis(200)).await;
}
