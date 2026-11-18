// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::i18n::i18n;
use adw::prelude::*;
use gtk::{gio, glib};
use std::{io::Read, path::PathBuf};

mod detection;

/// Call only during startup, before application workers and GTK widgets exist.
pub fn initialize() {
    // GTK must not later reset our message locale from an unavailable or C
    // environment locale while constructing the first window.
    gtk::disable_setlocale();
    #[cfg(target_os = "android")]
    initialize_android_backend();
    let available: Vec<_> = available_languages(&catalog_dir())
        .into_iter()
        .map(|(code, _, _)| code)
        .collect();
    let language = detection::choose(&preference(), &detection::system_languages(), &available);
    // On Windows GLib updates both the Win32 and C-runtime environment;
    // gettext reads the latter, which std::env alone need not synchronize.
    if let Err(error) = unsafe { glib::setenv("LANGUAGE", &language, true) } {
        eprintln!("Hopea: unable to set the translation language: {error}");
    }
    #[cfg(target_os = "android")]
    {
        // Bionic only implements C/C.UTF-8. GNU gettext on Android therefore
        // reads the message locale from the environment instead of setlocale.
        std::env::remove_var("LC_ALL");
        std::env::set_var("LC_MESSAGES", format!("{language}.UTF-8"));
    }
    // GNU gettext ignores LANGUAGE in the plain C locale. Native GUI launches
    // on Android/macOS/Windows often have no POSIX locale environment at all.
    // gettext-rs' LocaleCategory enum uses glibc's numeric values, which
    // differ on Windows and BSD/macOS. Use each target's libc constants.
    #[cfg(windows)]
    let messages_category = libc::LC_ALL;
    #[cfg(not(windows))]
    let messages_category = libc::LC_MESSAGES;
    set_locale(libc::LC_ALL, "");
    let current = set_locale(messages_category, "");
    if current
        .as_deref()
        .is_none_or(|value| value == b"C" || value == b"POSIX" || value.starts_with(b"C."))
    {
        for locale in ["en_US.UTF-8", "en_US.utf8", ".UTF-8", "C.UTF-8"] {
            if set_locale(messages_category, locale).is_some() {
                break;
            }
        }
    }
}

// Only called by initialize(), before GTK and application workers start.
fn set_locale(category: libc::c_int, locale: &str) -> Option<Vec<u8>> {
    let locale = std::ffi::CString::new(locale).ok()?;
    unsafe {
        let result = libc::setlocale(category, locale.as_ptr());
        (!result.is_null()).then(|| std::ffi::CStr::from_ptr(result).to_bytes().to_vec())
    }
}

#[cfg(target_os = "android")]
fn initialize_android_backend() {
    // GLib's Android build uses proxy-libintl. Connect it to the very same
    // GNU gettext instance as Rust, before either side creates translated UI.
    #[link(name = "intl")]
    unsafe extern "C" {
        fn g_libintl_use_backend(handle: *mut libc::c_void) -> libc::c_int;
    }
    unsafe {
        let handle = libc::dlopen(
            b"libhopea.so\0".as_ptr().cast(),
            libc::RTLD_NOW | libc::RTLD_NOLOAD,
        );
        if g_libintl_use_backend(handle) == 0 {
            eprintln!("Hopea: native translation backend unavailable; using English UI fallback");
        }
        if !handle.is_null() {
            libc::dlclose(handle);
        }
    }
}

