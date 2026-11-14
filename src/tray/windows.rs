// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Native Windows/macOS menu, created on GTK's platform event-loop thread.
use super::{TrayEvent, TrayState};
use crate::{config::APPLICATION_ID, i18n::i18n};
use log::warn;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

pub struct Tray {
    icon: TrayIcon,
    title: MenuItem,
    previous: MenuItem,
    play: MenuItem,
    next: MenuItem,
    events: Option<gtk::glib::SourceId>,
}
impl Tray {
    pub fn show(events: async_channel::Sender<TrayEvent>, state: TrayState) -> Option<Self> {
        match Self::create(events, state) {
            Ok(tray) => Some(tray),
            Err(error) => {
                warn!("No tray icon: {error}");
                None
            }
        }
    }

    fn create(
        events: async_channel::Sender<TrayEvent>,
        state: TrayState,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let title = MenuItem::new(menu_title(&state.title()), false, None);
        let previous = MenuItem::new(i18n("Previous Track"), state.available, None);
        let play = MenuItem::new(state.play_label(), state.available, None);
        let next = MenuItem::new(i18n("Next Track"), state.available, None);
        let show = MenuItem::new(i18n("Show Hopea"), true, None);
        let quit = MenuItem::new(i18n("Quit"), true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &title,
            &previous,
            &play,
            &next,
            &PredefinedMenuItem::separator(),
            &show,
            &quit,
        ])?;
        let pixbuf = gdk_pixbuf::Pixbuf::from_read(std::io::Cursor::new(include_bytes!(
            "../../data/icons/cat-kz/32.png"
        )))?;
        let pixels = pixbuf.read_pixel_bytes();
        let channels = pixbuf.n_channels() as usize;
        let mut rgba = Vec::with_capacity((pixbuf.width() * pixbuf.height() * 4) as usize);
        for y in 0..pixbuf.height() as usize {
            for x in 0..pixbuf.width() as usize {
                let offset = y * pixbuf.rowstride() as usize + x * channels;
                rgba.extend_from_slice(&pixels[offset..offset + 3]);
                rgba.push(if channels == 4 {
                    pixels[offset + 3]
                } else {
                    255
                });
            }
        }
        let icon = Icon::from_rgba(rgba, pixbuf.width() as u32, pixbuf.height() as u32)?;
        let icon = TrayIconBuilder::new()
            .with_id(APPLICATION_ID)
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(true)
            .with_tooltip(format!("Hopea · {}", state.title()))
            .build()?;
        let actions = [
            (previous.id().clone(), TrayEvent::Previous),
            (play.id().clone(), TrayEvent::PlayPause),
            (next.id().clone(), TrayEvent::Next),
            (show.id().clone(), TrayEvent::Show),
            (quit.id().clone(), TrayEvent::Quit),
        ];
        // Poll the documented receiver on the GTK thread. muda's global
        // handler is a OnceLock and cannot actually be removed on shutdown.
        let events =
            gtk::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                while let Ok(event) = MenuEvent::receiver().try_recv() {
                    if let Some((_, action)) = actions.iter().find(|(id, _)| id == event.id()) {
                        let _ = events.try_send(*action);
                    }
                }
                gtk::glib::ControlFlow::Continue
            });
        Ok(Self {
            icon,
            title,
            previous,
            play,
            next,
            events: Some(events),
        })
    }

    pub fn update(&self, state: TrayState) {
        self.title.set_text(menu_title(&state.title()));
        self.play.set_text(state.play_label());
        for item in [&self.previous, &self.play, &self.next] {
            item.set_enabled(state.available);
        }
        if let Err(error) = self.icon.set_tooltip(Some(format!(
            "Hopea · {} · {}",
            state.title(),
            state.status()
        ))) {
            log::debug!("Unable to update tray tooltip: {error}");
        }
    }
}
fn menu_title(title: &str) -> String {
    if cfg!(target_os = "windows") {
        title.replace('&', "&&")
    } else {
        title.to_string()
    }
}
impl std::fmt::Debug for Tray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tray")
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        if let Some(events) = self.events.take() {
            events.remove();
        }
    }
}
