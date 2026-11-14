// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! A compact entry point; details and recovery advice live in their own view.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Session,
    Access,
    Connection,
    Other,
}
impl Kind {
    fn title(self) -> String {
        i18n(match self {
            Self::Session => "Sign-in needs attention",
            Self::Access => "Unavailable albums",
            Self::Connection => "Connection problems",
            Self::Other => "Incomplete library",
        })
    }
    fn advice(self) -> String {
        i18n(match self {
            Self::Session => "Sign in again using your browser, then retry the import with the same account.",
            Self::Access => "Open the album in the service using the same account. Check access or availability there; a private or removed album cannot be imported until access is restored.",
            Self::Connection => "Check your connection and retry later. If the service limits requests, wait before starting another import.",
            Self::Other => "You can import the available music now. Check the details below and retry the import later; previously imported tracks will be recognized.",
        })
    }
}

#[derive(Debug)]
struct Warning {
    kind: Kind,
    title: String,
    detail: String,
}

fn parse(message: &str) -> Vec<Warning> {
    let has_albums = message.lines().any(|line| line.starts_with('•'));
    let mut items: Vec<_> = message
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty()
                || (has_albums
                    && (line == "Only part of the library was read."
                        || line.contains("VK albums could not be fully read.")))
            {
                return None;
            }
            let (title, detail) = if let Some(album) = line.strip_prefix("• ") {
                album.rsplit_once(": ").unwrap_or((album, ""))
            } else {
                ("", line)
            };
            let lower = detail.to_lowercase();
            let kind = if ["expired", "sign in", "unauthorized", "invalid session"]
                .iter()
                .any(|s| lower.contains(s))
            {
                Kind::Session
            } else if ["access", "private", "removed", "restricted"]
                .iter()
                .any(|s| lower.contains(s))
            {
                Kind::Access
            } else if [
                "timeout",
                "timed out",
                "connection",
                "network",
                "too many requests",
                "rate limit",
            ]
            .iter()
            .any(|s| lower.contains(s))
            {
                Kind::Connection
            } else {
                Kind::Other
            };
            Some(Warning {
                kind,
                title: if title.is_empty() {
                    kind.title()
                } else {
                    title.to_owned()
                },
                detail: detail.to_owned(),
            })
        })
        .collect();
    items.sort_by_cached_key(|w| (w.kind, w.title.to_lowercase(), w.detail.clone()));
    items.dedup_by(|a, b| a.kind == b.kind && a.title == b.title && a.detail == b.detail);
    items
}

pub(super) fn banner(message: &str, parent: &adw::Dialog) -> gtk::Box {
    let warnings = Rc::new(parse(message));
    let row = gtk::Box::builder()
        .spacing(8)
        .css_classes(["import-warning-banner"])
        .build();
    let icon = gtk::Image::from_icon_name("hopea-dialog-warning-symbolic");
    icon.add_css_class("warning");
    row.append(&icon);
    let label = gtk::Label::builder()
        .name("import-catalog-warning")
        .label(i18n("Some music needs attention"))
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(24)
        .build();
    row.append(&label);
    let button = gtk::Button::builder()
        .name("import-show-warnings")
        .label(i18n_f("Warnings ({})", &[&warnings.len().to_string()]))
        .css_classes(["flat"])
        .build();
    button.connect_clicked(clone!(
        #[weak]
        parent,
        move |_| show(&warnings, &parent)
    ));
    row.append(&button);
    row
}

fn show(warnings: &[Warning], parent: &adw::Dialog) {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(20)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(20)
        .margin_end(20)
        .build();
    content.append(&gtk::Label::builder().label(i18n("Available songs and complete albums are ready to import. Warnings are grouped by cause, then sorted by name."))
        .wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).xalign(0.0).css_classes(["dim-label"]).build());
    for kind in [Kind::Session, Kind::Access, Kind::Connection, Kind::Other] {
        let items: Vec<_> = warnings.iter().filter(|w| w.kind == kind).collect();
        if items.is_empty() {
            continue;
        }
        let group = adw::PreferencesGroup::builder()
            .title(i18n_f(
                "{} · {}",
                &[&kind.title(), &items.len().to_string()],
            ))
            .description(kind.advice())
            .build();
        // Virtualize each category so a large failed catalogue does not
        // allocate thousands of detail widgets up front.
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        for item in items {
            model.append(&glib::BoxedAnyObject::new((
                item.title.clone(),
                item.detail.clone(),
                kind == Kind::Access,
            )));
        }
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let data = item
                .item()
                .unwrap()
                .downcast::<glib::BoxedAnyObject>()
                .unwrap();
            let data = data.borrow::<(String, String, bool)>();
            let detail = gtk::Label::builder()
                .label(&data.1)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .selectable(true)
                .xalign(0.0)
                .margin_top(12)
                .margin_bottom(12)
                .margin_start(16)
                .margin_end(16)
                .build();
            let row = adw::ExpanderRow::builder()
                .title(&data.0)
                .use_markup(false)
                .title_lines(2)
                .subtitle(i18n("Show details"))
                .build();
            if data.2 {
                row.add_css_class("import-unavailable");
            }
            row.add_row(&detail);
            let box_ = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .build();
            box_.append(&row);
            item.set_child(Some(&box_));
            item.set_activatable(false);
        });
        factory.connect_unbind(|_, item| {
            item.downcast_ref::<gtk::ListItem>()
                .unwrap()
                .set_child(gtk::Widget::NONE)
        });
        let list = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(model)))
            .factory(&factory)
            .build();
        group.add(
            &gtk::ScrolledWindow::builder()
                .child(&list)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .min_content_height(80)
                .max_content_height(320)
                .propagate_natural_height(true)
                .build(),
        );
        content.append(&group);
    }
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new(&i18n("Import Warnings"), ""))
        .build();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .child(&content)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build(),
    ));
    let dialog = adw::Dialog::builder()
        .title(i18n("Import Warnings"))
        .content_width(620)
        .content_height(560)
        .child(&toolbar)
        .build();
    dialog.set_widget_name("import-warnings");
    AdwDialogExt::present(&dialog, Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn warnings_are_grouped_sorted_and_deduplicated() {
        let warnings = parse("Only part of the library was read.\n• Z: VK refused access\n• A: VK refused access\n• Z: VK refused access\nReadTimeout\nSession expired");
        assert_eq!(warnings.len(), 4);
        assert_eq!(warnings[0].kind, Kind::Session);
        assert_eq!(warnings[1].title, "A");
        assert_eq!(warnings[2].title, "Z");
        assert_eq!(warnings[3].kind, Kind::Connection);
    }
}
