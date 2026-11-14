// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSBundle, NSString};

pub(super) fn detect() -> bool {
    autoreleasepool(|_| {
        let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
            return false;
        };
        if app.isTerminated() {
            return false;
        }
        let Some(url) = app.bundleURL() else {
            return false;
        };
        let Some(bundle) = NSBundle::bundleWithURL(&url) else {
            return false;
        };
        let Some(value) =
            bundle.objectForInfoDictionaryKey(&NSString::from_str("LSApplicationCategoryType"))
        else {
            return false;
        };
        let Ok(category) = value.downcast::<NSString>() else {
            return false;
        };
        // Apple's game subcategories end in "-games" (action-games, etc.).
        let category = category.to_string();
        category == "public.app-category.games"
            || (category.starts_with("public.app-category.") && category.ends_with("-games"))
    })
}
