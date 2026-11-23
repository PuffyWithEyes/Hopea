# Contributing to Hopea

The project does not prescribe how contributors write code. Fixes, suggestions
and translations are welcome, including work prepared with AI assistance.
Contributors remain responsible for checking their changes. Discussions and pull
requests belong in the Hopea repository, not the project it was forked from.

Briefly describe the problem, the change and how you checked it. Bug reports
should include reproduction steps, the operating system and application version.
Do not publish cookies, tokens, private links or browser profile contents.

## Building

Run these commands from the project root. On NixOS or with Nix installed:

```sh
nix-shell nix/shell.nix
meson setup _builddir . -Dprofile=development
meson compile -C _builddir
meson devenv -C _builddir ./src/hopea
```

You can also use `nix develop` from a complete Git checkout or an extracted source
archive. Both commands use the `nixpkgs` revision in `flake.lock`, as GitHub Actions
does; the system's Nix channel does not replace the pinned dependencies. After
changing the environment, run `nix develop --command bash tests/test_nix.sh`
before submitting the change.
Meson generates `src/config.rs` and the UI resources; `cargo run` alone is not
sufficient. Recompile after editing Blueprint files to update the GResource.
See [README](../README.md) for system dependencies and platform limitations.

## Checks

```sh
cargo fmt --all -- --check
cargo test --target-dir _builddir/src -- --test-threads=1
cargo test -p hopea-spotify --target-dir _builddir/src
cargo test --manifest-path common/Cargo.toml
cargo test --manifest-path node/Cargo.toml
meson test -C _builddir --suite import --print-errorlogs
bash tests/test_nix.sh
```

Do not use `cargo test --workspace`: the Spotify helper and P2P node are built
separately because their dependencies have incompatible feature combinations.
Run GUI tests separately with a GTK display, temporary XDG directories and
`GSETTINGS_BACKEND=memory`. Tests against live services require supplied links or
explicit permission to use an authenticated profile. See the
[import guide](IMPORTING.md) for details.

Keep testing proportional to the change. A regression test is useful for a bug
fix; link and build checks are enough for a small file relocation. Do not clear
working caches or the user's music library to run tests.

### Android lyrics and keyboard checks

Run these focused checks with Rust, OpenSSL and Python 3.11+ available; the
keyboard check also needs a JDK:

```sh
bash tests/test_lyrics_https.sh
python3 tests/test_android_keyboard.py
```

The lyrics harness runs the production Rustls transport against a local HTTPS
server with temporary certificates. It checks trusted replies, certificate and
hostname rejection, HTTPS downgrade rejection, status handling and response
size limits without contacting a lyrics service. The keyboard harness compiles
the production Java observer with Android API stand-ins and checks UI-thread
access, show/hide updates, activity cleanup and the API 28 fallback.
These host checks do not establish APK compatibility: verify lyrics lookup,
keyboard dismissal with focus retained, rotation and activity resume on actual
Android 9 and 14 devices or emulators.

## Further information

### Translating Hopea without a translation service

Translations are ordinary UTF-8 gettext `.po` files in `po/`. They can be edited
offline in a text editor; no API, account or automatic translation is required.
Generate a current template from the configured build:

```sh
nix-shell nix/shell.nix --run 'meson compile -C _builddir hopea-pot'
```

The output is `po/hopea.pot`. Initialize a new `.po` from it with `msginit`,
using the real locale code (for example `de` or `pt_BR`) and a new output path.
For an existing language, merge the template into a copy of its catalog:

```sh
msgmerge --output-file=/tmp/hopea-ru.po po/ru.po po/hopea.pot
```

Keep each English `msgid` unchanged and put its translation in `msgstr`.
Preserve placeholders such as `{}`, `{name}`, markup, and intentional newlines.
Fill every plural form where `msgid_plural` appears. Empty translations fall
back to English. Remove the `fuzzy` flag only after reviewing that entry.
Set the language and translator fields in the header, and validate the file:

```sh
msgfmt --check --output-file=/tmp/hopea.mo /tmp/hopea-ru.po
```

**`po/` is the only place to submit translations.** Save the finished catalog
as `po/<locale>.po`, for example `po/ru.po`, `po/az.po` or `po/pt_BR.po`.
Keep its `Language:` header consistent with the filename. Do not add a second
language list or edit Rust code: Meson discovers the catalogs, generates
`LINGUAS` in the build directory, and installs them for the language picker.
Known locales have native display names; new locales initially display their
locale code. After adding or removing a catalog, reconfigure and build:

```sh
nix-shell nix/shell.nix --run 'meson setup --reconfigure _builddir && meson compile -C _builddir'
```

Edits to an existing catalog only require `meson compile -C _builddir`.
Restart Hopea to use the rebuilt translation. Changing the language takes effect
at startup; already created widgets keep their current text. Files in
`dist/translations/` are working copies, not build inputs. Dropping a `.po`
beside an installed APK or executable does not load it at runtime.
Run `python3 tests/test_translations.py` inside the Nix shell to check discovery,
compilation, installation and desktop-file translation with temporary catalogs.

