#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
cp -R "$root/packaging/debian" debian
sed 's/@VERSION@/1.2/g' debian/changelog.in > debian/changelog
mkdir -p vendor-registry/example-1.0/src vendor/local/src .packaging/build
printf 'upstream manifest\n' > vendor-registry/example-1.0/Cargo.toml.orig
printf 'upstream backup-name file\n' > vendor-registry/example-1.0/src/example.orig
printf 'local dependency manifest\n' > vendor/local/Cargo.toml.orig
sha256sum vendor-registry/example-1.0/Cargo.toml.orig \
  vendor-registry/example-1.0/src/example.orig vendor/local/Cargo.toml.orig > checksums
touch temporary.orig .packaging/build/disposable
make -f debian/rules clean
sha256sum -c checksums
test ! -e temporary.orig
test ! -e .packaging/build
make -f debian/rules clean
sha256sum -c checksums
echo 'Debian clean preserves Cargo vendor checksums: OK'
