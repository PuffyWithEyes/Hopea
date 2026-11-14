# Working on Hopea

Hopea is a Rust/GTK4/libadwaita music player with an embedded P2P node.
This file is the repository guide for Codex and other coding tools.

- Read [README.md](README.md) for the layout and platform status,
  [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) for build/test commands, and
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) when changing player/import/P2P behavior.
- Use Meson to build the player: it generates `src/config.rs`, Blueprint UI and
  GResource assets. On this NixOS workspace use `nix-shell nix/shell.nix --run '…'`.
- The root workspace contains the player and Spotify helper. Test Spotify
  separately with `cargo test -p hopea-spotify`; `--workspace` combines incompatible
  features of its dependencies with the node. `common/` and `node/` have separate workspaces.
- All listening, including previews and conflict comparisons, uses the shared
  `AudioPlayer`. Only starred tracks belong in the personal playlist.
- Keep product logic in Rust. Python is used for external-service adapters.
- Test with temporary data directories. Do not expose cookies/tokens, alter the
  user's music or restart their running application as part of routine checks.
- Preserve migration paths, wire compatibility and validation limits. A network
  peer is untrusted even if it claims to be another Hopea client.
- Keep existing author/license notices. Use per-file SPDX headers or `REUSE.toml`
  for new files. Fork policies from other projects do not apply to contributions here.
- Describe checks that actually ran and distinguish them from untested platform
  recipes. Release/package instructions live in `packaging/` and `nix/`.
- Write project documentation in English.
