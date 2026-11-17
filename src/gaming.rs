// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detect gaming off the UI thread; temporary policy never rewrites speed settings.
use crate::i18n::i18n;
use gtk::{gio, glib, prelude::*};
use std::{
    cell::Cell,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

static ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
mod desktop;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "android")]
static ANDROID_STATUS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-2);
/// Only platforms with an implemented automatic detector expose Game Mode.
pub const fn supported() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

fn apply(active: bool) {
    if ACTIVE.swap(active, Ordering::Relaxed) != active {
        crate::library::set_gaming(active);
        log::debug!("Game mode active: {active}");
    }
}

pub fn init() {
    let settings = crate::utils::settings_manager();
    // The retired game-mode-force key is deliberately ignored, including
    // installations that previously persisted it as true.
    if !supported() {
        apply(false);
        return;
    }
    settings.connect_changed(Some("game-mode"), |settings, _| {
        if !settings.boolean("game-mode") {
            apply(false);
        }
    });
    let busy = Rc::new(Cell::new(false));
    let misses = Rc::new(Cell::new(0u8));
    let check = move || {
        if !settings.boolean("game-mode") {
            misses.set(0);
            apply(false);
            return;
        }
        if busy.replace(true) {
            return;
        }
        // GTK activity/class-loader discovery must stay on the GTK thread.
        // The actual Android usage query runs on the blocking worker via JNI.
        #[cfg(target_os = "android")]
        crate::android::prepare_gaming();
        let (busy, misses, settings) = (busy.clone(), misses.clone(), settings.clone());
        glib::spawn_future_local(async move {
            let detected = gio::spawn_blocking(detect).await.unwrap_or(false);
            busy.set(false);
            if !settings.boolean("game-mode") {
                apply(false);
                return;
            }
            // Short Alt-Tab/probe failures should not oscillate the traffic caps.
            misses.set(if detected {
                0
            } else {
                misses.get().saturating_add(1)
            });
            if detected || misses.get() >= 3 {
                apply(detected);
            }
        });
    };
    check();
    glib::timeout_add_local(Duration::from_secs(5), move || {
        check();
        glib::ControlFlow::Continue
    });
}

pub fn description() -> String {
    if active() {
        i18n("Active: P2P downloads up to 512 KiB/s, sharing up to 128 KiB/s. Lower limits are preserved.")
    } else if cfg!(target_os = "linux") {
        i18n("Automatically detect Steam games, Feral GameMode and installed desktop games.")
    } else if cfg!(target_os = "windows") {
        i18n("Automatically reduce P2P traffic and background work for detected full-screen games and apps.")
    } else if cfg!(target_os = "macos") {
        i18n("Automatically reduce traffic and background work while the active app is categorized as a game by macOS.")
    } else if cfg!(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )) {
        i18n("Automatically detect running games from installed desktop entries.")
    } else if cfg!(target_os = "android") {
        android_description()
    } else {
        i18n("Automatic detection is unavailable on this platform.")
    }
}

fn android_description() -> String {
    #[cfg(target_os = "android")]
    match ANDROID_STATUS.load(Ordering::Relaxed) {
        -1 => return i18n("Allow Usage Access below to detect games. Recent app activity is processed only on this device and is not saved or shared."),
        -2 => return i18n("Game detection is temporarily unavailable."),
        _ => {},
    }
    i18n("Automatically reduce traffic and background work while the foreground Android app is categorized as a game.")
}

#[cfg(target_os = "android")]
fn detect() -> bool {
    let status = crate::android::game_status();
    ANDROID_STATUS.store(status, Ordering::Relaxed);
    status == 1
}

#[cfg(target_os = "macos")]
fn detect() -> bool {
    macos::detect()
}

#[cfg(any(
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn detect() -> bool {
    desktop::detect()
}

#[cfg(target_os = "windows")]
fn detect() -> bool {
    use windows_sys::Win32::UI::Shell::{
        SHQueryUserNotificationState, QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN,
    };
    let mut state = 0;
    // This shell API can also report non-game full-screen apps; do not call
    // it a universal game detector. No hooks, injection or process privileges.
    (unsafe { SHQueryUserNotificationState(&mut state) >= 0 })
        && matches!(state, QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN)
}

