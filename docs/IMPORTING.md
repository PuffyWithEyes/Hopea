# Importing music from services

Open the player menu → **Bring Your Music Over… → Import Account…**.
Import reads an account's personal collection: saved songs, playlists and albums.
It does not crawl the service's entire catalogue.

Hopea's import rule requires an active subscription to the source music service.
Importing from that service without a subscription violates this application
rule. The welcome dialog and import chooser display this requirement. It is
not a new condition of the software's GPL license, and a subscription alone
does not grant permission to redistribute music. Share only music you have
permission to share. This notice does not perform subscription verification.

The **Import** button becomes available after you enter session data or a token;
VK also requires a positive numeric profile ID. The button is disabled during
sign-in, and missing information is listed below the form. Empty links and lists
cannot be submitted either. Startup errors appear in a separate dialog even when
the search panel is closed.

After sign-in, Hopea reads the collection and opens **Choose Music to Import**.
During collection reading, **Cancel** cancels preparation and stops the catalogue
worker. The chooser has a horizontal **Albums and Playlists** strip at the top.
The checkbox next to a name selects the entire collection; **Open Tracks →** opens
its songs. **Import Cover Art** controls cover downloads and updates on all
platforms; existing images are preserved when it is disabled.
All new songs are selected initially. Potential duplicates are unselected and
available through **Duplicates**. Tracks appear in one continuously scrolling
list without **Next / Previous** buttons. **Select all tracks** selects or clears
the entire open list, including rows outside the viewport. During a search,
**Select all matches** applies to every match. Selection survives scrolling,
searching and switching collections. **All Songs** returns to the complete list.
A track appearing in several collections is selected and imported only once.
The strip contains collections returned by the service. An album title in a
song's tags alone does not create a separate collection.

**Import Selected** starts a background import. You can listen to music, switch
tabs and close the main window: the application stays running until the job ends.
Quitting the application entirely stops the job; completed files and history are
preserved. **Import Progress** in the main header opens the counters while a job
is active. Once it finishes, the button disappears and the **View Result**
notification opens the result.
After import starts, **Close** dismisses the progress window without cancelling
the import. After music selection, a red **Stop Import** button stops background
workers while preserving tracks already added. **Close** remains after a stop,
error or completion; stale reading status and the progress bar are hidden on
failure. On a VK timeout, the importer retries the same request up to three
attempts and displays the reading stage. If part of the catalogue has already
been read, it remains available for selection, with an explicit warning that the
result is incomplete.

Covers come from service metadata, are downloaded without cookies, validated and
saved next to cached files as `*.cover.jpg`. Album title, album artist and track
number are written to the audio when provided by the service. An unavailable
cover does not block music import and is counted in the result.
Hopea first checks for selected tracks that already exist and updates their
covers, then downloads new tracks. Each new track appears with its cover as soon
as it is ready, without waiting for the entire import. For VK, cover metadata is
also requested through `reload_audio`, because the initial list may omit it.
HTTPS image downloads in Rust require the GIO TLS module (`glib-networking`).
It is included in the Nix package and `nix/shell.nix`. After changing the
environment, quit Hopea entirely and start it from a new `nix-shell`.
Missing service artwork and failed cover downloads are counted separately.
Spotify catalogue import uses Spotipy; audio comes directly from Spotify through
a separate Rust process, `hopea-spotify`, using librespot 0.8.0. Premium and a
compatible account are required: some accounts no longer receive audio keys
after Spotify changes. A refusal displays its reason; completed files remain.
Audio is never substituted with a YouTube recording. Disable audio downloading
to import only titles, albums and covers for P2P searches.

`hopea/import-history.json` records successful results by service and track ID
(existing installations continue using their previous storage location), without
tokens or signed URLs. Reimport checks the node for audio and compares cover
contents: existing audio is skipped, and new artwork updates the same entry
without duplicating it. Older imports are recognized through `shared-files.json`.
Deleted audio and failed downloads can be retried; completed files are reused
from the cache. Final deduplication across different services happens in the node
based on audio contents, since these records have no reliable shared identifier.

The shared music list refreshes automatically after audio is added. **Shared**
counts audio files added to the node; **names only** means search entries without
audio available to share. A zero upload rate alone does not mean sharing is
disabled: a transfer requires a requesting peer.

