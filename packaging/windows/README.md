# Windows installer

Hopea's Windows package targets **Windows 10/11 x64**. The installer also allows
Windows 11 ARM64 with x64 emulation; this is not a native ARM64 build and has not
been device-tested. Server 2016 and later need Desktop Experience and the media
components used by GTK/GStreamer; Server Core is not a supported desktop target.
This is a dependency baseline, not a claim that every Windows build, GPU driver
and codec has been tested.

## Why not XP, Vista, 7 or 8?

Compatibility was checked against upstream documentation on 2026-11-14:

| Component shipped by Hopea | Upstream Windows requirement |
| --- | --- |
| Rust `x86_64-pc-windows-gnu` | [Windows 10 / Server 2016 or later](https://doc.rust-lang.org/rustc/platform-support.html) |
| MSYS2 UCRT64 runtime packages | [Windows 10 / Server 2016 or later](https://www.msys2.org/docs/windows_support/); individual dependencies can impose higher requirements |
| Embedded CPython 3.14 | [Windows 10 or later](https://docs.python.org/3.14/using/windows.html#supported-windows-versions) |
| Installer architecture gate | [x64-compatible Windows](https://jrsoftware.org/ishelp/topic_setup_architecturesallowed.htm), including ARM64 Windows 11 emulation |

The MSYS2 *development environment installer* has a separate Windows 10 1809+
requirement. It is not installed on the user's machine by Hopea, so that alone
is not a reason to impose 1809 on Hopea's installed runtime.

Changing `MinVersion` in an installer cannot make newer Windows API imports work
on XP or Windows 7. Rust has separate [Tier 3 Windows 7 targets](https://doc.rust-lang.org/rustc/platform-support/win7-windows-msvc.html),
but these do not make our GNU/UCRT64 libraries, GTK stack or Python 3.14 compatible.
A legacy edition would require a separate toolchain, dependency builds, importer
runtime and testing on each old OS. XP needs an even more extensive port. Neither
is implemented or represented as a supported download. The x64 bundle cannot run
on 32-bit Windows either.

## What the installer does

- Produces `hopea-VERSION-windows-x86_64-setup.exe`, alongside the portable ZIP.
- Installs for the current user into `%LOCALAPPDATA%\Programs\Hopea`, without UAC.
- Includes the GTK/GStreamer/FFmpeg/Python runtime; installation is offline.
- Uses the existing cat icon, English/Russian messages and a light/dark wizard.
- Adds a Start menu shortcut and an optional desktop shortcut.
- Uses a stable application ID and installation path for subsequent updates.
- Registers an uninstaller in Windows Settings. It removes installer-owned files,
  preserving music, settings, library databases and files created by the user.
- Checks OS/CPU compatibility before installing the payload.

This does not enable automatic updates, add startup entries, change file
associations, open firewall ports or change Hopea's sharing preferences.
If Hopea is running, the interactive installer asks about closing it through
Windows Restart Manager; it does not force-kill the player.

## Build

First create the portable runtime in an MSYS2 UCRT64 shell:

```sh
bash build-aux/windows/build.sh
```

Dependency downloads use up to three Pacman attempts, waiting 10 and then 20
seconds between failures and reusing cached packages. Signature verification and
download timeouts remain enabled. Installation runs once after downloads
succeed; an installation failure stops the build. This handles temporary
[MSYS2 mirror](https://www.msys2.org/docs/mirrors/) outages without restarting
the entire player build. A persistent mirror outage still requires a later retry.

Then, from the repository in PowerShell 7:

```powershell
$env:HOPEA_REPOSITORY = 'OWNER/REPOSITORY'
./packaging/windows/build.ps1
```

The script downloads Inno Setup **7.1.1** from its upstream GitHub release,
checks the pinned SHA-256 before executing it, and installs the compiler into
`_windows/setup/inno` for the current build user. With an existing Inno Setup 7
compiler, pass `-IsccPath 'C:\path\to\ISCC.exe'` to avoid that download/install.
The version comes from `meson.build`; `HOPEA_VERSION`, when set, must match.

GitHub's `Packages` workflow builds both ZIP and Setup EXE and includes both in
the Windows artifact. Release preparation requires the EXE and includes its
checksum in `SHA256SUMS`. No GitHub Release is published by the build script.

## Verification and signing

`tests/test_windows_installer.ps1` is an integration check for a disposable
Windows CI account. It tests installation to a Unicode path with spaces, Start
menu integration, runtime launch with the developer PATH removed, the private
Python importer, repeat installation and uninstall. It verifies user-data
preservation and refuses to overwrite a pre-existing Hopea installation.
Do not run it against your everyday Windows account.

The checked-in workflow is not evidence of a successful native run: check the
Windows job result before distributing a release. Testing the installer under
Wine also does not prove the player's Windows runtime or old-OS compatibility.

The generated Hopea installer is currently **unsigned**. Windows may show an
unknown-publisher/SmartScreen warning. The compiler's signature does not sign
Hopea. Production Authenticode signing needs the project's own certificate or
signing service; none is configured, and checks are not bypassed.
