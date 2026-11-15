// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use futures::future::{select, Either};
use library::service_import::{Event, Worker};

impl LibraryPanel {
    pub(super) async fn sign_in_service(
        &self,
        service: &str,
    ) -> Result<Option<(String, String)>, String> {
        library::browser_import::require_browser_portal().await?;
        let client_id = if service == "spotify" {
            let entry = adw::EntryRow::builder().title("Client ID").build();
            let prompt = adw::AlertDialog::builder()
                .heading(i18n("Spotify Sign-In"))
                .body(i18n("Enter the public Client ID of your Spotify developer application. Register http://127.0.0.1:43821/callback as its Redirect URI. Your account must be allowed to use that application. No Client Secret is needed."))
                .extra_child(&entry).close_response("cancel").default_response("sign-in").build();
            prompt.add_response("cancel", &i18n("_Cancel"));
            prompt.add_response("sign-in", &i18n("_Sign In"));
            prompt.set_response_enabled("sign-in", false);
            entry.connect_changed(clone!(
                #[weak]
                prompt,
                move |entry| {
                    let text = entry.text();
                    let value = text.trim();
                    prompt.set_response_enabled(
                        "sign-in",
                        value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit()),
                    );
                }
            ));
            if prompt.choose_future(self.window().as_ref()).await != "sign-in" {
                return Ok(None);
            }
            entry.text().trim().to_owned()
        } else {
            String::new()
        };
        let worker = Worker::authenticate(service, &client_id).await?;
        let code = gtk::Label::builder()
            .selectable(true)
            .wrap(true)
            .css_classes(["title-1"])
            .build();
        let prompt = adw::AlertDialog::builder()
            .heading(i18n("Signing In"))
            .body(i18n("Connecting to the music service…"))
            .body_use_markup(false)
            .extra_child(&code)
            .close_response("cancel")
            .build();
        prompt.add_response("cancel", &i18n("_Cancel"));
        let result = {
            let flow = async {
                let mut opened = false;
                loop {
                    match worker.next().await? {
                        Some(Event::AuthPrompt { url, user_code }) if !opened => {
                            if !valid_auth_url(service, &url) || user_code.len() > 64 {
                                return Err(i18n("The service returned an invalid sign-in page"));
                            }
                            opened = true;
                            code.set_text(&user_code);
                            prompt.set_body(&if user_code.is_empty() {
                                i18n("Complete sign-in in your browser. Hopea will continue automatically. You can cancel here at any time.")
                            } else {
                                i18n("Enter this code on the page opened in your browser and approve access. Hopea will continue automatically.")
                            });
                            gtk::UriLauncher::new(&url)
                                .launch_future(self.window().as_ref())
                                .await
                                .map_err(|_| i18n("Unable to open the default browser"))?;
                        }
                        Some(Event::Credentials { secret, account }) if opened => {
                            return Ok(Some((secret, account)))
                        }
                        Some(Event::Error { message }) => return Err(message),
                        _ => return Err(i18n("The sign-in process stopped unexpectedly")),
                    }
                }
            };
            wait_for_sign_in(
                &prompt,
                self.window().as_ref(),
                flow,
                std::time::Duration::from_secs(360),
            )
            .await
        };
        // Stop polling and close the loopback listener on cancellation or error.
        if worker.process.identifier().is_some() {
            worker.process.force_exit();
        }
        result
    }
}

type SignInResult = Result<Option<(String, String)>, String>;

struct SignInPrompt {
    dialog: adw::AlertDialog,
    response: Option<glib::SignalHandlerId>,
}

impl Drop for SignInPrompt {
    fn drop(&mut self) {
        if let Some(response) = self.response.take() {
            self.dialog.disconnect(response);
        }
        self.dialog.close();
    }
}

