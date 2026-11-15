// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

impl LibraryPanel {
    pub(super) async fn show_sign_in_help(&self, service: u32) {
        let (title, instructions, example, documentation) = match service {
            0 => (
                "VK",
                i18n("1. Open https://vk.ru/audio in your usual browser and sign in.\n2. Press F12 (or Ctrl+Shift+I), open Network, then reload the page. Select a request to vk.ru, such as audio or al_audio.php.\n3. In Request Headers, copy the VALUE of Cookie, without the word Cookie:. Use request cookies, not response Set-Cookie headers.\n4. Paste it into Session Cookie or Access Token in Hopea.\n\nKeep the full Cookie value. It must contain remixsid or remixsid6; keep both when present. Also keep remixmid if present: it identifies the signed-in account. If it is absent, Hopea checks the signed-in account on VK. Do not substitute the profile you want to import.\n\nVK Profile ID to Import is a separate field: enter the numeric ID of the music owner, for example 123456789.\n\nIn Zen/Firefox you can also find these names under Storage → Cookies → https://vk.ru; in Chrome under Application → Cookies. Copy name=value pairs separated by a semicolon and a space."),
                "remixsid=YOUR_VALUE; remixsid6=YOUR_VALUE; remixmid=YOUR_SIGNED_IN_ID",
                None,
            ),
            1 => (
                "Yandex Music",
                i18n("Use Sign In with Browser to get a confirmation code. Enter it on the Yandex page and approve access; Hopea receives the token automatically.\n\nFor manual sign-in, this importer needs a Yandex Music OAuth access token, not browser cookies. Session_id, sessionid2 and yandexuid cannot be pasted here as a replacement.\n\n1. Open the yandex-music library’s token instructions using the link below.\n2. Follow its authorization instructions for your account.\n3. Paste only the resulting access token into Hopea, without quotes or the prefix OAuth.\n\nA successful sign-in on music.yandex.ru alone does not produce this token. Access to audio depends on your account."),
                "",
                Some("https://ym.marshal.dev/token/"),
            ),
            2 => (
                "Spotify",
                i18n("Use Sign In with Browser and enter your Spotify developer application’s public Client ID. Register http://127.0.0.1:43821/callback as its Redirect URI. Complete authorization in the browser; Hopea receives the token automatically. No Client Secret is needed.\n\nFor manual sign-in, this importer needs a Spotify OAuth access token, not cookies such as sp_dc or sp_key.\n\n1. Use a Spotify developer application and its user authorization flow, described at the link below. Client Credentials tokens cannot read your personal library.\n2. Request user-library-read, playlist-read-private and playlist-read-collaborative. For direct audio import also request streaming.\n3. Paste only access_token into Hopea, without quotes or the prefix Bearer. Do not paste a client secret or refresh token.\n\nIf the token has expired, authorize again. Direct audio requires Premium and a compatible account; you can turn off audio download to import the catalog only."),
                "",
                Some("https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow"),
            ),
            _ => (
                "YouTube Music",
                i18n("1. Open https://music.youtube.com and sign in. Press F12 (or Ctrl+Shift+I) and open Network.\n2. Open Library and filter requests by /browse. Select a successful POST request to music.youtube.com.\n3. From Request Headers copy the values of Cookie, Authorization and X-Goog-AuthUser into the JSON template below. Paste the completed JSON into Hopea.\n\nCopy the entire Cookie value, including SAPISID and __Secure-3PAPISID when present. Do not copy only one cookie. Keep Authorization exactly as sent by the browser, and use the request’s X-Goog-AuthUser value (usually 0). The JSON must use double quotes.\n\nIn Zen/Firefox these are under Headers → Request Headers. In Chrome use the same section; if cookies are hidden, Copy as fetch (Node.js) includes them. Extract only the headers; do not execute the copied code."),
                "{\n  \"cookie\": \"PASTE_COOKIE_VALUE\",\n  \"authorization\": \"PASTE_AUTHORIZATION_VALUE\",\n  \"x-goog-authuser\": \"0\",\n  \"x-origin\": \"https://music.youtube.com\",\n  \"content-type\": \"application/json\"\n}",
                Some("https://ytmusicapi.readthedocs.io/en/stable/setup/browser.html"),
            ),
        };
        let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
        let text = gtk::Label::builder()
            .label(instructions)
            .wrap(true)
            .selectable(true)
            .xalign(0.0)
            .max_width_chars(64)
            .build();
        content.append(&text);
        if !example.is_empty() {
            content.append(&gtk::Label::builder()
                .label(i18n("Example only — replace the placeholders with your own values; omit cookies that are absent."))
                .wrap(true).xalign(0.0).max_width_chars(64).build());
            content.append(
                &gtk::Label::builder()
                    .label(example)
                    .selectable(true)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .xalign(0.0)
                    .css_classes(["monospace"])
                    .max_width_chars(64)
                    .build(),
            );
        }
        if let Some(uri) = documentation {
            content.append(&gtk::LinkButton::with_label(
                uri,
                &i18n("Authorization Instructions"),
            ));
        }
        content.append(&gtk::Label::builder()
            .label(i18n("These values grant access to your account. Paste them only into Hopea’s sign-in field, never into chat, screenshots or bug reports. Hopea uses them for this import and does not save them."))
            .wrap(true).xalign(0.0).max_width_chars(64).build());
        let scroll = gtk::ScrolledWindow::builder()
            .child(&content)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(220)
            .max_content_height(460)
            .propagate_natural_height(true)
            .build();
        let dialog = adw::AlertDialog::builder()
            .heading(format!("{} — {}", title, i18n("Manual Sign-In")))
            .extra_child(&scroll)
            .build();
        dialog.add_response("close", &i18n("_Back"));
        dialog.choose_future(self.window().as_ref()).await;
    }
}
