// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! StatusNotifierItem backend. All callbacks hand work to the GTK thread.

use ksni::{
    blocking::TrayMethods,
    menu::{MenuItem, StandardItem},
    Category, ToolTip,
};
use log::{debug, warn};

use crate::{config::APPLICATION_ID, i18n::i18n};

use super::{TrayEvent, TrayState};

struct SharingTray {
    events: async_channel::Sender<TrayEvent>,
    icons: Vec<ksni::Icon>,
    state: TrayState,
}

impl SharingTray {
    fn send(&self, event: TrayEvent) {
        let _ = self.events.try_send(event);
    }
}

impl ksni::Tray for SharingTray {
    fn id(&self) -> String {
        APPLICATION_ID.to_string()
    }

    fn title(&self) -> String {
        self.state.title()
    }

    fn icon_name(&self) -> String {
        // The tray host cannot see the application's GResource. Supply pixels
        // directly so development/portable builds also show the cat.
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.clone()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_pixmap: self.icons.clone(),
            title: self.state.title(),
            description: gtk::glib::markup_escape_text(&self.state.status()).to_string(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayEvent::Show);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: self.state.title().replace('_', "__"),
                enabled: false,
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: i18n("Previous Track"),
                icon_name: "media-skip-backward".into(),
                enabled: self.state.available,
                activate: Box::new(|tray: &mut Self| tray.send(TrayEvent::Previous)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: self.state.play_label(),
                icon_name: if self.state.playing {
                    "media-playback-pause"
                } else {
                    "media-playback-start"
                }
                .into(),
                enabled: self.state.available,
                activate: Box::new(|tray: &mut Self| tray.send(TrayEvent::PlayPause)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: i18n("Next Track"),
                icon_name: "media-skip-forward".into(),
                enabled: self.state.available,
                activate: Box::new(|tray: &mut Self| tray.send(TrayEvent::Next)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: i18n("_Show Hopea"),
                activate: Box::new(|tray: &mut Self| tray.send(TrayEvent::Show)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: i18n("_Quit"),
                icon_name: "application-exit".into(),
                activate: Box::new(|tray: &mut Self| tray.send(TrayEvent::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// The tray icon; removed when dropped.
pub struct Tray {
    handle: ksni::blocking::Handle<SharingTray>,
}

impl Tray {
    pub fn update(&self, state: TrayState) {
        self.handle.update(|tray| tray.state = state);
    }

    /// Shows the icon. Returns `None` if the desktop has no tray at all.
    pub fn show(events: async_channel::Sender<TrayEvent>, state: TrayState) -> Option<Self> {
        let tray = SharingTray {
            events,
            icons: cat_icons(),
            state,
        };
        // Inside Flatpak the app can't own a name of its own on the bus
        let sandboxed = std::env::var_os("FLATPAK_ID").is_some();
        match tray.disable_dbus_name(sandboxed).spawn() {
            Ok(handle) => {
                debug!("Showing the tray icon");
                Some(Self { handle })
            }
            Err(err) => {
                warn!("No tray icon: {err}");
                None
            }
        }
    }
}

fn cat_icons() -> Vec<ksni::Icon> {
    [
        include_bytes!("../../data/icons/cat-kz/16.png").as_slice(),
        include_bytes!("../../data/icons/cat-kz/24.png").as_slice(),
        include_bytes!("../../data/icons/cat-kz/32.png").as_slice(),
        include_bytes!("../../data/icons/cat-kz/48.png").as_slice(),
        include_bytes!("../../data/icons/cat-kz/64.png").as_slice(),
    ]
    .iter()
    .copied()
    .filter_map(|png| {
        let pixbuf = gdk_pixbuf::Pixbuf::from_read(std::io::Cursor::new(png)).ok()?;
        let pixels = pixbuf.read_pixel_bytes();
        let channels = pixbuf.n_channels() as usize;
        let mut data = Vec::with_capacity((pixbuf.width() * pixbuf.height() * 4) as usize);
        for y in 0..pixbuf.height() as usize {
            for x in 0..pixbuf.width() as usize {
                let offset = y * pixbuf.rowstride() as usize + x * channels;
                let rgb = &pixels[offset..offset + channels];
                data.extend_from_slice(&[
                    if channels == 4 { rgb[3] } else { 255 },
                    rgb[0],
                    rgb[1],
                    rgb[2],
                ]);
            }
        }
        Some(ksni::Icon {
            width: pixbuf.width(),
            height: pixbuf.height(),
            data,
        })
    })
    .collect()
}

impl std::fmt::Debug for Tray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tray")
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let _ = self.handle.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray as _;

    #[test]
    fn menu_controls_send_actions_and_follow_player_state() {
        let (events, receiver) = async_channel::unbounded();
        let mut tray = SharingTray {
            events,
            icons: Vec::new(),
            state: TrayState::default(),
        };
        for item in tray.menu().iter().take(4) {
            let MenuItem::Standard(item) = item else {
                panic!("player menu item");
            };
            assert!(!item.enabled);
        }
        tray.state = TrayState {
            track: Some("Song_Name — Artist".into()),
            playing: true,
            available: true,
        };
        let menu = tray.menu();
        let MenuItem::Standard(title) = &menu[0] else {
            panic!()
        };
        assert_eq!(title.label, "Song__Name — Artist");
        let MenuItem::Standard(play) = &menu[2] else {
            panic!()
        };
        assert_eq!(play.label, i18n("Pause"));
        assert_eq!(play.icon_name, "media-playback-pause");
        for (index, action) in [
            (1, TrayEvent::Previous),
            (2, TrayEvent::PlayPause),
            (3, TrayEvent::Next),
            (5, TrayEvent::Show),
            (7, TrayEvent::Quit),
        ] {
            let MenuItem::Standard(item) = &menu[index] else {
                panic!()
            };
            assert!(item.enabled);
            (item.activate)(&mut tray);
            assert_eq!(receiver.try_recv().unwrap(), action);
        }
        tray.state.playing = false;
        let menu = tray.menu();
        let MenuItem::Standard(play) = &menu[2] else {
            panic!()
        };
        assert_eq!(play.label, i18n("Play"));
        assert_eq!(tray.tool_tip().title, "Song_Name — Artist");
    }

    #[test]
    fn cat_tray_icons_have_argb_pixels_and_transparency() {
        let icons = super::cat_icons();
        assert_eq!(
            icons.iter().map(|icon| icon.width).collect::<Vec<_>>(),
            [16, 24, 32, 48, 64]
        );
        for icon in icons {
            assert_eq!(icon.width, icon.height);
            assert_eq!(icon.data.len(), (icon.width * icon.height * 4) as usize);
            assert_eq!(icon.data[0], 0, "transparent corner, alpha is first");
            assert!(icon.data.chunks_exact(4).any(|pixel| pixel[0] == 255));
        }
    }
}
