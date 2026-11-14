// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared player state for native Linux and Windows tray menus.
use crate::{audio::AudioPlayer, i18n::i18n};

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
mod linux;
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
pub use linux::Tray;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod windows;
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use windows::Tray;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    Show,
    Quit,
    Previous,
    PlayPause,
    Next,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrayState {
    track: Option<String>,
    playing: bool,
    available: bool,
}
impl TrayState {
    pub fn from_player(player: &AudioPlayer) -> Self {
        let state = player.state();
        let track = state.current_song().map(|song| {
            let title = song.title();
            let artist = song.artist();
            let label = if artist.trim().is_empty() {
                title
            } else {
                format!("{title} — {artist}")
            };
            label
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(120)
                .collect()
        });
        Self {
            track,
            playing: state.playing(),
            available: state.current_song().is_some() || !player.queue().is_empty(),
        }
    }
    fn title(&self) -> String {
        self.track
            .clone()
            .unwrap_or_else(|| i18n("No Track Playing"))
    }
    fn play_label(&self) -> String {
        if self.playing {
            i18n("Pause")
        } else {
            i18n("Play")
        }
    }
    fn status(&self) -> String {
        if self.playing {
            i18n("Playing")
        } else if self.track.is_some() {
            i18n("Paused")
        } else {
            i18n("Hopea")
        }
    }
}