| Service | Sign-in | What is read | Audio |
| --- | --- | --- | --- |
| VK | **Sign In with Default Browser…**, then a profile ID to import | Audio recordings and every playlist page | Streams available to the current session through `vk-api` |
| Yandex Music | Account OAuth token; browser authorization can obtain it | Liked tracks, owned and liked playlists, saved albums | MP3 streams available to the account through `yandex-music` |
| YouTube Music | **Sign In with Default Browser…** | Library, liked tracks, playlists and albums | Available audio through `yt-dlp` |
| Spotify | OAuth access token; browser authorization requires a Client ID | Saved songs, playlists and albums, including every page | Direct audio through librespot with Premium and a compatible account; catalogue-only import is also available |

For Spotify and Yandex browser authorization, see the
[service sign-in guide](SERVICE_SIGN_IN.md). The browser-session workflow below
applies to VK and YouTube Music.

**Sign In with Default Browser…** opens the service in your default browser.
Sign in using a regular, non-private window, then select **Continue** in Hopea.
The adapter uses `yt-dlp` to read the browser's local cookie store and passes only
the selected music service's session to the application. Firefox, Zen, Chrome,
Chromium, Brave, Edge, Vivaldi, Opera and Safari are supported when their profile
and decryption key are accessible to the library. For Zen, it reads the default
profile from `profiles.ini`, including recent cookie changes while the browser
is still open.
If VK has not stored `remixmid`, the signed-in account is checked through a VK
page; the profile ID being imported is not substituted for the signed-in user's
ID. Sign-in always opens the default browser. If you have several profiles,
choose the default profile and sign in to the service there. If the browser
session cannot be read, you can enter it manually. A separate Firefox instance,
geckodriver and extensions are not needed.

If automatic session reading fails, choose **Manual Sign-In Instructions** below
the session field. The same instructions are available from the sign-in error
dialog and the Flatpak sign-in dialog. They apply to the selected service:

- **VK:** open `https://vk.ru/audio`, sign in, press F12 → Network and reload the
  page. Select a request to `vk.ru`, such as `audio` or `al_audio.php`. Under
  **Request Headers**, copy the entire **Cookie** value without the `Cookie:`
  prefix. It must include `remixsid` or `remixsid6`; preserve both if present.
  Keep `remixmid` too if present. Example:
  `remixsid=YOUR_VALUE; remixmid=SIGNED_IN_ACCOUNT_ID`.
  Do not copy the response's `Set-Cookie` header. If `remixmid` is missing,
  Hopea checks the signed-in account on VK itself; the profile ID being imported
  does not replace it. Zen/Firefox also exposes individual pairs under
  Storage → Cookies.
