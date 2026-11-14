#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
# LFS/BLFS has no standard package manager; stage a native installation archive.
set -euo pipefail
[[ $(uname -s) == Linux ]] || exit 2
version=$(sed -n "s/.*version: '\([^']*\)'.*/\1/p" meson.build | head -1)
[[ $version =~ ^[0-9]+(\.[0-9]+)+$ ]] || exit 2
export HOPEA_PREFIX=${HOPEA_PREFIX:-/usr/local}
for tool in cargo meson ninja pkg-config blueprint-compiler python3 ffmpeg glib-compile-schemas; do
  command -v "$tool" >/dev/null || { echo "Missing LFS/BLFS prerequisite: $tool" >&2; exit 1; }
done
pkg-config --atleast-version=4.20 gtk4
pkg-config --atleast-version=1.8 libadwaita-1
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
bash packaging/native.sh prepare
bash packaging/native.sh configure
bash packaging/native.sh build
DESTDIR="$stage/root" bash packaging/native.sh install
mkdir -p dist
tar -czf "dist/hopea-$version-lfs-$(uname -m).tar.gz" -C "$stage/root" .
echo 'Staged archive only; nothing has been installed in your system.'
