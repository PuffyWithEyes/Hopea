// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::i18n::i18n;
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::{cell::Cell, rc::Rc};

// Parse only hexadecimal RGB, never interpolate arbitrary entry text into CSS.
fn rgb(code: &str) -> Option<[u8; 3]> {
    let code = code.trim();
    let code = code.strip_prefix('#').unwrap_or(code);
    if !code.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match code.len() {
        3 => Some([
            u8::from_str_radix(&code[0..1], 16).ok()? * 17,
            u8::from_str_radix(&code[1..2], 16).ok()? * 17,
            u8::from_str_radix(&code[2..3], 16).ok()? * 17,
        ]),
        6 => Some([
            u8::from_str_radix(&code[0..2], 16).ok()?,
            u8::from_str_radix(&code[2..4], 16).ok()?,
            u8::from_str_radix(&code[4..6], 16).ok()?,
        ]),
        _ => None,
    }
}
fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}
fn css(c: [u8; 3]) -> String {
    let linear = |v: u8| {
        let x = f64::from(v) / 255.0;
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2]);
    let fg = if luminance > 0.179 {
        "#000000"
    } else {
        "#FFFFFF"
    };
    format!(":root, .main-window {{ --accent-bg-color: {}; --accent-fg-color: {fg}; --accent-color: oklab(from var(--accent-bg-color) var(--standalone-color-oklab)); }}",hex(c))
}
fn standard_css() -> String {
    let c = adw::StyleManager::default().accent_color_rgba();
    css([
        (c.red() * 255.0).round() as u8,
        (c.green() * 255.0).round() as u8,
        (c.blue() * 255.0).round() as u8,
    ])
}
pub fn init() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_USER,
    );
    let settings = crate::utils::settings_manager();
    let update = move |settings: &gio::Settings| {
        let text = if settings.string("accent-mode") == "custom" {
            rgb(&settings.string("accent-color"))
                .map(css)
                .unwrap_or_default()
        } else {
            String::new()
        };
        provider.load_from_string(&text);
    };
    update(&settings);
    settings.connect_changed(None, move |settings, key| {
        if matches!(key, "accent-mode" | "accent-color") {
            update(settings);
        }
    });
    // Keep the settings object alive for display-wide changes.
    SETTINGS.with(|slot| slot.replace(Some(settings)));
}
thread_local! {static SETTINGS:std::cell::RefCell<Option<gio::Settings>>=const{std::cell::RefCell::new(None)};}