- **YouTube Music:** open your library at `music.youtube.com` with the Network
  panel open. Find a successful POST request to `/browse` and copy `Cookie`,
  `Authorization` and `X-Goog-AuthUser` into the JSON template in Hopea's
  instructions. Copy the entire Cookie value, including `SAPISID` and
  `__Secure-3PAPISID` if present. Use the account number from the request. See the
  [ytmusicapi documentation](https://ytmusicapi.readthedocs.io/en/stable/setup/browser.html).
- **Yandex Music:** an OAuth token is required; `Session_id`, `sessionid2` and
  `yandexuid` cookies do not work. The built-in instructions link to the
  [library's guide](https://yandex-music.readthedocs.io/en/main/token.html).
  Paste only the token, without the `OAuth` prefix.
- **Spotify:** a user OAuth `access_token` with the permissions listed in the
  import dialog is required. This field does not accept `sp_dc` or `sp_key`
  cookies, a client secret or a refresh token. Use
  [Spotify user authorization](https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow)
  and paste the token without `Bearer`.

Session data grants access to your account. Paste it only into Hopea's sign-in
field, never into a chat, screenshot or bug report. Examples contain placeholders
only; the application never inserts real credentials into the instructions.

For VK, **VK Profile ID to Import** identifies the owner of the music you want,
for example `123456789`. This can be another profile whose audio your account can
access. Sign-in does not replace an ID you have already entered. The signed-in
user's identifier is obtained separately from the session. Song names are saved
even without audio URLs. Access refusals, unexpected responses and empty
collections open a separate explanation dialog instead of reporting a successful
zero-song import.

A Spotify token needs `user-library-read`, `playlist-read-private` and
`playlist-read-collaborative`, plus `streaming` for audio. It must be a **user**
access token, not a client secret or Client Credentials token. See
[Spotify OAuth](https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow)
for authorization and the [yandex-music documentation](https://ym.marshal.dev/)
for Yandex token methods. Never send tokens in chat.

If the default browser session is unavailable, VK accepts a manually pasted
`Cookie` request header from your own session and a numeric profile ID.
YouTube Music accepts headers as JSON in the
[ytmusicapi browser authentication format](https://ytmusicapi.readthedocs.io/en/stable/setup/browser.html).
No extension is needed for this method either.

Credentials are passed to the adapter through stdin, not command-line arguments.
They are not written to application settings or reports; responses containing
cookies and signed URLs are not logged.

## Dependencies and running

The player, import UI and default-browser integration are written in Rust.
`hopea-spotify` is also written in Rust and installed alongside the player by
Meson. Build the player and helper with separate commands: `cargo build --workspace`
combines incompatible clock features of librespot and P2P node dependencies.
The usual `meson compile -C _builddir` already runs both required commands.
`src/library/service_import.py` is a separate adapter for service libraries and
`yt-dlp`; it has no web server or background daemon of its own.

Create the environment once in the project directory:

```sh
nix-shell -p python3 python3Packages.virtualenv --run \
  'python3 -m virtualenv .venv-import && .venv-import/bin/python -m pip install --upgrade pip==26.3 && .venv-import/bin/python -m pip install -r build-aux/import-requirements.txt'
```

`nix/shell.nix` supplies FFmpeg and selects `.venv-import/bin/python` if that
environment exists. Continue to build and run through Meson:

```sh
nix-shell nix/shell.nix --run 'meson compile -C _builddir'
nix-shell nix/shell.nix --run 'meson devenv -C _builddir ./src/hopea'
```

On other systems, install the same requirements into a separate Python
environment and install FFmpeg. Before starting Hopea, set
`HOPEA_IMPORT_PYTHON=/absolute/path/to/environment/bin/python`.
If unset, the application searches for `python3` on `PATH`.
Older Flatpak/Nix archives do not include this functionality; those archives have
not been rebuilt.

## Processing results

- All pages are read. Duplicates within an import are removed by service ID.
- Available files are converted to FLAC using FFmpeg, with title and artist tags.
  This does not improve the source recording's quality. FLAC is a supported
  container that avoids additional lossy compression; files may be larger than
  their sources.
- Every file passes the node's normal validation before being added or shared.
  Size, format and title limits still apply.
- Completed downloads are stored in the user's `hopea/imports/<service>/`
  directory. A subsequent run reuses them instead of downloading them again.
  Cancellation preserves names already read and completed files.
- Songs without available audio are saved in `wanted.json` for network searches.
  Authorization errors and site changes are not reported as a successful,
  complete import.
- A run processes at most 20,000 unique songs. Exceeding this limit produces an
  incomplete-import notice. YouTube requests for individual sections are also
  limited to this number.

Checks that do not require accounts:

```sh
nix-shell nix/shell.nix --run 'meson test -C _builddir --suite import --print-errorlogs'
```

The VK, Yandex and YouTube libraries use unofficial service interfaces. Their
operation depends on site changes, region, session and account permissions.
Automated checks cover service responses and the worker protocol; they do not
replace verification with a real account.

## Dependency provenance and checks

The adapters use the original projects: [vk-api](https://github.com/python273/vk_api),
[yandex-music](https://github.com/MarshalX/yandex-music-api),
[ytmusicapi](https://github.com/sigma67/ytmusicapi),
[Spotipy](https://github.com/spotipy-dev/spotipy),
[yt-dlp](https://github.com/yt-dlp/yt-dlp),
[Beautiful Soup](https://www.crummy.com/software/BeautifulSoup/bs4/),
[PyCryptodome](https://github.com/Legrandin/pycryptodome) and
[SecretStorage](https://github.com/mitya57/secretstorage).
These third-party adapters are not official SDKs of the music services.

Direct Spotify audio uses the published
[librespot 0.8.0 release](https://github.com/librespot-org/librespot/releases/tag/v0.8.0),
with only its core/audio/metadata components. Versions are pinned in
`spotify/Cargo.toml` and the shared `Cargo.lock`; the compatible `vergen` build
dependency is 9.0.6. Connect, device discovery and credential persistence are
disabled. The worker does not initialize logging because library trace logs may
contain tokens. FFmpeg receives only a temporary audio file and tags, without a
token; its network protocols are disabled. Size and time limits, duration
comparison and node audio validation apply before sharing. Received Vorbis audio
is converted to FLAC for the shared import path; this does not improve source
quality.

A `cargo-audit 0.22.1` check on November 12, 2026 found outdated TLS dependencies
in the original Rustls configuration. It was replaced with `native-tls`, using
system TLS and normal certificate verification. The old rustls 0.22 and
rustls-webpki 0.102 dependencies were removed. Auditing the shared lockfile still
reports:

- `RUSTSEC-2026-0194/0195`, quick-xml 0.38.4: librespot uses the ordinary `Reader`
  and does not iterate attributes. The vulnerable `NsReader` and duplicate
  attribute checks are not called in the inspected ProductInfo path.
- `RUSTSEC-2023-0071`, rsa 0.9.10: librespot verifies the server signature using a
  public key. The vulnerable private-key operation is not used in this path.
- `RUSTSEC-2026-0293`, ringbuf 0.4.8: an existing P2P node dependency that is not
  part of the separate Spotify process. This warning was not fixed here.

This is an assessment of known warnings' applicability, not a clean audit result
or a guarantee that no other issues exist. No warnings were added to an ignore
list. Repeat the audit and call-path inspection when updating librespot.
To repeat the audit, run `cargo audit --json`. Worker tests:
`cargo test -p hopea-spotify --target-dir _builddir/src -- --include-ignored`
inside `nix-shell`.

On November 12, 2026, `pip-audit 2.10.1` checked 22 installed packages in the import
environment against the PyPI database. The old `pip 25.3` had known
vulnerabilities. After upgrading to `26.3`, a repeat check found no known
vulnerabilities and skipped no packages. This checks published vulnerabilities;
it is not a complete source audit or a safety guarantee.
Direct versions are pinned in `build-aux/import-requirements.txt`, transitive
versions in `build-aux/import-constraints.txt`. Updates require repeat checks.

Passwords are entered on the service's website in a browser. The application does
not retain cookies for later sign-ins or write them to settings or logs. Cookies
from other sites are not passed to the player. When reading a regular profile,
yt-dlp creates a temporary copy of the cookie database and removes it after
reading; a process crash may leave the copy behind. Automatic loading of
third-party yt-dlp plugins is disabled. The adapter runs as a separate process,
but it is not an OS sandbox: it has the launching user's permissions.

The GTK regression test
`window::tests::navigation_preserves_playback_and_settings_open_without_search`
checks that the queue, song and position survive tab changes and that settings
open with search hidden. After a Meson build, run
`cargo test --target-dir _builddir/src navigation_preserves -- --ignored --test-threads=1`
inside `nix-shell`, with a test GTK display, `GSETTINGS_BACKEND=memory`,
`GSETTINGS_SCHEMA_DIR` pointing to `_builddir/data`, and temporary
`XDG_DATA_HOME`, `XDG_CONFIG_HOME` and `XDG_CACHE_HOME` directories under `/tmp`.
It also checks Import availability for different fields and services. To test a
startup failure without contacting services, set
`HOPEA_IMPORT_PYTHON=/nonexistent/hopea-test-python` only for the test.

An opt-in test, `live_zen_music_import`, is available for explicit checks against
your own account. In the user's normal environment, with an authenticated Zen
session and a temporary `HOPEA_LIVE_IMPORT_DIR` under `/tmp`, it reads the catalogue
and imports one track with a cover. Cookies pass only through stdin; the test does
not publish the track or modify the main library. The resulting `selected.flac`
and `selected.cover.jpg` can be checked in an isolated GTK test by supplying the
same `HOPEA_LIVE_IMPORT_DIR`. `HOPEA_TEST_REAL_COVER_SCREENSHOT` sets the output
path for a screenshot showing that cover.

## Personal albums

**My Albums** appears above the track queue and in the main menu.
**New Album** opens an editor with a required name, a PNG/JPEG cover and
**Add Songs** for selecting tracks from My Music, moving them up/down and
removing them from the collection. **Save** persists the collection across
restarts. Original audio tags are unchanged. **Play Album** places the selected
songs at the beginning of My Music in album order; other songs stay in the queue.

**Share Album…** shares local songs through the embedded node. For network songs,
it enables saving and waits for a validated, complete copy. It then saves a
`.hopea-album.json` file containing the name, cover and ordered file hashes.
You can send this file to another Hopea user. The open button in **My Albums**
reads it and finds audio by exact hash through Shared Music. Local paths, cookies
and tokens are excluded from the shared file. This exchanges a collection file;
there is no separate global catalogue of personal albums yet.

## Progress and cache

Import Progress shows the stage, track, completed count and estimated remaining
time based on the average time spent processing completed tracks. The estimate
appears after the first two tracks and is refined as work continues; slow service
responses may increase it. Errors appear in red separately from regular counters;
completion appears in green if there were no download errors.

In **Settings → Storage → Clear Cache…**, you can separately select cached covers,
waveforms and downloaded lyrics. Sizes are shown before cleanup and the actual
space freed afterward. Music, album files, import history, original covers and
queue restoration data are preserved. The player's active cover is kept; other
cached images can be generated again.

VK collection reading uses the structured `owner_playlists` endpoint with
50-entry pagination instead of obsolete mobile HTML. An unavailable collection
does not hide the others. A check using an authorized Zen session read 61
collection entries; contents and covers were available for 59, yielding 3,128
unique tracks. The two unreadable collections appear in a warning. This check
read the catalogue without downloading audio.

### Selecting albums and comparing versions

Albums and playlists are selected as complete collections. If a collection has
potential duplicates, the whole collection is initially unselected; its main
checkbox can select it. Search and hiding duplicates change the visible list,
not the contents of a selected album. A track belonging to several selected
collections is downloaded only once.

The comparison shows the existing and incoming versions with separate preview
controls. **Keep Left** uses the existing track; **Download Right** replaces the
library entry after a successful download; **Keep Both** preserves both different
versions. Exact hash matches do not create a second audio copy. **Back to Songs**
returns to the previous list while preserving your decisions.

During replacement, the old audio file is retained under `replaced-audio` in
Hopea's data directory. The saved file's size and SHA-256 are checked before
removing the old entry. If downloading or saving fails, the old entry stays.
The user's original file and trash settings are untouched.

### Favorites, albums and playback

The personal playlist contains starred songs. Clicking a search result starts a
preview; starring it saves the song to the playlist. A single-song preview ends
after that song; within an album, playback continues to the next song in that
album. Favorites are unchanged by previewing.

My Albums and the contents of a discovered album open in place of the left-hand
list. **Back** returns to the previous page; the **My Music** tab returns to the
playlist. **Edit Album** opens the separate editor.

Collections selected during import are saved in My Albums with their original
song order. Reimport updates the same collection. If import is interrupted, songs
without downloaded audio remain marked as unavailable. Albums skipped by the
service are listed under the warning's **Details**; incomplete lists are not
presented as complete albums.

All music added to Hopea and available locally is shared when sharing is enabled,
subject to the VPN setting. Removing a star does not move or delete the file.
Arbitrary folders on the computer are not scanned automatically. Music discovery
hides local entries and known audio-hash matches; final hash verification after
download still applies.

An empty search shows a prompt to find music, albums and artists. Old catalogue
entries are not displayed automatically. A new query rechecks sources; open
results are checked roughly every 30 seconds. A song disappears if no sources
remain available and stays if at least one confirmed copy is available. Its
metadata is not deleted.

If a cached cover file disappears, the node clears its stale cache marker and
fetches the image again, verifying its signed hash. An unavailable cover returns
404 instead of an internal ENOENT error. This fix does not alter audio files.
