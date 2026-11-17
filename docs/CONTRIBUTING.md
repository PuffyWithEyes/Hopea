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

- [Architecture and component interactions](ARCHITECTURE.md).
- [Node security and protocol](../node/README.md).
- [Nix, NixOS and Home Manager](../nix/README.md).
- [Packages and GitHub Actions](../packaging/README.md).

Preserve licenses and existing author notices. Add SPDX headers or an entry in
`REUSE.toml` for new files. Licensing terms are in [LICENSES](../LICENSES) and
[ADDITIONAL-TERMS.md](../ADDITIONAL-TERMS.md).
