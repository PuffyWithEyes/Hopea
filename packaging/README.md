# Hopea packages

These build recipes target this project's repository and GitHub Releases. They do
not mean that Hopea is already available in AUR, nixpkgs, Flathub or official OS
repositories. A full build has been tested locally on NixOS x86_64. Other platforms
still need successful native CI builds and launch tests; YAML and shell checks
cannot establish that compatibility.

Targeted checks have also passed in disposable containers: Debian's clean step
preserves vendored Cargo files, ALT installs the build dependencies, Void glibc and
musl bootstrap Bash, Alpine locates its generated signing key, and openSUSE provides
Python's SQLite module. These checks do not constitute full application builds.

## Available recipes

| Platform | Recipe / output | Conditions |
| --- | --- | --- |
| Arch Linux and derivatives | `arch/PKGBUILD.in` → `.pkg.tar.zst` | Arch CI, x86_64; the recipe also allows aarch64 with a native builder |
| Artix Linux | The same `PKGBUILD`, built on Artix | No systemd dependency; using an Arch binary depends on Artix library versions. No separate Artix CI run yet |
| Void Linux | `void/build.sh` → `.xbps` and repository index | Separate glibc and musl CI containers, x86_64; packages for different libc implementations are not interchangeable |
| Alpine Linux | `alpine/APKBUILD.in` → signed `.apk` | Edge CI, x86_64, musl; aarch64 needs a native builder. This is an Alpine package, not an Android APK |
| LFS / BLFS | `lfs/build.sh` → installation tree archive | Built against the host libraries; optional CI requires a self-hosted LFS runner |
| Debian / Ubuntu | `debian/` → `.deb` | Debian sid and Ubuntu 26.10 CI; these DEBs are not intended for older stable releases |
| Fedora | `rpm/hopea.spec.in` → `.rpm` | Rawhide CI; additional codecs and FFmpeg must be available from the system repositories |
| openSUSE | The RPM recipe with openSUSE dependencies | Tumbleweed CI |
| ALT Linux | The RPM recipe with ALT dependencies | Sisyphus CI; stable branches need library version checks |
| Gentoo | `gentoo/` → ebuild overlay and optional `.gpkg.tar` | Portage source build; import dependencies are enabled by default, with ebuilds for VK, Yandex and Cryptodome |
| NixOS, Nix, Home Manager | [Nix recipe and modules](../nix/README.md) | x86_64/aarch64 Linux; CI also exports a binary cache |
| CentOS Stream / RHEL / Rocky / Alma | Flatpak | Native RPMs need GTK 4.20+ and libadwaita 1.8+; older system libraries are not replaced automatically |
| Linux Mint and other stable Linux distributions | Flatpak | Requires Flatpak and access to the GNOME runtime on Flathub |
| FreeBSD / DragonFly | `bsd/build.sh` → `.pkg` and installation archive | Experimental BSD CI builds; depend on the availability of recent libraries |
| NetBSD / OpenBSD | `bsd/build.sh` → `.tgz` using native `pkg_create` | Dependencies and ABI match the build system; not yet validated on these OSes or submitted to ports/pkgsrc |
| GhostBSD / HardenedBSD and other derivatives | The corresponding base BSD recipe | ABI compatibility and package availability must be checked for each release |
| Windows 10/11 x64 | MSYS2 UCRT64 → Setup EXE, ZIP and Scoop JSON | Per-user installation; includes GTK, GStreamer, FFmpeg and private Python. See [compatibility and build instructions](windows/README.md) |
| macOS Apple Silicon / Intel | Homebrew formula and bottle | Native build with Homebrew dependencies; not a standalone `.app`, no Apple notarization |
| Android ARM64 / x86-64 | Preview APK | Separate expensive jobs with a persistent preview key; Android 12+. x86-64 still needs a full APK build and launch test |
| Flatpak | `build-aux/flatpak/` → `hopea.flatpak` | Shared Linux distribution format, GNOME 51 runtime |

Native Linux packages require GTK **4.20+**, libadwaita **1.8+** and GStreamer
**1.20+**. The build script checks versions before compiling Rust. Systems without
these libraries need a separate port, a library upgrade or Flatpak; lowering the
requirements in package metadata does not make the application compatible.

Gentoo, BSD, Android and LFS are enabled through manual workflow inputs. BSD jobs
use build VMs because GitHub does not provide standard BSD runners. SSH connects
the runner to its local BSD guest and executes build commands. Docker supplies
Linux jobs with the selected distribution's libraries and packaging tools.
Installed Hopea packages run natively and do not require Docker, SSH or a VM.

## GitHub Actions

1. Push the current sources to GitHub, excluding caches, profiles, cookies and
   keys. After fixing a workflow, use **Run workflow** on `main`: **Re-run jobs**
   on an old run uses its original commit and retains the old errors.
2. Open **Actions → Packages → Run workflow**. Leave `draft_release` disabled for
   a test build. Each successful job uploads its own **Artifact**.
3. Download the artifacts and test them. Jobs are independent: a BSD failure does
   not remove successful Windows or Linux artifacts. For failures, inspect
   `bsd-logs-*` or `macos-logs-*` for full diagnostics rather than only an SSH or
   compiler exit code.
4. To prepare a release, push a tag such as `v2026.2` matching the Meson version.
   A tag run creates a release **draft** after the required build matrix succeeds.
   A manual run on that tag with `draft_release=true` does the same. Publish the
   draft through the GitHub interface after reviewing it.

The `Checks` workflow tests Rust code, the importer, Nix modules and packaging
scripts. No branch protection, mandatory reviews or commit restrictions are set.
Build jobs have `contents: read`; only the release draft job receives write access.
Actions are pinned by commit SHA. Pull requests do not receive release secrets.
Rolling distribution containers change over time; their actual image digests are
saved alongside the packages. This does not imply that all builds are reproducible
byte for byte.

