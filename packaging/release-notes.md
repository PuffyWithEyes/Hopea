# Hopea 2026.3.3 Alpha

This alpha connects the updated Russian translation, makes PO files the single source for shipped languages, and improves Android input and waveform recovery.

## Changes

- Place a translation at `po/<locale>.po` and reconfigure Meson. Catalogs, desktop metadata and the language picker discover it automatically; no Rust language whitelist or manually maintained LINGUAS file is required.
- System language detection uses native preferences on Windows/macOS, Android configuration (including per-app language settings), and the message locale environment on Linux/BSD. Manual selection takes priority; unavailable languages fall back to English. Language changes still require restarting Hopea.
- Android loads compiled translations from the extracted app assets. The updated Russian catalog includes “On this computer” and “For You”; some final editor messages remain untranslated.
- Android touch events retain the same GTK sequence during a drag, and copied event objects are released after processing. Tapping a search result no longer gives the surrounding list row focus before activation.
- Missing remote waveforms can recover after late download verification, including while paused and after the six initial retries. Catalog notifications are coalesced into bounded local API requests without downloading audio again.
- Network search uses a compact status indicator. After a minute of inconclusive checks, it explains that connectivity remains unconfirmed instead of showing an indefinite checking state. Green Online requires confirmed direct, IPv6 or relayed reachability.

## Known limitations

- Hopea has no permanently available public relay. Restrictive NATs still require a reachable source or relay; these changes do not guarantee connectivity between every pair of networks.
- The Redmi 7A / Android 10 keyboard crash has not been confirmed fixed on the device. The Android touch-sequence fix is regression-tested, but the reported Android 14 volume freeze still needs confirmation on the affected device.
- Opening VK or Yandex Music on Android does not complete account sign-in or import. Full Android account import is not implemented.
- Android APKs support API 28 (Android 9) and later and use the persistent preview signing key. They are alpha previews, not production-signed store releases.

## Downloads

Choose the package for your OS and architecture. Android: ARM64 for most current phones, ARMv7 for 32-bit ARM devices, x86-64 for compatible emulators/devices. Alpine `.apk` files are Linux packages, not Android apps.

Linux native packages require GTK 4.20+ and libadwaita 1.8+; older distributions can use the Flatpak bundle. Windows Setup EXE and portable ZIP include their runtime; the per-user installer is unsigned. macOS bottles use Homebrew dependencies. BSD packages remain experimental.

`SHA256SUMS` records the attached files. See `packaging/README.md` in the source archive for installation and platform limits. Gentoo and LFS recipes remain available for manual builds.
