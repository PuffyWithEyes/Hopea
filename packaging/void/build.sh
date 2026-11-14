#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
[[ ${HOPEA_CI_CONTAINER:-} == 1 ]] || exit 2
archive=${1:?Pass source archive}
version=${HOPEA_VERSION:?Set release version}
repo=${HOPEA_REPOSITORY:?Set release repository}
[[ $version =~ ^[0-9]+(\.[0-9]+)+$ && $repo =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || exit 2
xbps-install -Suy xbps
xbps-install -Suy base-devel rust cargo meson ninja pkg-config blueprint-compiler gettext \
  gtk4-devel libadwaita-devel libsoup3-devel openssl-devel gstreamer1-devel \
  gst-plugins-base1-devel gst-plugins-bad1-devel python3-devel python3-pip libffi-devel ffmpeg \
  adwaita-icon-theme desktop-file-utils
pkg-config --atleast-version=4.20 gtk4
pkg-config --atleast-version=1.8 libadwaita-1
root=$PWD
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tar -xzf "$archive" -C "$work"
cd "$work/hopea-$version"
bash packaging/native.sh prepare
bash packaging/native.sh configure
bash packaging/native.sh build
DESTDIR="$work/stage" bash packaging/native.sh install
arch=$(xbps-uhelper arch)
mkdir -p "$root/dist"
cd "$root/dist"
# xbps-create emits the target architecture (including -musl) in the filename.
xbps-create -A "$arch" -n "hopea-${version}_1" \
  -s 'Music player with peer-to-peer sharing' -l GPL-3.0-or-later \
  --homepage "https://github.com/$repo" \
  -D 'gtk4>=4.20 libadwaita>=1.8 libsoup3 openssl gstreamer1 gst-plugins-base1 gst-plugins-good1 gst-plugins-bad1 gst-plugins-ugly1 gst-libav python3 ffmpeg adwaita-icon-theme' \
  "$work/stage"
xbps-rindex -a ./*.xbps
