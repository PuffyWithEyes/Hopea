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
Hopea explicitly requests only `music:read` and `music:content`. It does not
request Yandex ID profile, birthday, email or avatar permissions, `mobile:all`,
or playlist modification. The pinned adapter otherwise requests all permissions
registered for its OAuth client, so Hopea restricts the device-code request
through its HTTP request adapter. A rejected scope does not trigger a retry
with broader permissions. See [Yandex's scope parameters](https://yandex.ru/dev/id/doc/ru/codes/screen-code-oauth).
This applies to new authorizations; it does not revoke previously granted access
or restrict manually supplied tokens. Account import with the reduced scopes
still needs verification with an authorized Music account.
The live endpoint can return `https://ya.ru/device`; the documented
`https://oauth.yandex.ru/device` and `https://oauth.yandex.com/device` pages are
also accepted. Both the adapter and the desktop UI validate the exact host and
device-confirmation path before opening it.
An adapter version without Device Flow reports an update/manual-sign-in hint.
See the [adapter's authorization guide](https://ym.marshal.dev/token/).

### Android service links

**Open Service App…** tries the official client first: VK, Yandex Music,
Spotify or YouTube Music, according to the link. The client must handle the
original URL; Hopea never substitutes its home screen for the requested page.
If the official client is unavailable, public music links are offered to other
installed apps that advertise support for that URL. A single compatible client
opens directly; several clients produce Android's app chooser. There is no
hard-coded list of third-party clients: Kate Mobile, Sova variants and other
clients are eligible only if their installed version registers a matching link
handler. Installing an app alone does not establish link compatibility.
The Android VK music entry uses `https://vk.com/audio` so clients that registered
the older `vk.com` domain can participate in this selection. This does not
guarantee that every Sova/Kate variant handles that path.

If no client handles the link, Hopea opens the default browser, or a browser
chooser if no default is available. It uses a plain external activity, without
Custom Tabs extras. The routing uses APIs available on Android 9 and queries
HTTPS handlers without requesting `QUERY_ALL_PACKAGES`; see Android's
[package visibility guidance](https://developer.android.com/training/package-visibility/use-cases).
Explicitly targeted intents and unrelated links retain their normal handling.
Yandex device-confirmation links are offered only to the official client and
then a browser, never automatically to third-party music clients.

This opening policy does not implement Android account authorization or account
import: those remain unavailable until the adapter workflow is ported. Opening
an app does not return its session to Hopea. Automated tests cover routing and
fallback decisions; installed-client behavior still needs device verification.

### Import timeouts

Yandex Music import uses a 45-second HTTP timeout instead of the adapter's
five-second default. Timed-out read operations get at most three attempts,
with two- and four-second delays. Progress and errors identify the operation
(account status, liked songs, playlists, albums or track details) without
including tokens, account identifiers or provider response bodies.
An access refusal is reported separately and is not retried or used to request
broader permissions. If a timeout interrupts catalogue reading after some songs
have been found, the chooser retains them and explicitly warns that the library
is incomplete. A timeout before any results remains a failure.

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