For an Android emulator, `tests/test_android_gettext.c` exercises the actual
native gettext and GLib bridge, plural forms, and missing-message fallback.
Compile it with the matching NDK API 28 compiler and `-ldl`, then run it in an
isolated test directory containing the APK's native libraries and locale assets:
`LD_LIBRARY_PATH=. ./test_android_gettext ./libhopea.so ./share/locale ru`.
Repeat with `en` and `xx` to check English and unsupported-language fallback.
Do not use a real user's app data for this test.

### Updating Rust dependencies

Check the latest non-yanked stable release of each actual package before proposing
an upstream fix. In particular, Hopea's `gtk` dependency is an alias for `gtk4`:
the `gtk` crate targets GTK 3 and is not an upgrade of the GTK 4 bindings.
Update related bindings together and preserve their selected features.

Refresh the root and standalone lockfiles using the compiler from the pinned Nix
shell. Select versions compatible with that compiler:

```sh
cargo --config 'resolver.incompatible-rust-versions="fallback"' update
cargo --config 'resolver.incompatible-rust-versions="fallback"' update --manifest-path node/Cargo.toml
cargo --config 'resolver.incompatible-rust-versions="fallback"' update --manifest-path common/Cargo.toml
# librespot 0.8 uses vergen-gitcl 1.x, which is incompatible with vergen 9.1.
cargo update -p vergen --precise 9.0.6
```

New incompatible releases require an explicit `Cargo.toml` change and migration;
these commands alone do not select them. Keep the resulting lockfiles in Git.
For example, the stable Nix shell currently supplies Rust 1.95, so the lockfile
retains `kstring` 2.0.2 instead of 2.0.5, which requires Rust 1.96.
Keep `vergen` at 9.0.6 until librespot updates its build dependencies: 9.1 uses
a different `vergen-lib` API from the one used by `vergen-gitcl` 1.x.

Cargo updates do not refresh `[patch.crates-io]` path dependencies. Compare each
vendored crate with its published release, preserve author/license notices and
carry forward only fixes still needed. The librqbit 9.1.0 refresh still needs our
HTTP interface-binding guard on BSD. Quanta 0.12.6 carries the upstream 32-bit
musl `timespec` fix because governor 0.10.4 still requires quanta 0.12; quanta 0.13
already includes the fix. See [its patch notes](../vendor/quanta/HOPEA.md) before
removing the override.

The `dontfrag` override was removed after the BSD IPv4 fix was accepted in
[upstream PR #1](https://github.com/nicbn/dontfrag-rs/pull/1) and published in
`dontfrag` 1.0.2 on November 17, 2026. Both workspaces now use that registry
release; their lockfiles pin its checksum
`f949ddc2e911eaf995d2cabc132a478c73cacd1cc0a04c1f0636544b9586178d`.
The published archive identifies upstream commit
`3f2a6f5ba0ba36aeb1e7d4b457481c4902f2691a` and contains the same BSD socket
implementation as the former Hopea patch, along with the upstream license notices.

`if-addrs` 0.15.0, `librqbit-dualstack-sockets` 0.7.0 and `socket-pktinfo` 0.4.2
still have no newer published versions as checked on November 23, 2026. Their
local patches remain; a merged PR alone is not a reason to remove an override
before the release used by the lockfile contains the required fix.

Run the player, Spotify, common and node checks separately as described above.
Network and ingest regression tests must continue to enforce authentication,
resource limits and file validation. A successful Linux build does not establish
Android, Windows, macOS or BSD runtime compatibility.

The Android container runs `tests/test_android_runtime.py subprojects/gtk`
after applying the GTK compatibility patches. It exercises the patched Java
methods with recording UI/GTK queues, verifies touch sequence stability and
event-copy lifetime during long drags, and checks JNI pointer arguments using the
selected NDK compiler. To run it separately, provide a staged GTK directory,
a host C compiler and a JDK; `ANDROID_RUNTIME_CC` selects an additional NDK
compiler for the width checks. These are boundary regression checks, not a
substitute for keyboard and document-picker tests on an Android device.

Android builds save unhandled Java exceptions locally and offer to copy the
report on the next launch, without developer mode or USB debugging. Reports
contain the app version, device model, process bitness and bounded Java stack
traces. Exception messages, input text and file names are omitted. Nothing is
uploaded automatically; the dialog also offers deletion. Reports are stored
outside GTK's runtime asset directory and excluded from backups.
This does not capture native signals, low-memory kills, initialization before
the application handler is installed, or Android ANRs. Absence of a report does
not mean the app did not crash. Run `python3 tests/test_android_crash_report.py`
with a JDK to verify formatting and privacy limits, and
`python3 tests/test_android_manifest.py` to check the application registration.

### Project references

- [Architecture and component interactions](ARCHITECTURE.md).
- [Node security and protocol](../node/README.md).
- [Nix, NixOS and Home Manager](../nix/README.md).
- [Packages and GitHub Actions](../packaging/README.md).

Preserve licenses and existing author notices. Add SPDX headers or an entry in
`REUSE.toml` for new files. Licensing terms are in [LICENSES](../LICENSES) and
[ADDITIONAL-TERMS.md](../ADDITIONAL-TERMS.md).
