<!-- SPDX-FileCopyrightText: 2026 Daniil Klimov -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Spotify and Yandex Music sign-in

On desktop, open **Import Account…**, select the service and choose
**Sign In with Default Browser**. Passwords are entered on the service's website.
Hopea receives an access token automatically when authorization completes.
Manual access-token entry remains available. Android account imports are
currently unavailable.

## Spotify setup

Someone must register a Spotify developer application. Hopea does not ship
with a shared application registration. In that application's settings, register
this exact Redirect URI: `http://127.0.0.1:43821/callback`.

Enter its public **Client ID** in Hopea, then approve access in the browser.
Do not enter a Client Secret. The implementation uses authorization code with
PKCE (S256), a random state and a listener bound only to `127.0.0.1`.
The listener closes after the callback, cancellation or timeout. A busy port
produces an error; close the other sign-in attempt and try again.

Spotify must allow the account to use the registered application. Its current
application access rules still apply. Requested scopes are `user-library-read`,
`playlist-read-private`, `playlist-read-collaborative` and `streaming`.
Direct audio import additionally depends on Premium and compatibility with the
separate Spotify helper. Catalog-only import remains available.

References: [PKCE authorization](https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow),
[redirect URIs](https://developer.spotify.com/documentation/web-api/concepts/redirect_uri).

## Yandex Music

Hopea displays a short code and opens Yandex's confirmation page. Enter the
code and approve access. The `yandex-music` adapter's Device Flow polls for
completion and returns the access token without reading browser cookies.
The live endpoint can return `https://ya.ru/device`; the documented
`https://oauth.yandex.ru/device` and `https://oauth.yandex.com/device` pages are
also accepted. Both the adapter and the desktop UI validate the exact host and
device-confirmation path before opening it.
An adapter version without Device Flow reports an update/manual-sign-in hint.
See the [adapter's authorization guide](https://ym.marshal.dev/token/).

On Android, service links first target the official Yandex Music package
(`ru.yandex.music`). If it is missing, disabled or cannot handle the original
link, Hopea falls back to normal system URL handling and the user's browser.
Confirmation URLs are never replaced with the app's home screen. This opening
policy does not implement Android account authorization or account import:
those remain unavailable until the desktop adapter workflow is ported.
Native app handling still needs physical-device verification.

## Lifetime and verification

Tokens pass between Hopea and its disposable adapters through pipes, never
command-line arguments. They are not saved in settings or a token cache.
Cancel stops the worker. Authorization expires after a bounded wait; authorize
again if an access token expires during a later operation. Persistent sessions
and refresh-token storage are not implemented.
The progress dialog uses its response signal for cancellation. It does not race
`AlertDialog::choose_future` against the worker: cancelling that future can pass
a null response through the current Rust binding and abort the application.

Offline tests cover the PKCE reference vector, callback state and host checks,
ambiguous responses, token exchange parameters, Yandex confirmation events,
provider-error redaction and allowed browser hosts. These tests use simulated
providers. Live Spotify and Yandex account authorization has not been verified.
Run `meson test -C _builddir --suite import --print-errorlogs` after configuring
Meson, or `python3 -I tests/test_service_auth.py` for the protocol tests alone.
The ignored GUI test `sign_in_dialog_handles_completion_error_timeout_and_cancel`
checks dialog teardown with simulated outcomes; run it on an isolated GTK display
with temporary XDG directories and `GSETTINGS_BACKEND=memory`.
