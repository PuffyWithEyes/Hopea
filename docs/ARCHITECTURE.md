# Hopea Architecture

Technical reference for developers. File paths below are relative to the repository root.


The application icon is the generated cat with KZ earphones in `data/icons/cat-kz/`.
`master.png` preserves the source illustration without generation metadata;
`build-aux/prepare-cat-icons.sh` regenerates packaging sizes with ImageMagick.
Meson installs PNG images as `${APPLICATION_ID}.Cat`; `APPLICATION_ICON` is used by GTK,
the desktop entry and About. Embedded GResource icons support uninstalled builds.
The tray supplies ARGB pixmaps directly because external panels cannot read GResource.
Do not add libadwaita's `devel` CSS class: its diagonal header stripes were removed
at the user's request. Import Progress buttons appear only while the shared job is
active; a persistent View Result toast opens the completed job's result.

### Ownership and message passing

On Android, hardware volume keys remain with the OS and target the media stream.
The in-app volume slider dispatches synchronous GStreamer volume changes to one
worker with a single pending wake-up and the latest requested value. Slow audio
driver calls therefore do not run on GTK's main thread. Playback volume feedback
updates the slider without emitting another user volume command.

Android touch forwarding uses the input device ID, gesture down-time and
pointer ID to identify a GTK touch sequence. A Java `MotionEvent` object's
identity is not stable across a drag and must not participate in that key:
otherwise GTK can receive a move or release for a different sequence than the
press. Event copies remain alive until the GTK callback finishes and are then
recycled, including on callback failure. The Android runtime regression test
exercises a long drag made of distinct event objects and cancellation. This
does not establish that all device-specific volume hangs have been resolved.

Remote waveforms may become available only after download verification finishes.
After playback starts, a missing waveform permits at most six delayed requests
to the configured library API (1, 2, 4, 8, 16 and 32 seconds). These requests do
not search peers or download audio again. Success stops retries, changing tracks
cancels the timer, and stale replies cannot update the new track's waveform.
After those retries, embedded-node catalog changes and client reconnection wake
a missing waveform again, including while paused. A coalescing watch channel
and a two-second delayed request bound this fallback; it does not poll peers,
redownload audio or bypass download verification. Notifications received during
an API request are retained until it completes. Disposing the generator cancels
its subscription and pending timer.

All audible playback must go through the application's shared `AudioPlayer`,
including search, album previews, imports and duplicate comparisons. Never create
a separate `gst_play::Play` for a dialog: it bypasses tray/MPRIS controls and the
main transport. Import auditions must not enter the saved playlist or be shared;
returning to the chooser must preserve selection, conflicts and scroll position.

`Application` (`src/application.rs`, an `adw::Application`) owns the single `Rc<AudioPlayer>` and the `gio::Settings`. `Window` gets the player from the application through `Window::player()`.

The audio subsystem uses channels instead of shared mutable state. The design is explained in the header comment of `src/audio/mod.rs`:
- `AudioPlayer` (`audio/player.rs`) owns an `async_channel` receiver of `PlaybackAction` and handles it on the GLib main context. `GstBackend` (a `gst_play::Play` wrapper) and `MprisController` send actions into it from other contexts.
- **Controllers** implement the `Controller` trait and are registered in `AudioPlayer::new` (MPRIS, session inhibit, waveform generator). Every playback change is broadcast to all of them.
- `PlayerState` is a GObject whose read-only properties the UI binds to. It's only mutated on the main context.
- `Queue` wraps a `gio::ListStore<Song>` inside a `ShuffleListModel`. Positions passed to `skip_to` are positions in the shuffled model.

Enabling Shuffle moves the current song to position zero and randomizes all
remaining songs, including the ones that preceded it. Without a current song,
the entire queue is randomized. Disabling Shuffle restores the underlying
playlist order. Queue indices are updated before model notifications, so row
observers never see a transient different current song; the audio backend is not
reloaded or seeked. The view scrolls to the top on enabling Shuffle. An empty
search preserves queue order rather than applying filename sorting. Imports
append to the shuffled order, and removal rebases indices without another shuffle.

### UI

Language selection happens before GTK widgets and application workers start.
A saved manual choice overrides the OS. Linux/BSD use the message locale
environment (`LC_ALL`, `LC_MESSAGES`, `LANG`, and the GNU `LANGUAGE` list).
Windows uses `GetUserPreferredUILanguages`, macOS uses `NSLocale` preferred
languages, and Android exports `Resources.Configuration.getLocales()` before
starting the native runtime, including per-app locale overrides. Android also
supplies the absolute path to the extracted translation assets. The Android
packager explicitly copies Meson's MO targets, since Pixiewood's library
closure omits them. Before widgets are created, proxy-libintl is connected to
the GNU gettext instance already linked into the player; Rust and Blueprint
strings then use the same domain. Android sets the message locale explicitly
because Bionic only implements C/C.UTF-8. Its gettext build disables iconv:
all catalogs are UTF-8, and the API 34 Bionic converter loops on the trailing
NUL in gettext messages. Cached gettext objects are rebuilt once when moving
to this configuration.
Locale tags are matched to installed catalogs, first specifically and then by
base language; Chinese script tags map to the appropriate shipped catalog.
An absent, unsupported or invalid language falls back to English. The picker
discovers installed catalogs rather than restricting them to a Rust whitelist.
Adding `po/<locale>.po` and reconfiguring Meson is sufficient to ship a language.
Live retranslation of existing widgets is not implemented.