// Optional friendly labels for known languages, not a whitelist. Any installed
// po/<locale>.po catalog is selectable; new locales fall back to their code.
const LANGUAGES: &[(&str, &str, &str)] = &[
    ("en", "English", "Select language"),
    ("ru", "Русский", "Выберите язык"),
    ("be", "Беларуская", "Выберыце мову"),
    ("bg", "Български", "Изберете език"),
    ("ca", "Català", "Trieu l’idioma"),
    ("cs", "Čeština", "Vyberte jazyk"),
    ("da", "Dansk", "Vælg sprog"),
    ("de", "Deutsch", "Sprache auswählen"),
    ("el", "Ελληνικά", "Επιλέξτε γλώσσα"),
    ("en_GB", "English (UK)", "Select language"),
    ("eo", "Esperanto", "Elektu lingvon"),
    ("es", "Español", "Selecciona el idioma"),
    ("eu", "Euskara", "Hautatu hizkuntza"),
    ("fi", "Suomi", "Valitse kieli"),
    ("fr", "Français", "Choisissez la langue"),
    ("fur", "Furlan", "Sielç la lenghe"),
    ("gl", "Galego", "Escolle o idioma"),
    ("he", "עברית", "בחרו שפה"),
    ("hi", "हिन्दी", "भाषा चुनें"),
    ("hr", "Hrvatski", "Odaberite jezik"),
    ("hu", "Magyar", "Válasszon nyelvet"),
    ("ia", "Interlingua", "Selige le lingua"),
    ("is", "Íslenska", "Veldu tungumál"),
    ("it", "Italiano", "Seleziona la lingua"),
    ("ka", "ქართული", "აირჩიეთ ენა"),
    ("kk", "Қазақша", "Тілді таңдаңыз"),
    ("ko", "한국어", "언어 선택"),
    ("kw", "Kernewek", "Dewis yeth"),
    ("lt", "Lietuvių", "Pasirinkite kalbą"),
    ("ml", "മലയാളം", "ഭാഷ തിരഞ്ഞെടുക്കുക"),
    ("my", "မြန်မာ", "ဘာသာစကား ရွေးချယ်ပါ"),
    ("nb", "Norsk bokmål", "Velg språk"),
    ("ne", "नेपाली", "भाषा छान्नुहोस्"),
    ("nl", "Nederlands", "Kies een taal"),
    ("nn", "Norsk nynorsk", "Vel språk"),
    ("oc", "Occitan", "Causissètz la lenga"),
    ("pl", "Polski", "Wybierz język"),
    ("pt", "Português", "Escolha o idioma"),
    ("pt_BR", "Português (Brasil)", "Escolha o idioma"),
    ("ro", "Română", "Alegeți limba"),
    ("sk", "Slovenčina", "Vyberte jazyk"),
    ("sl", "Slovenščina", "Izberite jezik"),
    ("sr", "Српски", "Изаберите језик"),
    ("sv", "Svenska", "Välj språk"),
    ("te", "తెలుగు", "భాషను ఎంచుకోండి"),
    ("th", "ไทย", "เลือกภาษา"),
    ("uk", "Українська", "Виберіть мову"),
    ("vi", "Tiếng Việt", "Chọn ngôn ngữ"),
    ("zh_CN", "简体中文", "选择语言"),
    ("zh_TW", "繁體中文", "選擇語言"),
];

fn preference_path() -> PathBuf {
    glib::user_config_dir().join("hopea").join("language")
}

fn valid_code(code: &str) -> bool {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\A[a-z]{2,3}(?:_[A-Za-z0-9]{2,8}){0,2}(?:@[a-z0-9-]{1,16})?\z").unwrap()
    });
    PATTERN.is_match(code)
}

fn available_languages(directory: &std::path::Path) -> Vec<(String, String, String)> {
    let mut codes = std::collections::BTreeSet::from(["en".to_string()]);
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let Some(code) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if valid_code(&code) && entry.path().join("LC_MESSAGES/hopea.mo").is_file() {
                codes.insert(code);
            }
        }
    }
    codes
        .into_iter()
        .map(|code| {
            let (name, invitation) = LANGUAGES
                .iter()
                .find(|(known, _, _)| *known == code)
                .map(|(_, name, invitation)| ((*name).to_string(), (*invitation).to_string()))
                .unwrap_or_else(|| (code.clone(), i18n("Select language")));
            (code, name, invitation)
        })
        .collect()
}