#[cfg(target_os = "linux")]
fn detect() -> bool {
    if let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        if let Ok(reply) = bus.call_sync(
            Some("com.feralinteractive.GameMode"),
            "/com/feralinteractive/GameMode",
            "org.freedesktop.DBus.Properties",
            "Get",
            Some(&("com.feralinteractive.GameMode", "ClientCount").to_variant()),
            None,
            gio::DBusCallFlags::NO_AUTO_START,
            500,
            gio::Cancellable::NONE,
        ) {
            if reply
                .get::<(glib::Variant,)>()
                .and_then(|(v,)| v.get::<i32>())
                .is_some_and(|n| n > 0)
            {
                return true;
            }
        }
    }
    steam_game_in(std::path::Path::new("/proc")) || desktop::detect()
}

#[cfg(target_os = "linux")]
fn steam_game_in(proc: &std::path::Path) -> bool {
    use std::{fs, io::Read, os::unix::fs::MetadataExt, time::Instant};
    let Ok(owner) = fs::metadata(proc.join("self")) else {
        return false;
    };
    let Ok(processes) = fs::read_dir(proc) else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_millis(75);
    // Fixed CPU/IO/memory budget; only our UID and two Steam markers matter.
    for entry in processes.flatten().take(2048) {
        if Instant::now() >= deadline {
            break;
        }
        let path = entry.path();
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() || !entry.metadata().is_ok_and(|m| m.uid() == owner.uid()) {
            continue;
        }
        let Ok(name) = fs::read_to_string(path.join("comm")) else {
            continue;
        };
        if matches!(
            name.trim(),
            "steam" | "steamwebhelper" | "reaper" | "steam-runtime-l" | "pressure-vessel"
        ) {
            continue;
        }
        let Ok(file) = fs::File::open(path.join("environ")) else {
            continue;
        };
        let mut data = Vec::new();
        if file.take(32 * 1024).read_to_end(&mut data).is_ok() && steam_marker(&data) {
            return true;
        }
    }
    false
}

#[cfg(any(target_os = "linux", test))]
fn steam_marker(data: &[u8]) -> bool {
    data.split(|b| *b == 0).any(|part| {
        let value = part
            .strip_prefix(b"SteamAppId=")
            .or_else(|| part.strip_prefix(b"SteamGameId="));
        value.is_some_and(|v| {
            !v.is_empty()
                && v.len() <= 20
                && v.iter().all(u8::is_ascii_digit)
                && v.iter().any(|b| *b != b'0')
        })
    })
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "android",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
fn detect() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn steam_probe_ignores_launcher_and_detects_game_exit() {
        use std::fs;
        let root = glib::mkdtemp(std::env::temp_dir().join("hopea-game-proc-XXXXXX")).unwrap();
        fs::create_dir(root.join("self")).unwrap();
        let game = root.join("4294967295");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("comm"), "steam\n").unwrap();
        fs::write(game.join("environ"), b"SteamAppId=730\0").unwrap();
        assert!(!steam_game_in(&root));
        fs::write(game.join("comm"), "game.exe\n").unwrap();
        assert!(steam_game_in(&root));
        // An oversized environment cannot defeat the read budget.
        let mut oversized = vec![b'x'; 32 * 1024];
        oversized.extend_from_slice(b"\0SteamAppId=730\0");
        fs::write(game.join("environ"), oversized).unwrap();
        assert!(!steam_game_in(&root));
        fs::remove_dir_all(&game).unwrap();
        assert!(!steam_game_in(&root));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn steam_markers_do_not_match_launcher_or_substrings() {
        assert!(steam_marker(b"X=1\0SteamAppId=730\0Y=2\0"));
        assert!(steam_marker(b"SteamGameId=18446744073709551615\0"));
        for bytes in [
            b"SteamAppId=0\0".as_slice(),
            b"X=SteamAppId=730\0",
            b"SteamAppId=-1\0",
            b"SteamAppId=\0",
            b"SteamAppId=invalid\0",
        ] {
            assert!(!steam_marker(bytes));
        }
    }
}
