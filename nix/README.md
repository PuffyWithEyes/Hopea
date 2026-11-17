# Hopea with Nix

The flake tracks the stable `nixos-26.05` branch. `flake.lock` pins an exact
revision shared by `nix build`, `nix develop`, and `nix-shell nix/shell.nix`.
Run `nix flake update nixpkgs` to receive updates within that stable release.
Moving to a newer stable release requires changing the input in `flake.nix`
and updating the lock file.

Only `flake.nix` and `flake.lock` live at the repository root. The package recipe
is in `nix/package.nix`, and the development environment is in `nix/shell.nix`.
All five installation modules share one implementation in the flake.

No system image is needed. `nix run` builds the package or downloads available
dependencies from the cache, then starts the application. `nix profile add`
installs it into your profile. Hopea is not yet published in nixpkgs and does not
have its own binary cache.

## Building from source

Run these commands from the root of an extracted source archive:

```sh
nix build .#hopea
nix run .#hopea
nix profile add .#hopea
```

Older Nix versions use `profile install` instead of `profile add`. If flakes are
disabled, add `--extra-experimental-features 'nix-command flakes'` after `nix`.
The build includes resources, icon themes, codecs, FFmpeg, Python import libraries
and the Spotify helper. The application does not run pip at startup.
Dependency versions are pinned by `flake.lock`, `Cargo.lock` and source hashes.
Python packages from nixpkgs may differ from those in the development pip environment.

Git flakes only include files added to Git. To build a working copy containing
source files that have not been added yet, create a filtered source archive:

```sh
bash build-aux/make-dist.sh
mkdir -p /tmp/hopea-install
# The archive already contains a hopea directory.
tar -xzf dist/hopea-src.tar.gz -C /tmp/hopea-install
cd /tmp/hopea-install/hopea
nix build .#hopea
```

The archive uses an allowlist of source directories. Local environments, hidden
files, symlinks and build caches are excluded. Do not put secrets in source files.
For development, use `nix develop`, or `nix-shell nix/shell.nix` without flakes.

## NixOS, Home Manager, nix-darwin, Nix-on-Droid and NixBSD

Add the Hopea source as an input to your configuration flake. Before publishing
the repository, this can be an absolute path to an extracted archive:

```nix
inputs.hopea.url = "path:/absolute/path/to/hopea";
```

Pass `hopea` to modules through `specialArgs = { inherit hopea; };`
(NixOS/nix-darwin) or `extraSpecialArgs = { inherit hopea; };` (Home Manager).
Example Home Manager module:

```nix
{ hopea, pkgs, ... }: {
  imports = [ hopea.homeManagerModules.default ];
  programs.hopea.enable = true;
  programs.hopea.package = hopea.packages.${pkgs.stdenv.hostPlatform.system}.hopea;
}
```

Choose the appropriate export:

| Configuration | Module | Package destination |
| --- | --- | --- |
| Home Manager, standalone or within a system | `homeManagerModules.default` | `home.packages` |
| NixOS, including NixOS-WSL | `nixosModules.default` | `environment.systemPackages` |
| macOS with nix-darwin | `darwinModules.default` | `environment.systemPackages` |
| Nix-on-Droid | `nixOnDroidModules.default` | `environment.packages` |
| NixBSD (experimental) | `nixbsdModules.default` | `environment.systemPackages` |

Every module exposes the same options: `programs.hopea.enable` and
`programs.hopea.package`. Selecting the flake package uses Hopea's pinned
dependencies. Without an explicit `package`, the recipe uses your configuration's
`pkgs`. Older nixpkgs revisions may lack the required GTK/Rust versions.
Do not enable both the system and user modules if one installation is sufficient.
The modules do not enable sharing, autostart, VPN or network permissions, and do
not change user settings.

For the overlay, use `nixpkgs.overlays = [ hopea.overlays.default ];`, then
`pkgs.hopea`. To supply your own package set, use `hopea.lib.mkPackage pkgs`.

## Platforms and limitations

| System | How it runs | Verification / limitation |
| --- | --- | --- |
| Linux, NixOS, standalone Nix | Native x86_64/aarch64 package | Both recipes can be evaluated; build checks run on x86_64 Linux |
| macOS / nix-darwin | Native Intel/Apple Silicon package | Recipes can be evaluated; native build and GUI checks are still needed. Upstream nixpkgs announced the end of Intel Mac support after 26.05 |
| Windows with Nix | WSL2 + WSLg, Linux package | WSL2 uses a virtual machine; audio, browser and networking need testing on Windows |
| Windows without a virtual machine | Native `.exe` build | `build-aux/windows/build.sh` under MSYS2 UCRT64; not a Nix package; requires Windows testing |
| Nix-on-Droid | Linux aarch64 package in PRoot | Requires separate graphical and audio sessions; installation does not create an Android Activity or media service |
| Android without a Linux environment | Native APK | See [Android preview](../README.md#android-preview) |
| FreeBSD / GhostBSD / HardenedBSD / NixBSD | Native recipe with a BSD package set | The previously checked upstream nixpkgs revision blocked `libproxy` on FreeBSD. The module is available, but no verified build exists; a compatible dependency set is needed |
| NetBSD / OpenBSD / DragonFly and derivatives | Native source build with OS packages | Hopea's full Nix stack is unverified; see `build-aux/desktop/build.sh`. Dependency versions and native builds need checking |
| Redox | Separate port | Requires substantial GTK/GStreamer and platform backend work; a Nix module does not provide that port |

None of Hopea's commands automatically creates or starts a virtual machine.
WSL is an optional, separate route for running Nix on Windows.
Nix-on-Droid cannot access the private data of Android browsers.
In WSL/PRoot, a host VPN may be invisible to the application. VPN detection is not
a firewall; do not disable it automatically to make the application start there.

Other Linux distributions may need their own GPU/portal configuration.
BSD needs a session D-Bus for MPRIS and tray integration. On macOS, this currently
starts `hopea` from a terminal; it does not produce a signed standalone `.app`.

Run `bash tests/test_nix.sh` to check 20 module combinations covering enablement
and package overrides, then configure the actual package in a Nix sandbox.
These checks do not replace runtime testing on each OS.

## Primary sources

- [Nix: supported platforms](https://nix.dev/manual/nix/2.34/installation/supported-platforms)
- [NixBSD: native FreeBSD and nixpkgs status](https://github.com/nix-community/nixbsd)
- [Microsoft: GUI applications in WSLg](https://learn.microsoft.com/en-us/windows/wsl/tutorials/gui-apps)
- [Nix-on-Droid and Home Manager integration](https://github.com/nix-community/nix-on-droid)
- [Home Manager](https://nix-community.github.io/home-manager/)
