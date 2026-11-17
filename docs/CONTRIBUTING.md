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
HTTP interface-binding guard on BSD. Other vendored crates remain at their latest
published stable versions as checked on November 17, 2026; their local fixes have
not become removable merely by updating the lockfile.

Run the player, Spotify, common and node checks separately as described above.
Network and ingest regression tests must continue to enforce authentication,
resource limits and file validation. A successful Linux build does not establish
Android, Windows, macOS or BSD runtime compatibility.

### Project references

- [Architecture and component interactions](ARCHITECTURE.md).
- [Node security and protocol](../node/README.md).
- [Nix, NixOS and Home Manager](../nix/README.md).
- [Packages and GitHub Actions](../packaging/README.md).

Preserve licenses and existing author notices. Add SPDX headers or an entry in
`REUSE.toml` for new files. Licensing terms are in [LICENSES](../LICENSES) and
[ADDITIONAL-TERMS.md](../ADDITIONAL-TERMS.md).
