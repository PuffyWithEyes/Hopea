# Hopea 2026.3.5 Alpha

This alpha adds an experimental Android service importer, improves library startup and album editing, and expands peer discovery and relay recovery.

## Changes

- Android now includes the service import adapters and a private import worker. Sign in, choose a collection, then select what to import. VK uses a separate sign-in window; Yandex Music supports device authorization. Spotify requires a registered public Client ID and imports catalog entries for network matching. YouTube Music supports manual headers or exported playlists.
- The Android importer uses a source-built CPython 3.11.18 runtime on ARM64, ARMv7 and x86-64. Packaging checks reject unexpected permissions, exported components and stale Python runtime files. Import libraries do not request access to the photo library, contacts, camera or microphone.
- Update the Rust URL parser to 2.5.9 to address its nested-origin stack overflow advisory.
- Show saved music progressively during startup and simplify album editing, including changing artwork by selecting the cover.
- Keep completed waveforms visible during repeated playback updates.
- Reuse bounded public peer history, forward searches through connected users and maintain redundant reverse relay connections. Existing validation and resource limits remain in place.
- Add a background-running preference: when disabled, closing Hopea also removes its tray icon and stops background operation.
- Refresh the README and document service-import testing status and security boundaries.
- Improve Android packaging, DragonFly FFmpeg package selection and ALT repository retry handling.

## Known limitations

- Android account import is experimental. Automated and build checks do not confirm successful sign-in or import with real accounts. Linux VK and Yandex Music import were manually tested; Android VK previously failed and needs retesting. Other service/platform combinations remain untested.
- Android cannot reuse another app's private login session, including Sova, Kate Mobile or the official VK client. VK requires signing in within Hopea's dedicated window. Android Spotify imports catalog metadata, not audio through the desktop Spotify helper.
- Devices with restrictive NAT or blocked transports still need a reachable user device or user relay. Hopea has no permanently available public relay and cannot guarantee that every track or peer is reachable.
- Previously reported Android device-specific keyboard crashes and volume freezes still need confirmation on affected phones. This release does not claim they are all resolved.
- Dependency scans are not a guarantee of safety. Remaining Rust advisories and their reviewed usage paths are documented in `docs/IMPORTING.md`; the desktop importer runs with the current user's permissions.

## Downloads

Choose the package for your OS and architecture. Android previews require Android 9 or later: ARM64 for most current phones, ARMv7 for 32-bit ARM devices, x86-64 for compatible emulators/devices. APK updates use the existing preview signing key. Alpine `.apk` files are Linux packages, not Android apps.

Linux native packages require GTK 4.20+ and libadwaita 1.8+; older distributions can use the Flatpak bundle. Windows Setup EXE and portable ZIP include their runtime; the per-user installer is unsigned. macOS bottles use Homebrew dependencies. BSD packages remain experimental.

`SHA256SUMS` records the attached files. See `packaging/README.md` in the source archive for installation and platform limits. Gentoo and LFS recipes remain available for manual builds.