Release files use `github.repository`, the Meson version and actual artifact
SHA-256 hashes, without links to the original fork's repository. Source archives
include pinned Rust crates for offline builds. Python wheels are downloaded in a
preparation step for each OS and Python version; installing a finished package
does not download dependencies through pip.

### Platform-specific setup

- **Android:** set `ANDROID_PREVIEW_KEYSTORE_BASE64` to the base64 representation
  of one persistent debug keystore, with alias `androiddebugkey` and the standard
  `android` passwords. This is preview signing; production needs a separate
  release signing pipeline. The key is excluded from source, artifacts and caches
  and removed after the job. Without the secret, the job stops instead of creating
  a new key incompatible with installed APKs. See the [release guide](../docs/RELEASING.md).
- **Nix cache:** the optional `NIX_SIGNING_KEY` secret signs the export. Users need
  its public key in `trusted-public-keys`. Without it, the cache artifact is
  unsigned; use `nix build` or `nix run` from source without globally disabling
  signature checks.
- **macOS:** formulas and bottles use Homebrew. For a persistent tap, copy the
  generated formula for your architecture to `Formula/hopea.rb`. The two platform
  source archives contain different Python wheels and are not interchangeable.
- **Gentoo:** the recipes archive contains an overlay layout. Add it to Portage,
  create manifests with `ebuild … manifest`, then install `media-sound/hopea`.
  This remains a private overlay. The optional `gentoo` job builds a binary
  package; a persistent Gentoo binhost is not configured.
- **LFS:** the `lfs` input requires a runner labelled `self-hosted`, `Linux` and
  `hopea-lfs`, with BLFS dependencies already installed. Otherwise leave it off.
  This job runs manually, not from pull requests; use a separate build account
  without permission to modify the system.
- **Alpine:** CI creates a key for each build and exports only its public part
  alongside the APK. A persistent repository needs a persistent signing key;
  automatic updates across different signing keys are not configured.

## Installing artifacts

Download published files from this repository's **Releases**. `SHA256SUMS` lets
you check that a download matches the published file. After checking it, use the
package manager for the matching system release:

```sh
# Arch / Artix with compatible library versions
sudo pacman -U ./hopea-*.pkg.tar.zst
# Matching Debian / Ubuntu release
sudo apt install ./hopea-*.deb
# Fedora
sudo dnf install ./hopea-*.rpm
# openSUSE
sudo zypper install ./hopea-*.rpm
# ALT
sudo apt-get install ./hopea-*.rpm
# Flatpak, including CentOS and stable Linux releases
flatpak install --user ./hopea.flatpak
flatpak run io.hopea.Hopea
# FreeBSD / DragonFly with a matching ABI
sudo pkg add ./hopea-*.pkg
```

For Void, place `.xbps` and its matching `*-repodata` in one local directory, then
run `sudo xbps-install --repository=/absolute/path/to/directory hopea`. Choose
glibc or musl to match the system. A signed online XBPS repository is not configured.

For Alpine, verify the package source and public key, copy the `.pub` file to
`/etc/apk/keys/`, then run `sudo apk add ./hopea-2026.2-r0.apk`, substituting the
actual version. Signature verification remains enabled.

For Windows, run `hopea-VERSION-windows-x86_64-setup.exe`. No administrator access
is required, and newer versions can be installed over an existing installation.
Alternatively, extract the ZIP and run `Hopea/bin/hopea.exe`, or add the generated
`hopea.json` to a Scoop bucket. XP, Vista, Windows 7/8 and 32-bit Windows are not
supported by this build.

For macOS, install the bottle through Homebrew or use the corresponding formula
to build from source. For Android, enable USB debugging and run
`adb install -r Hopea-android-arm64-preview.apk`, choosing the APK for your ABI.

NetBSD and OpenBSD packages use native `pkg_create`, with dependencies from the
build system, and are installed with `pkg_add`. They still need native validation
and have not been submitted to official ports/pkgsrc. Additional `.tar.gz` files
contain installation trees for inspection; use the package manager instead of
extracting them over a working system.

## Local builds

From the Nix environment:

```sh
nix develop --command bash packaging/source.sh
HOPEA_REPOSITORY=OWNER/REPO HOPEA_VERSION=2026.2 \
  bash packaging/render.sh dist/hopea-2026.2-source.tar.gz
bash tests/test_packaging.sh
```

`native.sh prepare` downloads dependencies, `configure/build` compiles with Cargo
network access disabled, and `install` requires a separate `DESTDIR`. The
`linux-deps.sh` script is intended for disposable CI containers, not for managing
dependencies on a working desktop.

For LFS, prepare Rust/Cargo, a C/C++ compiler, Meson/Ninja, pkg-config, Blueprint,
gettext, desktop-file-utils, Python with pip/venv, FFmpeg, and development libraries
for GTK 4.20+, libadwaita 1.8+, libsoup 3, OpenSSL, GStreamer and its plugins. See the
[BLFS book](https://www.linuxfromscratch.org/blfs/view/svn/x/libadwaita.html) for the
library stack. From extracted sources, run `bash packaging/lfs/build.sh`.
The result is `dist/hopea-VERSION-lfs-ARCH.tar.gz`, with `/usr/local` as the default
prefix. Inspect the archive and register its files using your local package
management approach. The build does not write to `/usr` or configure the system;
the archive is not portable across arbitrary LFS installations.

Remaining substantial work includes a complete GTK/GStreamer port for Redox,
support for older system libraries, Android production signing, a signed standalone
macOS `.app`, and inclusion in official OS repositories.
