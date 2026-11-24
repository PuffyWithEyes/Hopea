<!-- SPDX-FileCopyrightText: 2026 Daniil Klimov -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Spotify and Yandex Music sign-in

On desktop, open **Import Account…**, select the service and choose
**Sign In with Default Browser**. Passwords are entered on the service's website.
Hopea receives an access token automatically when authorization completes.
Manual access-token entry remains available. Android uses the same account
catalog and selection adapters, with the platform-specific limits below. See
the [manual testing matrix](../README.md#service-import-testing): implementation
and automated tests do not establish successful real-account imports.

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

### Android sign-in and import

Choose **Import Account…**, select a service and press **Sign In…**. Sign-in data
fills automatically when the authorization finishes; then press **Import** to
read the catalog and choose songs, playlists or albums. Nothing is downloaded
merely by opening the sign-in window.

- **Yandex Music:** approve the device code, as on desktop. The confirmation
  link tries the official client and then the browser. An existing authorized
  session can avoid entering a password again, subject to Yandex's own checks.
- **Spotify:** use the registered public Client ID and PKCE flow described
  above. Android imports the selected catalog entries for network matching;
  the desktop Spotify audio helper is not shipped in the APK.
- **VK:** a dedicated VK web sign-in window returns its session to Hopea.
  The session and the window's cookies are discarded after use. An installed
  VK/Sova/Kate client does not expose the web session needed by this adapter;
  opening it cannot authorize Hopea. The optional profile ID defaults to the
  signed-in account; supplying another ID does not bypass access controls.
- **YouTube Music:** automatic sign-in is unavailable. Exported ytmusicapi
  session headers or an exported playlist remain the available inputs. Hopea
  does not embed Google's sign-in page or read another app's private storage.

The APK includes Python 3.11 to retain ARMv7 support. Hopea builds CPython
3.11.18 from its checksum-verified upstream source using Chaquopy's pinned
Android recipe: Chaquopy 17.2's published target still contains 3.11.17.
Gradle resolves only this locally built target and fails if it is missing;
it cannot silently fall back to the older runtime. Provider adapters run in a
private, non-exported Android service process, started only for an import or
authorization. A private socket carries credentials and bounded JSON messages;
credentials are not placed in Intent extras or logs. Cancel terminates that
process without terminating GTK or playback. The service process never starts a
second GTK runtime or P2P node.

Selected downloadable audio is fetched into the import cache and converted
locally with GStreamer, without executing FFmpeg from Android storage. Size,
duration and conversion-time limits apply; only a completed FLAC file is passed
to the normal library validation. Android's crypto provider handles encrypted
HLS segments. Unsupported sources remain names for network matching.

### Data access and dependency checks

On November 24, 2026, `pip-audit 2.10.1` checked the pinned import requirements:
21 desktop packages and 15 Android packages, with no reported known
vulnerabilities or skipped packages. This is a published-vulnerability check,
not proof that every dependency is benign. It does not cover the embedded
CPython runtime, Chaquopy Java/native runtime, GStreamer or Rust dependencies;
see the separate audit notes in [IMPORTING.md](IMPORTING.md).
The runtime review also found that the published Chaquopy Python target predates
the [CPython 3.11.18 security fixes](https://www.python.org/downloads/release/python-31118/),
including TLS and cookie-domain validation changes. The source-built target above
replaces it; updating only the host/build Python would not fix the APK.

The merged Android manifest was checked after adding Chaquopy. It requests no
photo-library, broad storage, contacts, camera or microphone permission. Packaging
checks the merged manifest against an explicit permission allowlist and rejects
unexpected exported components, including ones introduced by dependencies.
The existing optional usage-access permission is for automatic game detection.
File access granted through the document picker still applies to files the user
selects. The importer shares Hopea's app UID and can access Hopea's private data;
a separate process is a cancellation/crash boundary, **not a permission sandbox**.

Desktop adapters also run with the launching user's file permissions. Python's
`-I` mode and disabled yt-dlp plugin loading prevent some unintended code loading,
but do not isolate the home directory. The desktop VK/YouTube session reader
deliberately reads the selected browser's cookie database when requested; only
the selected service's cookies are returned to Hopea. VK session cookies may
grant broader account access than music-only OAuth scopes. Treat them as secrets.
Android never reads another app's private cookie database: its VK login uses a
dedicated WebView with file/content access disabled, HTTPS validation and no
JavaScript-to-native bridge or file-upload handler. Its cookies are cleared on
exit and before the next attempt after an interrupted login.

Credentials travel over private pipes, not command-line arguments, Intent extras,
settings or logs. These controls reduce exposure; they cannot guarantee safety
against a compromised dependency with the app's permissions. Repeat dependency
audits and manifest checks when updating the runtime or adapters.

### Android link routing

External service links try the official client first: VK, Yandex Music,
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

This opening policy alone does not return another app's session to Hopea.
Authorization is a separate flow, described above. Automated tests cover routing
and fallback decisions; installed-client behavior still needs device verification.

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
