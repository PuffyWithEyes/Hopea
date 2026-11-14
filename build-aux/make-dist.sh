#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/.."

# Share the package's source allowlist. Never archive the entire working tree:
# it may contain browser sessions, local environments and large build caches.
source_path=$(nix --extra-experimental-features 'nix-command flakes' eval --impure --raw --expr '
  let
    lock = (builtins.fromJSON (builtins.readFile ./flake.lock)).nodes.nixpkgs.locked;
    pkgs = import (builtins.fetchTree lock) {};
  in (pkgs.callPackage ./nix/package.nix {}).src
')
mkdir -p dist
tar --create --gzip --file dist/hopea-src.tar.gz \
    --mode='u+rwX,go+rX' --transform 's,^\.,hopea,' --directory "$source_path" .
echo "dist/hopea-src.tar.gz ($(du -h dist/hopea-src.tar.gz | cut -f1))"
