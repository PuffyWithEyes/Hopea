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

## Further information

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
removing the override. Other vendored crates remain at their latest
published stable versions as checked on November 17, 2026; their local fixes have
not become removable merely by updating the lockfile.

Run the player, Spotify, common and node checks separately as described above.
Network and ingest regression tests must continue to enforce authentication,
resource limits and file validation. A successful Linux build does not establish
Android, Windows, macOS or BSD runtime compatibility.

The Android container runs `tests/test_android_runtime.py subprojects/gtk`
after applying the GTK compatibility patches. It exercises the patched Java
methods with recording UI/GTK queues and checks JNI pointer arguments using the
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
