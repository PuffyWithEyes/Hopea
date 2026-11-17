// SPDX-FileCopyrightText: 2022  John Toohey <john_t@mailo.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::{glib, prelude::*, subclass::prelude::*};

/// Editing only Unicode whitespace must not restart a search or reset its UI.
pub(crate) fn text_key(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

mod imp {

    use std::cell::RefCell;

    use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
    use gtk::{
        glib::{self, ParamSpec, ParamSpecString, Value},
        prelude::*,
        subclass::prelude::*,
    };
    use once_cell::sync::Lazy;

    use crate::audio::Song;

    #[derive(Default)]
    pub struct FuzzyFilter {
        pub search: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FuzzyFilter {
        const NAME: &'static str = "HopeaFuzzyFilter";
        type Type = super::FuzzyFilter;
        type ParentType = gtk::Filter;
    }

    impl ObjectImpl for FuzzyFilter {
        fn properties() -> &'static [ParamSpec] {
            static PROPERTIES: Lazy<Vec<ParamSpec>> =
                Lazy::new(|| vec![ParamSpecString::builder("search").build()]);
            PROPERTIES.as_ref()
        }

        fn set_property(&self, _id: usize, value: &Value, pspec: &ParamSpec) {
            match pspec.name() {
                "search" => {
                    let p = value
                        .get::<Option<String>>()
                        .expect("Value must be a string");
                    self.obj().set_search(p);
                }
                _ => unimplemented!(),
            }
        }
    }

    impl FilterImpl for FuzzyFilter {
        fn strictness(&self) -> gtk::FilterMatch {
            gtk::FilterMatch::Some
        }

        fn match_(&self, song: &glib::Object) -> bool {
            let song = song.downcast_ref::<Song>().unwrap();

            if let Some(search) = self.search.borrow().as_ref() {
                let key = song.search_key();
                let matcher = SkimMatcherV2::default();
                matcher.fuzzy_match(&key, search).is_some() || search.is_empty()
            } else {
                true
            }
        }
    }
}

glib::wrapper! {
    pub struct FuzzyFilter(ObjectSubclass<imp::FuzzyFilter>)
        @extends gtk::Filter;

}

impl Default for FuzzyFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl FuzzyFilter {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn search(&self) -> Option<String> {
        self.imp().search.borrow().as_ref().map(ToString::to_string)
    }

    pub fn set_search(&self, search: Option<String>) {
        let search = search.map(|s| {
            s.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        });
        if text_key(self.imp().search.borrow().as_deref().unwrap_or_default())
            != text_key(search.as_deref().unwrap_or_default())
        {
            *self.imp().search.borrow_mut() = search;
            self.changed(gtk::FilterChange::Different);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::text_key;

    #[test]
    fn whitespace_edits_do_not_change_search_identity() {
        let original = text_key("Егор Летов");
        for query in [
            " Егор Летов ",
            "Егор\tЛетов",
            "Егор\nЛетов",
            "Егор\u{a0}\u{2003}Летов",
            "ЕгорЛетов",
        ] {
            assert_eq!(text_key(query), original);
        }
        assert_eq!(text_key("\t\n\u{a0} "), "");
        assert_ne!(text_key("Егор Летов Н"), original);
    }
}