pub fn preference() -> String {
    let mut value = String::new();
    if let Ok(file) = std::fs::File::open(preference_path()) {
        let _ = file.take(64).read_to_string(&mut value);
    }
    let value = value.trim();
    if valid_code(value) {
        value.into()
    } else {
        String::new()
    }
}

pub fn catalog_dir() -> PathBuf {
    #[cfg(target_os = "android")]
    if let Some(directory) = std::env::var_os("HOPEA_LOCALE_DIR") {
        return PathBuf::from(directory);
    }
    if std::env::var_os("MESON_DEVENV").is_some()
        || std::env::current_exe().ok().is_some_and(|exe| {
            exe.starts_with(
                std::path::Path::new(crate::config::DEVEL_LOCALEDIR)
                    .parent()
                    .unwrap(),
            )
        })
    {
        return crate::config::DEVEL_LOCALEDIR.into();
    }
    let installed = PathBuf::from(crate::config::LOCALEDIR);
    crate::beside_program("share/locale")
        .filter(|_| !installed.exists())
        .unwrap_or(installed)
}

pub struct Settings {
    pub group: adw::PreferencesGroup,
    row: adw::ComboRow,
    codes: Vec<String>,
}

impl Settings {
    pub fn new() -> Self {
        let directory = catalog_dir();
        let available = available_languages(&directory);
        let group = adw::PreferencesGroup::builder()
            .description(i18n(
                "Language changes apply after restarting Hopea. Some translations are incomplete.",
            ))
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let invitation = gtk::Stack::builder()
            .name("language-invitation")
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(500)
            .hhomogeneous(false)
            .build();
        let phrases: Vec<_> = available
            .iter()
            .map(|(_, _, phrase)| phrase.clone())
            .collect();
        for (i, phrase) in phrases.iter().enumerate() {
            let label = gtk::Label::builder()
                .label(phrase)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .css_classes(["heading"])
                .build();
            invitation.add_named(&label, Some(&i.to_string()));
        }
        invitation.set_visible_child_name("0");
        let weak = invitation.downgrade();
        let mut index = 0;
        glib::timeout_add_seconds_local(3, move || {
            let Some(stack) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if stack.is_mapped() && stack.settings().is_gtk_enable_animations() {
                index = (index + 1) % phrases.len();
                stack.set_visible_child_name(&index.to_string());
            }
            glib::ControlFlow::Continue
        });
        content.append(&invitation);
        let mut names = vec![i18n("System language")];
        names.extend(available.iter().map(|(_, name, _)| name.to_string()));
        let mut codes = vec![String::new()];
        codes.extend(available.iter().map(|(code, _, _)| code.clone()));
        let model = gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let row = adw::ComboRow::builder()
            .name("language-choice")
            .title(i18n("Language"))
            .model(&model)
            .selected(
                codes
                    .iter()
                    .position(|code| *code == preference())
                    .unwrap_or(0) as u32,
            )
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        list.append(&row);
        content.append(&list);
        group.add(&content);
        Self { group, row, codes }
    }

