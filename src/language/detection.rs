// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

//! Resolve OS preferences against catalogs actually present in this build.

pub(super) fn choose(manual: &str, system: &[String], available: &[String]) -> String {
    let preferences = if manual.is_empty() {
        system.to_vec()
    } else {
        vec![manual.to_string()]
    };
    for locale in preferences {
        let locale = locale.trim();
        let locale = locale.split('.').next().unwrap_or(locale).replace('-', "_");
        if locale.eq_ignore_ascii_case("C") || locale.eq_ignore_ascii_case("POSIX") {
            return "en".into();
        }
        if !super::valid_code(&locale) {
            continue;
        }
        let mut candidates = vec![locale.clone()];
        let parts: Vec<_> = locale.split('_').collect();
        // The shipped Chinese catalogs use region names rather than BCP 47
        // script tags. Do not turn Traditional Chinese into Simplified Chinese.
        if parts[0] == "zh" {
            if parts
                .iter()
                .any(|p| ["hant", "tw", "hk", "mo"].contains(&p.to_ascii_lowercase().as_str()))
            {
                candidates.push("zh_TW".into());
            } else if parts
                .iter()
                .any(|p| ["hans", "cn", "sg"].contains(&p.to_ascii_lowercase().as_str()))
            {
                candidates.push("zh_CN".into());
            }
        }
        if parts.len() > 2 {
            candidates.push(parts[..2].join("_"));
        }
        let base = parts[0].split('@').next().unwrap();
        candidates.push(base.to_string());
        for candidate in candidates {
            if candidate == "en" {
                return "en".into();
            }
            if let Some(code) = available
                .iter()
                .find(|code| code.eq_ignore_ascii_case(&candidate))
            {
                return code.clone();
            }
        }
    }
    "en".into()
}

#[cfg(target_os = "android")]
pub(super) fn system_languages() -> Vec<String> {
    // Supplied from Resources.Configuration before GTK's native runtime starts.
    // This also respects Android 13+ per-app language settings.
    std::env::var("HOPEA_SYSTEM_LANGUAGES")
        .unwrap_or_default()
        .split(',')
        .map(str::to_owned)
        .collect()
}

#[cfg(target_os = "windows")]
pub(super) fn system_languages() -> Vec<String> {
    use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    let mut count = 0;
    let mut size = 0;
    unsafe {
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            std::ptr::null_mut(),
            &mut size,
        ) == 0
            || size == 0
            || size > 65536
        {
            return Vec::new();
        }
        let mut buffer = vec![0u16; size as usize];
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            buffer.as_mut_ptr(),
            &mut size,
        ) == 0
        {
            return Vec::new();
        }
        buffer
            .split(|c| *c == 0)
            .filter(|part| !part.is_empty())
            .filter_map(|part| String::from_utf16(part).ok())
            .collect()
    }
}

#[cfg(target_os = "macos")]
pub(super) fn system_languages() -> Vec<String> {
    objc2::rc::autoreleasepool(|_| {
        objc2_foundation::NSLocale::preferredLanguages()
            .iter()
            .map(|language| language.to_string())
            .collect()
    })
}

#[cfg(not(any(target_os = "android", target_os = "windows", target_os = "macos")))]
pub(super) fn system_languages() -> Vec<String> {
    unix_languages(|key| std::env::var(key).ok())
}

#[cfg(any(
    test,
    not(any(target_os = "android", target_os = "windows", target_os = "macos"))
))]
fn unix_languages(get: impl Fn(&str) -> Option<String>) -> Vec<String> {
    // LC_ALL overrides the message category, which overrides LANG. LANGUAGE
    // supplies an ordered list only outside the explicit C/POSIX locale.
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|key| get(key).filter(|value| !value.is_empty()));
    if matches!(locale.as_deref(), Some("C" | "POSIX")) {
        return vec!["en".into()];
    }
    if let Some(languages) = get("LANGUAGE").filter(|value| !value.is_empty()) {
        return languages.split(':').map(str::to_owned).collect();
    }
    locale.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_language_tags_and_missing_catalogs_have_explicit_fallbacks() {
        let catalogs: Vec<_> = ["ru", "pt", "pt_BR", "en_GB", "zh_CN", "zh_TW", "az"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        for (preferences, expected) in [
            (vec!["ru-RU"], "ru"),       // Android / Windows / macOS
            (vec!["ru_RU.UTF-8"], "ru"), // Linux / BSD
            (vec!["pt-BR"], "pt_BR"),
            (vec!["pt-PT"], "pt"),
            (vec!["zh-Hant-HK"], "zh_TW"),
            (vec!["zh-Hans-CN"], "zh_CN"),
            (vec!["en-US", "ru-RU"], "en"),
            (vec!["xx-YY", "az-AZ"], "az"),
            (vec!["xx-YY"], "en"),
            (vec!["C.UTF-8"], "en"),
            (vec!["../../ru", "\u{fffd}"], "en"),
            (vec![], "en"),
        ] {
            let system: Vec<_> = preferences.iter().map(|s| s.to_string()).collect();
            assert_eq!(choose("", &system, &catalogs), expected, "{preferences:?}");
        }
        assert_eq!(choose("en", &["ru-RU".into()], &catalogs), "en");
        assert_eq!(choose("ru", &["en-US".into()], &catalogs), "ru");
        assert_eq!(choose("fr", &["ru-RU".into()], &catalogs), "en");
    }

    #[test]
    fn unix_message_locale_precedence() {
        let resolve = |pairs: &[(&str, &str)]| {
            unix_languages(|key| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string())
            })
        };
        assert_eq!(resolve(&[("LANG", "ru_RU.UTF-8")]), ["ru_RU.UTF-8"]);
        assert_eq!(resolve(&[("LC_ALL", "C"), ("LANGUAGE", "ru:en")]), ["en"]);
        assert_eq!(
            resolve(&[("LC_MESSAGES", "pt_BR.UTF-8"), ("LANG", "ru_RU.UTF-8")]),
            ["pt_BR.UTF-8"]
        );
        assert_eq!(
            resolve(&[("LANGUAGE", "ru:en"), ("LANG", "en_US.UTF-8")]),
            ["ru", "en"]
        );
        assert!(resolve(&[]).is_empty());
    }
}
