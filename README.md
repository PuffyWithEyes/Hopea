# Hopea

![Hopea icon](data/icons/cat-kz/256.png)

Hopea is a music player built with Rust, GTK 4 and libadwaita. It plays local
music, imports personal music collections, and shares songs through its
built-in peer-to-peer node.

Source code and issue reports: [PuffyWithEyes/Hopea on GitHub](https://github.com/PuffyWithEyes/Hopea).

Features include background imports with album and playlist selection,
optional cover-art downloads, favorites, recommendations, lyrics, MPRIS and a
system tray with the current track, previous/next and play/pause controls on
Linux and Windows. See [docs/IMPORTING.md](docs/IMPORTING.md) for supported services,
sign-in instructions and import limitations.

Search shows album covers in a horizontal strip above the vertical song list.
Open a card to see the album's tracks; scrolling down loads more results
automatically. Drag songs in the playback queue to change their order.

## Build and run

For installation with Nix, NixOS or Home Manager, see [the Nix guide](nix/README.md).
No system image is required. The source package also provides integration modules
for nix-darwin, Nix-on-Droid and experimental NixBSD configurations.

On NixOS or with Nix installed:

```sh
nix-shell nix/shell.nix --run 'meson setup _builddir . -Dprofile=development'
nix-shell nix/shell.nix --run 'meson compile -C _builddir'
nix-shell nix/shell.nix --run 'meson devenv -C _builddir ./src/hopea'
```

The other binaries are `hopea-node` (standalone sharing node) and
`hopea-spotify` (Spotify audio worker). See [node/README.md](node/README.md),
[package and CI guide](packaging/README.md) and [release guide](docs/RELEASING.md).
The repository includes Flatpak manifests and a [Windows installer/portable build](packaging/windows/README.md);
no published store listing is configured here. The Windows tray backend is
implemented, but its native runtime has not yet been tested on Windows.

Settings → Accent Color offers the system default or a custom color, selected
with a palette or entered as #RGB/#RRGGBB. Changes preview immediately; Save
persists them and Cancel restores the previous appearance.

Settings → Speed Limits → Game Mode is enabled by default. When a game is
detected, Hopea temporarily caps BitTorrent downloads at 512 KiB/s and uploads
at 128 KiB/s, preserving lower configured limits. It skips speculative track
downloads and background recommendation searches, reduces concurrent outgoing
search requests, and pauses account imports between tracks. Current playback
continues. Imports finish their current file before pausing; HTTP, DHT and
provider traffic are not covered by the BitTorrent bandwidth cap. Normal limits
return after the game ends (about 15 seconds), or when Game Mode is disabled.

Detection is best effort, with different mechanisms on each platform:

| Platform | Detection |
| --- | --- |
| Linux | Steam process markers, Feral GameMode and installed `Game` desktop entries matched against running executables |
| FreeBSD, OpenBSD, NetBSD, DragonFly | Installed `Game` desktop entries matched against current-user process names; independent of X11/Wayland |
| macOS | The active app's `LSApplicationCategoryType`, including Apple's game subcategories, through AppKit |
| Android | The foreground app's game category through UsageStatsManager, after the user grants Usage Access |
| Windows | System full-screen notification state; may include other full-screen apps and miss windowed games |

On Android, select **Allow Game Detection…** in these settings and enable
Usage Access for Hopea on the system page. No accessibility service or root is
used. Recent events are processed only in memory, never saved or sent to peers.
Denied/revoked access, locked screens and unavailable APIs do not crash playback.
Android may suspend background execution; detection cannot run while the app is
suspended. Games lacking a category, wrapped desktop executables and Flatpak's
restricted view of host processes may not be detected. Flatpak can still use the
host's Feral GameMode service. BSD needs its usual `ps` utility on PATH.

**Keep Game Mode Active** is a manual override on every platform, off by default.
It keeps reduced activity enabled without detection or special permissions until
disabled; the main Game Mode switch always takes precedence. Redox still lacks a
working Hopea GUI dependency stack, so it has no usable automatic detector yet;
the portable override is ready for that port. No game-monitoring SDK was added.

## Source layout

`src/` contains the player, `node/` the sharing node, `common/` the shared
protocol and validation, and `spotify/` the isolated audio helper. UI resources
and metadata are in `data/`, translations in `po/`, and patched dependencies in
`vendor/`. Build scripts live in `build-aux/`, Nix recipes in `nix/`, tests in
`tests/`, distribution recipes in `packaging/`, GitHub workflows in `.github/`,
and documentation in `docs/`. Root-level Cargo/Meson manifests and
license files remain where their tools expect them. Local build caches and
`dist/` are ignored and excluded from source archives.

## Platform status

Portability changes are not a claim of device-tested support. The same player,
collections, import selection, metadata validation and P2P protocol are reused;
OS integration is checked separately.

| Platform | Implemented / checked | Remaining native work |
| --- | --- | --- |
| Linux | Native build, unit and isolated GTK integration tests | Desktop-specific tray/portal behavior |
| Windows 10/11 x64 | Native tray controls, route-based VPN probe, per-user offline Setup EXE and portable ZIP, relocatable resources, bundled importer/FFmpeg; Win32 API type-check | Native installer/runtime CI checks and device verification; Windows system media panel and Authenticode signing are not implemented |
| macOS | Native tray controls, background lifetime, utun/tun route detection, browser/profile paths; native source-install recipe | Full native build/run, Now Playing/media keys, standalone signed/notarized .app distribution |
| FreeBSD | MPRIS, StatusNotifier tray, background handling, tun/tap/wg route detection; network dependency cross-check | Full desktop build/run |
| NetBSD | Same desktop integration; fixed unsupported multicast API calls; network dependency cross-check | Full desktop build/run |
| OpenBSD / DragonFly BSD | Same desktop integration; packet-info portability patch | Native compiler and runtime checks; these are not yet verified builds |
| Android ARM64 | APK build, system media service/notification, shared-player controls, audio focus, headphone-disconnect pause, private document copies and per-app VPN detection | Physical-device tests; service-account imports and independent background P2P require a separate port |
| Redox OS | Common protocol/format/Lyrics crate cross-compiles for x86_64-unknown-redox | No verified GTK4/libadwaita/GStreamer application stack or full build; requires a dedicated dependency/backend port assessment |

BSD derivatives use their base OS target (for example GhostBSD uses FreeBSD).
StatusNotifier and MPRIS require a session D-Bus and a compatible desktop/panel;
they are not assumed to exist on every BSD installation. VPN detection checks
the active route, not just a VPN process. Unknown drivers/interfaces keep
VPN-only sharing blocked. This detection is not an OS firewall kill switch.

For native macOS/BSD source installs, install Rust, Meson, Ninja, pkg-config,
Blueprint compiler, Python with venv/pip, FFmpeg, GTK4, libadwaita, libsoup3,
gettext, GStreamer and its base/good/bad/ugly audio plugins using the OS package
manager, then run `sh build-aux/desktop/build.sh`. Meson checks minimum library
versions. The default prefix is `~/.local`; override with `HOPEA_PREFIX`.
This recipe uses the system libraries; it does not produce a standalone bundle.
The private importer environment is created under that prefix. For MSYS2 UCRT64
use `bash build-aux/windows/build.sh` instead.

## Android preview

Android builds target Android 12 or newer, including Android 17. ARM64 is the
default; an x86-64 build recipe is also available for compatible devices and
emulators. The x86-64 recipe still needs a complete APK/runtime verification.
There is no Google Play Services dependency or phone-brand restriction. The
manifest allows small screens, tablets, foldables and devices without touch.
Both APK variants retain the 16 KiB alignment check.

Android 11 and older and 32-bit ARM/x86 are not supported by the current GTK
toolchain. It only provides ARM64/x86-64 targets and directly loads Android 12's
`system_accent1_600` resource during GTK initialization. Simply lowering minSdk
would permit installation of an app that fails at runtime. Supporting older
devices requires a separate GTK/backend and native-library compatibility port,
plus device/emulator tests, rather than a packaging-only change.
It uses the same Rust/GTK player and sharing node. Account imports and the
desktop browser-cookie reader are not available on Android. Service links
try an installed app first, then fall back to the browser. Playback, file
selection and background behavior still need testing on a physical device;
this is a debug-signed preview, not a production release.

After building desktop resources, `build-aux/android/build.sh` stages a separate
source tree in `/tmp/hopea-android-src`, builds the Docker toolchain and writes
`dist/Hopea-android-arm64-preview.apk`. The first build downloads the Android
SDK/NDK and compiles GTK and GStreamer. Keep the staging directory, especially
`.android-user` (the debug signing key), to update an existing installation.
The complete incremental recipe has produced a signed APK with 16 KiB alignment
checked. A clean toolchain rebuild and physical-device behavior remain unverified.
Existing toolchains and build caches are reused; set
`HOPEA_ANDROID_REBUILD_TOOLCHAIN=1` when deliberately updating the toolchain.
After updating from the ARM64-only toolchain, rebuild it once for x86-64:

```sh
HOPEA_ANDROID_ARCH=x86_64 HOPEA_ANDROID_REBUILD_TOOLCHAIN=1 bash build-aux/android/build.sh
```

This uses a separate staging directory and produces
`dist/Hopea-android-x86_64-preview.apk`. The Packages workflow builds both
architectures independently when its Android option is enabled, using the same
configured signing key. Install the APK matching the device's Android ABI.
The media service uses Android's mediaPlayback foreground-service type rather
than a separate audio engine. This is required for background audio on
[Android 17](https://developer.android.com/about/versions/17/changes/bg-audio).
Android can still stop background transfers; unlimited P2P seeding while idle
is not promised. Account import is explicitly disabled in this preview: its
desktop Python/FFmpeg and Spotify helper processes need an Android integration.
Other apps' private browser cookies cannot be read by this application.

For USB installation, enable Developer options on the phone (tap Build number
seven times), enable USB debugging, connect a data-capable USB cable, unlock the
phone and accept its debugging prompt. On NixOS, from the project directory:

```sh
nix-shell -p android-tools
adb devices -l
adb -d install -r dist/Hopea-android-arm64-preview.apk
```

The device must appear as `device`, not `unauthorized`. The Android package is
`io.hopea.hopea`. See the [official ADB instructions](https://developer.android.com/tools/adb).

## Existing installations

Hopea keeps existing music, node identity, import history and cached playlists
in place. Compatibility paths and wire-format identifiers remain unchanged
where changing them would lose settings or disconnect existing peers.
New installations use `hopea` data and cache directories. The desktop app ID
is `io.hopea.Hopea` (`io.hopea.Hopea.Devel` for development builds).

## Contributing and license

See [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md).
The player retains the original work and copyright notices of Emmanuele Bassi
and its contributors. Hopea's sharing and import features are by Daniil Klimov.

Code is licensed under GPL-3.0-or-later; see the per-file SPDX notices,
[LICENSES](LICENSES) and [ADDITIONAL-TERMS.md](ADDITIONAL-TERMS.md).