    pub fn apply(&self) -> Result<(), String> {
        let code = self
            .codes
            .get(self.row.selected() as usize)
            .map(String::as_str)
            .unwrap_or("");
        if code == preference() {
            return Ok(());
        }
        let path = preference_path();
        std::fs::create_dir_all(path.parent().unwrap())
            .map_err(|_| i18n("Unable to save language"))?;
        gio::File::for_path(path)
            .replace_contents(
                code.as_bytes(),
                None,
                false,
                gio::FileCreateFlags::PRIVATE,
                gio::Cancellable::NONE,
            )
            .map(|_| ())
            .map_err(|_| i18n("Unable to save language"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(any(target_os = "android", target_os = "windows", target_os = "macos")))]
    fn startup_uses_system_language_manual_choice_and_english_fallback() {
        const EXPECTED: &str = "HOPEA_TEST_EXPECTED_TRANSLATION";
        if let Ok(expected) = std::env::var(EXPECTED) {
            initialize();
            gettextrs::bindtextdomain("hopea", catalog_dir()).unwrap();
            gettextrs::bind_textdomain_codeset("hopea", "UTF-8").unwrap();
            gettextrs::textdomain("hopea").unwrap();
            if std::env::var_os("HOPEA_TEST_TRANSLATION_GTK").is_some() {
                adw::init().unwrap();
            }
            assert_eq!(
                i18n("For You"),
                expected,
                "language={:?}, catalogs={:?}",
                std::env::var("LANGUAGE"),
                catalog_dir()
            );
            return;
        }
        // Locale/gettext caches are process-global. Test real startup in fresh
        // processes with temporary preferences, never mutate the test runner's
        // locale or use the listener's config directory.
        let root = std::env::temp_dir().join(format!(
            "hopea-locale-startup-{}",
            glib::uuid_string_random()
        ));
        std::fs::create_dir_all(root.join("hopea")).unwrap();
        for (manual, os_language, expected) in [
            ("", "ru_RU.UTF-8", "Для вас"),
            ("en", "ru_RU.UTF-8", "For You"),
            ("ru", "en_US.UTF-8", "Для вас"),
            ("ru", "C.UTF-8", "Для вас"),
            ("", "xx_YY.UTF-8", "For You"),
            ("", "", "For You"),
        ] {
            std::fs::write(root.join("hopea/language"), manual).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "language::tests::startup_uses_system_language_manual_choice_and_english_fallback", "--nocapture"])
                .env(EXPECTED, expected)
                .env("XDG_CONFIG_HOME", &root)
                .env("LANG", os_language)
                .env_remove("LANGUAGE")
                .env_remove("LC_ALL")
                .env_remove("LC_MESSAGES")
                .output().unwrap();
            assert!(
                output.status.success(),
                "manual={manual:?}, os={os_language:?}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn known_languages_have_valid_codes_and_labels() {
        let mut codes = std::collections::HashSet::new();
        for (code, name, invitation) in LANGUAGES {
            assert!(codes.insert(*code));
            assert!(valid_code(code));
            assert!(!name.is_empty() && !invitation.is_empty());
        }
        for code in [
            "ar",
            "fa",
            "tr",
            "az",
            "uz",
            "id",
            "sr_Latn_RS",
            "sr@latin",
            "es_419",
        ] {
            assert!(valid_code(code));
        }
        for code in [
            "",
            "../../ru",
            "ru/",
            "ru.po",
            "ru\n",
            "ru:en",
            "System language",
        ] {
            assert!(!valid_code(code));
        }
    }

    #[test]
    fn installed_catalogs_are_discovered_without_a_rust_registry_entry() {
        let directory =
            std::env::temp_dir().join(format!("hopea-languages-{}", glib::uuid_string_random()));
        for code in ["ru", "az", "sr_Latn_RS", "not-a-language"] {
            let messages = directory.join(code).join("LC_MESSAGES");
            std::fs::create_dir_all(&messages).unwrap();
            std::fs::write(messages.join("hopea.mo"), b"fixture").unwrap();
        }
        std::fs::create_dir_all(directory.join("fr")).unwrap();
        let languages = available_languages(&directory);
        assert_eq!(
            languages
                .iter()
                .map(|(code, _, _)| code.as_str())
                .collect::<Vec<_>>(),
            ["az", "en", "ru", "sr_Latn_RS"]
        );
        assert_eq!(
            languages
                .iter()
                .find(|(code, _, _)| code == "az")
                .unwrap()
                .1,
            "az"
        );
        assert_eq!(
            languages
                .iter()
                .find(|(code, _, _)| code == "ru")
                .unwrap()
                .1,
            "Русский"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
