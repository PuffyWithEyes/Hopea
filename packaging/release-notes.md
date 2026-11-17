# Hopea 2026.3.2 Alpha

This alpha adds visible sharing status and connection help, improves relay discovery, and includes the Android playback changes from the audio preview.

## Changes

- Network search shows whether sharing is unconfirmed, directly reachable, IPv6-only, relayed, or unreachable. Details include practical guidance for VPNs, multiple routers and provider NAT.
- Configured and active relays participate in searches even without DHT. Inconclusive dialback checks no longer prevent relay attempts, and candidate selection rotates between retries.
- Public DHT discovery excludes internal and reserved destinations. Relay probes, polling and background tasks are bounded; existing request admission, signature checks and file validation remain enabled.
- Android volume keys are passed to the media stream. Software volume updates run off the GTK thread and coalesce slider changes.
- Pending network waveforms are retried with bounded delays after playback begins, allowing download verification to finish first.
- Android Java crashes can leave a local report offered on the next launch. Native crashes and system kills are not covered by this reporter.
- UI resources rebuild reliably after Blueprint changes.

## Known limitations

- Hopea has no permanently available public relay. Restrictive NATs still require a reachable source or relay; these changes do not guarantee connectivity between every pair of networks.
- The Redmi 7A / Android 10 keyboard crash has not been confirmed fixed on the device. Android 14 volume and waveform changes still need device verification.
- Opening VK or Yandex Music on Android does not complete account sign-in or import. Full Android account import is not implemented.
- Android APKs support API 28 (Android 9) and later and use the persistent preview signing key. They are alpha previews, not production-signed store releases.

## Downloads

Choose the package for your OS and architecture. Android: ARM64 for most current phones, ARMv7 for 32-bit ARM devices, x86-64 for compatible emulators/devices. Alpine `.apk` files are Linux packages, not Android apps.

Linux native packages require GTK 4.20+ and libadwaita 1.8+; older distributions can use the Flatpak bundle. Windows Setup EXE and portable ZIP include their runtime; the per-user installer is unsigned. macOS bottles use Homebrew dependencies. BSD packages remain experimental.

`SHA256SUMS` records the attached files. See `packaging/README.md` in the source archive for installation and platform limits. Gentoo and LFS recipes remain available for manual builds.