`src/gaming.rs` polls game activity every five seconds on a blocking worker,
without concurrent probes, and requires three misses before restoring normal
activity. The `game-mode` GSettings preference defaults to true; disabling it
removes the temporary policy immediately. Linux probes Feral GameMode's
`ClientCount` over session D-Bus without auto-starting it, then checks bounded
same-user `/proc` environments for Steam game markers. Only the detection result
is logged. Linux also uses the BSD desktop-category detector as a fallback.
Windows uses `SHQueryUserNotificationState`. A Tokio watch channel carries changes into the embedded node, including
when it is restarted while gaming.

`gaming/desktop.rs` supports FreeBSD, OpenBSD, NetBSD and DragonFly without a
window-manager-specific API. It caches game executable names from bounded desktop
entries for one minute, respecting user overrides and rejecting general-purpose
launchers. A bounded, 500 ms `ps -x -c -o comm=` probe reads only current-user
executable names, never arguments or environments. No desktop entry is executed.
Directory/file/output budgets prevent an oversized entry or stuck helper from
growing memory or accumulating processes. Wrapped executables, unclassified
games and truncated process names can be missed.

`gaming/macos.rs` uses the already-present objc2/AppKit dependency family to read
the foreground app's bundle category; it requests neither screen recording nor
Accessibility access. On Android, `GameDetection.java` checks AppOps Usage Access
before reading at most five minutes of usage events, retains only the latest
foreground package/activity in memory, and classifies it using ApplicationInfo.
Replays exceeding 2,048 events are discarded. Locked screens, revoked permission,
clock changes, app suspension and activity transitions clear or refresh state.
The manifest exposes launchable apps for classification without QUERY_ALL_PACKAGES.
The usage-settings page opens only from the user's settings action. C initializes
the JNI bridge on GTK's thread, then attaches/detaches the blocking worker to the
VM without accessing GTK objects there. No event/package data is logged, saved
or shared. JNI readiness and API/permission failures are reflected in Settings.

The default-on `game-mode` switch enables automatic detection only. The retired
`game-mode-force` schema key remains for compatibility but is ignored. Disabling
the master switch clears the temporary policy immediately. Settings omit Game
Mode on platforms without an implemented detector; Redox's unported GUI stack
is not represented as completed platform support.