pub struct Settings {
    pub group: adw::PreferencesGroup,
    mode: adw::ComboRow,
    code: adw::EntryRow,
    preview: gtk::CssProvider,
}
impl Drop for Settings {
    fn drop(&mut self) {
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_remove_provider_for_display(&display, &self.preview);
        }
    }
}
impl Settings {
    pub fn new(parent: &impl IsA<gtk::Widget>) -> Self {
        let settings = crate::utils::settings_manager();
        let names = [i18n("Standard Accent Color"), i18n("Custom Accent Color")];
        let mode = adw::ComboRow::builder()
            .title(i18n("Accent Color"))
            .model(&gtk::StringList::new(
                &names.iter().map(String::as_str).collect::<Vec<_>>(),
            ))
            .selected(u32::from(settings.string("accent-mode") == "custom"))
            .build();
        mode.set_widget_name("accent-mode");
        let code = adw::EntryRow::builder()
            .title(i18n("Color Code (#RGB or #RRGGBB)"))
            .text(settings.string("accent-color"))
            .build();
        code.set_widget_name("accent-code");
        let choose = adw::ButtonRow::builder()
            .title(i18n("Choose from Palette…"))
            .build();
        let group = adw::PreferencesGroup::builder()
            .title(i18n("Accent Color"))
            .description(i18n(
                "Preview changes here. Save applies the color; Cancel restores it.",
            ))
            .build();
        group.add(&mode);
        group.add(&code);
        group.add(&choose);
        let preview = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &preview,
                gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
            );
        }
        let refresh = Rc::new(glib::clone!(
            #[weak]
            mode,
            #[weak]
            code,
            #[weak]
            choose,
            #[strong]
            preview,
            move || {
                let custom = mode.selected() == 1;
                code.set_visible(custom);
                choose.set_visible(custom);
                match rgb(&code.text()) {
                    Some(color) => {
                        code.remove_css_class("error");
                        let style = if custom { css(color) } else { standard_css() };
                        preview.load_from_string(&style);
                    }
                    None => {
                        code.add_css_class("error");
                        if !custom {
                            preview.load_from_string(&standard_css());
                        }
                    }
                }
            }
        ));
        refresh();
        mode.connect_selected_notify({
            let refresh = refresh.clone();
            move |_| refresh()
        });
        code.connect_changed(move |_| refresh());
        let parent = parent.as_ref().downgrade();
        choose.connect_activated(glib::clone!(
            #[weak]
            code,
            move |_| {
                if let Some(parent) = parent.upgrade() {
                    picker(&parent, &code);
                }
            }
        ));
        Self {
            group,
            mode,
            code,
            preview,
        }
    }
    pub fn apply(&self) -> Result<(), String> {
        let custom = self.mode.selected() == 1;
        let color = rgb(&self.code.text());
        if custom && color.is_none() {
            return Err(i18n("Enter a color such as #3584E4"));
        }
        let settings = crate::utils::settings_manager();
        if let Some(color) = color {
            settings
                .set_string("accent-color", &hex(color))
                .map_err(|_| i18n("Unable to save accent color"))?;
        }
        settings
            .set_string("accent-mode", if custom { "custom" } else { "standard" })
            .map_err(|_| i18n("Unable to save accent color"))?;
        Ok(())
    }
}
fn hsv(rgb: [u8; 3]) -> (f64, f64, f64) {
    let [r, g, b] = rgb.map(|v| f64::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let hue = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (hue, if max == 0.0 { 0.0 } else { d / max }, max)
}
fn from_hsv(h: f64, s: f64, v: f64) -> [u8; 3] {
    let c = v * s;
    let h = h / 60.0;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r, g, b].map(|f| ((f + m).clamp(0.0, 1.0) * 255.0).round() as u8)
}
fn picker(parent: &gtk::Widget, entry: &adw::EntryRow) {
    let dialog = adw::Dialog::builder()
        .title(i18n("Choose Accent Color"))
        .content_width(360)
        .content_height(460)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .margin_start(18)
        .margin_end(18)
        .margin_top(12)
        .margin_bottom(18)
        .build();
    let color = Rc::new(Cell::new(hsv(rgb(&entry.text()).unwrap_or([53, 132, 228]))));
    let square = gtk::DrawingArea::builder()
        .height_request(180)
        .hexpand(true)
        .vexpand(true)
        .build();
    square.set_widget_name("accent-palette");
    square.set_draw_func(glib::clone!(
        #[strong]
        color,
        move |_, cr, width, height| {
            let (h, s, v) = color.get();
            let [r, g, b] = from_hsv(h, 1.0, 1.0).map(|x| f64::from(x) / 255.0);
            cr.set_source_rgb(r, g, b);
            let _ = cr.paint();
            let white = gtk::cairo::LinearGradient::new(0.0, 0.0, width as f64, 0.0);
            white.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 1.0);
            white.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
            let _ = cr.set_source(&white);
            let _ = cr.paint();
            let black = gtk::cairo::LinearGradient::new(0.0, 0.0, 0.0, height as f64);
            black.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 0.0);
            black.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 1.0);
            let _ = cr.set_source(&black);
            let _ = cr.paint();
            cr.arc(
                s * width as f64,
                (1.0 - v) * height as f64,
                6.0,
                0.0,
                std::f64::consts::TAU,
            );
            cr.set_line_width(3.0);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cr.stroke_preserve();
            cr.set_line_width(1.0);
            cr.set_source_rgb(0.0, 0.0, 0.0);
            let _ = cr.stroke();
        }
    ));
    content.append(&square);
    let hue = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 359.0, 1.0);
    hue.set_value(color.get().0);
    hue.set_draw_value(false);
    hue.set_tooltip_text(Some(&i18n("Hue")));
    content.append(&hue);
    let sample = gtk::Label::new(Some(&hex(from_hsv(
        color.get().0,
        color.get().1,
        color.get().2,
    ))));
    sample.set_selectable(true);
    content.append(&sample);
    let update = Rc::new(glib::clone!(
        #[weak]
        square,
        #[weak]
        sample,
        #[strong]
        color,
        move || {
            let (h, s, v) = color.get();
            sample.set_text(&hex(from_hsv(h, s, v)));
            square.queue_draw();
        }
    ));
    hue.connect_value_changed(glib::clone!(
        #[strong]
        color,
        #[strong]
        update,
        move |scale| {
            let (_, s, v) = color.get();
            color.set((scale.value(), s, v));
            update();
        }
    ));
    let drag = gtk::GestureDrag::new();
    let origin = Rc::new(Cell::new((0.0, 0.0)));
    let point = Rc::new(glib::clone!(
        #[weak]
        square,
        #[strong]
        color,
        #[strong]
        update,
        move |x: f64, y: f64| {
            let (h, _, _) = color.get();
            color.set((
                h,
                (x / f64::from(square.width().max(1))).clamp(0.0, 1.0),
                (1.0 - y / f64::from(square.height().max(1))).clamp(0.0, 1.0),
            ));
            update();
        }
    ));
    drag.connect_drag_begin(glib::clone!(
        #[strong]
        point,
        #[strong]
        origin,
        move |_, x, y| {
            origin.set((x, y));
            point(x, y);
        }
    ));
    drag.connect_drag_update(move |_, x, y| {
        let (ox, oy) = origin.get();
        point(ox + x, oy + y);
    });
    square.add_controller(drag);
    let use_color = gtk::Button::with_label(&i18n("Use Color"));
    use_color.add_css_class("suggested-action");
    content.append(&use_color);
    use_color.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        entry,
        move |_| {
            let (h, s, v) = color.get();
            entry.set_text(&hex(from_hsv(h, s, v)));
            dialog.close();
        }
    ));
    toolbar.set_content(Some(&crate::dialogs::scroll(&content)));
    dialog.set_child(Some(&toolbar));
    AdwDialogExt::present(&dialog, Some(parent));
}
#[cfg(test)]
pub async fn test_settings(window: &crate::window::Window) {
    let saved = crate::utils::settings_manager();
    assert_eq!(saved.string("accent-mode"), "standard");
    {
        let editor = Settings::new(window);
        editor.mode.set_selected(1);
        editor.code.set_text("#f93");
        assert_eq!(
            saved.string("accent-mode"),
            "standard",
            "preview must not persist"
        );
        picker(window.upcast_ref(), &editor.code);
        gtk::glib::timeout_future(std::time::Duration::from_millis(200)).await;
        let dialog = window.visible_dialog().unwrap();
        crate::dialogs::test_fit(window, &dialog).await;
        dialog.close();
        gtk::glib::timeout_future(std::time::Duration::from_millis(200)).await;
        editor.code.set_text("#123456; }");
        assert!(editor.apply().is_err());
        editor.code.set_text("#f93");
        editor.apply().unwrap();
        assert_eq!(saved.string("accent-color"), "#FF9933");
    }
    let editor = Settings::new(window);
    assert_eq!(editor.mode.selected(), 1);
    editor.mode.set_selected(0);
    editor.apply().unwrap();
    assert_eq!(saved.string("accent-mode"), "standard");
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hex_codes_cannot_inject_css() {
        assert_eq!(rgb("#aB3"), Some([170, 187, 51]));
        assert_eq!(rgb("3584e4"), Some([53, 132, 228]));
        for value in [
            "red; color:red",
            "#000000; }",
            "#12345678",
            "#-10000",
            "NaN",
            "#１２３",
            "",
        ] {
            assert!(rgb(value).is_none());
        }
        for c in [[0, 0, 0], [255, 255, 255], [30, 50, 240], [54, 210, 99]] {
            let (h, s, v) = hsv(c);
            assert_eq!(from_hsv(h, s, v), c);
        }
    }
}
