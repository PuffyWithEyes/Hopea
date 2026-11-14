// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::i18n::i18n;
use adw::prelude::*;
use gtk::{gio, glib};
use std::{io::Read, path::PathBuf};

// Native names and invitations stay readable even before a language is chosen.
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

pub fn preference() -> String {
    let mut value = String::new();
    if let Ok(file) = std::fs::File::open(preference_path()) {
        let _ = file.take(64).read_to_string(&mut value);
    }
    let value = value.trim();
    if LANGUAGES.iter().any(|(code, _, _)| *code == value) {
        value.into()
    } else {
        String::new()
    }
}

pub fn catalog_dir() -> PathBuf {
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
    codes: Vec<&'static str>,
}

impl Settings {
    pub fn new() -> Self {
        let directory = catalog_dir();
        let available: Vec<_> = LANGUAGES
            .iter()
            .filter(|(code, _, _)| {
                *code == "en" || directory.join(code).join("LC_MESSAGES/hopea.mo").is_file()
            })
            .collect();
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
        let phrases: Vec<_> = available.iter().map(|(_, _, phrase)| *phrase).collect();
        for (i, phrase) in phrases.iter().enumerate() {
            let label = gtk::Label::builder()
                .label(*phrase)
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
        let mut codes = vec![""];
        codes.extend(available.iter().map(|(code, _, _)| *code));
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
            .copied()
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
    fn every_shipped_translation_has_a_native_name_and_invitation() {
        let mut codes = std::collections::HashSet::new();
        for (code, name, invitation) in LANGUAGES {
            assert!(codes.insert(*code));
            assert!(!name.is_empty() && !invitation.is_empty());
        }
        for code in ["ar", "fa", "tr", "az", "uz", "id"] {
            assert!(!codes.contains(code));
        }
        for code in include_str!("../po/LINGUAS").split_whitespace() {
            assert!(codes.contains(code), "missing language {}", code);
        }
    }
}
