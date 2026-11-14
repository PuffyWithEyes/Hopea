Hopea build artifacts

Choose the package for your OS and architecture. Linux native packages require
GTK 4.20+ and libadwaita 1.8+; older distributions can use the Flatpak bundle.
Windows 10/11 x64 Setup EXE and portable ZIP include their runtime; the installer
is per-user and currently unsigned. macOS bottles use Homebrew dependencies.
Android APK is a preview, not a production-signed store release.
Alpine .apk files are Linux packages, not Android apps. Void glibc and musl
packages are separate. LFS archives match the build machine's library stack.

SHA256SUMS records every attached file. Package recipes refer to this release's
source archive and checksums. These builds are not automatically submitted to
AUR, Flathub, Homebrew core or distribution repositories.

See packaging/README.md in the source for installation and known platform limits.