async fn wait_for_sign_in(
    prompt: &adw::AlertDialog,
    parent: Option<&Window>,
    flow: impl std::future::Future<Output = SignInResult>,
    timeout: std::time::Duration,
) -> SignInResult {
    // Dropping choose_future cancels its GTask. libadwaita can return NULL
    // from choose_finish on cancellation, but the Rust binding expects a
    // non-null GString and aborts inside its C callback. A response signal
    // needs no cancellable GTask when the worker finishes first.
    let (sender, receiver) = async_channel::bounded(1);
    let response = prompt.connect_response(None, move |_, _| {
        let _ = sender.try_send(());
    });
    let _guard = SignInPrompt {
        dialog: prompt.clone(),
        response: Some(response),
    };
    AdwDialogExt::present(prompt, parent);
    let bounded = async {
        match select(Box::pin(flow), Box::pin(glib::timeout_future(timeout))).await {
            Either::Left((result, _)) => result,
            Either::Right(_) => Err(i18n("Sign-in timed out. Please try again.")),
        }
    };
    match select(Box::pin(bounded), Box::pin(receiver.recv())).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => Ok(None),
    }
}

fn valid_auth_url(service: &str, url: &str) -> bool {
    let Ok(uri) = glib::Uri::parse(url, glib::UriFlags::NONE) else {
        return false;
    };
    if uri.scheme() != "https" || uri.userinfo().is_some() || !matches!(uri.port(), -1 | 443) {
        return false;
    }
    match (service, uri.host().as_deref()) {
        ("spotify", Some("accounts.spotify.com")) => uri.path() == "/authorize",
        ("yandex", Some("oauth.yandex.ru" | "oauth.yandex.com")) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a GTK display and isolated XDG directories"]
    fn sign_in_dialog_handles_completion_error_timeout_and_cancel() {
        assert_eq!(std::env::var("GSETTINGS_BACKEND").as_deref(), Ok("memory"));
        adw::init().unwrap();
        gtk::Settings::default()
            .unwrap()
            .set_gtk_enable_animations(false);
        let context = glib::MainContext::default();
        context.block_on(async {
            let dialog = || {
                let prompt = adw::AlertDialog::builder().close_response("cancel").build();
                prompt.add_response("cancel", "Cancel");
                prompt
            };
            for expected in [
                Ok(Some(("fixture-token".into(), String::new()))),
                Err("fixture failure".into()),
            ] {
                let prompt = dialog();
                let result = expected.clone();
                let flow = async {
                    glib::timeout_future(std::time::Duration::from_millis(10)).await;
                    result
                };
                assert_eq!(
                    wait_for_sign_in(&prompt, None, flow, std::time::Duration::from_secs(1)).await,
                    expected
                );
                // Drain completion callbacks: the original crash happened in
                // a later main-loop iteration after the losing future dropped.
                glib::timeout_future(std::time::Duration::from_millis(30)).await;
            }
            let prompt = dialog();
            assert!(wait_for_sign_in(
                &prompt,
                None,
                futures::future::pending(),
                std::time::Duration::from_millis(10)
            )
            .await
            .is_err());
            glib::timeout_future(std::time::Duration::from_millis(30)).await;
            let prompt = dialog();
            let flow = async {
                glib::timeout_future(std::time::Duration::from_millis(10)).await;
                prompt.close();
                futures::future::pending::<SignInResult>().await
            };
            assert_eq!(
                wait_for_sign_in(&prompt, None, flow, std::time::Duration::from_secs(1)).await,
                Ok(None)
            );
            glib::timeout_future(std::time::Duration::from_millis(30)).await;
        });
    }

    #[test]
    fn sign_in_pages_are_restricted_to_the_selected_provider() {
        assert!(valid_auth_url(
            "spotify",
            "https://accounts.spotify.com/authorize?state=fixture"
        ));
        assert!(valid_auth_url("yandex", "https://oauth.yandex.ru/device"));
        for url in [
            "http://accounts.spotify.com/authorize",
            "https://accounts.spotify.com.evil.test/authorize",
            "https://evil.test@accounts.spotify.com/authorize",
            "https://accounts.spotify.com:8443/authorize",
            "file:///tmp/page",
            "https://oauth.yandex.ru/device",
            "https://accounts.spotify.com/api/token",
        ] {
            assert!(!valid_auth_url("spotify", url));
        }
    }
}
