// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Android system controls feed the same player as every in-app audition.
use crate::audio::{Controller, PlaybackAction, PlaybackState, RepeatMode, Song};
use gtk::{gio, prelude::*};
use std::{
    cell::{Cell, RefCell},
    ffi::CString,
    sync::OnceLock,
};
static ACTIONS: OnceLock<async_channel::Sender<PlaybackAction>> = OnceLock::new();
extern "C" {
    fn hopea_android_update(
        playing: i32,
        title: *const std::ffi::c_char,
        artist: *const std::ffi::c_char,
        duration: i64,
        position: i64,
    );
    fn hopea_android_vpn() -> i32;
    fn hopea_android_prepare_gaming();
    fn hopea_android_game_status() -> i32;
    fn hopea_android_open_usage_settings() -> i32;
}
#[no_mangle]
pub extern "C" fn hopea_android_action(action: i32) {
    let command = match action {
        0 => PlaybackAction::Pause,
        1 => PlaybackAction::Play,
        2 => PlaybackAction::SkipPrevious,
        3 => PlaybackAction::SkipNext,
        4 => PlaybackAction::Stop,
        _ => return,
    };
    if let Some(sender) = ACTIONS.get() {
        let _ = sender.try_send(command);
    }
}
pub fn vpn() -> i32 {
    unsafe { hopea_android_vpn() }
}
pub fn prepare_gaming() {
    unsafe { hopea_android_prepare_gaming() }
}
pub fn game_status() -> i32 {
    unsafe { hopea_android_game_status() }
}
pub fn open_usage_settings() -> bool {
    unsafe { hopea_android_open_usage_settings() != 0 }
}
pub struct MediaController {
    song: RefCell<Option<Song>>,
    playing: Cell<bool>,
    position: Cell<u64>,
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
}
impl MediaController {
    pub fn new(sender: async_channel::Sender<PlaybackAction>) -> Self {
        let _ = ACTIONS.set(sender);
        Self {
            song: RefCell::new(None),
            playing: Cell::new(false),
            position: Cell::new(0),
            hold: RefCell::new(None),
        }
    }
    fn update(&self) {
        let song = self.song.borrow();
        let title = CString::new(
            song.as_ref()
                .map(|s| s.title())
                .unwrap_or_default()
                .replace('\0', ""),
        )
        .unwrap();
        let artist = CString::new(
            song.as_ref()
                .map(|s| s.artist())
                .unwrap_or_default()
                .replace('\0', ""),
        )
        .unwrap();
        let duration = song
            .as_ref()
            .map(|s| s.duration())
            .unwrap_or(0)
            .saturating_mul(1000)
            .min(i64::MAX as u64) as i64;
        unsafe {
            hopea_android_update(
                i32::from(self.playing.get()),
                title.as_ptr(),
                artist.as_ptr(),
                duration,
                self.position.get().min(i64::MAX as u64) as i64,
            );
        }
    }
}
impl Controller for MediaController {
    fn set_song(&self, song: &Song) {
        self.song.replace(Some(song.clone()));
        self.position.set(0);
        self.update();
    }
    fn set_playback_state(&self, state: &PlaybackState) {
        self.playing.set(*state == PlaybackState::Playing);
        if self.playing.get() && self.hold.borrow().is_none() {
            self.hold
                .replace(gio::Application::default().map(|app| app.hold()));
        } else if !self.playing.get() {
            self.hold.borrow_mut().take();
        }
        self.update();
    }
    fn set_position(&self, position: u64, _notify: bool) {
        let ms = position.saturating_mul(1000);
        if self.position.replace(ms) != ms {
            self.update();
        }
    }
    fn set_repeat_mode(&self, _repeat: RepeatMode) {}
}
