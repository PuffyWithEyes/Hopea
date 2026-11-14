// SPDX-FileCopyrightText: 2026  Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Default browser and profile detection. The service adapter reads the
//! existing browser session only after an explicit Continue.

use std::path::{Path, PathBuf};

use gtk::{gio, glib, prelude::*};

use crate::i18n::i18n;

pub fn sandboxed() -> bool {
    Path::new("/.flatpak-info").exists()
}

/// GTK's URI launcher can fail to complete when the desktop portal exists
/// without an OpenURI backend. Check it before starting a sandboxed launch.
pub async fn require_browser_portal() -> Result<(), String> {
    if !sandboxed() {
        return Ok(());
    }
    let available = async {
        let bus = gio::bus_get_future(gio::BusType::Session).await.ok()?;
        let reply = bus
            .call_future(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.DBus.Properties",
                "Get",
                Some(&("org.freedesktop.portal.OpenURI", "version").to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                3000,
            )
            .await
            .ok()?;
        reply.child_value(0).as_variant()?.get::<u32>()
    }
    .await;
    if available.is_some_and(|version| version > 0) {
        Ok(())
    } else {
        Err(i18n("Your desktop does not provide the browser-opening portal needed by Flatpak. Install or enable an OpenURI backend such as xdg-desktop-portal-gtk or your desktop’s portal package, then sign out and back in. You can also open the music service yourself and paste its session data here."))
    }
}

/// Detects the application that will actually handle the HTTPS login link.
pub fn default_browser() -> Result<String, String> {
    let app = gio::AppInfo::default_for_uri_scheme("https")
        .ok_or_else(|| i18n("No default web browser is configured"))?;
    let identity = format!(
        "{} {} {}",
        app.id().unwrap_or_default(),
        app.executable().display(),
        app.display_name()
    )
    .to_lowercase();
    browser_kind(&identity).map(str::to_owned).ok_or_else(|| i18n("The default browser’s saved sessions are not supported. Choose a supported default browser or paste session data."))
}

fn browser_kind(identity: &str) -> Option<&'static str> {
    // Zen uses Firefox's cookie format, but a separate profile directory.
    if identity
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|part| part == "zen")
    {
        return Some("zen");
    }
    for (marker, browser) in [
        ("firefox", "firefox"),
        ("brave", "brave"),
        ("chromium", "chromium"),
        ("chrome", "chrome"),
        ("msedge", "edge"),
        ("microsoft-edge", "edge"),
        ("vivaldi", "vivaldi"),
        ("opera", "opera"),
        ("safari", "safari"),
    ] {
        if identity.contains(marker) {
            return Some(browser);
        }
    }
    None
}

/// Resolve the profile after the browser has opened and the user pressed Continue.
pub fn saved_browser(browser: &str) -> Result<(String, Option<PathBuf>), String> {
    if browser != "zen" {
        return Ok((browser.into(), None));
    }
    let home = glib::home_dir();
    let mut roots = vec![
        glib::user_config_dir().join("zen"),
        home.join(".zen"),
        home.join(".var/app/app.zen_browser.zen/config/zen"),
        home.join(".var/app/app.zen_browser.zen/.zen"),
    ];
    #[cfg(target_os = "windows")]
    if let Some(roaming) = std::env::var_os("APPDATA") {
        roots.insert(0, PathBuf::from(roaming).join("zen"));
    }
    #[cfg(target_os = "macos")]
    roots.insert(0, home.join("Library/Application Support/zen"));
    roots.dedup();
    for root in roots {
        if root.join("profiles.ini").is_file() {
            return zen_profile(&root).map(|profile| ("firefox".into(), Some(profile)));
        }
    }
    Err(i18n(
        "Zen’s profile was not found. Open Zen in a normal window, sign in and try Continue again.",
    ))
}

fn zen_profile(root: &Path) -> Result<PathBuf, String> {
    let ini = glib::KeyFile::new();
    ini.load_from_file(root.join("profiles.ini"), glib::KeyFileFlags::NONE)
        .map_err(|_| i18n("Unable to read Zen’s profile list"))?;
    let mut defaults = Vec::new();
    let mut profiles = Vec::new();
    for section in ini.groups().iter() {
        if section.starts_with("Install") {
            if let Ok(path) = ini.string(section, "Default") {
                defaults.push(root.join(path.as_str()));
            }
        } else if section.starts_with("Profile") {
            if let Ok(path) = ini.string(section, "Path") {
                let path = if ini.integer(section, "IsRelative").unwrap_or(1) == 1 {
                    root.join(path.as_str())
                } else {
                    PathBuf::from(path.as_str())
                };
                if ini.integer(section, "Default").unwrap_or(0) == 1 {
                    defaults.push(path.clone());
                }
                profiles.push(path);
            }
        }
    }
    let mut candidates = if defaults.is_empty() {
        profiles
    } else {
        defaults
    };
    candidates.sort();
    candidates.dedup();
    candidates.retain(|p| p.join("cookies.sqlite").is_file());
    match candidates.as_slice() {
        [profile] => Ok(profile.clone()),
        [] => Err(i18n("Zen has not saved a browser session yet. Sign in in a normal window and try Continue again.")),
        _ => Err(i18n("Several default Zen profiles were found. Set a single default profile in Zen and sign in there.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_zen_without_confusing_it_with_firefox() {
        assert_eq!(
            browser_kind("zen.desktop /nix/store/browser/bin/zen zen browser"),
            Some("zen")
        );
        assert_eq!(browser_kind("app.zen_browser.zen.desktop zen"), Some("zen"));
        assert_eq!(browser_kind("firefox.desktop firefox"), Some("firefox"));
        assert_eq!(browser_kind("frozen-browser"), None);
    }

    #[test]
    fn zen_uses_its_declared_default_profile_and_rejects_ambiguity() {
        let root = std::env::temp_dir().join(format!("hopea-zen-profiles-{}", std::process::id()));
        for name in ["work", "personal"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join("cookies.sqlite"), []).unwrap();
        }
        std::fs::write(root.join("profiles.ini"), "[Profile0]\nPath=work\nIsRelative=1\n[Profile1]\nPath=personal\nIsRelative=1\nDefault=1\n[InstallABC]\nDefault=personal\n").unwrap();
        assert_eq!(zen_profile(&root).unwrap(), root.join("personal"));
        std::fs::write(
            root.join("profiles.ini"),
            "[Profile0]\nPath=work\nDefault=1\n[Profile1]\nPath=personal\nDefault=1\n",
        )
        .unwrap();
        assert!(zen_profile(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