Platform contracts: [Android usage access and events](https://developer.android.com/reference/android/app/usage/UsageStatsManager),
[Android application categories](https://developer.android.com/reference/android/content/pm/ApplicationInfo),
[Apple application categories](https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/LaunchServicesKeys.html),
[desktop entry keys](https://specifications.freedesktop.org/desktop-entry/latest/recognized-keys.html),
and the [FreeBSD](https://man.freebsd.org/cgi/man.cgi?query=ps&sektion=1),
[OpenBSD](https://man.openbsd.org/ps) and [NetBSD](https://man.netbsd.org/ps.1) `ps` manuals.

`Torrents` serializes the requested speed limits and the temporary game flag under
one mutex. `SpeedLimits::during_game` derives effective limits, preserving lower
fair limits; it never rewrites stored preferences. Editing speed settings during
a game updates the values to restore afterward. Game mode also skips prefetch
and recommendation network work, lowers outgoing search concurrency to two, and
pauses account imports between tracks (with cancellation available and paused
time excluded from ETA). Explicit playback remains available. DHT announcements
continue to keep shared tracks discoverable; these limits are not a global
process bandwidth or CPU quota.

UI is defined in Blueprint files (`src/gtk/*.blp`), compiled to `.ui`, and bundled via `src/hopea.gresource.xml` under `/io/hopea/Hopea`. Each `.blp` has a matching composite-template widget in `src/*.rs`. A new `.blp` must be added to `src/gtk/meson.build`, to `hopea.gresource.xml` (as `name.ui`, sorted), and to `POTFILES.in`.

`Window` (`src/window.rs`) installs the `win.*` and `queue.*` actions in `class_init`; accelerators are set in `Application::constructed`. Calling `present`/`activate_action` on these types is ambiguous between traits, so use the fully qualified form (`AdwDialogExt::present(&d, …)`). The playlist model chain built in `setup_playlist` is `Queue::model()` → `FilterListModel(FuzzyFilter)` → `SortListModel(FuzzySorter)`. That search only covers songs already in the queue. Library search happens in the node: fuzzy matching (`node/src/fuzzy.rs`) on what it knows, then other nodes.

There is no start screen: the window always opens on the main view. `Window::open_with_music` restores the saved playlist right away and keeps the shared music sidebar and mobile sheet closed until explicitly opened. While the listener wants to be on the network (`library-enabled` and `library::wanted()`), `Application::update_sharing` holds the application, so it keeps sharing with the window closed, or waits to join again (as when a VPN comes back). The desktop tray starts independently of sharing and exposes current title/artist, Previous, Play/Pause, Next, Show and Quit. `src/tray.rs` shares player state; `src/tray/linux.rs` uses ksni/StatusNotifierItem and `src/tray/windows.rs` uses Tauri tray-icon on the GTK Win32 message-loop thread. Player-state notifications refresh menus; callbacks return actions to the main thread. Windows supports the same sharing/background lifetime holds, without the Linux portal. Native Windows execution remains unverified. `library::set_client` and `set_wanted` call it.

The main header has a "My Music"/"For You" switch (`Window::set_for_you`) and a direct Settings button (`win.library-settings`) accessible with Shared Music closed. Changing tabs only changes the view; playback and its position continue. Pressing Play calls `switch_playback_queue` to swap the queues (`my_music_songs`/`for_you_songs`). `utils::store_playlist` is never called while playing For You, so the saved playlist stays the listener's. The inactive queue's page has a big Play button (`player_stack` "for-you"). `start_for_you` fetches 20 recommendations before changing playback (`more_for_you`, refreshing from the network when the known ones run out); an empty response or navigating away during the fetch keeps the current music. `prepare_next_song` adds more near the end of the queue and prefetches the next network song (`LibraryClient::prefetch`). Every 2 s, `refresh_transfers` asks the node how network songs are coming along (`/tracks/{id}/transfer`) and sets the `status` and `progress` properties of `Song`, shown by `QueueRow`.

Lyrics (`src/lyrics.rs`, the `win.lyrics` toggle, and `cover_stack` in place of the cover art) come from a kept copy, an `.lrc` next to a local song, or its tags. Only when the listener presses "Search Online" are they looked up on lrclib.net, which then learns the title and artist. Synced (LRC) lines are lit up and scrolled to as the song plays (`update_lyrics_line`).

`Window::show_rules_once` presents a short welcome on first start (`library-rules-seen`), with Get Started and Import Music actions. It shows the active-subscription rule for service imports and a brief sharing/privacy notice; technical settings stay outside the welcome dialog. Import Music opens the existing chooser without signing in or transferring anything automatically. Closing either way records that the welcome was shown. `library/import.rs` brings music over from streaming services. For exported lists, names come over: JSON, CSV and plain-list exports, and Yandex Music playlist links. The same dialog also imports downloaded files and whole service accounts (see below). They go to `wanted.json` in the user data dir. `Window::look_for_wanted` searches for each one three seconds apart, on joining and every hour, and stars what it finds. Songs not found are retried after 12 hours. Its dialog is `LibraryPanel::import_music`, reached from the welcome page, the panel menu, the settings and the main menu.

### Importing service accounts

`library_panel/account_jobs.rs` owns a two-phase background job shared across windows. The adapter emits a deduplicated catalog with collection memberships, then waits on stdin for individual download requests; it keeps provider callbacks and credentials only in memory. `library_panel/import_picker.rs` provides a continuous virtualized chooser with explicit collection checkboxes, per-collection track views and selection independent of visible rows. New songs start selected; possible duplicates start unchecked. The Other Songs scope contains only tracks with no named service collection membership; album tags alone do not create collection cards. Search matches both remaining songs and collection cards (including their tracks), while duplicate review and Hide Duplicates apply only to the open list, with independent filter state for Other Songs and each collection. Preparation has Cancel, which stops its workers; Close appears after preparation or completion. The application is held during the job. `win.import-progress` reopens progress; hiding it does not cancel. Each accepted audio upload refreshes the local search model without erasing the query. `library/import_history.rs` checkpoints successful service IDs, node IDs, metadata and normalized cover hashes, with disk writes off the GTK thread. Existing audio is checked against the node; changed images go through the owner-only cover endpoint. Legacy source paths are recognized through shared-files.json. Images are fetched without credentials from provider image hosts with HTTPS, size limits and off-thread decoding; no signed image URLs are persisted. Album tags are passed through to FFmpeg, and per-file image sidecars refresh local song artwork. Metadata-only jobs do not count names as shared audio.

The import dialog now has **Your Online Library → Import Account…**. Rust in `src/library_panel.rs` and `src/library/service_import.rs` owns the dialog, cancellation, saved wanted list, node validation and adding accepted files to My Music (also while For You is playing). A separate embedded adapter, `src/library/service_import.py`, uses vk-api, yandex-music, ytmusicapi, spotipy and yt-dlp to read all available pages and retrieve audio. Spotify's catalogue still uses Spotipy; its selected audio uses the separate Rust `hopea-spotify` binary in `spotify/`, linked only to pinned librespot core/audio/metadata 0.8.0. `src/library/spotify_audio.rs` talks to it via stdin/stdout; credentials are never arguments, logs or cached files. No silent substitution from other providers. Premium and a compatible account are required, and metadata-only import remains available. Imported files are FLAC under `hopea/imports/<service>/`; completed files are reused on a repeated import. A run is limited to 20,000 unique songs, reported as partial if exceeded.

The root workspace includes the player and `spotify/`, excluding the standalone `node/` and `common/` workspaces. Default members contain only the player. Build/test Spotify with `-p hopea-spotify` in a separate Cargo invocation: `--workspace` unifies incompatible governor clock features from librespot and librqbit. Meson builds and installs both binaries next to each other. System TLS is intentional: librespot's Rustls feature also pulls an old vulnerable TLS stack through hyper-proxy2. Keep vergen at the compatible 9.0.6 lockfile version. See docs/IMPORTING.md for the remaining audit findings and their applicability; do not describe the full dependency audit as clean.

`src/library/browser_import.rs` detects the default HTTPS browser and its saved profile. VK and YouTube Music open only the system browser; after Continue the adapter reads that profile's cookies through yt-dlp. Zen uses the Firefox cookie database format, without a Firefox executable or geckodriver. Multiple profiles or browser encryption may require choosing a default profile or pasting the session manually. Spotify uses authorization code with PKCE and a loopback callback, with a public Client ID supplied in the dialog. Yandex Music uses the adapter library’s Device Flow: the user approves a code in the browser. `service_auth.py` performs provider authorization in a disposable worker; Rust owns the dialog, browser launch, timeout and cancellation. Manual token entry remains available. See [Service sign-in](SERVICE_SIGN_IN.md) for setup and verification limits. No browser extensions are required. Credentials go to the worker through stdin and are not stored in application settings or logs. The worker's stderr is suppressed to avoid provider exception messages leaking secrets. VK's source profile ID is separate from the signed-in viewer ID and is preserved during login; metadata is read even for songs without download hashes. Failures and empty VK results use persistent dialogs.

See `docs/IMPORTING.md` and `build-aux/import-requirements.txt` for optional runtime dependencies. `nix/shell.nix` supplies FFmpeg and Python, and selects `.venv-import/bin/python` through `HOPEA_IMPORT_PYTHON` if present. The Python portion is limited to adapting service libraries; keep product logic in Rust. Meson runs offline adapter tests with `meson test -C _builddir --suite import`. Set `HOPEA_TEST_DOWNLOAD=1` when invoking `tests/test_service_import.py` with the import environment to also test real yt-dlp/FFmpeg conversion against a local fixture server. Live account tests require explicit user authorization and remain ignored by default. The Zen/VK path has been checked through real catalogue reading, audio download, Rust HTTPS cover fetching and isolated GTK display; this does not verify other services.

### Local songs

The file chooser, drag and drop, and `Application::open` all go through `Window::add_files_to_queue`. That calls `Window::queue_songs`, which builds `Song`s in an idle loop via `SongData::from_uri` (tags read with `lofty`), adds them, and saves `~/.cache/hopea/playlists/current.pls` (`utils::store_playlist`). `SongData::from_uri` returns the "invalid" default for non-local URIs, so they are skipped.

### Shared library (client side)

- `src/library/node.rs` runs the node on its own thread with a Tokio runtime and a fresh random API token on every start. `library::ensure_started()` starts it (once, shared between concurrent callers) and connects. `Application::startup` calls it when the GSettings key `library-enabled` is set, `library::joins_at_start` says so, and `library::allowed` lets it. `Application::shutdown` stops it with `library::shutdown()`. `library-join-at-start` is `auto`, `yes` or `no`: `auto` joins on computers and not on phones and tablets (`device::is_handheld`, from the chassis type in `/etc/machine-info`, systemd-hostnamed, the device tree, or DMI). `library::wanted()` says whether the listener wants to be on the network this session: set at startup, by Join, cleared by Leave. `Application::guard_connection` runs every 2 s and on `gio::NetworkMonitor` changes: it leaves the network when `library::allowed` turns false and joins again when it turns true while wanted. That matters with `library-only-protected`, where the network is allowed only while `library::protection::check()` finds every route to the internet going through a tunnel. See that module's docs: it asks the kernel which interface reaches the internet for IPv4 and IPv6, and counts tun/tap, WireGuard and link-layer-less interfaces, plus Whonix and Tails. `protection::status()` also names running VPN and proxy tools (process names, usual ports) and the system proxy, for the indicator in the settings: a proxy that isn't in TUN mode doesn't carry this app's traffic. `library-name` is the publisher name, and `library-peers` lists nodes that are always searched.
- `src/library/mod.rs` holds the connected `LibraryClient` in a thread-local, used from the main context only. It also keeps the origin and token in a `Mutex`, because GStreamer's `source-setup` (connected in `GstBackend::new`) runs off the main thread. `configure_source` adds the `Authorization` header **only** when the URI is under the library's origin and `/api/v1/`, and turns off redirects.
- `src/library/client.rs` uses libsoup. `Origin::parse` rejects `http://` except on loopback, and rejects userinfo, query strings and fragments. Requests never follow redirects, and response bodies are read with size caps.
- Remote songs are `Song::from_remote(track, audio_uri, cover)`, with `SongData.remote_id = Some(id)`, an https `gio::File` with no local path, and uuid `hopea-<sha256>`. Code that needs a path must check `song.remote_id()` / `file().path()`. `store_playlist` skips remote songs, `open_song_directory` ignores them, and `WaveformGenerator` fetches their peaks from the node instead of decoding (a 404 until the node has the whole file).
- `LibraryPanel` (`src/library_panel.rs` + `library-panel.blp`, an `AdwBin`) is the side panel at the right of the window: `library_split` in `window.blp` is an `AdwOverlaySplitView` with `sidebar-position: end` and `pin-sidebar: true` that wraps `main_stack`. The "Shared Music" toggle buttons in both header bars are bound to its `show-sidebar`, and `win.shared-library` (Ctrl+U) toggles it. Only one `AdwBreakpoint` applies at a time, so the 560sp one (`narrow_breakpoint`) collapses both split views and the 1000sp one only the library. On phones (`narrow_breakpoint` applied), `Window::setup_library_sheet` moves the panel out of the split view into `library_sheet`, an `AdwBottomSheet` around everything whose "Shared Music" bar always shows at the bottom and pulls up. The panel gets the `in-sheet` CSS class there. Use `Window::toggle_library` and `hide_library_if_overlaid`, which handle both places. The panel's header bars show no window buttons: a close button there reads as closing the panel. Its settings (`LibraryPanel::edit_settings`) are also in the main menu as `win.library-settings`, which works before joining and with the panel hidden. The panel is an `AdwNavigationView`: the main page (joining and leaving, sharing single files or a whole folder recursively through the two buttons at the bottom, and the Search page, with an Albums section above the songs and a "Best Match"/"Most Shared" order (`SearchOrder`); rows say how many computers share a track) and an album page (the node's `/album` endpoint, first local, then `network=true`). "Download…" on the album page makes every track a favorite, polls until each is `local`, then saves the verified files from `/tracks/{id}/file` into `<Artist> - <Album>/NN - Title.ext`; file names from other people go through `safe_file_name`. The library is the listener's own tracks plus their favorites, all kept on this computer and shared, and it is always in the playlist. There is no Favorites tab. `Song` has `favorite`, `library_id` and `mine`. On joining, the panel calls `Window::show_favorites_in_playlist`, which queues files shared from here as the local files and everything else from the network. Which local file became which track is kept in `library/shared_files.rs` (`shared-files.json` in the user data dir). The star in the playback controls is always shown. On a local file that isn't shared it shares the file (`LibraryPanel::share_file`); otherwise it calls `set_favorite`. Taking a song out goes through `Window::take_out_of_library`: the stars go out at once, and after a five-second toast with Undo (starring again also cancels it: `PENDING_REMOVALS`), `leave_library` acts as `library-unliked-action` says. `move` puts the original file, or the node's verified copy, in `library-unliked-folder` (default `~/Music/Unliked`). `delete` trashes own originals. Then it stops sharing (`delete` for own tracks, `set_favorite(false)` for others) and takes the song out of the playlist. `Window::set_library_state` keeps every star in step. Track rows show the title without its file extension (`Song::from_remote` strips it too) and the format in an accent-colored `.format-badge` label (`format_label`). An owner can update cover art without replacing audio, through PUT /tracks/{id}/cover. `ask_cover` offers a picture for the whole batch. It is sent first (`LibraryClient::add_cover`, `POST /api/v1/covers`) and named in each upload's `x-anysik-cover` header. Files without a picture of their own get their folder's `cover.jpg`, `folder.jpg` and the like as `x-anysik-fallback-cover` (`folder_cover`). Rows show a 40 px cover thumbnail, fetched when the row is first mapped, four at a time, scaled while decoding, into a cache of 300 (`library/thumbnails.rs`). Search waits 400 ms after typing before asking the network, fetches pages automatically near the scroll bottom, and uses a virtualized GtkListView. Metadata stays in a GListStore while row widgets and thumbnails are recycled; the old 500-row cap is removed. `search_problem` turns the node's `SearchReport` into a message saying what stopped the search and what to try. The menu's Settings… (`edit_settings`) holds joining at startup, sharing only through a VPN with a live indicator, speed limits, and what unstarring does. Search uses a generation counter to drop stale answers. `Window::add_remote_track` queues and plays a track. Text from other people (titles, names, error messages) is shown with `use_markup(false)` on rows and toasts, so it is never parsed as Pango markup. Keep it that way.

### Node

The module docs in `node/src/lib.rs`, `p2p.rs`, `keywords.rs`, `entry.rs` and `stream.rs` explain the design. The essentials:

- There is no shared catalog. A node knows only its own tracks, its downloads, and recent search results (`db.rs`, with `State`: Found → Fetching → Held, or Own, or Rejected). It announces in the DHT under `sha1("anysik/word/v1/" + word)` for each normalized word of what it holds, and for the first three letters of longer words (`keywords::announced_keys`). `p2p::search` looks up the query's longest words and their first three letters (`keywords::query_keys`), and asks those nodes' public `GET /p2p/v1/search`. They match with `fuzzy.rs` (prefixes, substrings, Damerau-Levenshtein typos by word length; file extensions in the query are stripped). It stores what verifies (capped at `db::MAX_FOUND`).
- Entries (`entry.rs`) are signed JSON bodies: the signature is checked over the exact bytes before parsing, then every field is checked as strictly as an upload. Version 2 bodies carry only the SHA-1 and SHA-256 of the info dictionary, and optionally the SHA-256 of the cover art, which keeps search answers around 1 KiB per result. The dictionary is fetched when a track is played (`p2p::fetch_info`, `GET /p2p/v1/info`) and accepted only if both hashes match (`Entry::attach_info`). It's stored in `entries.info`, which is empty until then. Version 1 bodies with the dictionary inline are still accepted. Torrent info dictionaries (`torrent_info.rs`) have exactly one canonical shape: a single file `track.<ext>`, with a piece length derived from the size.
- `POST /api/v1/covers` validates and re-encodes an image and keeps the 16 most recent pending pictures. New uploads name one in `COVER_HEADER`; `PUT /api/v1/tracks/{id}/cover` accepts a pending cover for an own track only. `library::announce_cover` preserves the track ID, audio and torrent and signs a newer entry with the artwork hash. `db::insert_entry` accepts newer signed artwork from the same publisher, invalidating cached remote artwork; older entries and different publishers cannot replace it. `cover_for_player` verifies remote image hashes before re-encoding for display.
- Others' tracks reach the player only as WAV/PCM decoded by Symphonia in `stream.rs`, read through the torrent (`TorrentFile` blocks on pieces with a timeout) or from disk once held. The WAV length is fixed from the frame count, so Range requests map to sample-accurate seeks. Own tracks are served as files.
- A completed download goes through `ingest::verify_download`: full ingest, and the rebuild must be byte-identical to the file. Then it becomes Held (seeded, announced, cached; `library::make_room` evicts least recently played, never favorites) or Rejected (deleted, never fetched again).
- Speed limits are `common::api::SpeedLimits`. `fair()` holds the rule that sharing is at most `MAX_SHARING_SLOWDOWN` (7) times slower than downloading, and the node applies it (`Torrents::set_limits`, live through librqbit's `session.ratelimits`) whatever it is sent. The player stores them in the GSettings keys `library-download-limit` and `library-upload-limit` (KiB/s, 0 for none) and its settings dialog enforces the same rule as you type.
- Favorites are explicit (`TRACK_COLUMNS`, `Db::favorites`), independent of local sharing. `library::set_favorite(false)` retains the audio and torrent. Discovery uses `?discovery=true` to exclude held/own records, favorites and known matching audio hashes before pagination; internal import matching continues using the full catalog.
- Uploads need a findable name: title and artist together must have at least `MIN_NAME_CHARS` (5) letters or digits (`api::name_is_findable`). Re-uploads of audio already shared are exempt. Search results are ordered by fuzzy score with a title coverage factor ("numb" is closer to "Numb" than to "Numb Encore"). Ties go to the most shared: `Track::sources` is the number of known sources plus this computer when it has the file. With `order=popular`, the most shared come first. `POST /tracks/{id}/prefetch` starts a download without starring, and `GET /tracks/{id}/transfer` reports progress, peers and speeds from librqbit.
- `p2p::search` returns a `SearchReport` (common): whether the DHT had contact, how many nodes were asked and answered, the most common failure, and whether it timed out. Answers are stored as they arrive, eight nodes at a time, each answer at most 1 MiB, and at most three network searches run at once (`AppState::searches`), which bounds the memory a burst of searches takes.
- `recommend.rs` builds the feed from favorites, play counts and own tracks, searches the network for the top genres and artists, and ranks unplayed, non-favorite results.
- `reach.rs` decides how others reach the node: UPnP for the search port, then dial-back checks by other nodes over IPv4 and IPv6 (`/p2p/v1/dialback` only connects to the requester's own address), then a relay. `relay.rs` is both sides of relaying: an unreachable client long-polls a reachable node (`register`/`poll`/`answer`), and the relay announces the client's words, forwards matching searches and marks those results `relayed`. To get a relayed track, the downloader calls the relay's `fetch`. The relay asks the client to `upload`, validates the file, and seeds it. `Source.relay` in the database marks such sources. `AppState::tracks_changed()` wakes both the DHT announcer and the relay client, so the relay's words stay current. Test unreachable nodes with `Config::force_relay` and `Config::relays`, as `tests/p2p.rs` does.

`node/src/ingest/` is the security boundary for file contents. Every file goes through: extension allowlist → signature must match the claimed format (`AudioFormat::sniff`) → a format-specific rebuild into a new file (`flac.rs`, `mp3.rs`, `wav.rs` rewrite headers; `mp4.rs` zeroes metadata boxes in place to keep sample offsets valid and rejects non-`soun` tracks; `ogg.rs` replaces the comment header via lofty) → a full Symphonia decode of the *rebuilt* file using the reader for that format only (`analyze.rs`) → byte accounting: non-audio bytes must fit `BASE_OVERHEAD + PER_PACKET_OVERHEAD × packets + size/200`. Only the rebuilt file is stored and shared. The network is audio only: MPEG-4 is accepted as `.m4a` only, and `.mp4` is refused because that extension names video. A new format needs all of these steps, plus fixtures and hostile cases in `tests/ingest.rs`, and it must pass `validated_files_validate_again_unchanged` in `tests/p2p.rs`, because downloads are checked by rebuilding them.

Media lives in `<data>/media/<info hash>/track.<ext>` (`storage.rs`). Own duplicates are detected by `audio_sha256`, a hash of the audio packets only, so retagged copies still match. Routes and JSON types of the player's API live in `common/src/api.rs`.

Product naming: use Hopea, `hopea`, `hopea-node` and `hopea-spotify`. `src/paths.rs` keeps existing libraries and caches in place. The GSettings storage path and protocol/hash identifiers retain legacy values for compatibility; do not rename those blindly. New installs use hopea data/cache directories.

Search result wrappers are neither selectable nor activatable. Their nested
action rows handle activation on release, so a pointer press does not steal
focus from the search field and dismiss the Android keyboard before the tap
finishes.

### Sharing status

The network search page shows the node's last reachability result and a short
explanation above the search field. A compact green Online indicator requires
confirmed direct IPv4, IPv6 or relay reachability. Checking and limited incoming
connectivity use an amber indicator; an unavailable local service is shown as
Offline in red. Problems include an inline explanation and a Connection Help
button. After one minute without confirmation, Checking becomes Connection
Unconfirmed, explaining that no reachable helper may be available or networking
may be blocked. This is an unknown result, not proof of being online or offline;
background checks continue. Online details retain the connection type and its limitations. While
visible, it refreshes the authenticated local `/me` response every two seconds,
with at most one request in flight and stale replies from replaced nodes ignored.
This does not trigger extra peer probes or change network admission limits.
Reachability checks retain their existing schedule; a successful check is not a
guarantee that every search will find a source.

Bootstrap discovery still advertises a node optimistically when no peers are
available to test it. The API reports that state as `checking`, never as confirmed
direct reachability. No router settings or port mappings are changed by the UI. Its Details dialog
includes connection recommendations without advising users to disable their
firewall or expose the authenticated local API. See `node/README.md` for discovery
retry and relay limits. Public DHT endpoints are filtered before Hopea HTTP
connections; explicit peers and LAN discovery retain their separate trust paths.

### Personal albums and cache maintenance

`src/library/collections.rs` stores UUID-named local collections atomically under
`paths::data_dir()/collections`. `src/window/collections.rs` provides My Albums in the playlist’s NavigationView,
its editor, ordered preview playback and Share Album. Opening/browsing an album
preserves the personal playlist. `AudioPlayer::preview_album` keeps a separate
sequence and stops at its end; standalone preview stops after one song. Saved albums have a labeled destructive Delete Album action with confirmation; it removes only the UUID manifest, preserving audio, playback and sharing. Sharing uses the
existing track uploader/favorites and verifies local availability before exporting
`.hopea-album.json`; the public manifest strips local URIs and node-local IDs.
Recipients resolve exact file SHA-256 matches. Personal collections are not yet
indexed as independent albums by the P2P search protocol. Never put browser
sessions, local file paths or bearer audio URLs into exported manifests.

`src/window/song_drag.rs` uses a vertical GestureDrag. Only the lifted row
is copied to a transparent overlay, at its exact allocated size. Neighbours
translate their existing render trees in QueueRow::snapshot, with one animated
gap; do not rebuild or pack a second list of screenshots. Keep native row
allocations for hit-testing and virtualization, and permit overflow on their
ListItemWidget parents during the gesture. Reset offsets/visibility on rebind,
release, Escape and unmap. The queue changes only on release. Shuffle edits its
permutation through `ShuffleListModel::move_shuffled`, preserving its enabled
state and the playing song. Edge dragging scrolls.

`src/cache_cleanup.rs` exposes three optional categories in Settings: covers,
waveforms and downloaded lyrics. It only removes regular files with the expected
extension in those direct cache subdirectories, never follows symlinks and skips
the active MPRIS artwork. Do not treat imports, albums, session credentials or the
saved playlist as disposable cache. The UI and storage deletion tests use isolated
XDG directories. The service importer reads VK's structured `owner_playlists`
endpoint (50 per page); never evaluate the returned `templates_script`.

VPN status distinguishes an inactive tunnel from partial protection: IPv4
through a tunnel with IPv6 routed directly is `Protection::Partial`, not
“VPN off”. VPN-only sharing accepts that case using `Config::ipv4_only`: the
HTTP client, public listener, BitTorrent and DHT all restrict themselves to
IPv4; UPnP and local discovery are off. A socket-mode change restarts the node.
The Shared Music welcome screen has a connection card and a direct Settings
button; other pages use a compact warning when sharing is blocked. Status
refreshes on map and every two seconds. Keep the panel's
GSettings object alive so its changed handler remains connected. Route probes
only ask the kernel for a route; they do not send packets. The interface test
uses isolated XDG directories and checks the banner and Settings entry point.

The existing playback menu opens `src/window/song_editor.rs` for cover and
Lyrics edits. `src/song_edits.rs` keeps raw LRC and normalized artwork in the
durable `song-edits` data directory, never in cache or audio tags. Own cover
edits use the existing cover-only publication API when connected. Covers and
Lyrics also use the audio-hash-bound metadata variant protocol described in
`node/README.md`: independent copy counts, deduplication, manual comparison and
the `popular-metadata` default-on setting. Never forward received Lyrics before
the corresponding audio's duration has been verified. Client and node share
`common::lyrics`; semantic correspondence of otherwise valid text is not proven.
GStreamer position updates now carry milliseconds (50 ms interval); controller
positions and public `PlayerState::position()` remain seconds. Lyrics use
`position_ms()` directly with no global timing offset. Active lines fade via CSS; a 320 ms AdwTimedAnimation scrolls to them from the current position, including seeks.

Import preview and import workers share a reference-counted process lifetime;
dropping a preview clone must never terminate the background importer (including
Spotify). The chooser's compact album shelf hides when viewing an album to leave
room for its virtualized song list. Both cards and rows load actual artwork,
with four concurrent requests and a bounded thumbnail cache. Full-resolution
normalized JPEG bytes live in a job-scoped temporary CoverCache shared by the
chooser and downloader. Concurrent requests for one URL share a lock and reuse
the saved bytes; files are named by URL hashes, never signed URLs or credentials,
and are removed when the last cache owner is released. Thumbnail eviction must
not cause another network download.

Import collection selection is atomic per album/playlist: individual rows have
no checkboxes inside collections. Overlapping memberships download audio once.
Possible duplicates have paired previews and Existing/Incoming/Both decisions;
Back preserves scope and decisions. An Incoming replacement retires the old
entry only after successful import and a verified size/SHA-256 backup in durable
`replaced-audio` storage. On failure keep both entries; never use trash settings
or delete the user's source file. Candidate IDs must be verified against hashes.

### Progressive local playlist loading

`Window::queue_songs_with_feedback` reads tags and computes or loads the file
hash through `Song::load_local` on a blocking worker. GTK objects and artwork
textures are created on the main thread. The first valid song is published
immediately; following songs are appended in batches of eight, with main-loop
yields between files. Playback can start while the remaining files load.
Concurrent file-open requests are serialized. Duplicate detection covers the
current personal queue and the pending batch, including while For You plays.
The loading indicator counts attempted files, including missing and invalid
ones, and is cleared even when no songs can be added. Sharing starts after the
load completes. This improves time to the first playable row; artwork decoding
still runs on the main thread, so total indexing time depends on the files.
Favorites restored after joining the embedded node use the same asynchronous
local-file reader and yield between entries, including already queued tracks.
That path must not run a synchronous file-reading loop on GTK's thread. It
preserves the user's scroll position and keeps personal favorites separate if
the listener switches to For You while a read is pending.

Shuffling temporarily detaches the view's selection model before permuting the
queue and reattaches the same model afterwards. This discards GTK's previous
row anchor; scrolling to row zero then shows the current song at the beginning.
The audio player and current playback position are preserved.

### Search budgets and pages

The player requests 50 tracks per page. The first page waits for one bounded
network search; scrolling near the bottom requests the next page from the local
result database with `network=false`. Loading stops at the reported total or an
empty page. Each network search asks at most 40 nodes, up to eight concurrently
(two during Game Mode). DHT lookups have a four-second budget after an initial
bootstrap wait of up to five seconds; each HTTP search group has an eight-second
default deadline. There are at most three active network searches. These are
separate phase limits, not a single eight-second end-to-end deadline.

While the search page is visible, a 30-second timer rechecks known sources to
remove unavailable songs. This uses bounded peer requests without another DHT
lookup. Typing another query invalidates stale UI responses; it does not cancel
an HTTP request that has already reached the node.

### Catalogue discovery coverage

Precise announcements retain the legacy normalized words and three-character
prefixes. When these cannot represent every searchable term, the node also
announces occupied routing groups: the first byte of the existing word-key hash
of each normalized three-character prefix, encoded as `hopeacatalogroutev1XX`.
Scanning the full held catalogue into a 256-bit map avoids permanently omitting
tracks beyond the old 300-word cutoff. Groups take precedence within the existing
400-key announcement and signed relay-registration limit; the remaining slots
retain precise legacy keys. Relays derive groups for their clients' precise keys
and retain all occupied groups within their existing 1,000-key budget.

A query uses at most three words and nine distinct routes (group, exact and
prefix). Groups start first; no more than six lookups run concurrently and all
share the four-second DHT deadline. Discovered addresses are interleaved across
routes, deduplicated, and share the same 40-node HTTP budget. With DHT enabled,
at most half those slots go to configured, LAN and remembered nodes. A relay
still forwards to at most eight clients, prioritizing precise matches and rotating
eligible clients between requests. Cancellation removes outstanding answer slots.
Routing groups neither trigger recursive network searches nor bypass ingress
rate limits, body limits, signature checks or the final fuzzy result filtering.

Groups are routing hints, not proof that a node has a matching track. Collisions,
crowded groups, DHT sampling and finite candidate budgets can still miss an online
source; this removes deterministic catalogue truncation, not the possibility of
every discovery miss. There is no tested hundred-million-user guarantee. Existing
wire messages and signatures remain compatible, but both publisher and searcher
need the updated routing logic to benefit from overflow coverage. Older searchers
can still use retained precise keys.
